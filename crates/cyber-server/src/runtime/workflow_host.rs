//! The server's [`WorkflowHost`]: every host call of a running script becomes a durable,
//! observable effect (`workflows` → agent() call, Phases, logs and checkpoints, Model tiers
//! and mixing, Concurrency limit, Run budgets, Replay of workflow host calls).
//!
//! It lives on the run's engine thread. Each call is sequenced when the engine dispatches
//! it and matched against the replay record; a recorded result is delivered in its
//! recorded order without calling a model. Otherwise an agent call checks pause, waits for
//! a slot from the server-wide fair scheduler, checks pause again and the run budget, is
//! recorded as started, and crosses to the tokio runtime to spawn (or resume) a child
//! Session through [`ToolHost::spawn_workflow_agent`]. A call the host rejects (invalid
//! options, an unknown tier, `ask()`) is sequenced and recorded like the others. The
//! settlement is appended before the future returns, so the script never observes an
//! unrecorded result; when an append fails the run fails closed through
//! [`WorkflowHost::fatal`]. A stopped or shut-down run never records the cancellation of an
//! in-flight agent, so a resume continues its child Session instead. `ctx.budget` is the
//! snapshot recorded with the latest delivered result, so a replay shows the same values.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cyber_core::budget::RunBudget;
use cyber_store::Store;
use cyber_workflow::{AgentCall, Diagnostic, DiagnosticKind, HostFuture, WorkflowHost};
use futures::future::Either;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::budget::RunSpend;
use super::host::{
    RunWorktree, ToolHost, TurnContext, WorkflowAgentOutput, WorkflowAgentRequest,
    WorkflowToolRequest,
};
use super::workflow_ask::{Asked, ScriptQuestion, no_user};
use super::workflow_control::{Control, Signal};
use super::workflow_replay::{Outcome, Plan, Replay};
use super::workflow_scheduler::{Slot, WorkflowScheduler};
use super::workflow_warmup::{self, Groups, Release, Role};
use super::workflows::{
    AGENT_SETTLED, AGENT_STARTED, CALL_RECORDED, LOG, PHASE, RunLog, WorkflowAgent, WorkflowRun,
};
use super::{Runtime, WeakRuntime};

/// `agent()` options that are validated by the engine but not available in this build.
const UNAVAILABLE_OPTIONS: &[(&str, &str)] = &[
    (
        "tools",
        "tool allow lists for workflow agents are not enforced yet",
    ),
    ("budget", "per-agent budgets are not enforced yet"),
];
/// Durable records a run may write: host calls, agent starts and settlements, phase
/// changes and logs. Past either bound the run fails closed, so a script cannot fill the
/// store (`workflows` → Phases, logs and checkpoints).
pub const MAX_RUN_RECORDS: u64 = 100_000;
/// The JSON bytes of every record of a run together.
pub const MAX_RUN_RECORD_BYTES: u64 = 256 * 1024 * 1024;
/// The largest `checkpoint()` value, as JSON.
pub const MAX_CHECKPOINT_BYTES: usize = super::workflow_results::INLINE_RESULT_BYTES;
const WALL: &str = "max_wall_seconds";
/// The prompt a resumed child receives after its run was interrupted.
const CONTINUE: &str = "Your previous attempt at this workflow task was interrupted. Continue it from where you stopped and give your final answer.\n\nThe task:\n";

pub(super) struct RunHost {
    pub(super) store: Arc<Store>,
    pub(super) log: RunLog,
    /// `<state>/workflows`, for return values larger than 1 MiB.
    results: Option<std::path::PathBuf>,
    tools: Arc<dyn ToolHost>,
    runtime: WeakRuntime,
    scheduler: WorkflowScheduler,
    /// The run's share of the scheduler's slots (`max_concurrent`).
    share: usize,
    handle: tokio::runtime::Handle,
    control: Arc<Control>,
    run_id: String,
    owner: TurnContext,
    /// `ask()` and the asks of direct tool calls may reach the owner's clients.
    interactive: bool,
    /// The run's `args`, whose `answers` declare `ask()` answers in advance.
    args: Value,
    solve_base: Option<String>,
    tiers: BTreeMap<String, Option<String>>,
    budget: RunBudget,
    replay: Replay,
    /// Calls seen per replay key, for the ordinal of the next one.
    ordinals: RefCell<HashMap<String, u32>>,
    phases: RefCell<Vec<String>>,
    phase: RefCell<Option<String>>,
    fatal: RefCell<Option<Diagnostic>>,
    /// Agents of the run ever started, against `max_agents`.
    started: Cell<u64>,
    /// Agents spawned by this attempt and not yet settled.
    in_flight: Cell<usize>,
    /// The first exhausted run limit.
    exceeded: RefCell<Option<String>>,
    /// The attempt ended for pause, stop or shutdown; its status is already durable.
    halted: Cell<bool>,
    /// Edit-capable agents without an `isolation` choice, issued and not yet settled, of
    /// every run in the Location (shared through `Inner::workflow_writers`).
    writers: Arc<AtomicUsize>,
    /// What `ctx.budget` shows: the snapshot of the latest delivered result.
    budget_view: RefCell<Value>,
    /// Records of the run so far, against [`MAX_RUN_RECORDS`] and [`MAX_RUN_RECORD_BYTES`].
    records: RecordCount,
    /// Prompt-prefix groups for cache warm-up, when the host warms.
    warmup: Option<Groups>,
    /// A host call other than `agent()` was already rejected for the wall-clock budget.
    wall_rejected: Cell<bool>,
}

/// How many records a run holds and their bytes.
#[derive(Debug, Default)]
pub(super) struct RecordCount {
    records: Cell<u64>,
    bytes: Cell<u64>,
}

impl RecordCount {
    /// The records `run_id` already holds; a store that cannot say counts as full, so the
    /// run fails closed at its first record.
    fn of(store: &Store, run_id: &str) -> Self {
        let run = run_id.to_owned();
        let (records, bytes) = store
            .read(move |db| {
                Ok(db.query_row(
                    "SELECT COUNT(*), COALESCE(SUM(LENGTH(data)), 0) FROM event WHERE aggregate_id = ?1",
                    [run],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )?)
            })
            .map(|(records, bytes)| (records.max(0) as u64, bytes.max(0) as u64))
            .unwrap_or((MAX_RUN_RECORDS, MAX_RUN_RECORD_BYTES));
        Self {
            records: Cell::new(records),
            bytes: Cell::new(bytes),
        }
    }

