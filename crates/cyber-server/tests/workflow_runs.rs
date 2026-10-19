//! Workflow Runs in the server runtime with a fake agent host (`workflows` → agent() call,
//! Durable agent results, Concurrency limit, Model tiers and mixing, Phases and logs).
mod support;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cyber_core::budget::RunBudget;
use cyber_server::runtime::*;
use cyber_store::Store;
use cyber_workflow::{Diagnostic, DiagnosticKind, Limits, WorkflowModule};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// One `spawn_workflow_agent` call as the host saw it.
#[derive(Debug, Clone)]
struct Seen {
    prompt: String,
    run_id: String,
    agent_id: String,
    call_key: String,
    model: Option<String>,
    mode: String,
    owner: String,
    resume: bool,
    worktree: Option<RunWorktree>,
}

/// Answers every agent after `delay` with `<prompt> done`, tracking concurrency.
#[derive(Default)]
struct Agents {
    store: OnceLock<Arc<Store>>,
    delay: Duration,
    /// This prompt takes three times `delay`, so permits are released one at a time.
    slow: Option<&'static str>,
    running: AtomicUsize,
    peak: AtomicUsize,
    seen: Mutex<Vec<Seen>>,
    /// Facts the script could observe before they were durable.
    unrecorded: Mutex<Vec<String>>,
    /// Prompts starting with `hold` wait for [`Agents::release`]; `stubborn` ones also
    /// ignore cancellation, like a child that is slow to exit.
    released: AtomicBool,
    gate: Notify,
    holding: AtomicUsize,
    cancelled: AtomicUsize,
    /// Prompts starting with `real` run real child Sessions on the scripted model.
    runtime: OnceLock<WeakRuntime>,
    /// Every agent profile can edit files in the Location.
    writers: AtomicBool,
    /// Prompts starting with `gate` each wait for one [`Gate::open`], in arrival order, or
    /// for a [`Gate::open_for`] naming their task.
    gate_permits: Gate,
    /// Offer a `stall` tool that ignores cancellation for [`STALL`]; `realstall` children
    /// call it, like a child that is slow to exit.
    stall: AtomicBool,
    /// `stall` calls entered.
    stalled: AtomicUsize,
}

/// How long a `stall` tool call ignores cancellation: longer than a subtree stop waits.
const STALL: Duration = Duration::from_millis(2500);

/// Releases gated agents one at a time: the longest waiting one, or one by its task.
struct Gate {
    any: tokio::sync::Semaphore,
    named: Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            any: tokio::sync::Semaphore::new(0),
            named: Mutex::default(),
        }
    }
}

impl Gate {
    fn open(&self) {
        self.any.add_permits(1);
    }

    /// Release the agent whose task is `task`, whichever gated agent waits longest.
    fn open_for(&self, task: &str) {
        self.named(task).add_permits(1);
    }

    fn named(&self, task: &str) -> Arc<tokio::sync::Semaphore> {
        let mut named = self.named.lock().unwrap();
        let gate = named
            .entry(task.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(0)));
        Arc::clone(gate)
    }

    /// Wait for one opening; false when cancelled first.
    async fn pass(&self, task: &str, cancel: &CancellationToken) -> bool {
        let named = self.named(task);
        tokio::select! {
            permit = self.any.acquire() => {
                permit.unwrap().forget();
                true
            }
            permit = named.acquire() => {
                permit.unwrap().forget();
                true
            }
            _ = cancel.cancelled() => false,
        }
    }
}

impl Agents {
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn prompts(&self) -> Vec<String> {
        self.seen().into_iter().map(|s| s.prompt).collect()
    }

    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.gate.notify_waiters();
    }

    /// Wait for the release; false when cancelled first.
    async fn hold(&self, stubborn: bool, cancel: &CancellationToken) -> bool {
        self.holding.fetch_add(1, Ordering::SeqCst);
        let released = loop {
            let notified = self.gate.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.released.load(Ordering::SeqCst) {
                break true;
            }
            tokio::select! {
                _ = &mut notified => {}
                _ = cancel.cancelled(), if !stubborn => break false,
            }
        };
        self.holding.fetch_sub(1, Ordering::SeqCst);
        released
    }

    /// A real child Session of the owner that runs one provider Turn per `+` in the
    /// prompt (plus one), as a host's child would.
    async fn real_child(
        &self,
        owner: &TurnContext,
        run_id: &str,
        agent_id: &str,
        prompt: &str,
    ) -> Result<WorkflowAgentOutput, Diagnostic> {
        let runtime = self.runtime.get().and_then(WeakRuntime::upgrade).unwrap();
        let model = if prompt.starts_with("realstall") {
            "test/stall"
        } else {
            "test/main"
        };
        let child = runtime
            .create_session(CreateSession {
                directory: owner.directory.clone(),
                model: model.into(),
                parent_id: Some(owner.session_id.clone()),
                subagent_name: Some(agent_id.to_owned()),
                workflow_run_id: Some(run_id.to_owned()),
                ..Default::default()
            })
            .await
            .unwrap()
            .id;
        for _ in 0..=prompt.matches('+').count() {
            let before = runtime.state(&child).await.unwrap().totals.steps;
            runtime
                .admit(&child, Admission::text(prompt, Delivery::Queue))
                .await
                .unwrap();
            runtime.downgrade().wait_idle(&child).await.unwrap();
            if runtime.state(&child).await.unwrap().totals.steps == before {
                return Err(Diagnostic::new(
                    DiagnosticKind::AgentFailed,
                    "the child Turn was refused: BudgetExceededError",
                ));
            }
        }
        let usage = runtime.job_usage(&child).unwrap();
        Ok(WorkflowAgentOutput {
            session_id: child,
            text: "real done".into(),
            structured: None,
            cost: usage.total_cost(),
            tokens: usage.total_tokens(),
            branch: None,
        })
    }

    /// `after:<prompt>` may only start once the settlement of `<prompt>` is stored.
    fn check_recorded(&self, run_id: &str, prompt: &str) {
        let Some(previous) = prompt.strip_prefix("after:") else {
            return;
        };
        let store = self.store.get().unwrap();
        let events = store
            .read_events(run_id, -1, cyber_store::MAX_PAGE_LIMIT)
            .unwrap()
            .events;
        let recorded = events.iter().any(|event| {
            event.kind == "workflow.agent.settled.1"
                && event.data["result"]["text"] == json!(format!("{previous} done"))
        });
        if !recorded {
            self.unrecorded.lock().unwrap().push(previous.to_owned());
        }
    }
}

impl ToolHost for Agents {
    fn definitions(&self, _turn: &TurnContext) -> Vec<ToolDef> {
        if self.stall.load(Ordering::SeqCst) {
            return vec![support::def("stall", RetrySafety::ReadOnly, true)];
        }
        Vec::new()
    }

    fn execute(&self, call: Invocation, _cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async move {
            if call.name != "stall" {
                return ToolOutcome::Failed("no tools".into());
            }
            self.stalled.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(STALL).await;
            ToolOutcome::Ok("stalled".into())
        })
    }

    fn spawn_workflow_agent(
        &self,
        request: WorkflowAgentRequest,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<WorkflowAgentOutput, Diagnostic>> {
        Box::pin(async move {
            // Held until this agent settles, as a real host does.
            let _slot = request.slot;
            self.check_recorded(&request.run_id, &request.prompt);
            self.seen.lock().unwrap().push(Seen {
                prompt: request.prompt.clone(),
                run_id: request.run_id.clone(),
                agent_id: request.agent_id.clone(),
                call_key: request.call_key.clone(),
                model: request.model.clone(),
                mode: request.owner.mode.clone(),
                owner: request.owner.session_id.clone(),
                resume: request.resume,
                worktree: request.worktree.clone(),
            });
            if request.prompt.starts_with("real") {
                return self
                    .real_child(
                        &request.owner,
                        &request.run_id,
                        &request.agent_id,
                        &request.prompt,
                    )
                    .await;
            }
            let task = request
                .prompt
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .to_owned();
            // `flaky` holds on its first attempt and fails when resumed.
            if task.starts_with("flaky") && request.resume {
                return Err(Diagnostic::new(DiagnosticKind::AgentFailed, "flaky failed"));
            }
            if task.starts_with("gate") && !self.gate_permits.pass(&task, &cancel).await {
                return Err(Diagnostic::new(DiagnosticKind::AgentFailed, "cancelled"));
            }
            if (task.starts_with("hold")
                || task.starts_with("stubborn")
                || task.starts_with("flaky"))
                && !self.hold(task.starts_with("stubborn"), &cancel).await
            {
                self.cancelled.fetch_add(1, Ordering::SeqCst);
                return Err(Diagnostic::new(DiagnosticKind::AgentFailed, "cancelled"));
            }
            let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(running, Ordering::SeqCst);
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = tokio::time::sleep(if self.slow == Some(request.prompt.as_str()) { self.delay * 3 } else { self.delay }) => {}
            }
            self.running.fetch_sub(1, Ordering::SeqCst);
            if request.prompt.starts_with("fail") {
                return Err(Diagnostic::new(
                    DiagnosticKind::AgentFailed,
                    "provider error",
                ));
            }
            Ok(WorkflowAgentOutput {
                session_id: format!("ses_child_{}", request.agent_id),
                text: format!("{task} done"),
                structured: request.schema.map(|_| json!({"ok": true})),
                cost: 0.25,
                tokens: 7,
                branch: request.worktree.map(|tree| tree.branch),
            })
        })
    }

    fn workflow_agent_writes(&self, _directory: &str, agent: Option<&str>) -> bool {
        self.writers.load(Ordering::SeqCst) && agent != Some("explore")
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    runtime: Runtime,
    store: Arc<Store>,
}

fn fixture(agents: &Arc<Agents>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("repo")).unwrap();
    let store = Arc::new(support::open_store(&dir.path().join("cyber.db")));
    let _ = agents.store.set(Arc::clone(&store));
    let runtime = runtime_over(agents, &store, dir.path());
    let _ = agents.runtime.set(runtime.downgrade());
    Fixture {
        dir,
        runtime,
        store,
    }
}

/// A runtime over `store`, as after a process restart when called again.
fn runtime_over(agents: &Arc<Agents>, store: &Arc<Store>, dir: &std::path::Path) -> Runtime {
    let steps = (0..4)
        .map(|n| support::text(&format!("turn {n}")))
        .collect();
    let stalls = (0..2)
        .map(|n| support::tools(&[(&format!("call_stall_{n}"), "stall", "{}")]))
        .chain((0..2).map(|n| support::text(&format!("stalled {n}"))))
        .collect();
    let models = support::models(
        vec![("test/main", steps), ("test/stall", stalls)],
        200_000,
        &[],
    );
    let runtime = Runtime::new(RuntimeOptions {
        store: Arc::clone(store),
        resolver: models as Arc<dyn ModelResolver>,
        tools: Arc::clone(agents) as Arc<dyn ToolHost>,
        global_config_dir: dir.join("global"),
        shell: "zsh".into(),
        claude_compat: false,
        compaction: CompactionConfig::default(),
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: Some("2026-10-16".into()),
        interactive: false,
        snapshots: Arc::new(NoSnapshots),
    });
    runtime.configure_workflow_relaunch(Arc::new(StoredRelaunch));
    runtime
}

/// A relaunch as the HTTP layer builds one after a restart: the run's script file as the
/// user left it, else its stored source, with the fixture's limits and tiers.
struct StoredRelaunch;

impl WorkflowRelaunch for StoredRelaunch {
    fn relaunch<'a>(
        &'a self,
        runtime: &'a Runtime,
        run: &'a WorkflowRun,
    ) -> BoxFuture<'a, Result<ResumeWorkflowRun, String>> {
        Box::pin(async move {
            let edited = run
                .script_file
                .as_ref()
                .and_then(|file| std::fs::read_to_string(file).ok());
            let source = match edited {
                Some(source) => source,
                None => runtime
                    .workflow_run_source(&run.id)
                    .map_err(|error| error.to_string())?
                    .ok_or("no stored source")?,
            };
            Ok(ResumeWorkflowRun {
                run_id: run.id.clone(),
                module: WorkflowModule::inline(source).map_err(|error| error.to_string())?,
                limits: Limits::default(),
                tiers: BTreeMap::new(),
                budget: None,
                revision: false,
                share: RunShare::requested(run.max_concurrent),
            })
        })
    }
}

fn default_budget() -> RunBudget {
    RunBudget::resolve(&json!({}), &[]).unwrap()
}

fn module(body: &str) -> WorkflowModule {
    WorkflowModule::inline(format!(
        "export const meta = {{ name: 'test', description: 'Test', phases: [{{ title: 'Scan' }}] }};\n{body}"
    ))
    .unwrap()
}

/// Poll until `done` holds.
async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what} did not happen");
}

