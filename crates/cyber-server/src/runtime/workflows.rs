//! Workflow Runs (`workflows` → Run lifecycle states, Background execution on the server,
//! Durable agent results and resume, Concurrency limit): the durable `run_` aggregate, its
//! SQL projections and background execution of a script on a dedicated engine thread.
//!
//! QuickJS values are thread-bound, so each execution attempt drives its engine with a
//! local executor on its own OS thread. Host calls cross to the tokio runtime through
//! `Handle::spawn`; the durable records (`workflow.agent.settled.1`, checkpoints, phases,
//! logs) are appended on the engine thread before the script can observe the outcome.
//! Pause, stop, resume, shutdown and recovery live in `workflow_control`; replay of
//! recorded host calls in `workflow_replay`.

use std::collections::BTreeMap;
use std::sync::Arc;

use cyber_core::budget::RunBudget;
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoredEvent};
use cyber_workflow::{Diagnostic, DiagnosticKind, Limits, RunInput, WorkflowModule};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::bus::Bus;
use super::workflow_control::{Control, Signal};
use super::workflow_host::{RunHost, budget_snapshot};
use super::workflow_queue::{
    Admission, DEFAULT_MAX_RUNNING_RUNS, Pending, Reservation, RunSlot, WorkflowQueue,
};
use super::workflow_replay::Replay;
use super::workflow_scheduler::WorkflowScheduler;
use super::{LiveEvent, Runtime, RuntimeError};

pub const RUN_STARTED: &str = "workflow.run.started.1";
pub const RUN_PAUSED: &str = "workflow.run.paused.1";
pub const RUN_RESUMED: &str = "workflow.run.resumed.1";
/// A queued run got a run slot and its engine launches (`workflows` → Run queue).
pub const RUN_DISPATCHED: &str = "workflow.run.dispatched.1";
pub const RUN_REVISED: &str = "workflow.run.revised.1";
pub const RUN_COMPLETED: &str = "workflow.run.completed.1";
pub const RUN_FAILED: &str = "workflow.run.failed.1";
pub const RUN_STOPPED: &str = "workflow.run.stopped.1";
pub const AGENT_STARTED: &str = "workflow.agent.started.1";
pub const AGENT_SETTLED: &str = "workflow.agent.settled.1";
/// A host call other than a spawned agent: checkpoints, reused agent results and
/// budget rejections (`workflows` → Replay of workflow host calls).
pub const CALL_RECORDED: &str = "workflow.call.recorded.1";
pub const PHASE: &str = "workflow.phase.1";
pub const LOG: &str = "workflow.log.1";
/// The run summary was admitted to the starting Session (`workflows` → Run results).
pub const RUN_NOTIFIED: &str = "workflow.run.notified.1";
/// `ask()` waits for the owner's answer: the run is `waiting_input` (`workflows` → ask()
/// user interaction).
pub const RUN_WAITING: &str = "workflow.run.waiting.1";
/// The answer arrived: the run is `running` again.
pub const RUN_ANSWERED: &str = "workflow.run.answered.1";
const KINDS: [&str; 16] = [
    RUN_STARTED,
    RUN_PAUSED,
    RUN_RESUMED,
    RUN_DISPATCHED,
    RUN_REVISED,
    RUN_COMPLETED,
    RUN_FAILED,
    RUN_STOPPED,
    AGENT_STARTED,
    AGENT_SETTLED,
    CALL_RECORDED,
    PHASE,
    LOG,
    RUN_NOTIFIED,
    RUN_WAITING,
    RUN_ANSWERED,
];
/// Lifecycle events that carry the whole run.
pub(super) const LIFECYCLE: [&str; 9] = [
    RUN_PAUSED,
    RUN_RESUMED,
    RUN_DISPATCHED,
    RUN_REVISED,
    RUN_COMPLETED,
    RUN_FAILED,
    RUN_STOPPED,
    RUN_WAITING,
    RUN_ANSWERED,
];
/// The refusal when a workflow agent, or a Session below one, would own a new run
/// (`workflows` → Permissions for runs).
pub const WORKFLOW_AGENT_STARTS_RUN: &str = "Workflow agents cannot start Workflow Runs";
/// `workflows.max_concurrent` before the core count is applied.
const DEFAULT_MAX_CONCURRENT: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    Queued,
    Running,
    /// Reserved until `ask()` lands.
    WaitingInput,
    Paused,
    Completed,
    Failed,
    BudgetExceeded,
    Stopped,
}

impl WorkflowRunStatus {
    /// Whether the engine may be executing (or owes a durable end) in some process.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::WaitingInput)
    }

    /// Whether the run ended: completed, failed, budget-exceeded or stopped.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::BudgetExceeded | Self::Stopped
        )
    }
}

/// Who started a run (`workflows` → Permissions for runs, Run results).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RunStarter {
    /// The HTTP API or CLI: non-interactive, so an agent's `ask` is rejected, and the
    /// summary waits in the owner's queue without waking it.
    #[default]
    Api,
    /// The owner Session's `workflow` tool: the owner continues when the summary arrives,
    /// and agents may ask its attached clients when `interactive`.
    Session { interactive: bool },
}