    /// Count one more record of `bytes`, or refuse it past a bound.
    pub(super) fn charge(&self, bytes: usize) -> Result<(), Diagnostic> {
        let records = self.records.get() + 1;
        let total = self.bytes.get().saturating_add(bytes as u64);
        if records > MAX_RUN_RECORDS {
            return Err(Diagnostic::execution(format!(
                "the run made more than {MAX_RUN_RECORDS} durable host calls, phase changes and logs"
            )));
        }
        if total > MAX_RUN_RECORD_BYTES {
            return Err(Diagnostic::execution(format!(
                "the run recorded more than {} MiB of host call data",
                MAX_RUN_RECORD_BYTES / (1024 * 1024)
            )));
        }
        self.records.set(records);
        self.bytes.set(total);
        Ok(())
    }
}

/// A validated call, ready to spawn.
struct Prepared {
    record: WorkflowAgent,
    prompt: String,
    agent: Option<String>,
    schema: Option<Value>,
    isolation: Isolation,
    /// `goal: {condition, check?, budget?}`: the child's active Goal.
    goal: Option<AgentGoal>,
}

/// The `goal` option of `agent()` (`goals` → Goals on workflow agents and subagents).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct AgentGoal {
    condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget: Option<cyber_core::budget::Budget>,
}

/// Parse and check `options.goal`.
fn goal_option(options: &Map<String, Value>) -> Result<Option<AgentGoal>, Diagnostic> {
    let Some(value) = options.get("goal").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let goal: AgentGoal = serde_json::from_value(value.clone())
        .map_err(|e| Diagnostic::execution(format!("agent() option goal: {e}")))?;
    cyber_core::goals::check_condition(&goal.condition)
        .map_err(|e| Diagnostic::execution(format!("agent() option goal: {e}")))?;
    if let Some(budget) = &goal.budget {
        budget
            .validate()
            .map_err(|e| Diagnostic::execution(format!("agent() option goal.budget.{e}")))?;
    }
    Ok(Some(goal))
}

/// The child Goal's Budget: its own limits, each lowered to what the run has left
/// (`goals` → a workflow agent's goal budget is capped by the run's remaining budget).
fn capped_goal_budget(
    own: Option<cyber_core::budget::Budget>,
    run: &cyber_core::budget::Budget,
    spend: &RunSpend,
) -> cyber_core::budget::Budget {
    let mut budget = own.unwrap_or_default();
    let lower_u = |own: Option<u64>, left: Option<u64>| match (own, left) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let lower_f = |own: Option<f64>, left: Option<f64>| match (own, left) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    budget.max_turns = lower_u(
        budget.max_turns,
        run.max_turns.map(|m| m.saturating_sub(spend.turns)),
    );
    budget.max_tokens = lower_u(
        budget.max_tokens,
        run.max_tokens.map(|m| m.saturating_sub(spend.tokens)),
    );
    budget.max_cost_usd = lower_f(
        budget.max_cost_usd,
        run.max_cost_usd.map(|m| (m - spend.cost).max(0.0)),
    );
    budget.max_wall_seconds = lower_f(
        budget.max_wall_seconds,
        run.max_wall_seconds
            .map(|m| (m - spend.elapsed_seconds).max(0.0)),
    );
    budget
}

/// Where an agent writes (`workflows` → Write isolation default).
enum Isolation {
    /// `isolation: "none"`, or an agent that cannot edit: the owner's checkout.
    InPlace,
    /// `isolation: "worktree"`: its own managed worktree.
    Worktree,
    /// An edit-capable agent without a choice: a worktree when another such agent of any
    /// run in the Location is in flight when it starts. The slot counts it until the call
    /// settles.
    Auto { _slot: WriterSlot },
}

/// Counts an issued, unsettled edit-capable agent; calls issued together (a `parallel()`
/// fan-out) are all counted before the first one starts.
struct WriterSlot(Arc<AtomicUsize>);

impl WriterSlot {
    fn new(writers: &Arc<AtomicUsize>) -> Self {
        writers.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(writers))
    }
}

impl Drop for WriterSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Runtime {
    /// The writer count of a Location, shared by every run there; keyed by the canonical
    /// directory so two spellings of one checkout count together.
    pub(super) fn workflow_writers(&self, directory: &str) -> Arc<AtomicUsize> {
        let key = super::workflow_recovery::location_key(directory);
        let mut writers = self
            .inner
            .workflow_writers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A count no run holds is recreated on demand.
        writers.retain(|_, count| Arc::strong_count(count) > 1);
        Arc::clone(writers.entry(key).or_default())
    }
}

impl RunHost {
    pub(super) fn new(
        runtime: &Runtime,
        run: &WorkflowRun,
        owner: TurnContext,
        tiers: BTreeMap<String, Option<String>>,
        replay: Replay,
        control: Arc<Control>,
        started: u64,
    ) -> Self {
        Self {
            store: Arc::clone(&runtime.inner.store),
            log: runtime.run_log(),
            results: runtime.workflow_dir(),
            tools: Arc::clone(&runtime.inner.tools),
            runtime: runtime.downgrade(),
            scheduler: runtime.workflow_scheduler(),
            share: run.share.map_or(usize::MAX, |share| share as usize),
            handle: tokio::runtime::Handle::current(),
            control,
            run_id: run.id.clone(),
            owner,
            interactive: run.interactive,
            args: run.args.clone(),
            solve_base: (run.origin == json!({"kind":"bundled", "name":"solve"})
                && cyber_workflow::bundled::module("solve")
                    .is_some_and(|module| module.digest == run.digest))
            .then(|| {
                run.args["solve_admission"]["base"]["commit"]
                    .as_str()
                    .map(str::to_owned)
            })
            .flatten(),
            tiers,
            budget: run_budget(run),
            replay,
            ordinals: Default::default(),
            phases: RefCell::new(run.phases.clone()),
            phase: RefCell::new(run.phase.clone()),
            fatal: Default::default(),
            started: Cell::new(started),
            in_flight: Cell::new(0),
            exceeded: Default::default(),
            halted: Cell::new(false),
            writers: runtime.workflow_writers(&run.directory),
            budget_view: RefCell::new(Value::Null),
            records: RecordCount::of(&runtime.inner.store, &run.id),
            warmup: runtime.workflow_warmup().then(Groups::default),
            wall_rejected: Cell::new(false),
        }
        .with_initial_budget()
    }