impl Fixture {
    async fn session(&self, mode: &str) -> String {
        self.runtime
            .create_session(CreateSession {
                directory: self.dir.path().join("repo").display().to_string(),
                model: "test/main".into(),
                mode: Some(mode.into()),
                ..Default::default()
            })
            .await
            .unwrap()
            .id
    }

    async fn start(&self, session: &str, body: &str) -> WorkflowRun {
        self.start_with(session, body, BTreeMap::new()).await
    }

    async fn start_with(
        &self,
        session: &str,
        body: &str,
        tiers: BTreeMap<String, Option<String>>,
    ) -> WorkflowRun {
        self.start_budgeted(session, body, tiers, default_budget())
            .await
    }

    async fn start_budgeted(
        &self,
        session: &str,
        body: &str,
        tiers: BTreeMap<String, Option<String>>,
        budget: RunBudget,
    ) -> WorkflowRun {
        self.runtime
            .start_workflow_run(StartWorkflowRun {
                module: module(body),
                args: json!({"n": 3}),
                session_id: session.into(),
                limits: Limits::default(),
                tiers,
                budget,
                share: Default::default(),
                starter: Default::default(),
            })
            .await
            .unwrap()
    }

    /// Relaunch `run_id` with `body` (its stored script when `None`).
    async fn resume(
        &self,
        run_id: &str,
        body: Option<&str>,
        revision: bool,
    ) -> Result<WorkflowRun, RuntimeError> {
        let module = match body {
            Some(body) => module(body),
            None => {
                WorkflowModule::inline(self.runtime.workflow_run_source(run_id).unwrap().unwrap())
                    .unwrap()
            }
        };
        self.runtime
            .resume_workflow_run(ResumeWorkflowRun {
                run_id: run_id.into(),
                module,
                limits: Limits::default(),
                tiers: BTreeMap::new(),
                budget: None,
                revision,
                share: Default::default(),
            })
            .await
    }