/// A Workflow Run as clients see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowRun {
    pub id: String,
    /// `meta.name` of the script.
    pub name: String,
    /// The owner Session: agents are its children and run under its Mode.
    pub session_id: String,
    pub directory: String,
    /// The run Mode, a ceiling for every agent.
    pub mode: String,
    /// `inline`, or the saved module's path.
    pub origin: Value,
    /// An inline script's copy at `<state>/workflows/<run_id>.js`, to read, edit, relaunch
    /// or save into `.cyber/workflows/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_file: Option<String>,
    /// Direct read-only tools the invocation allowlists besides `meta.tools`
    /// (`workflows` → Direct read-only tools).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// `sha256:` digest of the current revision's script source.
    pub digest: String,
    /// Digest of the engine limits and model tiers the current revision runs with.
    #[serde(default)]
    pub config_digest: String,
    /// Script revision, starting at 1; a relaunch with a changed script or configuration
    /// needs a new revision.
    #[serde(default = "first_revision")]
    pub revision: u32,
    pub args: Value,
    pub status: WorkflowRunStatus,
    /// Why the run is paused, stopped or ended early.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<RunBudget>,
    /// Agents of this run that may hold a slot of the server-wide limit at once: its
    /// `max_concurrent`, capped at that limit (`workflows` → Fair agent scheduling).
    /// Absent on runs recorded before shares existed, which use the whole limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<u32>,
    /// The `max_concurrent` the start or latest resume gave itself, before `meta` and the
    /// configuration: a resume that gives none keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
    /// When a `queued` run entered the run queue: the time of the start or resume that
    /// queued it. Dispatch goes by it, then by run ID (`workflows` → Run queue).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_ms: Option<i64>,
    /// A `queued` run's place in the run queue, 1 for the next to be dispatched; computed
    /// when the run is read, never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_position: Option<u32>,
    /// Declared phases followed by titles first seen in `phase()`.
    pub phases: Vec<String>,
    /// The current phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// The script's return value once completed, when its JSON is at most 1 MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// A larger return value's file (`<state>/workflows/<run_id>.result.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_file: Option<String>,
    /// The start of a larger return value's JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_preview: Option<String>,
    /// Agents may ask the owner's attached clients; otherwise every `ask` is rejected
    /// with `No interactive approver for workflow run`.
    #[serde(default)]
    pub interactive: bool,
    /// The owner continues when the summary is admitted (runs started by its `workflow`
    /// tool); otherwise the summary waits in its queue.
    #[serde(default)]
    pub continue_owner: bool,
    /// The summary was admitted to the owner as `msg_workflow_<run_id>`.
    #[serde(default)]
    pub notified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Diagnostic>,
    pub started_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
}

fn first_revision() -> u32 {
    1
}

/// One `agent()` call (`workflow.agent.started.1`, then `workflow.agent.settled.1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowAgent {
    /// `agt_` ID; also the child Session's subagent name.
    pub id: String,
    pub run_id: String,
    /// `sha256(prompt + canonical options)` plus the ordinal of that key in the run.
    pub call_key: String,
    /// The child Session, when one was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The resolved model reference; `None` inherits the owner's model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The tier named by the call's `model` option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// The branch of the agent's own worktree (`workflows` → Write isolation default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// `running`, `completed`, `failed`, `cancelled` (stopped) or `interrupted` (the
    /// server stopped; a resume continues its child Session).
    pub status: String,
    /// The value the script received.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Diagnostic>,
    pub cost: f64,
    pub tokens: u64,
    /// When the agent started, from its first `workflow.agent.started.1`; set by the
    /// projection, never recorded in the event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<i64>,
    /// When the agent settled; absent while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
}

/// One `log()` call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowLogEntry {
    pub seq: i64,
    pub time_ms: i64,
    pub values: Vec<Value>,
}

/// What a run is started from.
pub struct StartWorkflowRun {
    pub module: WorkflowModule,
    pub args: Value,
    /// The owner Session.
    pub session_id: String,
    pub limits: Limits,
    /// Tier name → resolved model reference; `None` inherits the owner's model.
    pub tiers: BTreeMap<String, Option<String>>,
    /// Resolved from `budgets.run`, `meta.budget` and the invocation.
    pub budget: RunBudget,
    /// The run's share of the agent slots ([`super::workflow_start::share`]).
    pub share: RunShare,
    pub starter: RunStarter,
    /// Direct read-only tools the invocation allowlists besides `meta.tools`
    /// ([`super::workflow_start::invocation_tools`]).
    pub tools: Vec<String>,
}

/// A run's share of the agent slots as a start or resume resolved it (`workflows` → Fair
/// agent scheduling).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunShare {
    /// The invocation's own `max_concurrent`, recorded on the run so that a resume giving
    /// none keeps it.
    pub requested: Option<u32>,
    /// `requested`, else `meta.max_concurrent`, else `workflows.max_concurrent_per_run`;
    /// `None` for the whole server-wide limit.
    pub max_concurrent: Option<u32>,
}

impl RunShare {
    /// The share an invocation asked for, with no `meta` or configuration below it.
    pub fn requested(max_concurrent: Option<u32>) -> Self {
        Self {
            requested: max_concurrent,
            max_concurrent,
        }
    }
}