    fn with_initial_budget(mut self) -> Self {
        if self.solve_base.is_some()
            && let Some(directory) = self.args["solve_admission"]["base"]["directory"].as_str()
        {
            self.owner.directory = directory.to_owned();
        }
        let view = self
            .replay
            .initial_budget()
            .unwrap_or_else(|| self.live_budget());
        *self.budget_view.borrow_mut() = view;
        self
    }

    /// The run's spend now, as `ctx.budget` shows it.
    fn live_budget(&self) -> Value {
        self.pricing_snapshot(budget_snapshot(
            &self.budget,
            &self.spend(),
            self.started.get(),
        ))
    }

    fn spend(&self) -> RunSpend {
        super::budget::run_spend(&self.store, &self.run_id).unwrap_or_default()
    }

    /// Append one run event within the run's record bounds.
    fn append(&self, kind: &str, data: Value) -> Result<(), Diagnostic> {
        self.records.charge(data.to_string().len())?;
        self.log
            .append(&self.run_id, kind, data)
            .map_err(Diagnostic::execution)
    }

    /// The attempt is over: cancel calls it abandoned.
    pub(super) fn end(&self) {
        self.scheduler.remove(&self.run_id);
        if self.control.signal() == Signal::Running {
            self.control.cancel.cancel();
        }
    }

    pub(super) fn phases(&self) -> Vec<String> {
        self.phases.borrow().clone()
    }

    pub(super) fn phase_title(&self) -> Option<String> {
        self.phase.borrow().clone()
    }

    pub(super) fn halted(&self) -> bool {
        self.halted.get()
    }

    pub(super) fn exceeded(&self) -> Option<String> {
        self.exceeded.borrow().clone()
    }

    pub(super) fn results_dir(&self) -> Option<&std::path::Path> {
        self.results.as_deref()
    }

    /// The server runtime, unless it is gone.
    pub(super) fn runtime(&self) -> Option<Runtime> {
        self.runtime.upgrade()
    }

    /// The run's terminal record is written: hooks and the summary run on the server.
    pub(super) fn ended(&self, run: WorkflowRun) {
        if let Some(runtime) = self.runtime.upgrade() {
            self.handle
                .spawn(async move { runtime.workflow_run_ended(run).await });
        }
    }

    /// End the attempt without letting the script observe why.
    fn fail_closed(&self, error: Diagnostic) -> Diagnostic {
        self.fatal.borrow_mut().get_or_insert(error.clone());
        error
    }

    /// End the attempt for pause, stop or shutdown.
    fn halt(&self) -> Diagnostic {
        self.halted.set(true);
        self.fail_closed(Diagnostic::execution("the workflow run was halted"))
    }

    fn prepare(&self, call: AgentCall, call_key: String) -> Result<Prepared, Diagnostic> {
        let options = &call.options;
        if let Some((name, reason)) = UNAVAILABLE_OPTIONS
            .iter()
            .find(|(name, _)| options.contains_key(*name))
        {
            return Err(Diagnostic::execution(format!(
                "agent() option {name} is not available in this build: {reason}"
            )));
        }
        let agent = string_option(options, "agent")?.map(str::to_owned);
        let isolation = self.isolation(string_option(options, "isolation")?, agent.as_deref())?;
        let schema = match options.get("schema") {
            None => None,
            Some(schema) => {
                super::StructuredSchema::new(schema.clone())
                    .map_err(|e| Diagnostic::execution(format!("agent() option schema: {e}")))?;
                Some(schema.clone())
            }
        };
        let requested = string_option(options, "model")?;
        let model = self.model(requested)?;
        let goal = goal_option(options)?;
        Ok(Prepared {
            record: WorkflowAgent {
                id: String::new(),
                run_id: self.run_id.clone(),
                call_key,
                session_id: None,
                phase: call.phase.clone(),
                label: string_option(options, "label")?.map(str::to_owned),
                model,
                tier: requested.filter(|r| !r.contains('/')).map(str::to_owned),
                branch: None,
                status: "running".into(),
                result: None,
                error: None,
                cost: 0.0,
                tokens: 0,
                started_ms: None,
                ended_ms: None,
                compactions: 0,
            },
            agent,
            prompt: call.prompt,
            schema,
            isolation,
            goal,
        })
    }

    fn isolation(
        &self,
        choice: Option<&str>,
        agent: Option<&str>,
    ) -> Result<Isolation, Diagnostic> {
        match choice {
            Some("none") => Ok(Isolation::InPlace),
            Some("worktree") => Ok(Isolation::Worktree),
            Some(other) => Err(Diagnostic::execution(format!(
                "agent() option isolation must be \"none\" or \"worktree\", not {other:?}"
            ))),
            None if self.owner.mode != "plan"
                && self
                    .tools
                    .workflow_agent_writes(&self.owner.directory, agent) =>
            {
                Ok(Isolation::Auto {
                    _slot: WriterSlot::new(&self.writers),
                })
            }
            None => Ok(Isolation::InPlace),
        }
    }

    /// Whether a new agent gets its own worktree: chosen, or another edit-capable agent
    /// without a choice, of this run or another, is in flight in the same Location.
    fn isolates(&self, isolation: &Isolation) -> bool {
        match isolation {
            Isolation::InPlace => false,
            Isolation::Worktree => true,
            Isolation::Auto { .. } => self.writers.load(Ordering::SeqCst) > 1,
        }
    }

    /// A model reference (`provider/model`) passes through; anything else names a tier.
    fn model(&self, requested: Option<&str>) -> Result<Option<String>, Diagnostic> {
        let Some(requested) = requested else {
            return Ok(None);
        };
        if requested.contains('/') {
            return Ok(Some(requested.to_owned()));
        }
        self.tiers.get(requested).cloned().ok_or_else(|| {
            Diagnostic::new(
                DiagnosticKind::ModelUnavailable,
                format!("unknown model tier {requested:?}"),
            )
        })
    }

    /// `sha256(prompt + canonical options)` and the ordinal of that key in this run.
    /// Checkpoints hash their name in a separate namespace.
    fn call_key(&self, kind: &str, text: &str, options: &Map<String, Value>) -> String {
        let mut hash = Sha256::new();
        if kind != "agent" {
            hash.update(kind.as_bytes());
            hash.update([0]);
        }
        hash.update(text.as_bytes());
        hash.update([0]);
        hash.update(
            canonical(&Value::Object(options.clone()))
                .to_string()
                .as_bytes(),
        );
        let key = format!("{:x}", hash.finalize());
        let mut ordinals = self.ordinals.borrow_mut();
        let ordinal = ordinals.entry(key.clone()).or_insert(0);
        *ordinal += 1;
        format!("{key}:{ordinal}")
    }