    async fn settled(&self, id: &str) -> WorkflowRun {
        for _ in 0..1000 {
            let run = self.runtime.workflow_run(id).unwrap().unwrap();
            if run.status != WorkflowRunStatus::Running {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("run {id} did not settle");
    }

    /// Wait until the run's last attempt let go of its engine and agents.
    async fn quiet(&self, id: &str) {
        for _ in 0..1000 {
            if !self.runtime.workflow_agents_exiting(id).unwrap() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("run {id} kept its agents");
    }

    /// Every event of `kind` on the run, across pages.
    fn events(&self, run_id: &str, kind: &str) -> Vec<Value> {
        let (mut found, mut after) = (Vec::new(), -1);
        loop {
            let page = self
                .store
                .read_events(run_id, after, cyber_store::MAX_PAGE_LIMIT)
                .unwrap();
            after = page.events.last().map_or(after, |event| event.seq);
            found.extend(
                page.events
                    .into_iter()
                    .filter(|event| event.kind == kind)
                    .map(|event| event.data),
            );
            if !page.has_more {
                return found;
            }
        }
    }

    /// The run's events, without the summary admission recorded after it ended.
    fn kinds(&self, run_id: &str) -> Vec<String> {
        self.store
            .read_events(run_id, -1, cyber_store::MAX_PAGE_LIMIT)
            .unwrap()
            .events
            .into_iter()
            .map(|event| event.kind)
            .filter(|kind| kind != "workflow.run.notified.1")
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn settled_results_are_durable_before_the_script_observes_them() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("plan").await;
    let run = f
        .start(
            &owner,
            "export default async () => {\n  phase('Scan');\n  const first = await agent('first');\n  const second = await agent('after:first', { label: 'next' });\n  phase('Verify');\n  log('texts', first.text, second.text);\n  return [first, second];\n};\n",
        )
        .await;
    assert_eq!(run.status, WorkflowRunStatus::Running);
    assert_eq!(run.session_id, owner);
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    assert!(agents.unrecorded.lock().unwrap().is_empty());
    let seen = agents.seen();
    let result = run.result.unwrap();
    assert_eq!(result[0]["text"], "first done");
    assert_eq!(
        result[0]["session_id"],
        json!(format!("ses_child_{}", seen[0].agent_id))
    );
    assert_eq!(result[0]["cost"], 0.25);
    assert_eq!(result[0]["tokens"], 7);
    assert_eq!(result[1]["text"], "after:first done");
    // Declared phases first, then titles first seen in phase().
    assert_eq!(run.phases, ["Scan", "Verify"]);
    assert_eq!(run.phase.as_deref(), Some("Verify"));
    assert_eq!(
        f.kinds(&run.id),
        [
            "workflow.run.started.1",
            "workflow.phase.1",
            "workflow.agent.started.1",
            "workflow.agent.settled.1",
            "workflow.agent.started.1",
            "workflow.agent.settled.1",
            "workflow.phase.1",
            "workflow.log.1",
            "workflow.run.completed.1",
        ]
    );

    // Each call has an `agt_` ID and a stable key; children run under the owner's Mode.
    let recorded = f.runtime.workflow_agents(&run.id).unwrap();
    assert_eq!(recorded.len(), 2);
    for (agent, seen) in recorded.iter().zip(&seen) {
        assert!(agent.id.starts_with("agt_"));
        assert_eq!(agent.id, seen.agent_id);
        assert_eq!(agent.call_key, seen.call_key);
        assert!(agent.call_key.ends_with(":1"), "{}", agent.call_key);
        assert_eq!(agent.phase.as_deref(), Some("Scan"));
        assert_eq!(seen.mode, "plan");
        assert_eq!(seen.owner, owner);
        // The projection times each agent from its events, for the monitor's durations.
        let (started, ended) = (agent.started_ms.unwrap(), agent.ended_ms.unwrap());
        assert!(run.started_ms <= started && started <= ended, "{agent:?}");
    }
    assert!(recorded[0].ended_ms <= recorded[1].started_ms);
    for kind in ["workflow.agent.started.1", "workflow.agent.settled.1"] {
        let recorded = &f.events(&run.id, kind)[0];
        assert!(recorded.get("started_ms").is_none() && recorded.get("ended_ms").is_none());
    }
    assert_eq!(recorded[1].label.as_deref(), Some("next"));
    let runs = f.runtime.workflow_runs(Some(&owner)).unwrap();
    assert_eq!(runs.len(), 1);
    assert!(
        f.runtime
            .workflow_runs(Some("ses_other"))
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_calls_get_ordinals_and_failures_are_recorded() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let run = f
        .start(
            &owner,
            "export default async () => {\n  await agent('same');\n  await agent('same');\n  await agent('same', { label: 'other options' });\n  try { await agent('fail now'); } catch (e) { return e.kind; }\n};\n",
        )
        .await;
    let run = f.settled(&run.id).await;
    assert_eq!(run.result, Some(json!("AgentFailed")), "{run:?}");
    let keys: Vec<String> = agents.seen().into_iter().map(|s| s.call_key).collect();
    let (hash, _) = keys[0].split_once(':').unwrap();
    assert_eq!(keys[0], format!("{hash}:1"));
    assert_eq!(keys[1], format!("{hash}:2"));
    assert!(keys[2].ends_with(":1") && !keys[2].starts_with(hash));
    let recorded = f.runtime.workflow_agents(&run.id).unwrap();
    let failed = recorded.last().unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(
        failed.error.as_ref().unwrap().kind,
        DiagnosticKind::AgentFailed
    );
    assert_eq!(failed.session_id, None);
}

/// Fair agent scheduling: a run that queues behind a larger one is served on the next free
/// slot, then the two alternate, whatever the larger run's queue length.
#[tokio::test(flavor = "multi_thread")]
async fn the_global_limit_is_shared_fairly_across_runs() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    assert!(f.runtime.configure_workflow_concurrency(2));
    assert!(!f.runtime.configure_workflow_concurrency(5));
    let scheduler = f.runtime.workflow_scheduler();
    let owner = f.session("default").await;
    let fan_out = |prefix: &str, count: usize| {
        let calls: Vec<String> = (1..=count).map(|n| format!("'gate {prefix}{n}'")).collect();
        format!(
            "export default () => parallel([{}].map((p) => () => agent(p)));\n",
            calls.join(", ")
        )
    };
    let started = |n: usize| {
        let agents = Arc::clone(&agents);
        async move { eventually("the next agent start", || agents.seen().len() >= n).await }
    };
    let a = f.start(&owner, &fan_out("a", 6)).await;
    started(2).await;
    eventually("a's calls queue", || scheduler.snapshot(&a.id).queued == 4).await;
    let b = f.start(&owner, &fan_out("b", 2)).await;
    eventually("b's calls queue", || scheduler.snapshot(&b.id).queued == 2).await;
    assert_eq!(
        scheduler.snapshot(&a.id),
        RunSlots {
            queued: 4,
            running: 2
        }
    );
    // Gates open in arrival order: a1, a2, b1, a3, b2, a4, a5, a6.
    for n in 3..=8 {
        agents.gate_permits.open();
        started(n).await;
        let running = scheduler.snapshot(&a.id).running + scheduler.snapshot(&b.id).running;
        assert!(running <= 2, "{running} agents hold slots");
    }
    for _ in 0..2 {
        agents.gate_permits.open();
    }
    for run in [&a, &b] {
        assert_eq!(
            f.settled(&run.id).await.status,
            WorkflowRunStatus::Completed
        );
    }
    let order: Vec<String> = agents.prompts();
    assert_eq!(
        order,
        [
            "gate a1", "gate a2", "gate b1", "gate a3", "gate b2", "gate a4", "gate a5", "gate a6"
        ],
        "b's agents start on the first and third releases after b queues"
    );
    for run in [&a, &b] {
        f.quiet(&run.id).await;
        assert_eq!(scheduler.snapshot(&run.id), RunSlots::default());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tiers_resolve_models_and_unknown_tiers_are_model_unavailable() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let tiers = BTreeMap::from([
        ("fast".to_string(), Some("p/small".to_string())),
        ("verify".to_string(), Some("p/big".to_string())),
        ("smart".to_string(), None),
    ]);
    let run = f
        .start_with(
            &owner,
            "export default async () => {\n  await agent('a', { model: 'fast' });\n  await agent('b', { model: 'verify' });\n  await agent('c', { model: 'smart' });\n  await agent('d', { model: 'p/explicit' });\n  await agent('e');\n  try { await agent('f', { model: 'galaxy' }); } catch (e) { return [e.kind, e.message]; }\n};\n",
            tiers,
        )
        .await;
    let run = f.settled(&run.id).await;
    assert_eq!(
        run.result,
        Some(json!(["ModelUnavailable", "unknown model tier \"galaxy\""]))
    );
    let models: Vec<Option<String>> = agents.seen().into_iter().map(|s| s.model).collect();
    assert_eq!(
        models,
        [
            Some("p/small".into()),
            Some("p/big".into()),
            None,
            Some("p/explicit".into()),
            None
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_options_reject_before_any_agent_starts() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    for option in [
        "{ goal: 'tests pass' }",
        "{ tools: ['read'] }",
        "{ budget: { turns: 2 } }",
        "{ isolation: 'elsewhere' }",
        "{ schema: { type: 'nonsense' } }",
    ] {
        let run = f
            .start(
                &owner,
                &format!("export default async () => {{ try {{ await agent('x', {option}); }} catch (e) {{ return [e.kind, e.message]; }} }};\n"),
            )
            .await;
        let run = f.settled(&run.id).await;
        let result = run.result.unwrap();
        assert_eq!(result[0], "ExecutionFailure", "{option}: {result}");
    }
    assert!(agents.seen().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn hosts_without_workflow_agents_fail_closed_and_missing_owners_are_refused() {
    let h = support::Harness::new(support::Setup::default());
    let owner = h.session().await;
    let run = h
        .runtime
        .start_workflow_run(StartWorkflowRun {
            module: WorkflowModule::inline(
                "export const meta = { name: 'x', description: 'X' };\nexport default async () => { try { await agent('x'); } catch (e) { return e.kind; } };\n",
            )
            .unwrap(),
            args: json!({}),
            session_id: owner,
            limits: Limits::default(),
            tiers: BTreeMap::new(),
            budget: default_budget(),
            share: Default::default(),
            starter: Default::default(),
        })
        .await
        .unwrap();
    let mut settled = None;
    for _ in 0..500 {
        let current = h.runtime.workflow_run(&run.id).unwrap().unwrap();
        if current.status != WorkflowRunStatus::Running {
            settled = Some(current);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(settled.unwrap().result, Some(json!("AgentFailed")));
    let missing = h
        .runtime
        .start_workflow_run(StartWorkflowRun {
            module: WorkflowModule::inline(
                "export const meta = { name: 'x', description: 'X' };\nexport default () => 1;\n",
            )
            .unwrap(),
            args: json!({}),
            session_id: "ses_missing".into(),
            limits: Limits::default(),
            tiers: BTreeMap::new(),
            budget: default_budget(),
            share: Default::default(),
            starter: Default::default(),
        })
        .await;
    assert!(matches!(missing, Err(RuntimeError::SessionNotFound(_))));
}

const SEQUENTIAL: &str = "export default async () => {\n  phase('Scan');\n  const a = await agent('a');\n  const saved = await checkpoint('cursor', { at: a.text });\n  log('after a');\n  const b = await agent('hold b');\n  const c = await agent('c');\n  return [a.text, saved, b.text, c.text];\n};\n";

#[tokio::test(flavor = "multi_thread")]
async fn pause_lets_in_flight_agents_finish_and_resume_replays_from_the_store() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let run = f.start(&owner, SEQUENTIAL).await;
    eventually("b in flight", || agents.holding.load(Ordering::SeqCst) == 1).await;

    let paused = f.runtime.pause_workflow_run(&run.id).await.unwrap();
    assert_eq!(paused.status, WorkflowRunStatus::Paused);
    // The in-flight agent finishes and is recorded; the next one never starts.
    agents.release();
    f.quiet(&run.id).await;
    assert_eq!(agents.prompts(), ["a", "hold b"]);
    let stored = f.runtime.workflow_run(&run.id).unwrap().unwrap();
    assert_eq!(stored.status, WorkflowRunStatus::Paused, "{stored:?}");
    let recorded = f.runtime.workflow_agents(&run.id).unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(
        recorded.iter().all(|a| a.status == "completed"),
        "{recorded:?}"
    );
    assert_eq!(f.events(&run.id, "workflow.run.paused.1").len(), 1);

    // Pausing again is idempotent; a paused run is not paused into another state.
    f.runtime.pause_workflow_run(&run.id).await.unwrap();
    let resumed = f.resume(&run.id, None, false).await.unwrap();
    assert_eq!(resumed.status, WorkflowRunStatus::Running);
    assert_eq!(resumed.revision, 1);
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    assert_eq!(
        run.result,
        Some(json!(["a done", {"at": "a done"}, "hold b done", "c done"]))
    );
    // a and b replayed from the store: only c reached the host.
    assert_eq!(agents.prompts(), ["a", "hold b", "c"]);
    // The replayed prefix did not log or phase again.
    assert_eq!(f.events(&run.id, "workflow.log.1").len(), 1);
    assert_eq!(f.events(&run.id, "workflow.phase.1").len(), 1);
    assert_eq!(f.events(&run.id, "workflow.call.recorded.1").len(), 1);
    assert_eq!(f.events(&run.id, "workflow.run.resumed.1").len(), 1);
    // A completed run cannot be resumed.
    assert!(matches!(
        f.resume(&run.id, None, false).await,
        Err(RuntimeError::Conflict(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_script_fails_closed_without_a_revision_and_reuses_matching_calls_with_one() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let first = "export default async () => {\n  const a = await agent('a');\n  const saved = await checkpoint('pick', 1);\n  const b = await agent('hold b');\n  return [a.text, saved, b.text];\n};\n";
    let run = f.start(&owner, first).await;
    eventually("b in flight", || agents.holding.load(Ordering::SeqCst) == 1).await;
    f.runtime.stop_workflow_run(&run.id).await.unwrap();
    f.quiet(&run.id).await;
    assert_eq!(agents.cancelled.load(Ordering::SeqCst), 1);

    // The edited script changes b's prompt and the checkpoint's value.
    let edited = "export default async () => {\n  const a = await agent('a');\n  const saved = await checkpoint('pick', 2);\n  const b = await agent('b2');\n  return [a.text, saved, b.text];\n};\n";
    match f.resume(&run.id, Some(edited), false).await {
        Err(RuntimeError::Conflict(message)) => {
            assert!(message.starts_with("WorkflowReplayMismatch"), "{message}");
            assert!(message.contains("revision"), "{message}");
        }
        other => panic!("changed script accepted without a revision: {other:?}"),
    }
    assert_eq!(
        f.runtime.workflow_run(&run.id).unwrap().unwrap().status,
        WorkflowRunStatus::Stopped
    );
    let resumed = f.resume(&run.id, Some(edited), true).await.unwrap();
    assert_eq!(resumed.revision, 2);
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    // a was reused by key and the checkpoint returned its recorded value.
    assert_eq!(run.result, Some(json!(["a done", 1, "b2 done"])));
    assert_eq!(agents.prompts(), ["a", "hold b", "b2"]);
    assert_eq!(f.events(&run.id, "workflow.run.revised.1").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_cancels_agents_and_resume_waits_for_agents_still_exiting() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let run = f
        .start(
            &owner,
            "export default async () => {\n  const [x, y] = await parallel([() => agent('hold x'), () => agent('stubborn y')]);\n  return [x.text, y.text];\n};\n",
        )
        .await;
    eventually("both in flight", || {
        agents.holding.load(Ordering::SeqCst) == 2
    })
    .await;
    let stopped = f.runtime.stop_workflow_run(&run.id).await.unwrap();
    assert_eq!(stopped.status, WorkflowRunStatus::Stopped);
    eventually("x cancelled", || {
        agents.cancelled.load(Ordering::SeqCst) == 1
    })
    .await;
    let recorded = f.runtime.workflow_agents(&run.id).unwrap();
    assert!(
        recorded.iter().all(|a| a.status == "cancelled"),
        "{recorded:?}"
    );
    assert!(f.events(&run.id, "workflow.agent.settled.1").is_empty());

    // y ignores cancellation, so its task still holds the child.
    match f.resume(&run.id, None, false).await {
        Err(RuntimeError::Conflict(message)) => assert_eq!(message, AGENTS_EXITING),
        other => panic!("resume while an agent exits: {other:?}"),
    }
    agents.release();
    f.quiet(&run.id).await;
    assert_eq!(
        f.runtime.workflow_run(&run.id).unwrap().unwrap().status,
        WorkflowRunStatus::Stopped
    );
    f.resume(&run.id, None, false).await.unwrap();
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    // Both unsettled agents resumed under their original IDs.
    let seen = agents.seen();
    assert_eq!(seen.len(), 4);
    for retried in &seen[2..] {
        assert!(retried.resume, "{retried:?}");
        assert!(
            seen[..2]
                .iter()
                .any(|first| first.agent_id == retried.agent_id)
        );
        assert!(
            retried.prompt.contains("was interrupted"),
            "{}",
            retried.prompt
        );
    }
    assert_eq!(f.runtime.workflow_agents(&run.id).unwrap().len(), 2);
}

/// A run that ends while agents are in flight cancels them (its pending calls are dropped)
/// and records them `cancelled`, never leaving them `running` after the run ended.
#[tokio::test(flavor = "multi_thread")]
async fn agents_in_flight_when_the_script_ends_are_recorded_cancelled() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let failed = f
        .start(
            &owner,
            "export default async () => {\n  await parallel([() => agent('hold x'), async () => { await checkpoint('c', 1); throw new Error('boom'); }]);\n};\n",
        )
        .await;
    let completed = f
        .start(
            &owner,
            "export default async () => {\n  agent('hold y');\n  await checkpoint('c', 1);\n  return 'early';\n};\n",
        )
        .await;
    for (run, status) in [
        (failed, WorkflowRunStatus::Failed),
        (completed, WorkflowRunStatus::Completed),
    ] {
        let ended = f.settled(&run.id).await;
        assert_eq!(ended.status, status, "{ended:?}");
        f.quiet(&run.id).await;
        let recorded = f.runtime.workflow_agents(&run.id).unwrap();
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert_eq!(recorded[0].status, "cancelled", "{recorded:?}");
    }
    eventually("both children cancelled", || {
        agents.cancelled.load(Ordering::SeqCst) == 2
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_settlement_that_cannot_be_recorded_fails_the_run_closed() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let run = f
        .start(
            &owner,
            "export default async () => {\n  try { await agent('hold x'); } catch (e) { return 'observed'; }\n  return 'completed';\n};\n",
        )
        .await;
    eventually("x in flight", || agents.holding.load(Ordering::SeqCst) == 1).await;
    f.store
        .transaction(|db| {
            db.execute_batch("DROP TABLE workflow_call")?;
            Ok(())
        })
        .unwrap();
    agents.release();
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Failed, "{run:?}");
    assert_eq!(run.result, None);
    assert!(
        run.error
            .unwrap()
            .message
            .contains("workflow.agent.settled.1"),
        "the script must not observe an unrecorded settlement"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn max_agents_rejects_the_next_agent_and_the_run_ends_budget_exceeded() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let budget = RunBudget::resolve(&json!({}), &[Some(&json!({"max_agents": 1}))]).unwrap();
    let run = f
        .start_budgeted(
            &owner,
            "export default async () => {\n  const first = await agent('a');\n  const before = ctx.budget;\n  try { await agent('b'); } catch (e) { return { partial: first.text, kind: e.kind, message: e.message, before }; }\n};\n",
            BTreeMap::new(),
            budget,
        )
        .await;
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::BudgetExceeded, "{run:?}");
    let result = run.result.unwrap();
    assert_eq!(result["partial"], "a done");
    assert_eq!(result["kind"], "BudgetExceeded");
    assert_eq!(result["message"], "BudgetExceeded: max_agents");
    assert_eq!(result["before"]["agents_started"], 1);
    assert_eq!(result["before"]["remaining"]["max_agents"], 0);
    assert_eq!(run.reason.as_deref(), Some("BudgetExceeded: max_agents"));
    assert_eq!(agents.prompts(), ["a"]);
    let exceeded = f.events(&run.id, "budget.exceeded.1");
    assert_eq!(exceeded.len(), 1);
    assert_eq!(exceeded[0]["scope_id"], json!(run.id));
    assert_eq!(exceeded[0]["limit"], "max_agents");
}

#[tokio::test(flavor = "multi_thread")]
async fn ctx_budget_reflects_spend_and_wall_time_is_enforced() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let budget = RunBudget::resolve(
        &json!({}),
        &[Some(&json!({"max_wall_seconds": 1.5, "max_cost_usd": 1.0}))],
    )
    .unwrap();
    let run = f
        .start_budgeted(
            &owner,
            "export default async () => {\n  const start = ctx.budget;\n  const one = await agent('real one');\n  const spent = ctx.budget;\n  await agent('hold wait');\n  try { await agent('late'); } catch (e) { return { start, spent, kind: e.kind, message: e.message }; }\n};\n",
            BTreeMap::new(),
            budget,
        )
        .await;
    eventually("wait in flight", || {
        agents.holding.load(Ordering::SeqCst) == 1
    })
    .await;
    // The held agent outlives the wall budget; 1.5 s leaves room for the first agent and
    // the next start on a loaded CI runner.
    tokio::time::sleep(Duration::from_millis(1600)).await;
    agents.release();
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::BudgetExceeded, "{run:?}");
    let result = run.result.unwrap();
    assert_eq!(result["start"]["spent_cost_usd"], 0.0);
    assert_eq!(result["start"]["agents_started"], 0);
    let spent = &result["spent"];
    assert_eq!(spent["agents_started"], 1);
    assert_eq!(spent["spent_tokens"], 110);
    let cost = spent["spent_cost_usd"].as_f64().unwrap();
    assert!((cost - 0.00012).abs() < 1e-9, "{spent}");
    assert!(
        (spent["remaining"]["max_cost_usd"].as_f64().unwrap() - (1.0 - cost)).abs() < 1e-9,
        "{spent}"
    );
    assert_eq!(result["message"], "BudgetExceeded: max_wall_seconds");
    assert_eq!(agents.prompts(), ["real one", "hold wait"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_cost_cap_stops_the_next_child_turn() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    // One Turn costs $0.00012; the cap is reached after the first.
    let budget = RunBudget::resolve(&json!({}), &[Some(&json!({"max_cost_usd": 0.0001}))]).unwrap();
    let run = f
        .start_budgeted(
            &owner,
            "export default async () => {\n  try { await agent('real two + turns'); } catch (e) { return [e.kind, e.message]; }\n};\n",
            BTreeMap::new(),
            budget,
        )
        .await;
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::BudgetExceeded, "{run:?}");
    assert_eq!(
        run.result,
        Some(json!(["BudgetExceeded", "BudgetExceeded: max_cost_usd"]))
    );
    // The child's second Turn was refused before reaching the model.
    let parent = owner.clone();
    let children: Vec<String> = f
        .store
        .read(move |db| {
            let mut statement = db.prepare("SELECT id FROM session WHERE parent_id = ?1")?;
            let ids = statement
                .query_map([parent], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .unwrap();
    assert_eq!(children.len(), 1);
    let child = f.runtime.state(&children[0]).await.unwrap();
    assert_eq!(child.totals.steps, 1);
    let exceeded = f.events(&children[0], "budget.exceeded.1");
    assert_eq!(exceeded[0]["scope_id"], json!(run.id), "{exceeded:?}");
    let usage = f.runtime.run_usage(&run.id).unwrap();
    assert_eq!(usage.agents.len(), 1);
    assert_eq!(usage.models[0].key, "test/main");
    assert_eq!(usage.total_tokens, 110);
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_and_recovery_leave_running_runs_paused_for_resume() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let run = f.start(&owner, SEQUENTIAL).await;
    eventually("b in flight", || agents.holding.load(Ordering::SeqCst) == 1).await;

    // Another process opening the same store sees a run nobody executes and pauses it.
    let other = runtime_over(&agents, &f.store, f.dir.path());
    other.recover_workflow_runs(false).await.unwrap();
    let recovered = f.runtime.workflow_run(&run.id).unwrap().unwrap();
    assert_eq!(recovered.status, WorkflowRunStatus::Paused);
    assert!(recovered.reason.unwrap().contains("restarted"));
    let b = f
        .runtime
        .workflow_agents(&run.id)
        .unwrap()
        .into_iter()
        .find(|a| a.status != "completed")
        .unwrap();
    assert_eq!(b.status, "interrupted");
    other.shutdown().await;

    // Shutdown cancels b without recording it, so a resume continues it.
    f.runtime.shutdown().await;
    assert_eq!(agents.cancelled.load(Ordering::SeqCst), 1);
    assert!(
        f.events(&run.id, "workflow.agent.settled.1").len() == 1,
        "the cancelled agent was not settled"
    );
    let restarted = runtime_over(&agents, &f.store, f.dir.path());
    restarted.recover_workflow_runs(false).await.unwrap();
    agents.release();
    let module =
        WorkflowModule::inline(restarted.workflow_run_source(&run.id).unwrap().unwrap()).unwrap();
    restarted
        .resume_workflow_run(ResumeWorkflowRun {
            run_id: run.id.clone(),
            module,
            limits: Limits::default(),
            tiers: BTreeMap::new(),
            budget: None,
            revision: false,
            share: Default::default(),
        })
        .await
        .unwrap();
    let mut done = None;
    for _ in 0..1000 {
        let current = restarted.workflow_run(&run.id).unwrap().unwrap();
        if current.status != WorkflowRunStatus::Running {
            done = Some(current);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let done = done.unwrap();
    assert_eq!(done.status, WorkflowRunStatus::Completed, "{done:?}");
    let prompts = agents.prompts();
    assert_eq!(prompts.len(), 4, "{prompts:?}");
    assert!(agents.seen()[2].resume);
    assert_eq!(prompts[3], "c");
    restarted.shutdown().await;
}

/// Shutdown dispatches nothing once it starts pausing runs (regression: a slot freed while
/// it waited for one run's lock dispatched a queued run it then never paused). Run X's
/// stop holds X's lock while its stalled agent exits, so shutdown waits there; run Z ends
/// meanwhile and frees its slot. The queued run Q stays `queued`, undispatched, and a
/// restart dispatches it.
#[tokio::test(flavor = "multi_thread")]
async fn a_slot_freed_during_shutdown_dispatches_no_queued_run() {
    let agents = Arc::new(Agents {
        stall: AtomicBool::new(true),
        ..Default::default()
    });
    let f = fixture(&agents);
    f.runtime.configure_workflow_runs(2);
    let owner = f.session("default").await;
    let x = f
        .start(
            &owner,
            "export default () => agent('realstall x');
",
        )
        .await;
    eventually("X's agent in its tool", || {
        agents.stalled.load(Ordering::SeqCst) == 1
    })
    .await;
    let z = f
        .start(
            &owner,
            "export default async () => (await agent('gate z')).text;
",
        )
        .await;
    gated(&agents, 1).await;
    let q = f
        .start(
            &owner,
            "export default async () => (await agent('q')).text;
",
        )
        .await;
    assert_eq!(q.status, WorkflowRunStatus::Queued, "{q:?}");

    let stopping = {
        let (runtime, id) = (f.runtime.clone(), x.id.clone());
        tokio::spawn(async move { runtime.stop_workflow_run(&id).await })
    };
    let children = run_children(&f.store, &x.id);
    eventually("X's stop sweeping its child", || {
        children
            .iter()
            .all(|child| !f.events(child, "session.subtree.stopping.1").is_empty())
    })
    .await;
    let shutdown = {
        let runtime = f.runtime.clone();
        tokio::spawn(async move { runtime.shutdown().await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!stopping.is_finished(), "X's agent is still exiting");
    // Z ends (on its own, or paused by shutdown) and frees its slot while X's stop runs.
    agents.gate_permits.open();
    eventually("Z's end", || {
        f.status(&z.id).0 != WorkflowRunStatus::Running
    })
    .await;
    let freed = std::time::Instant::now();
    while f.status(&q.id).0 == WorkflowRunStatus::Queued
        && freed.elapsed() < Duration::from_millis(300)
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!stopping.is_finished(), "X's agent is still exiting");
    stopping.await.unwrap().unwrap();
    shutdown.await.unwrap();
    assert_eq!(f.status(&q.id), (WorkflowRunStatus::Queued, Some(1)));
    assert!(f.events(&q.id, "workflow.run.dispatched.1").is_empty());
    assert!(
        !agents.prompts().iter().any(|p| p == "q"),
        "Q started no agent"
    );

    let restarted = runtime_over(&agents, &f.store, f.dir.path());
    restarted.configure_workflow_runs(2);
    restarted.recover_workflow_runs(false).await.unwrap();
    let q = ended_on(&restarted, &q.id).await;
    assert_eq!(q.status, WorkflowRunStatus::Completed, "{q:?}");
    assert_eq!(f.events(&q.id, "workflow.run.dispatched.1").len(), 1);
    restarted.shutdown().await;
}

/// Poll `runtime` until the run ended.
async fn ended_on(runtime: &Runtime, id: &str) -> WorkflowRun {
    for _ in 0..1000 {
        let run = runtime.workflow_run(id).unwrap().unwrap();
        if run.status.is_terminal() {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {id} did not end");
}

/// A script whose first agent holds until released, then a second one.
fn held_pair(name: &str) -> String {
    format!(
        "export default async () => [(await agent('hold {name}')).text, (await agent('{name}2')).text];\n"
    )
}

/// `(run, agent)` of the agents the host was asked to resume, sorted.
fn resumed_agents(agents: &Agents) -> Vec<(String, String)> {
    let mut resumed: Vec<(String, String)> = agents
        .seen()
        .into_iter()
        .filter(|seen| seen.resume)
        .map(|seen| (seen.run_id, seen.agent_id))
        .collect();
    resumed.sort();
    resumed
}

/// `(run, agent)` of the first attempts of the `hold` agents, sorted.
fn held_agents(agents: &Agents) -> Vec<(String, String)> {
    let mut held: Vec<(String, String)> = agents
        .seen()
        .into_iter()
        .filter(|seen| !seen.resume && seen.prompt.starts_with("hold"))
        .map(|seen| (seen.run_id, seen.agent_id))
        .collect();
    held.sort();
    held
}

/// Recovery of several runs, "Crash with two runs mid-flight": a second runtime over the
/// store recovers while the first still executes, as after a crash. The two running runs
/// come back paused with their in-flight agents interrupted, the queued run keeps its
/// place and is dispatched, and resuming the interrupted runs continues each one's own
/// interrupted agent only.
#[tokio::test(flavor = "multi_thread")]
async fn a_crash_with_two_runs_mid_flight_pauses_both_and_dispatches_the_queued_one() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    f.runtime.configure_workflow_runs(2);
    let owner = f.session("default").await;
    let a = f.start(&owner, &held_pair("a")).await;
    let b = f.start(&owner, &held_pair("b")).await;
    eventually("a and b in flight", || {
        agents.holding.load(Ordering::SeqCst) == 2
    })
    .await;
    let c = f
        .start(
            &owner,
            "export default async () => (await agent('c')).text;\n",
        )
        .await;
    assert_eq!(c.status, WorkflowRunStatus::Queued, "{c:?}");

    let restarted = runtime_over(&agents, &f.store, f.dir.path());
    restarted.configure_workflow_runs(2);
    restarted.recover_workflow_runs(false).await.unwrap();
    for id in [&a.id, &b.id] {
        let run = restarted.workflow_run(id).unwrap().unwrap();
        assert_eq!(run.status, WorkflowRunStatus::Paused, "{run:?}");
        assert!(run.reason.unwrap().contains("restarted"));
        assert!(restarted.workflow_run_interrupted(id).unwrap());
        let summary = restarted.workflow_run_summary(id).unwrap().unwrap();
        assert!(summary.interrupted, "the summary says why the run paused");
        let statuses: Vec<String> = restarted
            .workflow_agents(id)
            .unwrap()
            .into_iter()
            .map(|agent| agent.status)
            .collect();
        assert_eq!(statuses, ["interrupted"], "{id}");
    }
    let c = ended_on(&restarted, &c.id).await;
    assert_eq!(c.status, WorkflowRunStatus::Completed, "{c:?}");
    assert_eq!(c.result, Some(json!("c done")));
    assert!(!restarted.workflow_run_interrupted(&c.id).unwrap());
    assert!(
        !restarted
            .workflow_run_summary(&c.id)
            .unwrap()
            .unwrap()
            .interrupted
    );

    // The crashed process's agents end without being recorded.
    f.runtime.shutdown().await;
    agents.release();
    let report = restarted
        .resume_interrupted_workflow_runs(None)
        .await
        .unwrap();
    assert_eq!(report.resumed, [a.id.clone(), b.id.clone()], "{report:?}");
    assert!(report.refused.is_empty(), "{report:?}");
    for (run, name) in [(&a, "a"), (&b, "b")] {
        let run = ended_on(&restarted, &run.id).await;
        assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
        assert_eq!(
            run.result,
            Some(json!([
                format!("hold {name} done"),
                format!("{name}2 done")
            ]))
        );
    }
    // Each run resumed its own interrupted agent, once.
    let held = held_agents(&agents);
    assert_eq!(held.len(), 2, "{held:?}");
    assert_eq!(resumed_agents(&agents), held);
    let again = restarted
        .resume_interrupted_workflow_runs(None)
        .await
        .unwrap();
    assert_eq!(again, InterruptedResume::default());
    restarted.shutdown().await;
}

/// "Automatic resume after restart": with `resume_after_restart`, the runs a shutdown
/// interrupted are resumed by recovery; the one whose script was edited stays paused with
/// the replay mismatch as its reason, and is still interrupted.
#[tokio::test(flavor = "multi_thread")]
async fn recovery_resumes_interrupted_runs_and_leaves_an_edited_one_paused() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    assert!(
        f.runtime
            .configure_workflow_dir(f.dir.path().join("state/workflows"))
    );
    let owner = f.session("default").await;
    let kept = f.start(&owner, &held_pair("k")).await;
    let edited = f.start(&owner, &held_pair("e")).await;
    eventually("both in flight", || {
        agents.holding.load(Ordering::SeqCst) == 2
    })
    .await;
    let file = edited.script_file.clone().unwrap();
    let source = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, format!("{source}// edited\n")).unwrap();

    // A shutdown's pause counts as interrupted, like recovery's.
    f.runtime.shutdown().await;
    assert!(f.runtime.workflow_run_interrupted(&kept.id).unwrap());
    agents.release();
    let restarted = runtime_over(&agents, &f.store, f.dir.path());
    restarted.recover_workflow_runs(true).await.unwrap();
    let kept = ended_on(&restarted, &kept.id).await;
    assert_eq!(kept.status, WorkflowRunStatus::Completed, "{kept:?}");
    let edited = restarted.workflow_run(&edited.id).unwrap().unwrap();
    assert_eq!(edited.status, WorkflowRunStatus::Paused, "{edited:?}");
    let reason = edited.reason.clone().unwrap();
    assert!(reason.starts_with("WorkflowReplayMismatch"), "{reason}");
    assert!(restarted.workflow_run_interrupted(&edited.id).unwrap());
    assert_eq!(edited.revision, 1);
    let resumed = resumed_agents(&agents);
    assert_eq!(resumed.len(), 1, "{resumed:?}");
    assert_eq!(resumed[0].0, kept.id);
    restarted.shutdown().await;
}

/// "Resume every interrupted run": three runs a crash interrupted are resumed, of the
/// asked Location only, and a run the user paused stays paused.
#[tokio::test(flavor = "multi_thread")]
async fn resuming_interrupted_runs_leaves_a_user_paused_run_paused() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    // Four agents in flight at once, whatever the runner's core count (the default cap).
    assert!(f.runtime.configure_workflow_concurrency(16));
    let owner = f.session("default").await;
    let mut interrupted = Vec::new();
    for name in ["a", "b", "c"] {
        interrupted.push(f.start(&owner, &held_pair(name)).await.id);
    }
    let paused = f.start(&owner, &held_pair("p")).await;
    eventually("four in flight", || {
        agents.holding.load(Ordering::SeqCst) == 4
    })
    .await;
    f.runtime.pause_workflow_run(&paused.id).await.unwrap();

    let restarted = runtime_over(&agents, &f.store, f.dir.path());
    restarted.recover_workflow_runs(false).await.unwrap();
    f.runtime.shutdown().await;
    agents.release();
    let elsewhere = restarted
        .resume_interrupted_workflow_runs(Some(f.dir.path()))
        .await
        .unwrap();
    assert_eq!(elsewhere, InterruptedResume::default());
    let report = restarted
        .resume_interrupted_workflow_runs(Some(&f.dir.path().join("repo")))
        .await
        .unwrap();
    assert_eq!(report.resumed, interrupted, "{report:?}");
    assert!(report.refused.is_empty(), "{report:?}");
    for id in &interrupted {
        let run = ended_on(&restarted, id).await;
        assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    }
    let paused = restarted.workflow_run(&paused.id).unwrap().unwrap();
    assert_eq!(paused.status, WorkflowRunStatus::Paused);
    assert_eq!(paused.reason.as_deref(), Some("paused by the user"));
    assert!(!restarted.workflow_run_interrupted(&paused.id).unwrap());
    let summary = restarted.workflow_run_summary(&paused.id).unwrap().unwrap();
    assert!(!summary.interrupted, "a user's pause is not a restart's");
    assert!(
        resumed_agents(&agents)
            .iter()
            .all(|(run, _)| interrupted.contains(run))
    );
    restarted.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_writers_default_to_numbered_run_worktrees_and_none_opts_out() {
    let agents = Arc::new(Agents {
        writers: AtomicBool::new(true),
        ..Default::default()
    });
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let run = f
        .start(
            &owner,
            "export default async () => {\n  const pair = await parallel(['a', 'b'].map((p) => () => agent('pair ' + p)));\n  const alone = await agent('alone');\n  const opted = await parallel(['c', 'd'].map((p) => () => agent('none ' + p, { isolation: 'none' })));\n  const readers = await parallel(['e', 'f'].map((p) => () => agent('read ' + p, { agent: 'explore' })));\n  const chosen = await agent('chosen', { isolation: 'worktree' });\n  return { pair, alone, opted, readers, chosen };\n};\n",
        )
        .await;
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    let id = run.id.strip_prefix("run_").unwrap().to_ascii_lowercase();
    let seen = agents.seen();
    let tree = |prompt: &str| {
        seen.iter()
            .find(|s| s.prompt == prompt)
            .unwrap()
            .worktree
            .clone()
    };
    // Concurrent writers each get their own worktree, numbered by start ordinal.
    let mut pair: Vec<RunWorktree> = ["pair a", "pair b"]
        .iter()
        .map(|p| tree(p).expect(p))
        .collect();
    pair.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(pair[0], RunWorktree::new(&run.id, 1));
    assert_eq!(pair[1].name, format!("run-{id}-2"));
    assert_eq!(pair[1].branch, format!("cyber/run-{id}/2"));
    // A single writer stays in place; `none` and read-only profiles opt out.
    for prompt in ["alone", "none c", "none d", "read e", "read f"] {
        assert_eq!(tree(prompt), None, "{prompt}");
    }
    assert_eq!(tree("chosen"), Some(RunWorktree::new(&run.id, 8)));
    // Results and records report the branch.
    let result = run.result.unwrap();
    let branches: Vec<&str> = result["pair"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["branch"].as_str().unwrap())
        .collect();
    assert!(branches.contains(&format!("cyber/run-{id}/1").as_str()));
    assert!(result["alone"].get("branch").is_none());
    assert_eq!(result["chosen"]["branch"], format!("cyber/run-{id}/8"));
    let recorded = f.runtime.workflow_agents(&run.id).unwrap();
    assert_eq!(
        recorded
            .iter()
            .filter(|agent| agent.branch.is_some())
            .count(),
        3
    );
}

/// The owner's queued workflow messages, by ID.
async fn handbacks(runtime: &Runtime, owner: &str) -> Vec<(String, String)> {
    runtime
        .state(owner)
        .await
        .unwrap()
        .inbox
        .iter()
        .filter(|row| row.message_id.starts_with("msg_workflow_"))
        .map(|row| (row.message_id.clone(), row.source.clone()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn ended_runs_queue_one_summary_and_large_results_go_to_a_file() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    assert!(
        f.runtime
            .configure_workflow_dir(f.dir.path().join("state/workflows"))
    );
    let owner = f.session("default").await;
    let run = f
        .start(
            &owner,
            &format!(
                "export default async () => {{ const a = await agent('one'); return {{ text: a.text, big: 'x'.repeat({}) }}; }};\n",
                INLINE_RESULT_BYTES
            ),
        )
        .await;
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    assert!(run.result.is_none());
    let file = run.result_file.clone().unwrap();
    assert_eq!(
        std::path::Path::new(&file),
        f.dir
            .path()
            .join("state/workflows")
            .join(format!("{}.result.json", run.id))
    );
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(stored["text"], "one done");
    assert!(run.result_preview.unwrap().starts_with("{"));
    // The inline script is kept next to it.
    let script = run.script_file.clone().unwrap();
    assert!(
        std::fs::read_to_string(&script)
            .unwrap()
            .contains("agent('one')")
    );

    let message = handback_message_id(&run.id);
    eventually("the summary admission", || {
        f.runtime.workflow_run(&run.id).unwrap().unwrap().notified
    })
    .await;
    let queued = handbacks(&f.runtime, &owner).await;
    assert_eq!(queued, [(message.clone(), "workflow".to_owned())]);
    let text = f
        .runtime
        .state(&owner)
        .await
        .unwrap()
        .inbox
        .iter()
        .find(|row| row.message_id == message)
        .map(|row| format!("{:?}", row.parts))
        .unwrap();
    assert!(text.contains("Workflow run handback"), "{text}");
    assert!(text.contains("result_file"), "{text}");
    assert!(text.contains("\\\"completed\\\":1"), "{text}");

    // A restart with the summary admitted but unrecorded admits it again exactly once.
    let fresh = Arc::new(Agents::default());
    let _ = fresh.store.set(Arc::clone(&f.store));
    f.store
        .transaction(move |db| {
            db.execute(
                "UPDATE workflow_run SET data = json_set(data, '$.notified', json('false'))",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let restarted = runtime_over(&fresh, &f.store, f.dir.path());
    restarted.recover_workflow_runs(false).await.unwrap();
    assert!(restarted.workflow_run(&run.id).unwrap().unwrap().notified);
    assert_eq!(handbacks(&restarted, &owner).await, queued);
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_of_api_runs_have_no_approver() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let run = f
        .start(
            &owner,
            "export default async () => (await agent('one')).text;\n",
        )
        .await;
    let run = f.settled(&run.id).await;
    assert!(!run.interactive && !run.continue_owner);
    assert_eq!(
        f.runtime
            .session_workflow_run(&format!("ses_child_{}", agents.seen()[0].agent_id))
            .await,
        None,
        "fake children are not Sessions"
    );
    assert_eq!(f.runtime.session_workflow_run(&owner).await, None);
    assert_eq!(
        NO_WORKFLOW_APPROVER,
        "No interactive approver for workflow run"
    );
}

/// Pause `run_id` once `hold a` is in flight and `c` settled, let `hold a` finish, and
/// check the run paused with only those two agents started.
async fn pause_after_c_settled(f: &Fixture, agents: &Agents, run_id: &str) {
    eventually("hold a in flight and c settled", || {
        agents.holding.load(Ordering::SeqCst) == 1
            && f.events(run_id, "workflow.agent.settled.1").len() == 1
    })
    .await;
    f.runtime.pause_workflow_run(run_id).await.unwrap();
    agents.release();
    f.quiet(run_id).await;
    let paused = f.runtime.workflow_run(run_id).unwrap().unwrap();
    assert_eq!(paused.status, WorkflowRunStatus::Paused, "{paused:?}");
    let mut prompts = agents.prompts();
    prompts.sort();
    assert_eq!(prompts, ["c", "hold a"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parallel_batch_settled_out_of_order_resumes_through_replay() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    // `hold a` is dispatched (sequenced) first but `c` settles first.
    let run = f
        .start(
            &owner,
            "export default async () => {\n  const [a, c] = await Promise.all([agent('hold a'), agent('c')]);\n  const d = await agent('d');\n  return [a.text, c.text, d.text];\n};\n",
        )
        .await;
    pause_after_c_settled(&f, &agents, &run.id).await;
    let seq = |text: &str| {
        f.events(&run.id, "workflow.agent.started.1")
            .into_iter()
            .find(|event| {
                agents
                    .seen()
                    .iter()
                    .any(|s| s.agent_id == event["id"] && s.prompt == text)
            })
            .map(|event| event["seq"].as_u64().unwrap())
            .unwrap()
    };
    assert_eq!((seq("hold a"), seq("c")), (0, 1));

    f.resume(&run.id, None, false).await.unwrap();
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    assert_eq!(run.result, Some(json!(["hold a done", "c done", "d done"])));
    // a and c replayed from the store in their recorded order; only d reached the host.
    assert_eq!(agents.prompts().len(), 3);
    assert_eq!(agents.prompts()[2], "d");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caught_rejection_beside_a_parallel_agent_replays_without_a_mismatch() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let run = f
        .start(
            &owner,
            "export default async () => {\n  const [a, c] = await Promise.all([\n    agent('hold a'),\n    (async () => { try { return await agent('b', { model: 'galaxy' }); } catch (e) { return agent('c'); } })(),\n  ]);\n  const d = await agent('d');\n  return [a.text, c.text, d.text];\n};\n",
        )
        .await;
    pause_after_c_settled(&f, &agents, &run.id).await;
    // The rejection was sequenced and recorded like any other call.
    let recorded = f.events(&run.id, "workflow.call.recorded.1");
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0]["seq"], 1);
    assert_eq!(recorded[0]["outcome"]["error"]["kind"], "ModelUnavailable");

    f.resume(&run.id, None, false).await.unwrap();
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    assert_eq!(run.result, Some(json!(["hold a done", "c done", "d done"])));
    assert_eq!(agents.prompts().len(), 3);
    assert_eq!(agents.prompts()[2], "d");
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_queued_behind_the_global_limit_do_not_start_after_a_pause() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    assert!(f.runtime.configure_workflow_concurrency(1));
    let owner = f.session("default").await;
    let run = f
        .start(
            &owner,
            "export default () => parallel(['hold a', 'b', 'c'].map((p) => () => agent(p)));\n",
        )
        .await;
    eventually("a in flight", || agents.holding.load(Ordering::SeqCst) == 1).await;
    f.runtime.pause_workflow_run(&run.id).await.unwrap();
    agents.release();
    f.quiet(&run.id).await;
    assert_eq!(agents.prompts(), ["hold a"]);
    let paused = f.runtime.workflow_run(&run.id).unwrap().unwrap();
    assert_eq!(paused.status, WorkflowRunStatus::Paused, "{paused:?}");
    // The queued calls were never recorded as started.
    let recorded = f.runtime.workflow_agents(&run.id).unwrap();
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].status, "completed");

    f.resume(&run.id, None, false).await.unwrap();
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    assert_eq!(agents.prompts(), ["hold a", "b", "c"]);
    assert!(agents.seen().iter().all(|seen| !seen.resume));
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_and_stop_never_overwrite_or_duplicate_the_scripts_own_end() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let terminal = |id: &str| {
        ["completed", "failed", "stopped"]
            .iter()
            .map(|kind| f.events(id, &format!("workflow.run.{kind}.1")).len())
            .sum::<usize>()
    };
    for _ in 0..20 {
        let run = f.start(&owner, "export default () => 1;\n").await;
        let paused = f.runtime.pause_workflow_run(&run.id).await;
        f.quiet(&run.id).await;
        let ended = f.runtime.workflow_run(&run.id).unwrap().unwrap();
        // Paused or not, the script ended on its own: its result is kept.
        assert_eq!(
            ended.status,
            WorkflowRunStatus::Completed,
            "{paused:?} {ended:?}"
        );
        assert_eq!(ended.result, Some(json!(1)));
        assert_eq!(terminal(&run.id), 1);

        let run = f.start(&owner, "export default () => 1;\n").await;
        let stopped = f.runtime.stop_workflow_run(&run.id).await;
        f.quiet(&run.id).await;
        let ended = f.runtime.workflow_run(&run.id).unwrap().unwrap();
        assert_eq!(terminal(&run.id), 1, "{stopped:?} {ended:?}");
        match stopped {
            Ok(_) => assert_eq!(ended.status, WorkflowRunStatus::Stopped),
            Err(_) => assert_eq!(ended.status, WorkflowRunStatus::Completed),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resume_keeps_the_mode_the_run_started_with_as_a_ceiling() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("plan").await;
    let run = f
        .start(
            &owner,
            "export default async () => {\n  const a = await agent('hold a');\n  const b = await agent('b');\n  return [a.text, b.text];\n};\n",
        )
        .await;
    eventually("a in flight", || agents.holding.load(Ordering::SeqCst) == 1).await;
    f.runtime.stop_workflow_run(&run.id).await.unwrap();
    f.quiet(&run.id).await;
    f.runtime.switch_mode(&owner, "bypass").await.unwrap();
    agents.release();
    f.resume(&run.id, None, false).await.unwrap();
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    let modes: Vec<String> = agents.seen().into_iter().map(|s| s.mode).collect();
    assert_eq!(modes, ["plan", "plan", "plan"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resumed_run_hands_back_again_and_drops_an_earlier_result_file() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    assert!(
        f.runtime
            .configure_workflow_dir(f.dir.path().join("state/workflows"))
    );
    let owner = f.session("default").await;
    let budget = RunBudget::resolve(&json!({}), &[Some(&json!({"max_agents": 1}))]).unwrap();
    let run = f
        .start_budgeted(
            &owner,
            &format!(
                "export default async () => {{\n  await agent('a');\n  try {{ await agent('b'); }} catch (e) {{ return 'x'.repeat({}); }}\n  return 'small';\n}};\n",
                INLINE_RESULT_BYTES
            ),
            BTreeMap::new(),
            budget,
        )
        .await;
    let ended = f.settled(&run.id).await;
    assert_eq!(ended.status, WorkflowRunStatus::BudgetExceeded, "{ended:?}");
    assert!(ended.result_file.is_some());
    eventually("the first summary", || {
        f.runtime.workflow_run(&run.id).unwrap().unwrap().notified
    })
    .await;

    let module =
        WorkflowModule::inline(f.runtime.workflow_run_source(&run.id).unwrap().unwrap()).unwrap();
    let resumed = f
        .runtime
        .resume_workflow_run(ResumeWorkflowRun {
            run_id: run.id.clone(),
            module,
            limits: Limits::default(),
            tiers: BTreeMap::new(),
            budget: Some(json!({"max_agents": 3})),
            revision: false,
            share: Default::default(),
        })
        .await
        .unwrap();
    assert!(!resumed.notified && resumed.result_file.is_none());
    let ended = f.settled(&run.id).await;
    assert_eq!(ended.status, WorkflowRunStatus::Completed, "{ended:?}");
    assert_eq!(ended.result, Some(json!("small")));
    assert_eq!(ended.result_file, None);
    assert_eq!(ended.result_preview, None);
    eventually("the second summary", || {
        f.runtime.workflow_run(&run.id).unwrap().unwrap().notified
    })
    .await;
    let queued: Vec<String> = handbacks(&f.runtime, &owner)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        queued,
        [
            handback_message_id(&run.id),
            attempt_handback_message_id(&run.id, 1)
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ctx_budget_replays_as_recorded() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let budget = RunBudget::resolve(&json!({}), &[Some(&json!({"max_agents": 4}))]).unwrap();
    // The script branches on what ctx.budget showed when each result arrived.
    let run = f
        .start_budgeted(
            &owner,
            "export default async () => {\n  const seen = [];\n  while (ctx.budget.agents_started < 2) { seen.push(ctx.budget.agents_started); await agent('step ' + seen.length); }\n  seen.push(ctx.budget.remaining.max_agents);\n  await agent('hold last');\n  await agent('final');\n  return seen;\n};\n",
            BTreeMap::new(),
            budget,
        )
        .await;
    eventually("last in flight", || {
        agents.holding.load(Ordering::SeqCst) == 1
    })
    .await;
    f.runtime.pause_workflow_run(&run.id).await.unwrap();
    agents.release();
    f.quiet(&run.id).await;
    // Three agents started by now; a live ctx.budget would end the loop at once.
    f.resume(&run.id, None, false).await.unwrap();
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    assert_eq!(run.result, Some(json!([0, 1, 2])));
    assert_eq!(agents.prompts(), ["step 1", "step 2", "hold last", "final"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_failed_when_resumed_is_retried_as_a_new_agent() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let run = f
        .start(
            &owner,
            "export default async () => (await agent('flaky x')).text;\n",
        )
        .await;
    eventually("x in flight", || agents.holding.load(Ordering::SeqCst) == 1).await;
    f.runtime.stop_workflow_run(&run.id).await.unwrap();
    f.quiet(&run.id).await;

    // Revision 2 issues the same call: it resumes the unsettled agent, which fails.
    let revised = "export default async () => {\n  const x = await agent('flaky x');\n  return [x.text, ctx.budget.agents_started];\n};\n";
    f.resume(&run.id, Some(revised), true).await.unwrap();
    let failed = f.settled(&run.id).await;
    assert_eq!(failed.status, WorkflowRunStatus::Failed, "{failed:?}");
    let seen = agents.seen();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].resume && seen[1].agent_id == seen[0].agent_id);

    // A relaunch retries the failed call with a new agent, charged against max_agents.
    agents.release();
    f.resume(&run.id, None, false).await.unwrap();
    let run = f.settled(&run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    assert_eq!(run.result, Some(json!(["flaky x done", 2])));
    let seen = agents.seen();
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(!seen[2].resume, "{seen:?}");
    assert_ne!(seen[2].agent_id, seen[0].agent_id);
    assert!(!seen[2].prompt.contains("was interrupted"));
    assert_eq!(f.runtime.workflow_agents(&run.id).unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_paused_while_queued_for_a_slot_leaves_the_queue_at_once() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    assert!(f.runtime.configure_workflow_concurrency(1));
    let owner = f.session("default").await;
    let holder = f
        .start(
            &owner,
            "export default async () => (await agent('hold a')).text;\n",
        )
        .await;
    eventually("a in flight", || agents.holding.load(Ordering::SeqCst) == 1).await;
    let queued = f
        .start(
            &owner,
            "export default async () => (await agent('b')).text;\n",
        )
        .await;
    // Let b queue behind the slot a holds.
    tokio::time::sleep(Duration::from_millis(50)).await;
    f.runtime.pause_workflow_run(&queued.id).await.unwrap();
    // The paused run lets go of its engine while a still holds every slot.
    f.quiet(&queued.id).await;
    assert_eq!(agents.holding.load(Ordering::SeqCst), 1);
    assert_eq!(agents.prompts(), ["hold a"]);
    assert!(f.runtime.workflow_agents(&queued.id).unwrap().is_empty());
    // It resumes while a still runs, and its call is served once the slot frees.
    let resumed = f.resume(&queued.id, None, false).await.unwrap();
    assert_eq!(resumed.status, WorkflowRunStatus::Running);
    agents.release();
    assert_eq!(
        f.settled(&holder.id).await.status,
        WorkflowRunStatus::Completed
    );
    let queued = f.settled(&queued.id).await;
    assert_eq!(queued.status, WorkflowRunStatus::Completed, "{queued:?}");
    assert_eq!(queued.result, Some(json!("b done")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_loop_ends_at_the_wall_budget_and_large_checkpoints_fail_closed() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let budget =
        RunBudget::resolve(&json!({}), &[Some(&json!({"max_wall_seconds": 0.3}))]).unwrap();
    // A script without agents: the first late call rejects, a caught one ends the run.
    let run = f
        .start_budgeted(
            &owner,
            "export default async () => {\n  let caught = 0;\n  for (let i = 0; ; i++) {\n    try { await checkpoint('k' + i, null); } catch (e) { caught++; }\n  }\n};\n",
            BTreeMap::new(),
            budget.clone(),
        )
        .await;
    let ended = f.settled(&run.id).await;
    assert_eq!(ended.status, WorkflowRunStatus::BudgetExceeded, "{ended:?}");
    assert_eq!(
        ended.reason.as_deref(),
        Some("BudgetExceeded: max_wall_seconds")
    );
    let exceeded = f.events(&run.id, "budget.exceeded.1");
    assert_eq!(exceeded.len(), 1, "{exceeded:?}");
    // A script that stops at the rejection keeps its partial result.
    let run = f
        .start_budgeted(
            &owner,
            "export default async () => {\n  for (let i = 0; ; i++) {\n    try { await checkpoint('k' + i, i); } catch (e) { return [e.kind, e.message, i > 0]; }\n  }\n};\n",
            BTreeMap::new(),
            budget,
        )
        .await;
    let ended = f.settled(&run.id).await;
    assert_eq!(ended.status, WorkflowRunStatus::BudgetExceeded, "{ended:?}");
    assert_eq!(
        ended.result,
        Some(json!([
            "BudgetExceeded",
            "BudgetExceeded: max_wall_seconds",
            true
        ]))
    );

    let run = f
        .start(
            &owner,
            &format!(
                "export default async () => {{ try {{ await checkpoint('big', 'x'.repeat({})); }} catch (e) {{ return 'observed'; }} }};\n",
                MAX_CHECKPOINT_BYTES
            ),
        )
        .await;
    let ended = f.settled(&run.id).await;
    assert_eq!(ended.status, WorkflowRunStatus::Failed, "{ended:?}");
    assert_eq!(ended.result, None);
    let error = ended.error.unwrap();
    assert!(error.message.contains("checkpoint \"big\""), "{error}");
    assert!(f.events(&run.id, "workflow.call.recorded.1").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn phase_loops_record_changes_only_and_stop_at_the_title_cap() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let run = f
        .start(
            &owner,
            "export default async () => {\n  for (let i = 0; i < 300; i++) { phase('Scan'); phase('Scan'); await checkpoint('c' + i, null); }\n  return 'ok';\n};\n",
        )
        .await;
    let ended = f.settled(&run.id).await;
    assert_eq!(ended.status, WorkflowRunStatus::Completed, "{ended:?}");
    let phases = f.events(&run.id, "workflow.phase.1");
    assert_eq!(phases.len(), 1, "{phases:?}");
    // A declared title adds nothing to the list, so the event does not carry it.
    assert!(phases[0].get("phases").is_none(), "{phases:?}");
    assert_eq!(ended.phases, ["Scan"]);

    let run = f
        .start(
            &owner,
            "export default async () => {\n  for (let i = 0; ; i++) { phase('p' + i); await checkpoint('x' + i, null); }\n};\n",
        )
        .await;
    let ended = f.settled(&run.id).await;
    assert_eq!(ended.status, WorkflowRunStatus::Failed, "{ended:?}");
    let error = ended.error.unwrap();
    assert!(
        error
            .message
            .contains("more than 256 distinct phase titles"),
        "{error}"
    );
    // Every change added a title; the events together stay bounded by the cap.
    let id = run.id.clone();
    let (count, last): (i64, String) = f
        .store
        .read(move |db| {
            Ok(db.query_row(
                "SELECT COUNT(*), (SELECT data FROM event WHERE aggregate_id = ?1 AND type = 'workflow.phase.1' ORDER BY seq DESC LIMIT 1)
                 FROM event WHERE aggregate_id = ?1 AND type = 'workflow.phase.1'",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(count, 255);
    let last: Value = serde_json::from_str(&last).unwrap();
    assert_eq!(last["phases"].as_array().unwrap().len(), 256);
    let stored = f.runtime.workflow_run(&run.id).unwrap().unwrap();
    assert_eq!(stored.phases.len(), 256);
    assert_eq!(stored.phase.as_deref(), Some("p254"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_agent_session_cannot_own_a_run() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let run = f
        .start(
            &owner,
            "export default async () => (await agent('real child')).text;\n",
        )
        .await;
    assert_eq!(
        f.settled(&run.id).await.status,
        WorkflowRunStatus::Completed
    );
    let child = f.runtime.workflow_agents(&run.id).unwrap()[0]
        .session_id
        .clone()
        .unwrap();
    // A Session below the agent's child is inside the run too.
    let grandchild = f
        .runtime
        .create_session(CreateSession {
            directory: f.dir.path().join("repo").display().to_string(),
            model: "test/main".into(),
            parent_id: Some(child.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    for session in [&child, &grandchild] {
        let refused = f
            .runtime
            .start_workflow_run(StartWorkflowRun {
                module: module("export default () => 1;\n"),
                args: json!({}),
                session_id: session.clone(),
                limits: Limits::default(),
                tiers: BTreeMap::new(),
                budget: default_budget(),
                share: Default::default(),
                starter: Default::default(),
            })
            .await;
        match refused {
            Err(RuntimeError::Conflict(message)) => {
                assert_eq!(message, WORKFLOW_AGENT_STARTS_RUN)
            }
            other => panic!("{session} started a nested run: {other:?}"),
        }
    }
    assert_eq!(f.runtime.workflow_runs(None).unwrap().len(), 1);
}

/// Child Sessions of a run's agents.
fn run_children(store: &Store, run_id: &str) -> Vec<String> {
    let run = run_id.to_owned();
    store
        .read(move |db| {
            let mut statement =
                db.prepare("SELECT id FROM session WHERE workflow_run_id = ?1 ORDER BY id")?;
            let ids = statement
                .query_map([run], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .unwrap()
}

/// `workflows` → Concurrent runs, "Stop does not block another run": run A's stop waits
/// for its agents' child Sessions, which ignore cancellation for 2.5 s; meanwhile run B
/// is paused and its script ends, and both are recorded at once.
#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_run_whose_agents_are_slow_to_exit_does_not_block_another_run() {
    let agents = Arc::new(Agents {
        stall: AtomicBool::new(true),
        ..Default::default()
    });
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let a = f
        .start(
            &owner,
            "export default () => parallel([() => agent('realstall 1'), () => agent('realstall 2')]);\n",
        )
        .await;
    eventually("A's agents in their tool", || {
        agents.stalled.load(Ordering::SeqCst) == 2
    })
    .await;
    let b = f
        .start(
            &owner,
            "export default async () => (await agent('gate b')).text;\n",
        )
        .await;
    eventually("B's agent at its gate", || {
        agents.prompts().iter().any(|p| p == "gate b")
    })
    .await;

    let stopping = {
        let (runtime, id) = (f.runtime.clone(), a.id.clone());
        tokio::spawn(async move { runtime.stop_workflow_run(&id).await })
    };
    let children = run_children(&f.store, &a.id);
    assert_eq!(children.len(), 2);
    eventually("A's stop sweeping its children", || {
        children
            .iter()
            .all(|child| !f.events(child, "session.subtree.stopping.1").is_empty())
    })
    .await;

    let asked = std::time::Instant::now();
    let paused = f.runtime.pause_workflow_run(&b.id).await.unwrap();
    assert_eq!(paused.status, WorkflowRunStatus::Paused);
    assert!(
        asked.elapsed() < Duration::from_millis(100),
        "B's pause waited {:?}",
        asked.elapsed()
    );
    assert!(!stopping.is_finished(), "A's agents are still exiting");

    // B's in-flight agent finishes and its script ends on its own while A still stops.
    agents.gate_permits.open();
    let opened = std::time::Instant::now();
    eventually("B's end recorded", || {
        f.runtime.workflow_run(&b.id).unwrap().unwrap().status == WorkflowRunStatus::Completed
    })
    .await;
    assert!(
        opened.elapsed() < Duration::from_millis(100),
        "B's end waited {:?}",
        opened.elapsed()
    );
    assert!(!stopping.is_finished(), "A's agents are still exiting");
    let b = f.runtime.workflow_run(&b.id).unwrap().unwrap();
    assert_eq!(b.result, Some(json!("gate b done")), "{b:?}");

    let stopped = stopping.await.unwrap().unwrap();
    assert_eq!(stopped.status, WorkflowRunStatus::Stopped);
    f.quiet(&a.id).await;
    assert_eq!(
        f.runtime.workflow_run(&a.id).unwrap().unwrap().status,
        WorkflowRunStatus::Stopped
    );
}

/// `workflows` → Run isolation, "Budget halt stays in its run": run A's cost cap rejects
/// its next agent while run B of the same owner keeps starting agents, and the owner's
/// usage is the sum of both runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_budget_halt_stays_in_its_run_and_the_owner_sees_both_runs_spend() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let b = f
        .start(
            &owner,
            "export default async () => {\n  await agent('gate b');\n  return (await agent('real b')).text;\n};\n",
        )
        .await;
    eventually("B's first agent at its gate", || {
        agents.prompts().iter().any(|p| p == "gate b")
    })
    .await;
    // One Turn costs $0.00012; A's cap is reached after its first agent.
    let capped = RunBudget::resolve(&json!({}), &[Some(&json!({"max_cost_usd": 0.0001}))]).unwrap();
    let a = f
        .start_budgeted(
            &owner,
            "export default async () => {\n  await agent('real a');\n  try { await agent('real never'); } catch (e) { return [e.kind, e.message]; }\n};\n",
            BTreeMap::new(),
            capped,
        )
        .await;
    let a = f.settled(&a.id).await;
    assert_eq!(a.status, WorkflowRunStatus::BudgetExceeded, "{a:?}");
    assert_eq!(
        a.result,
        Some(json!(["BudgetExceeded", "BudgetExceeded: max_cost_usd"]))
    );
    assert_eq!(
        f.runtime.workflow_run(&b.id).unwrap().unwrap().status,
        WorkflowRunStatus::Running
    );

    agents.gate_permits.open();
    let b = f.settled(&b.id).await;
    assert_eq!(b.status, WorkflowRunStatus::Completed, "{b:?}");
    assert_eq!(b.result, Some(json!("real done")));
    assert!(!agents.prompts().iter().any(|p| p == "real never"));

    let (spent_a, spent_b) = (
        f.runtime.run_usage(&a.id).unwrap(),
        f.runtime.run_usage(&b.id).unwrap(),
    );
    assert!(spent_a.cost > 0.0 && spent_b.cost > 0.0);
    let sum = spent_a.cost + spent_b.cost;
    eventually("the owner charged for both runs", || {
        (f.runtime.children_usage(&owner).unwrap().children_cost - sum).abs() < 1e-9
    })
    .await;
    assert_eq!(
        f.runtime.children_usage(&owner).unwrap().children_tokens,
        spent_a.total_tokens + spent_b.total_tokens
    );
}

const REPLAYED: &str = "export default async () => {\n  const a = await agent('rep a');\n  const b = await agent('gate rep b');\n  const c = await agent('rep c');\n  return [a.text, b.text, c.text];\n};\n";

/// `workflows` → Run isolation, "Same script, separate replay": two runs of one inline
/// script share call keys but never results; the second run's resume replays only what
/// it recorded and spawns its own agent for the rest.
#[tokio::test(flavor = "multi_thread")]
async fn runs_of_the_same_script_replay_only_their_own_results() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let first = f.start(&owner, REPLAYED).await;
    agents.gate_permits.open();
    let first = f.settled(&first.id).await;
    assert_eq!(first.status, WorkflowRunStatus::Completed, "{first:?}");

    let second = f.start(&owner, REPLAYED).await;
    eventually("the second run's b at its gate", || {
        agents
            .prompts()
            .iter()
            .filter(|p| *p == "gate rep b")
            .count()
            == 2
    })
    .await;
    f.runtime.pause_workflow_run(&second.id).await.unwrap();
    agents.gate_permits.open();
    f.quiet(&second.id).await;
    assert_eq!(
        f.runtime.workflow_run(&second.id).unwrap().unwrap().status,
        WorkflowRunStatus::Paused
    );
    f.resume(&second.id, None, false).await.unwrap();
    let second = f.settled(&second.id).await;
    assert_eq!(second.status, WorkflowRunStatus::Completed, "{second:?}");
    assert_eq!(second.result, first.result);

    // The resume spawned only c; a and b replayed from the second run's own records.
    assert_eq!(
        agents.prompts(),
        [
            "rep a",
            "gate rep b",
            "rep c",
            "rep a",
            "gate rep b",
            "rep c"
        ]
    );
    assert!(!agents.seen()[5].resume);
    let (mine, theirs) = (
        f.runtime.workflow_agents(&second.id).unwrap(),
        f.runtime.workflow_agents(&first.id).unwrap(),
    );
    assert_eq!(mine.len(), 3);
    for (agent, other) in mine.iter().zip(&theirs) {
        assert_eq!(agent.call_key, other.call_key, "same script, same keys");
        assert_ne!(agent.id, other.id);
        assert_eq!(agent.run_id, second.id);
    }
    let settled = f.events(&second.id, "workflow.agent.settled.1");
    assert_eq!(settled.len(), 3, "{settled:?}");
    let ids: Vec<&str> = mine.iter().map(|agent| agent.id.as_str()).collect();
    assert!(
        settled
            .iter()
            .all(|event| ids.contains(&event["id"].as_str().unwrap_or_default())),
        "{settled:?}"
    );
}

/// `workflows` → Run isolation, "Worktrees of two runs": isolated agents of two runs in
/// one Location get six distinct worktrees and branches, each named with its run ID.
#[tokio::test(flavor = "multi_thread")]
async fn isolated_agents_of_two_runs_get_distinct_worktrees_named_by_run() {
    let agents = Arc::new(Agents {
        writers: AtomicBool::new(true),
        ..Default::default()
    });
    let f = fixture(&agents);
    let owner = f.session("default").await;
    let script = "export default () => parallel([1, 2, 3].map((n) => () => agent('tree ' + n, { isolation: 'worktree' })));\n";
    let (a, b) = (f.start(&owner, script).await, f.start(&owner, script).await);
    for run in [&a, &b] {
        assert_eq!(
            f.settled(&run.id).await.status,
            WorkflowRunStatus::Completed
        );
    }
    let seen: Vec<RunWorktree> = agents
        .seen()
        .into_iter()
        .map(|s| s.worktree.expect("isolated"))
        .collect();
    let expected: Vec<RunWorktree> = [&a, &b]
        .iter()
        .flat_map(|run| (1..=3).map(|n| RunWorktree::new(&run.id, n)))
        .collect();
    assert_eq!(seen.len(), 6);
    for tree in &expected {
        assert_eq!(seen.iter().filter(|s| *s == tree).count(), 1, "{tree:?}");
    }
    let mut names: Vec<&str> = seen.iter().map(|s| s.name.as_str()).collect();
    let mut branches: Vec<&str> = seen.iter().map(|s| s.branch.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    branches.sort_unstable();
    branches.dedup();
    assert_eq!((names.len(), branches.len()), (6, 6));
}

impl Fixture {
    /// Wait until the run ended (completed, failed, budget-exceeded or stopped).
    async fn ended(&self, id: &str) -> WorkflowRun {
        for _ in 0..1000 {
            let run = self.runtime.workflow_run(id).unwrap().unwrap();
            if run.status.is_terminal() {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("run {id} did not end");
    }

    fn status(&self, id: &str) -> (WorkflowRunStatus, Option<u32>) {
        let run = self.runtime.workflow_run(id).unwrap().unwrap();
        (run.status, run.queue_position)
    }

    /// The run budget's open interval start (`None` while its wall clock is stopped).
    fn activated_ms(&self, id: &str) -> Option<i64> {
        let id = id.to_owned();
        self.store
            .read(move |db| {
                Ok(db.query_row(
                    "SELECT activated_ms FROM workflow_run_budget WHERE run_id = ?1",
                    [id],
                    |row| row.get::<_, Option<i64>>(0),
                )?)
            })
            .unwrap()
    }
}

/// Wait until `n` gated agents wait for an opening.
async fn gated(agents: &Agents, n: usize) {
    eventually("gated agents", || {
        agents
            .prompts()
            .iter()
            .filter(|p| p.starts_with("gate"))
            .count()
            >= n
    })
    .await;
}

/// `workflows` → Run queue, "Ninth run queued" with a limit of 2: the third run is
/// recorded `queued` at position 1, starts no agent and accrues no wall time until a
/// running run ends, then is dispatched.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_started_while_every_run_slot_is_taken_is_queued_until_one_ends() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    assert!(f.runtime.configure_workflow_runs(2));
    assert!(!f.runtime.configure_workflow_runs(5));
    let owner = f.session("default").await;
    let a = f
        .start(&owner, "export default () => agent('gate a');\n")
        .await;
    let b = f
        .start(&owner, "export default () => agent('gate b');\n")
        .await;
    gated(&agents, 2).await;
    let c = f.start(&owner, "export default () => agent('c');\n").await;
    assert_eq!(
        (c.status, c.queue_position),
        (WorkflowRunStatus::Queued, Some(1))
    );
    assert_eq!(
        (a.status, a.queue_position),
        (WorkflowRunStatus::Running, None)
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(f.status(&c.id), (WorkflowRunStatus::Queued, Some(1)));
    assert!(f.runtime.workflow_agents(&c.id).unwrap().is_empty());
    assert_eq!(
        f.activated_ms(&c.id),
        None,
        "a queued run accrues no wall time"
    );
    assert!(f.activated_ms(&a.id).is_some());
    // Listed with its position too.
    let listed = f.runtime.workflow_runs(Some(&owner)).unwrap();
    let positions: Vec<_> = listed.iter().map(|run| run.queue_position).collect();
    assert_eq!(positions, [Some(1), None, None]);

    // Both gated agents wait; release A's, not whichever reached the gate first.
    agents.gate_permits.open_for("gate a");
    assert_eq!(f.ended(&a.id).await.status, WorkflowRunStatus::Completed);
    let c = f.ended(&c.id).await;
    assert_eq!(c.status, WorkflowRunStatus::Completed, "{c:?}");
    assert_eq!(c.queue_position, None);
    assert_eq!(f.events(&c.id, "workflow.run.dispatched.1").len(), 1);
    assert_eq!(
        f.kinds(&c.id)[..2],
        ["workflow.run.started.1", "workflow.run.dispatched.1"]
    );
    assert_eq!(f.status(&b.id).0, WorkflowRunStatus::Running);
    agents.gate_permits.open_for("gate b");
    assert_eq!(f.ended(&b.id).await.status, WorkflowRunStatus::Completed);
}

/// `workflows` → Run queue, "Dispatch order": Q1 queued before Q2 is dispatched first when
/// a running run ends, and Q2 moves up to position 1.
#[tokio::test(flavor = "multi_thread")]
async fn queued_runs_are_dispatched_in_queue_order() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    f.runtime.configure_workflow_runs(2);
    let owner = f.session("default").await;
    let a = f
        .start(&owner, "export default () => agent('gate a');\n")
        .await;
    let b = f
        .start(&owner, "export default () => agent('gate b');\n")
        .await;
    gated(&agents, 2).await;
    let q1 = f
        .start(&owner, "export default () => agent('gate q1');\n")
        .await;
    let q2 = f.start(&owner, "export default () => agent('q2');\n").await;
    assert_eq!((q1.queue_position, q2.queue_position), (Some(1), Some(2)));

    agents.gate_permits.open_for("gate a");
    assert_eq!(f.ended(&a.id).await.status, WorkflowRunStatus::Completed);
    gated(&agents, 3).await;
    assert_eq!(f.status(&q1.id), (WorkflowRunStatus::Running, None));
    assert_eq!(f.status(&q2.id), (WorkflowRunStatus::Queued, Some(1)));
    assert_eq!(f.events(&q2.id, "workflow.run.dispatched.1").len(), 0);
    for _ in 0..2 {
        agents.gate_permits.open();
    }
    for run in [&b, &q1, &q2] {
        assert_eq!(f.ended(&run.id).await.status, WorkflowRunStatus::Completed);
    }
    assert_eq!(agents.prompts(), ["gate a", "gate b", "gate q1", "q2"]);
}

/// `workflows` → Run queue, "Stop a queued run": it ends `stopped` without starting an
/// agent, the other queued runs keep their order, and its owner gets one summary.
#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_queued_run_starts_nothing_and_hands_back_one_summary() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    f.runtime.configure_workflow_runs(1);
    let owner = f.session("default").await;
    let other = f.session("default").await;
    let a = f
        .start(&owner, "export default () => agent('gate a');\n")
        .await;
    gated(&agents, 1).await;
    let q1 = f.start(&owner, "export default () => agent('q1');\n").await;
    let q2 = f.start(&other, "export default () => agent('q2');\n").await;
    let q3 = f.start(&owner, "export default () => agent('q3');\n").await;
    assert_eq!(q3.queue_position, Some(3));

    let stopped = f.runtime.stop_workflow_run(&q2.id).await.unwrap();
    assert_eq!(stopped.status, WorkflowRunStatus::Stopped);
    assert_eq!(stopped.queue_position, None);
    assert_eq!(f.status(&q1.id), (WorkflowRunStatus::Queued, Some(1)));
    assert_eq!(f.status(&q3.id), (WorkflowRunStatus::Queued, Some(2)));
    eventually("the summary admission", || {
        f.runtime.workflow_run(&q2.id).unwrap().unwrap().notified
    })
    .await;
    assert_eq!(handbacks(&f.runtime, &other).await.len(), 1);

    agents.gate_permits.open();
    for run in [&a, &q1, &q3] {
        assert_eq!(f.ended(&run.id).await.status, WorkflowRunStatus::Completed);
    }
    assert_eq!(agents.prompts(), ["gate a", "q1", "q3"]);
    assert_eq!(f.status(&q2.id).0, WorkflowRunStatus::Stopped);
    assert!(f.events(&q2.id, "workflow.run.dispatched.1").is_empty());
    assert_eq!(f.events(&q2.id, "workflow.run.notified.1").len(), 1);
    assert!(f.runtime.workflow_agents(&q2.id).unwrap().is_empty());
}

/// `workflows` → Run lifecycle states, "Pause a queued run": it becomes `paused` without
/// any agent having started and leaves the queue; a resume puts it at the end.
#[tokio::test(flavor = "multi_thread")]
async fn a_paused_queued_run_leaves_the_queue_and_a_resume_requeues_it_last() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    f.runtime.configure_workflow_runs(1);
    let owner = f.session("default").await;
    let a = f
        .start(&owner, "export default () => agent('gate a');\n")
        .await;
    gated(&agents, 1).await;
    let q1 = f.start(&owner, "export default () => agent('q1');\n").await;
    let q2 = f
        .start(&owner, "export default () => agent('gate q2');\n")
        .await;

    let paused = f.runtime.pause_workflow_run(&q1.id).await.unwrap();
    assert_eq!(
        (paused.status, paused.queue_position),
        (WorkflowRunStatus::Paused, None)
    );
    assert_eq!(f.status(&q2.id), (WorkflowRunStatus::Queued, Some(1)));
    assert_eq!(f.activated_ms(&q1.id), None);
    let resumed = f.resume(&q1.id, None, false).await.unwrap();
    assert_eq!(
        (resumed.status, resumed.queue_position),
        (WorkflowRunStatus::Queued, Some(2))
    );
    assert_eq!(
        f.activated_ms(&q1.id),
        None,
        "a requeued run's clock waits too"
    );

    agents.gate_permits.open();
    assert_eq!(f.ended(&a.id).await.status, WorkflowRunStatus::Completed);
    gated(&agents, 2).await;
    assert_eq!(f.status(&q1.id), (WorkflowRunStatus::Queued, Some(1)));
    agents.gate_permits.open();
    for run in [&q2, &q1] {
        assert_eq!(f.ended(&run.id).await.status, WorkflowRunStatus::Completed);
    }
    assert_eq!(agents.prompts(), ["gate a", "gate q2", "q1"]);
    assert_eq!(
        f.kinds(&q1.id),
        [
            "workflow.run.started.1",
            "workflow.run.paused.1",
            "workflow.run.resumed.1",
            "workflow.run.dispatched.1",
            "workflow.agent.started.1",
            "workflow.agent.settled.1",
            "workflow.run.completed.1",
        ]
    );

    // A paused queued run is not dispatched when a slot frees.
    let b = f
        .start(&owner, "export default () => agent('gate b');\n")
        .await;
    gated(&agents, 3).await;
    let q3 = f.start(&owner, "export default () => agent('q3');\n").await;
    f.runtime.pause_workflow_run(&q3.id).await.unwrap();
    agents.gate_permits.open();
    assert_eq!(f.ended(&b.id).await.status, WorkflowRunStatus::Completed);
    f.quiet(&b.id).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(f.status(&q3.id).0, WorkflowRunStatus::Paused);
    assert!(f.runtime.workflow_agents(&q3.id).unwrap().is_empty());
}

/// A pause racing dispatch is serialized under the run's lock: a run paused before its
/// dispatch was recorded never starts an engine.
#[tokio::test(flavor = "multi_thread")]
async fn a_pause_racing_dispatch_never_starts_the_paused_run() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    f.runtime.configure_workflow_runs(1);
    let owner = f.session("default").await;
    for _ in 0..10 {
        let a = f.start(&owner, "export default () => agent('a');\n").await;
        let q = f.start(&owner, "export default () => agent('q');\n").await;
        let _ = f.runtime.pause_workflow_run(&q.id).await;
        f.ended(&a.id).await;
        f.quiet(&a.id).await;
        let kinds = f.kinds(&q.id);
        let paused = kinds.iter().position(|k| k == "workflow.run.paused.1");
        let dispatched = kinds.iter().position(|k| k == "workflow.run.dispatched.1");
        match (paused, dispatched) {
            (Some(p), Some(d)) => assert!(d < p, "dispatched after its pause: {kinds:?}"),
            (Some(_), None) => {
                assert!(f.runtime.workflow_agents(&q.id).unwrap().is_empty());
                f.runtime.stop_workflow_run(&q.id).await.unwrap();
            }
            other => panic!("{other:?}: {kinds:?}"),
        }
        let _ = f.runtime.stop_workflow_run(&q.id).await;
        f.quiet(&q.id).await;
    }
}

/// `workflows` → Run queue, "Wall budget waits in the queue": a 0.3 s wall budget survives
/// a 1 s wait in the queue, and the script sees nearly all of it remaining.
#[tokio::test(flavor = "multi_thread")]
async fn the_wall_budget_does_not_run_while_queued() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    f.runtime.configure_workflow_runs(1);
    let owner = f.session("default").await;
    let a = f
        .start(&owner, "export default () => agent('gate a');\n")
        .await;
    gated(&agents, 1).await;
    // Longer than the whole wall budget is spent queued; only time after dispatch counts.
    const WALL: f64 = 3.0;
    let budget =
        RunBudget::resolve(&json!({}), &[Some(&json!({"max_wall_seconds": WALL}))]).unwrap();
    let q = f
        .start_budgeted(
            &owner,
            "export default async () => {\n  await agent('q');\n  checkpoint('after', 1);\n  return ctx.budget.remaining.max_wall_seconds;\n};\n",
            BTreeMap::new(),
            budget,
        )
        .await;
    assert_eq!(q.status, WorkflowRunStatus::Queued);
    tokio::time::sleep(Duration::from_secs_f64(WALL + 0.5)).await;
    agents.gate_permits.open();
    f.ended(&a.id).await;
    let q = f.ended(&q.id).await;
    assert_eq!(q.status, WorkflowRunStatus::Completed, "{q:?}");
    let remaining = q.result.unwrap().as_f64().unwrap();
    assert!(remaining > 0.0 && remaining <= WALL, "{remaining}");
}

/// The next `workflow.run.progress` of `run_id` that satisfies `wanted`, within `within`.
async fn progress_until(
    live: &mut tokio::sync::broadcast::Receiver<LiveEvent>,
    run_id: &str,
    within: Duration,
    wanted: impl Fn(&WorkflowRunProgress) -> bool,
) -> WorkflowRunProgress {
    tokio::time::timeout(within, async {
        loop {
            match live.recv().await {
                Ok(LiveEvent::WorkflowProgress { progress, .. })
                    if progress.run_id == run_id && wanted(&progress) =>
                {
                    return progress;
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .expect("a progress event")
}

/// `workflows` → Run monitor, "Queued agents counted": with a limit of 16 and 100 agents
/// issued at once, the summary reads 16 running and 84 queued, and a progress event with
/// the same counts follows each change.
#[tokio::test(flavor = "multi_thread")]
async fn queued_agents_are_counted_and_each_change_is_followed_by_a_progress_event() {
    let agents = Arc::new(Agents::default());
    let f = fixture(&agents);
    assert!(f.runtime.configure_workflow_concurrency(16));
    let owner = f.session("default").await;
    let mut live = f.runtime.subscribe();
    let calls: Vec<String> = (1..=100).map(|n| format!("'gate q{n}'")).collect();
    let run = f
        .start(
            &owner,
            &format!(
                "export default () => parallel([{}].map((p) => () => agent(p)));\n",
                calls.join(", ")
            ),
        )
        .await;
    gated(&agents, 16).await;
    let summary = || f.runtime.workflow_run_summary(&run.id).unwrap().unwrap();
    eventually("84 calls queue", || summary().agents.queued == 84).await;
    let listed = summary().agents;
    assert_eq!(
        (
            listed.total,
            listed.running,
            listed.queued,
            listed.completed
        ),
        (100, 16, 84, 0),
        "{listed:?}"
    );
    let counts = |p: &WorkflowRunProgress| (p.agents.running, p.agents.queued);
    let progress = progress_until(&mut live, &run.id, Duration::from_secs(2), |p| {
        counts(p) == (16, 84)
    })
    .await;
    assert_eq!(progress.agents, listed);
    assert_eq!(progress.status, WorkflowRunStatus::Running);

    // One agent ends: the next queued call starts, and an event follows within the bound.
    let opened = std::time::Instant::now();
    agents.gate_permits.open();
    let progress = progress_until(&mut live, &run.id, Duration::from_secs(2), |p| {
        p.agents.completed == 1 && counts(p) == (16, 83)
    })
    .await;
    assert!(
        opened.elapsed() < Duration::from_millis(250 + 750),
        "{:?}",
        opened.elapsed()
    );
    assert_eq!(progress.agents, summary().agents);

    f.runtime.stop_workflow_run(&run.id).await.unwrap();
    let stopped = progress_until(&mut live, &run.id, Duration::from_secs(5), |p| {
        p.status == WorkflowRunStatus::Stopped && p.agents.running == 0
    })
    .await;
    f.quiet(&run.id).await;
    assert_eq!(stopped.agents.queued, 0);
    assert_eq!(stopped.agents, summary().agents);
}