/// The engine settings an execution attempt uses.
pub(super) struct Engine {
    pub(super) module: WorkflowModule,
    pub(super) limits: Limits,
    pub(super) tiers: BTreeMap<String, Option<String>>,
}

/// Digest of the configuration a revision runs with: engine limits and model tiers.
pub fn config_digest(limits: &Limits, tiers: &BTreeMap<String, Option<String>>) -> String {
    let text = json!({
        "max_memory_bytes": limits.max_memory_bytes,
        "script_cpu_ms": limits.script_cpu.as_millis() as u64,
        "tiers": tiers,
    })
    .to_string();
    format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
}

pub(super) fn register(registry: &mut EventRegistry) {
    for kind in KINDS {
        registry.register(kind).expect("valid workflow event");
    }
}

pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> rusqlite::Result<()> {
    let kind = event.kind.as_str();
    match kind {
        RUN_STARTED => project_started(tx, event),
        PHASE => {
            // `phases` is present only when the change added a title.
            let data = &event.data;
            tx.execute(
                "UPDATE workflow_run SET data = json_set(data, '$.phase', ?2) WHERE id = ?1",
                params![event.aggregate_id, data["title"].as_str()],
            )?;
            if let Some(phases) = data.get("phases") {
                tx.execute(
                    "UPDATE workflow_run SET data = json_set(data, '$.phases', json(?2)) WHERE id = ?1",
                    params![event.aggregate_id, phases.to_string()],
                )?;
            }
            Ok(())
        }
        AGENT_STARTED | AGENT_SETTLED => project_agent(tx, event),
        RUN_NOTIFIED => {
            tx.execute(
                "UPDATE workflow_run SET data = json_set(data, '$.notified', json('true')) WHERE id = ?1",
                params![event.aggregate_id],
            )?;
            Ok(())
        }
        CALL_RECORDED => project_call(tx, event, None),
        _ if LIFECYCLE.contains(&kind) => project_lifecycle(tx, event),
        _ => Ok(()),
    }
}

fn project_started(tx: &Transaction<'_>, event: &StoredEvent) -> rusqlite::Result<()> {
    let data = &event.data;
    tx.execute(
        "INSERT INTO workflow_run (id, session_id, name, status, started_ms, data) VALUES (?1, ?2, ?3, ?4, ?5, json_remove(?6, '$.source', '$.budget_snapshot'))",
        params![
            event.aggregate_id,
            data["session_id"].as_str(),
            data["name"].as_str(),
            data["status"].as_str(),
            data["started_ms"].as_i64(),
            data.to_string()
        ],
    )?;
    project_revision(tx, event)?;
    if let Some(budget) = data.get("budget").filter(|b| b.is_object()) {
        let mut spec = budget.clone();
        if let Some(map) = spec.as_object_mut() {
            map.remove("max_agents");
        }
        // A queued run's wall clock starts when it is dispatched.
        let activated = (data["status"] != "queued").then_some(event.time_ms);
        tx.execute(
            "INSERT INTO workflow_run_budget (run_id, spec, activated_ms) VALUES (?1, ?2, ?3)",
            params![event.aggregate_id, spec.to_string(), activated],
        )?;
    }
    Ok(())
}

fn project_revision(tx: &Transaction<'_>, event: &StoredEvent) -> rusqlite::Result<()> {
    let data = &event.data;
    tx.execute(
        "INSERT INTO workflow_run_revision (run_id, revision, digest, config_digest, source) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            event.aggregate_id,
            data["revision"].as_i64().unwrap_or(1),
            data["digest"].as_str(),
            data["config_digest"].as_str().unwrap_or_default(),
            data["source"].as_str().unwrap_or_default()
        ],
    )?;
    Ok(())
}

fn project_lifecycle(tx: &Transaction<'_>, event: &StoredEvent) -> rusqlite::Result<()> {
    let (id, data) = (&event.aggregate_id, &event.data);
    tx.execute(
        "UPDATE workflow_run SET status = ?2, data = json_remove(?3, '$.source', '$.replay_from_order', '$.interrupted', '$.budget_snapshot') WHERE id = ?1",
        params![id, data["status"].as_str(), data.to_string()],
    )?;
    match event.kind.as_str() {
        RUN_REVISED => project_revision(tx, event)?,
        RUN_RESUMED | RUN_DISPATCHED | RUN_ANSWERED => {
            // A resume that queued the run starts its clock at dispatch; time spent
            // waiting for an answer is not running time.
            if data["status"] == "running" {
                tx.execute(
                    "UPDATE workflow_run_budget SET activated_ms = ?2 WHERE run_id = ?1",
                    params![id, event.time_ms],
                )?;
            }
            if let Some(order) = data["replay_from_order"].as_i64() {
                // Results delivered after the replay prefix are not replayed in order again.
                tx.execute(
                    "UPDATE workflow_call SET settled_order = NULL WHERE run_id = ?1 AND revision = ?2 AND settled_order >= ?3",
                    params![id, data["revision"].as_i64(), order],
                )?;
            }
        }
        _ => {
            tx.execute(
                "UPDATE workflow_run_budget SET active_ms = active_ms + MAX(0, ?2 - activated_ms), activated_ms = NULL WHERE run_id = ?1 AND activated_ms IS NOT NULL",
                params![id, event.time_ms],
            )?;
            // The run's end drops its pending agent calls, which cancels their children.
            let agents = match event.kind.as_str() {
                RUN_STOPPED | RUN_COMPLETED | RUN_FAILED => Some("cancelled"),
                RUN_PAUSED if data["interrupted"] == json!(true) => Some("interrupted"),
                _ => None,
            };
            if let Some(status) = agents {
                tx.execute(
                    "UPDATE workflow_agent SET status = ?2, data = json_set(data, '$.status', ?2) WHERE run_id = ?1 AND status = 'running'",
                    params![id, status],
                )?;
            }
        }
    }
    Ok(())
}