    /// Deliver a recorded result in its recorded order, with its recorded `ctx.budget`.
    async fn replayed(&self, index: usize, outcome: Outcome) -> Result<Value, Diagnostic> {
        self.replay
            .turn(index)
            .await
            .map_err(|error| self.fail_closed(error))?;
        let view = self
            .replay
            .delivered_budget(index)
            .unwrap_or_else(|| self.live_budget());
        *self.budget_view.borrow_mut() = view;
        outcome.into_result()
    }

    /// A call the host refused: recorded and delivered after the replay prefix, like a
    /// failed call, so a resume reaches it in the same place.
    async fn serve_rejected(
        &self,
        kind: &str,
        seq: u64,
        key: &str,
        error: Diagnostic,
    ) -> Result<Value, Diagnostic> {
        self.replay
            .after_prefix()
            .await
            .map_err(|error| self.fail_closed(error))?;
        self.record_call(kind, seq, key, Outcome::Error(error))
    }

    /// An agent call after the replay prefix: reuse, resume or spawn.
    async fn serve_agent(&self, mut prepared: Prepared, seq: u64) -> Result<Value, Diagnostic> {
        self.replay
            .after_prefix()
            .await
            .map_err(|error| self.fail_closed(error))?;
        let key = prepared.record.call_key.clone();
        if let Some(outcome) = self.replay.reusable(&key) {
            return self.record_call("agent", seq, &key, outcome);
        }
        let resumed = self.replay.unsettled(&key);
        let role = self.warm_role(&prepared, resumed.is_none());
        if let Some(Role::Follower(group)) = &role {
            // Paused or stopped meanwhile: `slot` ends the attempt.
            futures::future::select(Box::pin(group.released()), Box::pin(self.signalled())).await;
        }
        let release = match role {
            Some(Role::Leader(group)) => Some(Release(group)),
            _ => None,
        };
        let slot = self.slot().await?;
        if resumed.is_none()
            && let Err(error) = self.charge_agent()
        {
            return self.record_call("agent", seq, &key, Outcome::Error(error));
        }
        prepared.record.id = resumed
            .clone()
            .unwrap_or_else(|| cyber_core::ids::new_id("agt"));
        // Keep the planned checkout even if the interruption preceded child creation.
        // The tool host ignores this plan when it resumes an existing child, but needs
        // it when no child was created; otherwise that fallback would run in the owner.
        let mut worktree = match resumed.as_deref() {
            Some(id) => self.recorded_worktree(id, &key)?,
            None => self
                .isolates(&prepared.isolation)
                .then(|| RunWorktree::new(&self.run_id, self.started.get())),
        };
        if resumed.is_some()
            && matches!(prepared.isolation, Isolation::Worktree)
            && worktree.is_none()
        {
            return Err(failed("the isolated agent has no recorded worktree plan"));
        }
        if let Some(tree) = &mut worktree {
            tree.base = self.solve_base.clone();
            tree.common_dir = self.solve_base.as_ref().and_then(|_| {
                self.args["solve_admission"]["base"]["common_dir"]
                    .as_str()
                    .map(str::to_owned)
            });
        }
        prepared.record.branch = worktree.as_ref().map(|tree| tree.branch.clone());
        self.record_agent(AGENT_STARTED, &prepared.record, seq, None)?;
        if let Some(release) = &release {
            self.watch_first_response(&prepared.record.id, release);
        }
        self.in_flight.set(self.in_flight.get() + 1);
        let outcome = self
            .spawn(&prepared, resumed.is_some(), worktree, slot)
            .await;
        self.in_flight.set(self.in_flight.get() - 1);
        self.control.changed.notify_waiters();
        if self.control.cancel.is_cancelled() && self.control.signal() != Signal::Running {
            // Stopped or shutting down: leave the agent unsettled for a resume.
            return Err(self.halt());
        }
        let outcome = self.budget_failure(outcome);
        self.settle(prepared, seq, outcome)
    }

