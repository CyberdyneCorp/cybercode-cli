//! Workflow Runs in the server runtime with a fake agent host (`workflows` → agent() call,
//! Durable agent results, Concurrency limit, Model tiers and mixing, Phases and logs).
mod support;

use std::collections::BTreeMap;
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
        let child = runtime
            .create_session(CreateSession {
                directory: owner.directory.clone(),
                model: "test/main".into(),
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
        Vec::new()
    }

    fn execute(&self, _call: Invocation, _cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async { ToolOutcome::Failed("no tools".into()) })
    }

    fn spawn_workflow_agent(
        &self,
        request: WorkflowAgentRequest,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<WorkflowAgentOutput, Diagnostic>> {
        Box::pin(async move {
            // Held until this agent settles, as a real host does.
            let _permit = request.permit;
            self.check_recorded(&request.run_id, &request.prompt);
            self.seen.lock().unwrap().push(Seen {
                prompt: request.prompt.clone(),
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
    let models = support::models(vec![("test/main", steps)], 200_000, &[]);
    Runtime::new(RuntimeOptions {
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
    })
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

#[tokio::test(flavor = "multi_thread")]
async fn the_global_limit_is_fifo_across_runs() {
    let agents = Arc::new(Agents {
        delay: Duration::from_millis(100),
        slow: Some("a2"),
        ..Agents::default()
    });
    let f = fixture(&agents);
    assert!(f.runtime.configure_workflow_concurrency(2));
    assert!(!f.runtime.configure_workflow_concurrency(5));
    let owner = f.session("default").await;
    let fan_out = |prefix: &str| {
        format!(
            "export default () => parallel(['{prefix}1', '{prefix}2', '{prefix}3'].map((p) => () => agent(p)));\n"
        )
    };
    let first = f.start(&owner, &fan_out("a")).await;
    for _ in 0..500 {
        if agents.seen().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Let a3 queue behind the two running agents before the second run queues its own.
    tokio::time::sleep(Duration::from_millis(30)).await;
    let second = f.start(&owner, &fan_out("b")).await;
    assert_eq!(
        f.settled(&first.id).await.status,
        WorkflowRunStatus::Completed
    );
    assert_eq!(
        f.settled(&second.id).await.status,
        WorkflowRunStatus::Completed
    );
    assert_eq!(agents.peak.load(Ordering::SeqCst), 2);
    let order: Vec<String> = agents.seen().into_iter().map(|s| s.prompt).collect();
    let a3 = order.iter().position(|p| p == "a3").unwrap();
    assert!(
        order
            .iter()
            .filter(|p| p.starts_with('b'))
            .all(|b| { order.iter().position(|p| p == b).unwrap() > a3 }),
        "{order:?}"
    );
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
        &[Some(&json!({"max_wall_seconds": 0.3, "max_cost_usd": 1.0}))],
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
    tokio::time::sleep(Duration::from_millis(400)).await;
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
    other.recover_workflow_runs().await.unwrap();
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
    restarted.recover_workflow_runs().await.unwrap();
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
    restarted.recover_workflow_runs().await.unwrap();
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