/// An agent's row; `started_ms` comes from its first start (a resumed agent keeps it) and
/// `ended_ms` from its settlement, both from the event time.
fn project_agent(tx: &Transaction<'_>, event: &StoredEvent) -> rusqlite::Result<()> {
    let data = &event.data;
    let time = if event.kind == AGENT_STARTED {
        "$.started_ms"
    } else {
        "$.ended_ms"
    };
    tx.execute(
        "INSERT INTO workflow_agent (id, run_id, call_key, session_id, status, settled_seq, data)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, json_set(json_remove(?7, '$.revision', '$.seq', '$.order', '$.outcome', '$.budget_snapshot', '$.started_ms', '$.ended_ms'), ?8, ?9))
         ON CONFLICT(id) DO UPDATE SET call_key = excluded.call_key,
            session_id = COALESCE(excluded.session_id, workflow_agent.session_id),
            status = excluded.status, settled_seq = excluded.settled_seq,
            data = json_set(excluded.data, '$.started_ms', COALESCE(
                json_extract(workflow_agent.data, '$.started_ms'),
                json_extract(excluded.data, '$.started_ms')))",
        params![
            data["id"].as_str(),
            event.aggregate_id,
            data["call_key"].as_str(),
            data["session_id"].as_str(),
            data["status"].as_str(),
            event.seq,
            data.to_string(),
            time,
            event.time_ms
        ],
    )?;
    project_call(tx, event, data["id"].as_str())
}

/// The `workflow_call` row of an issued host call.
fn project_call(
    tx: &Transaction<'_>,
    event: &StoredEvent,
    agent: Option<&str>,
) -> rusqlite::Result<()> {
    let data = &event.data;
    let Some(seq) = data["seq"].as_i64() else {
        return Ok(());
    };
    let settled = event.kind != AGENT_STARTED;
    let outcome = data
        .get("outcome")
        .filter(|_| settled)
        .map(Value::to_string);
    tx.execute(
        "INSERT OR REPLACE INTO workflow_call (run_id, revision, seq, kind, call_key, agent_id, status, settled_order, outcome)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            event.aggregate_id,
            data["revision"].as_i64().unwrap_or(1),
            seq,
            data["kind"].as_str().unwrap_or("agent"),
            data["call_key"].as_str(),
            agent.or(data["agent_id"].as_str()),
            if settled { "settled" } else { "started" },
            data["order"].as_i64().filter(|_| settled),
            outcome
        ],
    )?;
    Ok(())
}

/// Where run events go: the store, whose projections commit with them, and the bus that
/// streams them live on the instance event stream (`workflows` → Run monitor).
#[derive(Clone)]
pub(super) struct RunLog {
    pub(super) store: Arc<Store>,
    bus: Bus,
}

impl RunLog {
    /// Append one run event.
    pub(super) fn append(&self, run_id: &str, kind: &str, data: Value) -> Result<(), String> {
        self.commit(run_id, NewEvent::new(kind, data))
            .map_err(|error| format!("could not record {kind}: {error}"))
    }

    /// Append a lifecycle event carrying `run`, plus `extra` fields.
    pub(super) fn record(
        &self,
        run: &WorkflowRun,
        kind: &str,
        extra: Value,
    ) -> Result<(), RuntimeError> {
        let mut data = stored_run(run);
        if let (Some(data), Value::Object(extra)) = (data.as_object_mut(), extra) {
            data.extend(extra);
        }
        self.commit(&run.id, NewEvent::new(kind, data))
    }

    fn commit(&self, run_id: &str, event: NewEvent) -> Result<(), RuntimeError> {
        for stored in self.store.append(run_id, Expected::Any, vec![event])? {
            self.bus.publish(LiveEvent::Durable {
                session_id: stored.aggregate_id,
                seq: stored.seq,
                kind: stored.kind,
                data: stored.data,
            });
        }
        Ok(())
    }
}

/// A run as its events carry it: without the queue position, which is computed on read.
fn stored_run(run: &WorkflowRun) -> Value {
    let mut data = serde_json::to_value(run).expect("runs serialize");
    if let Some(map) = data.as_object_mut() {
        map.remove("queue_position");
    }
    data
}