    /// Recover the original naming ordinal from the durable, host-generated branch.
    /// A resumed call must never pick a new checkout or lose its isolation choice.
    fn recorded_worktree(&self, id: &str, key: &str) -> Result<Option<RunWorktree>, Diagnostic> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or_else(|| failed("the server is shutting down"))?;
        let record = runtime
            .workflow_agents(&self.run_id)
            .map_err(|error| failed(error.to_string()))?
            .into_iter()
            .find(|record| record.id == id && record.call_key == key)
            .ok_or_else(|| failed("the resumed agent has no matching start record"))?;
        let Some(branch) = record.branch else {
            return Ok(None);
        };
        let index = branch
            .rsplit('/')
            .next()
            .and_then(|index| index.parse::<u64>().ok())
            .filter(|index| *index > 0)
            .ok_or_else(|| failed("the recorded worktree branch is invalid"))?;
        let tree = RunWorktree::new(&self.run_id, index);
        if tree.branch != branch {
            return Err(failed("the recorded worktree belongs to another run"));
        }
        Ok(Some(tree))
    }

    /// The call's place in its prompt-prefix group; a resumed agent continues at once.
    fn warm_role(&self, prepared: &Prepared, fresh: bool) -> Option<Role> {
        let groups = self.warmup.as_ref().filter(|_| fresh)?;
        Some(groups.join(
            prepared.agent.as_deref(),
            prepared.record.model.as_deref(),
            prepared.schema.as_ref(),
        ))
    }

    /// Release a leader's group at its child's first provider response. Subscribed before
    /// the child starts, so its first output is not missed.
    fn watch_first_response(&self, agent_id: &str, release: &Release) {
        let Some(runtime) = self.runtime.upgrade() else {
            return;
        };
        if release.0.is_released() {
            return;
        }
        let events = runtime.inner.bus.subscribe();
        self.handle
            .spawn(workflow_warmup::release_on_first_response(
                events,
                Arc::clone(&self.store),
                self.owner.session_id.clone(),
                agent_id.to_owned(),
                Arc::clone(&release.0),
            ));
    }

    /// Wait while the run is pausing; a paused run with nothing in flight ends.
    async fn admit(&self) -> Result<(), Diagnostic> {
        loop {
            let changed = self.control.changed.notified();
            futures::pin_mut!(changed);
            changed.as_mut().enable();
            match self.control.signal() {
                Signal::Running => return Ok(()),
                Signal::Pausing if self.in_flight.get() > 0 => changed.await,
                _ => return Err(self.halt()),
            }
        }
    }

    /// A slot from the server-wide fair scheduler, before the agent is recorded as started.
    /// A run paused or stopped while the call waited for it starts nothing: the call leaves
    /// the queue at once and stays unrecorded, so a resume serves it.
    async fn slot(&self) -> Result<Slot, Diagnostic> {
        self.admit().await?;
        let waiter = self.scheduler.enqueue(&self.run_id, self.share);
        let interrupted = futures::future::select(
            Box::pin(self.control.cancel.cancelled()),
            Box::pin(self.signalled()),
        );
        if let Either::Left((granted, _)) = futures::future::select(waiter, interrupted).await {
            let running = self.control.signal() == Signal::Running;
            match granted {
                Ok(slot) if running => return Ok(slot),
                Err(_) if running => return Err(failed("the workflow agent queue closed")),
                _ => {}
            }
        }
        // Paused, stopped or closing while queued: let the agents in flight of a paused run
        // finish, then end the attempt.
        self.admit().await?;
        Err(self.halt())
    }

    /// Completes once the run is no longer running: paused, stopped or closing.
    async fn signalled(&self) {
        loop {
            let changed = self.control.changed.notified();
            futures::pin_mut!(changed);
            changed.as_mut().enable();
            if self.control.signal() != Signal::Running {
                return;
            }
            changed.await;
        }
    }

    /// Count a new agent against the run budget, before it starts.
    fn charge_agent(&self) -> Result<(), Diagnostic> {
        if let Some(limit) = self.exceeded() {
            return Err(budget_exceeded(&limit));
        }
        let (_, blocked) = super::budget::check_run(
            &self.store,
            &self.run_id,
            self.started.get(),
            self.budget.max_agents,
        )
        .map_err(|error| self.fail_closed(Diagnostic::execution(error.to_string())))?;
        if let Some(limit) = blocked {
            *self.exceeded.borrow_mut() = Some(limit.clone());
            return Err(budget_exceeded(&limit));
        }
        self.started.set(self.started.get() + 1);
        Ok(())
    }

    /// A child stopped by the run budget (checked before each of its provider Turns)
    /// fails the call with `BudgetExceeded`.
    fn budget_failure(
        &self,
        outcome: Result<WorkflowAgentOutput, Diagnostic>,
    ) -> Result<WorkflowAgentOutput, Diagnostic> {
        let Err(error) = outcome else {
            return outcome;
        };
        let limit = super::budget::check_run(&self.store, &self.run_id, 0, u64::MAX)
            .ok()
            .and_then(|(_, blocked)| blocked);
        match limit {
            Some(limit) => {
                self.exceeded.borrow_mut().get_or_insert(limit.clone());
                Err(budget_exceeded(&limit))
            }
            None => Err(error),
        }
    }

    /// Run the child on the tokio runtime. Dropping the returned future (the attempt
    /// ended) cancels the child; the task's lease keeps the run busy until it returns.
    async fn spawn(
        &self,
        prepared: &Prepared,
        resume: bool,
        worktree: Option<RunWorktree>,
        slot: Slot,
    ) -> Result<WorkflowAgentOutput, Diagnostic> {
        let cancel = self.control.cancel.child_token();
        let _stop = cancel.clone().drop_guard();
        let lease = self.control.lease();
        let record = &prepared.record;
        let prompt = if resume {
            format!("{CONTINUE}{}", prepared.prompt)
        } else {
            prepared.prompt.clone()
        };
        let goal = prepared.goal.clone().map(|mut goal| {
            goal.budget = Some(capped_goal_budget(
                goal.budget,
                &self.budget.budget,
                &self.spend(),
            ));
            serde_json::to_value(goal).expect("goals serialize")
        });
        let request = (
            record.id.clone(),
            record.call_key.clone(),
            record.model.clone(),
            record.label.clone(),
            prompt,
            prepared.agent.clone(),
            prepared.schema.clone(),
        );
        let (tools, runtime) = (Arc::clone(&self.tools), self.runtime.clone());
        let (mut owner, run_id) = (self.owner.clone(), self.run_id.clone());
        if self.solve_base.is_some() && record.label.as_deref() == Some("solve judge") {
            owner.mode = "plan".into();
        }
        let task = self.handle.spawn(async move {
            let _lease = lease;
            let (agent_id, call_key, model, label, prompt, agent, schema) = request;
            if resume {
                reopen(&runtime, &owner.session_id, &agent_id).await?;
            }
            let request = WorkflowAgentRequest {
                run_id,
                agent_id,
                call_key,
                owner,
                prompt,
                agent,
                model,
                schema,
                label,
                resume,
                worktree,
                goal,
                slot,
            };
            tools.spawn_workflow_agent(request, cancel).await
        });
        task.await
            .unwrap_or_else(|error| Err(failed(format!("workflow agent task ended: {error}"))))
    }

    /// Persist the settlement before the script can observe it.
    fn settle(
        &self,
        prepared: Prepared,
        seq: u64,
        outcome: Result<WorkflowAgentOutput, Diagnostic>,
    ) -> Result<Value, Diagnostic> {
        let mut record = prepared.record;
        let result = match outcome {
            Ok(output) => {
                record.session_id = Some(output.session_id.clone());
                record.branch = output.branch.clone().or(record.branch);
                record.cost = output.cost;
                record.tokens = output.tokens;
                Ok(script_value(output, prepared.schema.is_some()))
            }
            Err(error) => Err(error),
        };
        record.status = if result.is_ok() {
            "completed"
        } else {
            "failed"
        }
        .into();
        record.result = result.as_ref().ok().cloned();
        record.error = result.as_ref().err().cloned();
        self.record_agent(AGENT_SETTLED, &record, seq, Some(Outcome::of(&result)))?;
        result
    }

    fn record_agent(
        &self,
        kind: &str,
        record: &WorkflowAgent,
        seq: u64,
        outcome: Option<Outcome>,
    ) -> Result<(), Diagnostic> {
        let mut data = serde_json::to_value(record).expect("agents serialize");
        data["revision"] = self.replay.revision.into();
        data["seq"] = seq.into();
        if let Some(outcome) = outcome {
            data["order"] = self.replay.take_order().into();
            data["outcome"] = serde_json::to_value(outcome).expect("outcomes serialize");
            data["budget_snapshot"] = self.deliver_budget(&self.spend());
        }
        self.append(kind, data)
            .map_err(|error| self.fail_closed(error))
    }

    /// Record a host call that spawned no agent and deliver its outcome. Past the
    /// wall-clock budget, a call other than `agent()` (whose start checks the budget
    /// itself) is rejected with `BudgetExceeded`, so a script without agents ends too.
    fn record_call(
        &self,
        kind: &str,
        seq: u64,
        call_key: &str,
        outcome: Outcome,
    ) -> Result<Value, Diagnostic> {
        let spend = self.spend();
        let outcome = match kind {
            "agent" => outcome,
            _ => self.within_wall(&spend, outcome)?,
        };
        let data = json!({
            "kind": kind,
            "revision": self.replay.revision,
            "seq": seq,
            "order": self.replay.take_order(),
            "call_key": call_key,
            "outcome": outcome,
            "budget_snapshot": self.deliver_budget(&spend),
        });
        self.append(CALL_RECORDED, data)
            .map_err(|error| self.fail_closed(error))?;
        outcome.into_result()
    }

    /// `outcome`, unless the run is past its wall-clock budget: then the first such call
    /// rejects with `BudgetExceeded` (the script may still return a partial result) and any
    /// later one ends the run closed.
    fn within_wall(&self, spend: &RunSpend, outcome: Outcome) -> Result<Outcome, Diagnostic> {
        let over = self
            .budget
            .budget
            .max_wall_seconds
            .is_some_and(|cap| spend.elapsed_seconds >= cap);
        if !over {
            return Ok(outcome);
        }
        if self.wall_rejected.replace(true) {
            return Err(self.fail_closed(budget_exceeded(WALL)));
        }
        // Records the budget signal on the run, as an agent start past the limit does.
        let _ = super::budget::check_run(&self.store, &self.run_id, 0, u64::MAX);
        self.exceeded.borrow_mut().get_or_insert(WALL.to_owned());
        Ok(Outcome::Error(budget_exceeded(WALL)))
    }

    /// The `ctx.budget` a result reaching the script now carries, recorded with it.
    fn deliver_budget(&self, spend: &RunSpend) -> Value {
        let view = self.pricing_snapshot(budget_snapshot(&self.budget, spend, self.started.get()));
        *self.budget_view.borrow_mut() = view.clone();
        view
    }

    fn pricing_snapshot(&self, mut value: Value) -> Value {
        let run = self.run_id.clone();
        let (unpriced, complete) = self.store.read(move |db| {
            db.query_row("SELECT EXISTS(SELECT 1 FROM session WHERE workflow_run_id=?1 AND (unpriced_steps>0 OR children_unpriced_steps>0)), NOT EXISTS(SELECT 1 FROM session WHERE workflow_run_id=?1 AND children_usage_complete=0)", [&run], |row| Ok((row.get::<_,bool>(0)?,row.get::<_,bool>(1)?))).map_err(Into::into)
        }).unwrap_or((true,false));
        value["unpriced"] = unpriced.into();
        value["usage_complete"] = complete.into();
        value["cost_complete"] = (complete && !unpriced).into();
        value
    }

    /// A direct tool call after the replay prefix: a recorded result of the same call
    /// (same tool, input and ordinal) is reused, so a resumed run sees the files as they
    /// were; otherwise the tool runs for the owner on the tokio runtime. A stop or shutdown
    /// meanwhile ends the attempt without recording the call.
    async fn serve_tool(
        &self,
        seq: u64,
        key: String,
        tool: String,
        input: Value,
    ) -> Result<Value, Diagnostic> {
        self.replay
            .after_prefix()
            .await
            .map_err(|error| self.fail_closed(error))?;
        if let Some(outcome) = self.replay.reusable(&key) {
            return self.record_call("tool", seq, &key, outcome);
        }
        let cancel = self.control.cancel.child_token();
        let _stop = cancel.clone().drop_guard();
        let lease = self.control.lease();
        let request = WorkflowToolRequest {
            run_id: self.run_id.clone(),
            call_key: key.clone(),
            owner: self.owner.clone(),
            tool,
            input,
            interactive: self.interactive,
        };
        let tools = Arc::clone(&self.tools);
        let runtime = self.runtime.clone();
        let outcome = self
            .handle
            .spawn(async move {
                let _lease = lease;
                if request.tool == "__solve" {
                    let runtime = runtime
                        .upgrade()
                        .ok_or_else(|| failed("runtime unavailable"))?;
                    runtime.verify_solve_candidate(request, cancel).await
                } else {
                    tools.workflow_tool(request, cancel).await
                }
            })
            .await
            .unwrap_or_else(|error| Err(failed(format!("workflow tool task ended: {error}"))));
        if self.control.cancel.is_cancelled() && self.control.signal() != Signal::Running {
            return Err(self.halt());
        }
        self.record_call("tool", seq, &key, Outcome::of(&outcome))
    }

    /// An `ask()` after the replay prefix: its recorded answer, an answer `args.answers`
    /// declares, `NoInteractiveUser` in a run without a user, else the owner's answer.
    async fn serve_ask(
        &self,
        seq: u64,
        key: String,
        question: ScriptQuestion,
    ) -> Result<Value, Diagnostic> {
        self.replay
            .after_prefix()
            .await
            .map_err(|error| self.fail_closed(error))?;
        if let Some(outcome) = self.replay.reusable(&key) {
            return self.record_call("ask", seq, &key, outcome);
        }
        let outcome = match question.declared(&self.args) {
            Some(answer) => Outcome::Value(answer),
            None if !self.interactive => {
                Outcome::Error(no_user("no interactive user can answer ask() in this run"))
            }
            None => self.answer(question).await?,
        };
        self.record_call("ask", seq, &key, outcome)
    }

    /// Wait for the owner's answer on the tokio runtime; a pause or stop meanwhile ends
    /// the attempt.
    async fn answer(&self, question: ScriptQuestion) -> Result<Outcome, Diagnostic> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or_else(|| self.fail_closed(failed("the server is shutting down")))?;
        let (run_id, control) = (self.run_id.clone(), Arc::clone(&self.control));
        let lease = self.control.lease();
        let asked = self
            .handle
            .spawn(async move {
                let _lease = lease;
                runtime.workflow_ask(&run_id, &question, control).await
            })
            .await;
        match asked {
            Ok(Ok(Asked::Answer(answer))) => Ok(Outcome::Value(answer)),
            Ok(Ok(Asked::Unanswered(error))) => Ok(Outcome::Error(error)),
            Ok(Ok(Asked::Halted)) => Err(self.halt()),
            Ok(Err(error)) => Err(self.fail_closed(failed(error.to_string()))),
            Err(error) => Err(self.fail_closed(failed(format!("ask() task ended: {error}")))),
        }
    }

    /// A checkpoint after the replay prefix: its recorded value, else the given one.
    async fn serve_checkpoint(
        &self,
        seq: u64,
        key: String,
        value: Value,
    ) -> Result<Value, Diagnostic> {
        self.replay
            .after_prefix()
            .await
            .map_err(|error| self.fail_closed(error))?;
        let outcome = self.replay.reusable(&key).unwrap_or(Outcome::Value(value));
        self.record_call("checkpoint", seq, &key, outcome)
    }
}

/// Every call is sequenced here, when the engine dispatches it, not when its future is
/// first polled: the engine dispatches a batch of calls before polling any of them, so a
/// recorded result can wait for a call of its batch that is polled later.
impl WorkflowHost for RunHost {
    fn solve(&self, input: Value) -> HostFuture<'_> {
        if self.solve_base.is_none() {
            return Box::pin(async {
                Err(Diagnostic::new(
                    DiagnosticKind::PermissionDenied,
                    "only the embedded solve revision may request verification",
                ))
            });
        }
        self.tool("__solve".into(), input)
    }

    fn agent(&self, call: AgentCall) -> HostFuture<'_> {
        let key = self.call_key("agent", &call.prompt, &call.options);
        let plan = self.replay.issue("agent", &key);
        let prepared = self.prepare(call, key.clone());
        Box::pin(async move {
            match (plan, prepared) {
                (Plan::Mismatch(error), _) => Err(self.fail_closed(error)),
                (Plan::Replay { index, outcome }, _) => self.replayed(index, outcome).await,
                (Plan::Free { seq }, Ok(prepared)) => self.serve_agent(prepared, seq).await,
                (Plan::Free { seq }, Err(error)) => {
                    self.serve_rejected("agent", seq, &key, error).await
                }
            }
        })
    }

    fn checkpoint(&self, name: String, value: Value) -> HostFuture<'_> {
        let size = value.to_string().len();
        if size > MAX_CHECKPOINT_BYTES {
            let error = self.fail_closed(Diagnostic::execution(format!(
                "checkpoint {name:?} holds {size} bytes of JSON; the limit is {MAX_CHECKPOINT_BYTES}"
            )));
            return Box::pin(async move { Err(error) });
        }
        let key = self.call_key("checkpoint", &name, &Map::new());
        let plan = self.replay.issue("checkpoint", &key);
        Box::pin(async move {
            match plan {
                Plan::Mismatch(error) => Err(self.fail_closed(error)),
                Plan::Replay { index, outcome } => self.replayed(index, outcome).await,
                Plan::Free { seq } => self.serve_checkpoint(seq, key, value).await,
            }
        })
    }

    fn tool(&self, name: String, input: Value) -> HostFuture<'_> {
        let key = self.call_key(
            &format!("tool {name}"),
            &canonical(&input).to_string(),
            &Map::new(),
        );
        let plan = self.replay.issue("tool", &key);
        Box::pin(async move {
            match plan {
                Plan::Mismatch(error) => Err(self.fail_closed(error)),
                Plan::Replay { index, outcome } => self.replayed(index, outcome).await,
                Plan::Free { seq } => self.serve_tool(seq, key, name, input).await,
            }
        })
    }

    fn ask(&self, question: Value) -> HostFuture<'_> {
        let parsed = ScriptQuestion::parse(&question);
        let key = self.call_key("ask", &canonical(&question).to_string(), &Map::new());
        let plan = self.replay.issue("ask", &key);
        Box::pin(async move {
            match (plan, parsed) {
                (Plan::Mismatch(error), _) => Err(self.fail_closed(error)),
                (Plan::Replay { index, outcome }, _) => self.replayed(index, outcome).await,
                (Plan::Free { seq }, Ok(question)) => self.serve_ask(seq, key, question).await,
                (Plan::Free { seq }, Err(error)) => {
                    self.serve_rejected("ask", seq, &key, error).await
                }
            }
        })
    }

    fn rejected(&self, kind: &str, payload: Value, error: Diagnostic) -> HostFuture<'_> {
        let kind = kind.to_owned();
        let key = self.call_key(
            &format!("rejected {kind}"),
            &payload.to_string(),
            &Map::new(),
        );
        let plan = self.replay.issue(&kind, &key);
        Box::pin(async move {
            match plan {
                Plan::Mismatch(error) => Err(self.fail_closed(error)),
                Plan::Replay { index, outcome } => self.replayed(index, outcome).await,
                Plan::Free { seq } => self.serve_rejected(&kind, seq, &key, error).await,
            }
        })
    }

    /// A phase change. The event carries the phase list only when it adds a title, so
    /// records stay small; past [`cyber_workflow::MAX_PHASES`] titles the run fails.
    fn phase(&self, title: &str) -> Result<(), Diagnostic> {
        let added = {
            let mut phases = self.phases.borrow_mut();
            let added = !phases.iter().any(|known| known == title);
            if added && phases.len() >= cyber_workflow::MAX_PHASES {
                return Err(cyber_workflow::too_many_phases());
            }
            if added {
                phases.push(title.to_owned());
            }
            added.then(|| phases.clone())
        };
        *self.phase.borrow_mut() = Some(title.to_owned());
        if !self.replay.record_note() {
            return Ok(());
        }
        let mut data = json!({"title": title, "revision": self.replay.revision});
        if let Some(phases) = added {
            data["phases"] = json!(phases);
        }
        self.append(PHASE, data)
    }

    fn log(&self, values: Vec<Value>) -> Result<(), Diagnostic> {
        if !self.replay.record_note() {
            return Ok(());
        }
        let data = json!({"values": values, "revision": self.replay.revision});
        self.append(LOG, data)
    }

    fn fatal(&self) -> Option<Diagnostic> {
        self.fatal.borrow().clone()
    }

    fn budget(&self) -> Value {
        self.budget_view.borrow().clone()
    }
}