fn rows<T: serde::de::DeserializeOwned + Send + 'static>(
    store: &Store,
    sql: &'static str,
    key: Option<String>,
) -> Result<Vec<T>, RuntimeError> {
    let texts = store.read(move |db| {
        let mut statement = db.prepare(sql)?;
        let texts = statement
            .query_map(params![key], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(texts)
    })?;
    texts
        .iter()
        .map(|text| serde_json::from_str(text).map_err(|e| RuntimeError::Corrupt(e.to_string())))
        .collect()
}

impl Runtime {
    pub(super) fn run_log(&self) -> RunLog {
        RunLog {
            store: Arc::clone(&self.inner.store),
            bus: self.inner.bus.clone(),
        }
    }

    /// Set `<state>/workflows` once, before any run starts: inline scripts and return
    /// values larger than 1 MiB are kept there. False when already set.
    pub fn configure_workflow_dir(&self, dir: std::path::PathBuf) -> bool {
        self.inner.workflow_dir.set(dir).is_ok()
    }

    /// `<state>/workflows`, when configured.
    pub fn workflow_dir(&self) -> Option<std::path::PathBuf> {
        self.inner.workflow_dir.get().cloned()
    }

    /// Set the server-wide Workflow agent limit once, before any run starts
    /// (`workflows.max_concurrent`, reduced to the core count). False when already set.
    pub fn configure_workflow_concurrency(&self, limit: usize) -> bool {
        self.inner
            .workflow_scheduler
            .set(WorkflowScheduler::new(limit.max(1)))
            .is_ok()
    }

    /// The fair scheduler of the slots shared by every Workflow agent of this server. It
    /// is separate from `agents.max_concurrent`, which bounds ordinary subagents per root
    /// Session.
    pub fn workflow_scheduler(&self) -> WorkflowScheduler {
        self.inner
            .workflow_scheduler
            .get_or_init(|| {
                let cores = std::thread::available_parallelism().map_or(1, usize::from);
                WorkflowScheduler::new(DEFAULT_MAX_CONCURRENT.min(cores))
            })
            .clone()
    }

    /// Set the run queue's limit once, before any run starts (`workflows.max_running_runs`).
    /// False when already set.
    pub fn configure_workflow_runs(&self, limit: usize) -> bool {
        self.inner
            .workflow_queue
            .set(WorkflowQueue::new(limit))
            .is_ok()
    }

    /// The run queue shared by every Workflow Run of this server.
    pub(super) fn workflow_queue(&self) -> WorkflowQueue {
        self.inner
            .workflow_queue
            .get_or_init(|| WorkflowQueue::new(DEFAULT_MAX_RUNNING_RUNS))
            .clone()
    }

    /// A free run slot for run `run_id` starting or resuming now, its release dispatching
    /// the next queued run; else its place in the run queue, entered at `queued_ms` and
    /// held from now on, so no later start or resume takes a slot ahead of it.
    pub(super) fn admit_workflow_run(&self, run_id: &str, queued_ms: i64) -> Admission {
        match self.workflow_queue().admit_or_reserve(run_id, queued_ms) {
            Admission::Slot(slot) => Admission::Slot(self.dispatching_on_release(slot)),
            Admission::Queued(place) => Admission::Queued(place.on_cancel(self.dispatch_later())),
        }
    }

    pub(super) fn dispatching_on_release(&self, slot: RunSlot) -> RunSlot {
        slot.on_release(self.dispatch_later())
    }

    /// Dispatch queued runs in the background, when called.
    fn dispatch_later(&self) -> impl FnOnce() + Send + 'static {
        let (inner, handle) = (
            Arc::downgrade(&self.inner),
            tokio::runtime::Handle::current(),
        );
        move || {
            if let Some(inner) = inner.upgrade() {
                let runtime = Runtime { inner };
                handle.spawn(async move { runtime.dispatch_workflow_runs().await });
            }
        }
    }

    /// Wait for a run slot at the place reserved for the run: it is recorded `queued` and
    /// dispatched in turn.
    pub(super) fn enqueue_workflow_run(&self, place: Reservation, pending: Pending) {
        place.fill(pending);
        // A slot may have been released since the run was refused one.
        let runtime = self.clone();
        tokio::spawn(async move { runtime.dispatch_workflow_runs().await });
    }

    /// Queued runs' IDs in dispatch order, from the durable queue times.
    fn queue_order(&self) -> Result<Vec<String>, RuntimeError> {
        Ok(self.inner.store.read(|db| {
            let mut statement = db.prepare(
                "SELECT id FROM workflow_run WHERE status = 'queued'
                 ORDER BY COALESCE(json_extract(data, '$.queued_ms'), started_ms), id",
            )?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })?)
    }

    /// Set `queue_position` on the queued runs among `runs`.
    pub(super) fn with_queue_positions(
        &self,
        mut runs: Vec<WorkflowRun>,
    ) -> Result<Vec<WorkflowRun>, RuntimeError> {
        if !runs
            .iter()
            .any(|run| run.status == WorkflowRunStatus::Queued)
        {
            return Ok(runs);
        }
        let order = self.queue_order()?;
        for run in runs
            .iter_mut()
            .filter(|run| run.status == WorkflowRunStatus::Queued)
        {
            run.queue_position = order
                .iter()
                .position(|id| *id == run.id)
                .and_then(|index| u32::try_from(index + 1).ok());
        }
        Ok(runs)
    }

    /// A run's share of the agent slots: `max_concurrent` capped at the server-wide limit,
    /// the whole limit when unset.
    pub(super) fn workflow_share(&self, max_concurrent: Option<u32>) -> u32 {
        let capacity = u32::try_from(self.workflow_scheduler().capacity()).unwrap_or(u32::MAX);
        max_concurrent.map_or(capacity, |share| share.clamp(1, capacity))
    }

    /// Runs, newest first, optionally of one owner Session.
    pub fn workflow_runs(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<WorkflowRun>, RuntimeError> {
        let runs = rows(
            &self.inner.store,
            "SELECT data FROM workflow_run WHERE ?1 IS NULL OR session_id = ?1 ORDER BY started_ms DESC, id DESC",
            session_id.map(str::to_owned),
        )?;
        self.with_queue_positions(runs)
    }

    pub fn workflow_run(&self, id: &str) -> Result<Option<WorkflowRun>, RuntimeError> {
        let id = id.to_owned();
        let text = self.inner.store.read(move |db| {
            Ok(db
                .query_row("SELECT data FROM workflow_run WHERE id = ?1", [id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?)
        })?;
        let Some(text) = text else {
            return Ok(None);
        };
        let run = serde_json::from_str(&text).map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
        Ok(self.with_queue_positions(vec![run])?.pop())
    }

    pub(super) fn existing_workflow_run(&self, id: &str) -> Result<WorkflowRun, RuntimeError> {
        self.workflow_run(id)?
            .ok_or_else(|| RuntimeError::Invalid(format!("No Workflow Run {id}")))
    }

    /// Agents of a run, in start or settlement order.
    pub fn workflow_agents(&self, run_id: &str) -> Result<Vec<WorkflowAgent>, RuntimeError> {
        rows(
            &self.inner.store,
            "SELECT data FROM workflow_agent WHERE run_id = ?1 ORDER BY settled_seq",
            Some(run_id.to_owned()),
        )
    }

    /// The run log (`log()` calls), oldest first.
    pub fn workflow_logs(&self, run_id: &str) -> Result<Vec<WorkflowLogEntry>, RuntimeError> {
        let run = run_id.to_owned();
        let rows = self.inner.store.read(move |db| {
            let mut statement = db.prepare(
                "SELECT seq, time, data FROM event WHERE aggregate_id = ?1 AND type = ?2 ORDER BY seq",
            )?;
            let rows = statement
                .query_map(params![run, LOG], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        rows.into_iter()
            .map(|(seq, time_ms, data)| {
                let data: Value = serde_json::from_str(&data)
                    .map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
                let values = serde_json::from_value(data["values"].clone())
                    .map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
                Ok(WorkflowLogEntry {
                    seq,
                    time_ms,
                    values,
                })
            })
            .collect()
    }

    /// The source of the run's current revision, for relaunching an inline script.
    pub fn workflow_run_source(&self, run_id: &str) -> Result<Option<String>, RuntimeError> {
        let run = run_id.to_owned();
        Ok(self.inner.store.read(move |db| {
            Ok(db
                .query_row(
                    "SELECT source FROM workflow_run_revision WHERE run_id = ?1 ORDER BY revision DESC LIMIT 1",
                    [run],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })?)
    }

    /// Record a run as started and execute it in the background, independent of any
    /// client connection. Its agents are children of the owner Session.
    pub async fn start_workflow_run(
        &self,
        start: StartWorkflowRun,
    ) -> Result<WorkflowRun, RuntimeError> {
        if self.workflow_closing() {
            return Err(RuntimeError::ShuttingDown);
        }
        self.ensure_workflow_progress();
        let owner = self.state(&start.session_id).await?.info;
        if self.workflow_run_of(&owner.id).await?.is_some() {
            return Err(RuntimeError::Conflict(WORKFLOW_AGENT_STARTS_RUN.into()));
        }
        let run = WorkflowRun {
            id: cyber_core::ids::new_id("run"),
            name: start.module.meta.name.clone(),
            session_id: owner.id.clone(),
            // Canonical, as runs are listed and their events routed by Location.
            directory: super::workflow_recovery::location_key(&owner.directory),
            mode: owner.mode.clone(),
            origin: serde_json::to_value(&start.module.origin).unwrap_or(Value::Null),
            script_file: None,
            tools: start.tools,
            digest: start.module.digest.clone(),
            config_digest: config_digest(&start.limits, &start.tiers),
            revision: 1,
            args: start.args.clone(),
            status: WorkflowRunStatus::Running,
            reason: None,
            budget: Some(start.budget),
            share: Some(self.workflow_share(start.share.max_concurrent)),
            max_concurrent: start.share.requested,
            queued_ms: None,
            queue_position: None,
            phases: start
                .module
                .meta
                .phases
                .iter()
                .map(|p| p.title.clone())
                .collect(),
            phase: None,
            result: None,
            result_file: None,
            result_preview: None,
            interactive: matches!(start.starter, RunStarter::Session { interactive: true }),
            continue_owner: matches!(start.starter, RunStarter::Session { .. }),
            notified: false,
            error: None,
            started_ms: chrono::Utc::now().timestamp_millis(),
            ended_ms: None,
        };
        let mut run = run;
        run.script_file = self.persist_inline_script(&run.id, &start.module)?;
        // Every run slot taken: the run waits `queued`, its wall clock not yet running.
        let admission = self.admit_workflow_run(&run.id, run.started_ms);
        if let Admission::Queued(place) = &admission {
            run.status = WorkflowRunStatus::Queued;
            run.queued_ms = Some(place.queued_ms());
        }
        // Nothing is spent and no agent started yet.
        let budget = budget_snapshot(
            run.budget.as_ref().expect("a new run has a budget"),
            &Default::default(),
            0,
        );
        // Held until the attempt is launched or queued, through the start hook: a pause,
        // stop or resume of the run, possible once it is recorded, waits for it.
        let lock = self.workflow_lock(&run.id);
        let lifecycle = lock.lock().await;
        self.record_start(&run, &start.module.source, &budget)?;
        // The summary continues the message chain of the run that started the Workflow.
        let summary = super::workflow_results::handback_message_id(&run.id);
        self.record_caused_input(&owner.id, &summary).await?;
        self.workflow_hook(&run, "WorkflowRunStart").await;
        let engine = Engine {
            module: start.module,
            limits: start.limits,
            tiers: start.tiers,
        };
        let replay = Replay::fresh(1).starting_with(budget);
        match admission {
            Admission::Slot(slot) => {
                if let Err(error) = self.launch(run.clone(), engine, replay, slot).await {
                    self.fail_launch(run, &error)?;
                    return Err(error);
                }
            }
            Admission::Queued(place) => {
                self.enqueue_workflow_run(place, Pending { engine, replay })
            }
        }
        drop(lifecycle);
        // Positions are read back: a run queued now may already be dispatched.
        Ok(self.workflow_run(&run.id)?.unwrap_or(run))
    }

    /// Keep an inline script at `<state>/workflows/<run_id>.js` (`workflows` →
    /// Model-authored workflows). A run without a state directory keeps only the stored
    /// revision source.
    fn persist_inline_script(
        &self,
        run_id: &str,
        module: &WorkflowModule,
    ) -> Result<Option<String>, RuntimeError> {
        let (cyber_workflow::Origin::Inline, Some(dir)) = (&module.origin, self.workflow_dir())
        else {
            return Ok(None);
        };
        let path = dir.join(format!("{run_id}.js"));
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, &module.source))
            .map_err(|error| {
                RuntimeError::Invalid(format!("could not keep the inline script: {error}"))
            })?;
        Ok(Some(path.display().to_string()))
    }

    /// Save an inline run's script as a project Workflow, `.cyber/workflows/<meta.name>.js`
    /// in the run's Location (`workflows` → Model-authored workflows): the edited
    /// `script_file` when it still exists, else the latest revision's source. An existing
    /// file is kept unless `force`. Returns the written path.
    pub fn save_workflow_script(
        &self,
        run_id: &str,
        force: bool,
    ) -> Result<std::path::PathBuf, RuntimeError> {
        let run = self.existing_workflow_run(run_id)?;
        if run.origin.get("kind").and_then(Value::as_str) != Some("inline") {
            return Err(RuntimeError::Conflict(format!(
                "Run {run_id} runs a saved Workflow; only an inline script can be saved"
            )));
        }
        let source = match run.script_file.as_deref().map(std::fs::read_to_string) {
            Some(Ok(source)) => source,
            _ => self.workflow_run_source(run_id)?.ok_or_else(|| {
                RuntimeError::Corrupt(format!("Run {run_id} has no stored script"))
            })?,
        };
        let module = WorkflowModule::inline(source)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        let dir = std::path::Path::new(&run.directory)
            .join(".cyber")
            .join("workflows");
        let path = dir.join(format!("{}.js", module.meta.name));
        if path.exists() && !force {
            return Err(RuntimeError::Conflict(format!(
                "{} exists; save with force to replace it",
                path.display()
            )));
        }
        std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(&path, &module.source))
            .map_err(|error| {
                RuntimeError::Invalid(format!("could not save {}: {error}", path.display()))
            })?;
        Ok(path)
    }

    /// The first event of a run, with its source for later revisions and relaunches and
    /// the `ctx.budget` its first revision starts with.
    fn record_start(
        &self,
        run: &WorkflowRun,
        source: &str,
        budget: &Value,
    ) -> Result<(), RuntimeError> {
        let mut data = stored_run(run);
        data["source"] = source.into();
        data["budget_snapshot"] = budget.clone();
        let stored = self.inner.store.append(
            &run.id,
            Expected::Seq(-1),
            vec![NewEvent::new(RUN_STARTED, data)],
        )?;
        self.inner.publish(&stored);
        Ok(())
    }

    /// Execute one attempt of `run` on its own engine thread, holding `slot` until the
    /// attempt's engine and agent tasks have all returned. The run's Mode stays a
    /// ceiling: an owner Session whose Mode was widened since the run started does not
    /// widen its agents.
    pub(super) async fn launch(
        &self,
        run: WorkflowRun,
        engine: Engine,
        replay: Replay,
        slot: RunSlot,
    ) -> Result<(), RuntimeError> {
        let owner = self.state(&run.session_id).await?.info;
        let control = self.claim_workflow_control(&run.id)?;
        // A pause or stop of an earlier attempt suspended the run's agent queue.
        self.workflow_scheduler().unsuspend(&run.id);
        let started = self.workflow_agents(&run.id)?.len() as u64;
        let mut context = super::drain::turn_context_for_info(&owner, false);
        context.mode = stricter_mode(&run.mode, &context.mode);
        let host = RunHost::new(
            self,
            &run,
            context,
            engine.tiers,
            replay,
            Arc::clone(&control),
            started,
        );
        let input = RunInput {
            args: run.args.clone(),
            ctx: json!({"run_id": run.id, "name": run.name, "session_id": run.session_id, "directory": run.directory}),
            tools: run.tools.clone(),
        };
        spawn_engine(
            run,
            engine.module,
            input,
            engine.limits,
            host,
            control,
            slot,
        )
    }
}

/// The less autonomous of two Modes; an unknown Mode counts as `plan`.
pub(super) fn stricter_mode(a: &str, b: &str) -> String {
    use cyber_core::config::mode_rank;
    match (mode_rank(a), mode_rank(b)) {
        (Some(x), Some(y)) if x <= y => a.into(),
        (Some(_), Some(_)) => b.into(),
        _ => "plan".into(),
    }
}

/// Drive the engine on its own thread, then record how the attempt ended.
fn spawn_engine(
    run: WorkflowRun,
    module: WorkflowModule,
    input: RunInput,
    limits: Limits,
    host: RunHost,
    control: Arc<Control>,
    slot: RunSlot,
) -> Result<(), RuntimeError> {
    let engine = control.engine_lease();
    // Released with the attempt's last lease, or here if the thread does not start.
    control.hold(slot);
    std::thread::Builder::new()
        .name(format!("workflow-{}", run.id))
        .spawn(move || {
            let _engine = engine;
            let outcome =
                futures::executor::block_on(cyber_workflow::run(&module, input, limits, &host));
            host.end();
            let mut run = run;
            run.phases = host.phases();
            run.phase = host.phase_title();
            finish(&host, &control, run, outcome);
        })
        .map(|_| ())
        .map_err(|error| {
            RuntimeError::Invalid(format!("could not start the workflow engine: {error}"))
        })
}

/// Whether the attempt's end must not be recorded: a stop (or another terminal record)
/// came first. A paused run still records how its script ended on its own.
fn superseded(runtime: Option<&Runtime>, control: &Control, run_id: &str) -> bool {
    let Some(runtime) = runtime else {
        return control.signal() == Signal::Stopping;
    };
    match runtime.workflow_run(run_id) {
        Ok(Some(current)) => !matches!(
            current.status,
            WorkflowRunStatus::Running | WorkflowRunStatus::Paused
        ),
        _ => true,
    }
}

/// Record the end of an attempt. A run halted by pause, stop or shutdown already has its
/// durable status; a budget halt ends as `budget_exceeded`, keeping any partial result.
fn finish(
    host: &RunHost,
    control: &Control,
    mut run: WorkflowRun,
    outcome: Result<cyber_workflow::RunOutput, Diagnostic>,
) {
    if host.halted() && outcome.is_err() {
        return;
    }
    run.ended_ms = Some(chrono::Utc::now().timestamp_millis());
    let exceeded = host.exceeded();
    let outcome = outcome.and_then(|output| {
        run.phases = output.phases;
        super::workflow_results::store_result(host.results_dir(), &mut run, output.value)
            .map_err(|error| Diagnostic::execution(format!("the result was not stored: {error}")))
    });
    let kind = match outcome {
        Ok(()) => RUN_COMPLETED,
        Err(error) => {
            run.error = Some(error);
            RUN_FAILED
        }
    };
    let budget_halt = exceeded.is_some()
        || run
            .error
            .as_ref()
            .is_some_and(|e| e.kind == DiagnosticKind::BudgetExceeded);
    run.status = match (kind, budget_halt) {
        (_, true) => WorkflowRunStatus::BudgetExceeded,
        (RUN_COMPLETED, false) => WorkflowRunStatus::Completed,
        _ => WorkflowRunStatus::Failed,
    };
    run.reason = exceeded.map(|limit| format!("BudgetExceeded: {limit}"));
    // Serialized with this run's pause, stop and resume, which record under its lock;
    // another run's lifecycle never delays it.
    let runtime = host.runtime();
    let lock = runtime
        .as_ref()
        .map(|runtime| runtime.workflow_lock(&run.id));
    let _lifecycle = lock.as_ref().map(|lock| lock.blocking_lock());
    if superseded(runtime.as_ref(), control, &run.id) {
        return;
    }
    if let Err(error) = host.log.record(&run, kind, json!({})) {
        cyber_core::log::log(
            cyber_core::log::Level::Error,
            "workflow",
            "run_end_unrecorded",
            json!({"run_id": run.id, "error": error.to_string()}),
        );
        return;
    }
    host.ended(run);
}