/// The run's budget; runs recorded without one have the defaults.
pub(super) fn run_budget(run: &WorkflowRun) -> RunBudget {
    run.budget
        .clone()
        .unwrap_or_else(|| RunBudget::resolve(&Value::Null, &[]).expect("default run budget"))
}

/// What `ctx.budget` shows: the run's spend, agents started and what remains of each
/// limit.
pub(super) fn budget_snapshot(budget: &RunBudget, spend: &RunSpend, started: u64) -> Value {
    let limits = &budget.budget;
    let mut remaining = Map::new();
    remaining.insert(
        "max_agents".into(),
        budget.max_agents.saturating_sub(started).into(),
    );
    if let Some(cap) = limits.max_cost_usd {
        remaining.insert("max_cost_usd".into(), (cap - spend.cost).max(0.0).into());
    }
    if let Some(cap) = limits.max_tokens {
        remaining.insert("max_tokens".into(), cap.saturating_sub(spend.tokens).into());
    }
    if let Some(cap) = limits.max_turns {
        remaining.insert("max_turns".into(), cap.saturating_sub(spend.turns).into());
    }
    if let Some(cap) = limits.max_wall_seconds {
        remaining.insert(
            "max_wall_seconds".into(),
            (cap - spend.elapsed_seconds).max(0.0).into(),
        );
    }
    json!({
        "spent_tokens": spend.tokens,
        "spent_cost_usd": spend.cost,
        "agents_started": started,
        "remaining": remaining,
    })
}

/// Reopen a stopped child's admission before it resumes.
async fn reopen(runtime: &WeakRuntime, owner: &str, agent_id: &str) -> Result<(), Diagnostic> {
    let runtime = runtime
        .upgrade()
        .ok_or_else(|| failed("the server is shutting down"))?;
    let child = match runtime.resolve_subagent(owner, agent_id).await {
        Ok(child) => child,
        // No child Session was created before the interruption: the host spawns one.
        Err(_) => return Ok(()),
    };
    runtime
        .reopen_stopped_child(&child.id)
        .await
        .map_err(|error| failed(error.to_string()))
}

/// What `await agent(...)` resolves to: the validated object for a schema, else
/// `{ text, session_id, cost, tokens }`. An isolated agent's result includes its `branch`
/// either way (`workflows` → Write isolation for concurrent agents); a schema's object
/// that already has a `branch` field keeps its own.
fn script_value(output: WorkflowAgentOutput, structured: bool) -> Value {
    match output.structured {
        Some(mut value) if structured => {
            if let Some(object) = value.as_object_mut() {
                let extras = [
                    ("branch", output.branch),
                    ("goal_status", output.goal_status),
                ];
                for (key, extra) in extras {
                    if let Some(extra) = extra {
                        object.entry(key).or_insert(extra.into());
                    }
                }
            }
            value
        }
        _ => {
            let mut value = json!({
                "text": output.text,
                "session_id": output.session_id,
                "cost": output.cost,
                "tokens": output.tokens,
            });
            if let Some(branch) = output.branch {
                value["branch"] = branch.into();
            }
            if let Some(status) = output.goal_status {
                value["goal_status"] = status.into();
            }
            value
        }
    }
}

fn string_option<'a>(
    options: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, Diagnostic> {
    match options.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(Diagnostic::execution(format!(
            "agent() option {key} must be a string"
        ))),
    }
}

fn failed(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::AgentFailed, message)
}

fn budget_exceeded(limit: &str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticKind::BudgetExceeded,
        format!("BudgetExceeded: {limit}"),
    )
}

/// Object keys sorted at every level, so equal options hash equally.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, Value> = map
                .iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect();
            Value::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_goal_budget_is_capped_by_what_the_run_has_left() {
        use cyber_core::budget::Budget;
        let run = Budget {
            max_cost_usd: Some(1.0),
            max_turns: Some(10),
            ..Budget::default()
        };
        let spend = RunSpend {
            cost: 0.75,
            turns: 4,
            ..RunSpend::default()
        };
        let own = Budget {
            max_turns: Some(20),
            max_tokens: Some(500),
            ..Budget::default()
        };
        let capped = capped_goal_budget(Some(own), &run, &spend);
        assert_eq!(capped.max_turns, Some(6));
        assert_eq!(capped.max_cost_usd, Some(0.25));
        assert_eq!(capped.max_tokens, Some(500));
        assert_eq!(capped.max_wall_seconds, None);
        let options: Map<String, Value> =
            serde_json::from_value(json!({"goal": {"condition": ""}})).unwrap();
        assert!(goal_option(&options).is_err());
        let options: Map<String, Value> =
            serde_json::from_value(json!({"goal": {"condition": "x", "bogus": 1}})).unwrap();
        assert!(goal_option(&options).is_err());
    }

    #[test]
    fn records_are_bounded_by_count_and_bytes() {
        let count = RecordCount::default();
        count.records.set(MAX_RUN_RECORDS - 1);
        count.charge(10).unwrap();
        let error = count.charge(10).unwrap_err();
        assert!(error.message.contains("more than 100000"), "{error}");
        assert_eq!(count.records.get(), MAX_RUN_RECORDS);

        let count = RecordCount::default();
        count.charge(MAX_RUN_RECORD_BYTES as usize - 1).unwrap();
        let error = count.charge(2).unwrap_err();
        assert!(error.message.contains("256 MiB"), "{error}");
        count.charge(1).unwrap();
    }

    #[test]
    fn canonical_options_ignore_key_order() {
        let a = json!({"b": 1, "a": {"y": [1, {"d": 2, "c": 3}], "x": null}});
        let b = json!({"a": {"x": null, "y": [1, {"c": 3, "d": 2}]}, "b": 1});
        assert_eq!(canonical(&a).to_string(), canonical(&b).to_string());
    }
}

#[cfg(test)]
mod script_value_tests {
    use super::*;

    fn output(structured: Option<Value>, branch: Option<&str>) -> WorkflowAgentOutput {
        WorkflowAgentOutput {
            session_id: "ses_1".into(),
            text: "done".into(),
            structured,
            cost: 0.5,
            tokens: 10,
            branch: branch.map(str::to_owned),
            goal_status: None,
        }
    }

    #[test]
    fn an_agent_with_a_goal_and_a_schema_reports_its_goal_status() {
        let mut met = output(Some(json!({"ok": true})), None);
        met.goal_status = Some("met".into());
        assert_eq!(
            script_value(met, true),
            json!({"ok": true, "goal_status": "met"})
        );
    }

    #[test]
    fn an_isolated_agent_with_a_schema_reports_its_branch() {
        let value = script_value(output(Some(json!({"ok": true})), Some("cyber/wf-1")), true);
        assert_eq!(value, json!({"ok": true, "branch": "cyber/wf-1"}));
        let own = script_value(
            output(Some(json!({"branch": "x"})), Some("cyber/wf-1")),
            true,
        );
        assert_eq!(own, json!({"branch": "x"}));
        let plain = script_value(output(Some(json!({"ok": true})), None), true);
        assert_eq!(plain, json!({"ok": true}));
        let text = script_value(output(None, Some("cyber/wf-2")), false);
        assert_eq!(text["branch"], "cyber/wf-2");
    }
}
