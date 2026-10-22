//! Workflow agents through the built-in agent path (`workflows` → agent() call,
//! Concurrency limit; `agents-subagents` → Concurrency cap): real child Sessions on a
//! scripted model, started by a Workflow Run in the runtime.
mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cyber_llm::adapters::{EventStream, ScriptStep};
use cyber_llm::{Adapter, ErrorKind, FinishReason, LlmError, LlmEvent, LlmRequest, Usage};
use cyber_server::runtime::*;
use cyber_workflow::{Limits, WorkflowModule};
use futures::future::BoxFuture;
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use support::Fixture;
use support::flow::{Flow, call, text};

fn module(body: &str) -> WorkflowModule {
    WorkflowModule::inline(format!(
        "export const meta = {{ name: 'agents', description: 'Workflow agents' }};\n{body}"
    ))
    .unwrap()
}

/// Start a run without waiting for it.
async fn start(flow: &Flow, owner: &str, body: &str) -> String {
    flow.runtime
        .start_workflow_run(StartWorkflowRun {
            module: module(body),
            args: json!({}),
            session_id: owner.into(),
            limits: Limits::default(),
            tiers: BTreeMap::new(),
            budget: cyber_core::budget::RunBudget::resolve(&json!({}), &[]).unwrap(),
            share: Default::default(),
            starter: Default::default(),
            tools: Vec::new(),
        })
        .await
        .unwrap()
        .id
}

/// Poll a run until it leaves `running`.
async fn settled(flow: &Flow, id: &str) -> WorkflowRun {
    for _ in 0..1000 {
        let run = flow.runtime.workflow_run(id).unwrap().unwrap();
        if run.status != WorkflowRunStatus::Running {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {id} did not settle");
}

async fn run(flow: &Flow, owner: &str, body: &str) -> WorkflowRun {
    let id = start(flow, owner, body).await;
    settled(flow, &id).await
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

/// A finished text answer, as a provider streams it.
fn answer(text: &str) -> EventStream {
    let events = vec![
        Ok(LlmEvent::TextDelta { text: text.into() }),
        Ok(LlmEvent::Usage(Usage {
            input: 10,
            output: 1,
            ..Usage::default()
        })),
        Ok(LlmEvent::Finish {
            reason: FinishReason::Stop,
        }),
    ];
    stream::iter(events).boxed()
}

fn schema() -> Value {
    json!({"type":"object","required":["findings"],"additionalProperties":false,
        "properties":{"findings":{"type":"array","items":{"type":"string"}}}})
}

#[tokio::test(flavor = "multi_thread")]
async fn agents_are_child_sessions_of_the_owner_under_the_run_mode() {
    let flow = Flow::new(
        vec![
            text("plain answer"),
            call("result", "return_result", json!({"findings": ["one"]})),
        ],
        false,
    );
    let owner = flow.session("plan").await;
    let body = format!(
        "export default async () => {{\n  const plain = await agent('first task');\n  const found = await agent('second task', {{ schema: {} }});\n  return {{ plain, found }};\n}};\n",
        schema()
    );
    let run = run(&flow, &owner, &body).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    let result = run.result.unwrap();
    assert_eq!(result["plain"]["text"], "plain answer");
    assert_eq!(result["found"], json!({"findings": ["one"]}));
    assert!(
        result["plain"]["tokens"].as_u64().unwrap() >= 110,
        "{result}"
    );

    let agents = flow.runtime.workflow_agents(&run.id).unwrap();
    assert_eq!(agents.len(), 2);
    for agent in &agents {
        let child = flow
            .runtime
            .state(agent.session_id.as_deref().unwrap())
            .await
            .unwrap()
            .info;
        assert_eq!(child.workflow_run_id.as_deref(), Some(run.id.as_str()));
        assert_eq!(child.parent_id.as_deref(), Some(owner.as_str()));
        assert_eq!(child.subagent_name.as_deref(), Some(agent.id.as_str()));
        assert!(agent.id.starts_with("agt_"));
        assert_eq!(child.agent, "general");
        // The run Mode is the ceiling of every agent.
        assert_eq!(child.mode, "plan");
        assert!(!flow.runtime.is_running(&child.id));
    }
    assert_eq!(result["plain"]["session_id"], json!(agents[0].session_id));
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .tools
            .iter()
            .any(|tool| tool.name == "return_result" && tool.input_schema == schema())
    );
    // The owner Session stays idle and usable.
    assert!(flow.runtime.state(&owner).await.unwrap().entries.is_empty());
}

/// `CYBER_WORKFLOW_RUN_ID` reaches the processes a workflow agent's tools start, and only
/// those (`workflows` → agent() call).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_agents_shell_sees_its_run_id() {
    let command = r#"printf 'run=[%s]' "$CYBER_WORKFLOW_RUN_ID""#;
    let flow = Flow::new(
        vec![
            call("b1", "bash", json!({"command": command})),
            text("done"),
            call("b2", "bash", json!({"command": command})),
            text("owner done"),
        ],
        false,
    );
    let owner = flow.session("bypass").await;
    let body = "export default async () => (await agent('task')).text;";
    let run = run(&flow, &owner, body).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    let agents = flow.runtime.workflow_agents(&run.id).unwrap();
    let child = agents[0].session_id.as_deref().unwrap();
    let seen = flow.output(child, "b1").await;
    assert!(seen.contains(&format!("run=[{}]", run.id)), "{seen}");

    // The owner's own tools run outside the run.
    flow.prompt(&owner, "go").await;
    flow.settle(&owner).await;
    let seen = flow.output(&owner, "b2").await;
    assert!(seen.contains("run=[]"), "{seen}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_result_is_retried_once_then_schema_mismatch() {
    let flow = Flow::new(vec![text("no result"), text("still no result")], false);
    let owner = flow.session("default").await;
    let body = format!(
        "export default async () => {{\n  try {{ await agent('inspect', {{ schema: {} }}); }} catch (e) {{ return [e.kind, e.message]; }}\n}};\n",
        schema()
    );
    let run = run(&flow, &owner, &body).await;
    let result = run.result.unwrap();
    assert_eq!(result[0], "SchemaMismatch", "{result}");
    assert!(!result[1].as_str().unwrap().starts_with("SchemaMismatch"));
    // The answer and exactly one result retry.
    assert_eq!(flow.main.requests().len(), 2);
    let agents = flow.runtime.workflow_agents(&run.id).unwrap();
    assert_eq!(agents[0].status, "failed");
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_errors_reject_with_agent_failed_and_denials_with_permission_denied() {
    let flow = Flow::new(
        vec![vec![ScriptStep::Error {
            error: ErrorKind::InvalidRequest,
            message: "bad request".into(),
        }]],
        false,
    );
    let owner = flow.session("default").await;
    let run_result = run(
        &flow,
        &owner,
        "export default async () => { try { await agent('x'); } catch (e) { return e.kind; } };\n",
    )
    .await;
    assert_eq!(
        run_result.result,
        Some(json!("AgentFailed")),
        "{run_result:?}"
    );

    flow.f
        .set_config(json!({"permissions":{"agent":{"general":"deny"}}}));
    let denied = run(
        &flow,
        &owner,
        "export default async () => { try { await agent('x'); } catch (e) { return e.kind; } };\n",
    )
    .await;
    assert_eq!(denied.result, Some(json!("PermissionDenied")), "{denied:?}");
}

/// Answers only once `expected` streams are open at the same time.
struct Rendezvous {
    expected: usize,
    arrived: AtomicUsize,
}

impl Adapter for Rendezvous {
    fn stream(&self, _request: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>> {
        self.arrived.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            for _ in 0..500 {
                if self.arrived.load(Ordering::SeqCst) >= self.expected {
                    let events = vec![
                        Ok(LlmEvent::TextDelta { text: "met".into() }),
                        Ok(LlmEvent::Usage(Usage {
                            input: 10,
                            output: 1,
                            ..Usage::default()
                        })),
                        Ok(LlmEvent::Finish {
                            reason: FinishReason::Stop,
                        }),
                    ];
                    return Ok(stream::iter(events).boxed());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(LlmError::new(
                ErrorKind::InvalidRequest,
                "agents never ran concurrently",
            ))
        })
    }
}

/// Resolves `reference` to `adapter` and every other model reference to `test/main`.
struct WithModel {
    inner: Arc<dyn ModelResolver>,
    reference: &'static str,
    adapter: Arc<dyn Adapter>,
}

impl ModelResolver for WithModel {
    fn resolve(&self, model_ref: &str) -> Result<ResolvedModel, String> {
        let mut resolved = self.inner.resolve("test/main")?;
        if model_ref == self.reference {
            resolved.adapter = Arc::clone(&self.adapter);
            resolved.model = model_ref.rsplit('/').next().unwrap_or_default().into();
        }
        Ok(resolved)
    }

    fn role(&self, role: cyber_llm::catalog::ModelRole) -> Option<String> {
        self.inner.role(role)
    }
}

/// A flow whose `reference` model is `adapter`, over `fixture`, with `script` on `test/main`.
fn flow_with(
    fixture: Fixture,
    script: Vec<Vec<ScriptStep>>,
    reference: &'static str,
    adapter: Arc<dyn Adapter>,
) -> Flow {
    Flow::with_resolver_factory(
        fixture,
        script,
        false,
        Arc::new(NoSnapshots),
        Vec::new(),
        move |inner| {
            Arc::new(WithModel {
                inner,
                reference,
                adapter,
            })
        },
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn workflow_agents_are_not_limited_by_agents_max_concurrent() {
    let adapter = Arc::new(Rendezvous {
        expected: 3,
        arrived: AtomicUsize::new(0),
    });
    let flow = flow_with(
        Fixture::new(),
        Vec::new(),
        "test/rendezvous",
        Arc::clone(&adapter) as Arc<dyn Adapter>,
    );
    flow.f.set_config(json!({"agents":{"max_concurrent":1}}));
    assert!(flow.runtime.configure_workflow_concurrency(3));
    let owner = flow.session("default").await;
    let run = run(
        &flow,
        &owner,
        "export default async () => (await parallel([1, 2, 3].map((i) => () => agent('task ' + i, { model: 'test/rendezvous' })))).map((r) => r.text);\n",
    )
    .await;
    assert_eq!(run.result, Some(json!(["met", "met", "met"])), "{run:?}");
    assert_eq!(adapter.arrived.load(Ordering::SeqCst), 3);
}

fn git(path: &std::path::Path, arguments: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "user.name=Workflow test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

/// A native-git fixture whose repository has one commit.
fn committed_checkout() -> Fixture {
    let fixture = support::Fixture::native_git();
    git(&fixture.repo, &["init", "--quiet", "-b", "main"]);
    fixture.write("tracked.txt", "baseline\n");
    git(&fixture.repo, &["add", "tracked.txt"]);
    git(&fixture.repo, &["commit", "--quiet", "-m", "initial"]);
    fixture
}

/// The child Session directory and branch of each settled agent, by label.
async fn checkouts(flow: &Flow, run: &WorkflowRun) -> BTreeMap<String, (String, Option<String>)> {
    let mut found = BTreeMap::new();
    for agent in flow.runtime.workflow_agents(&run.id).unwrap() {
        let child = flow
            .runtime
            .state(agent.session_id.as_deref().unwrap())
            .await
            .unwrap();
        assert_eq!(
            child.child_worktree().map(|tree| tree.branch.clone()),
            agent.branch,
            "{agent:?}"
        );
        found.insert(
            agent.label.clone().unwrap(),
            (child.info.directory, agent.branch),
        );
    }
    found
}

// Native git runs through the sandbox helper; `Fixture::native_git` selects explicit
// full access on Windows only, like the other native worktree fixtures.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_writers_get_numbered_run_worktrees_and_a_single_writer_stays_in_place() {
    let fixture = committed_checkout();
    let flow = Flow::with(
        fixture,
        (0..5).map(|n| text(&format!("answer {n}"))).collect(),
        false,
        Arc::new(NoSnapshots),
    );
    let owner = flow.session("default").await;
    let run = run(
        &flow,
        &owner,
        "export default async () => {\n  const pair = await parallel([\n    () => agent('first writer', { label: 'a' }),\n    () => agent('second writer', { label: 'b' }),\n  ]);\n  const alone = await agent('single writer', { label: 'alone' });\n  const shared = await parallel([\n    () => agent('third writer', { label: 'c', isolation: 'none' }),\n    () => agent('fourth writer', { label: 'd', isolation: 'none' }),\n  ]);\n  return { pair, alone, shared };\n};\n",
    )
    .await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    let id = run.id.strip_prefix("run_").unwrap().to_ascii_lowercase();
    let result = run.result.clone().unwrap();
    let found = checkouts(&flow, &run).await;
    let repo = flow.f.repo.display().to_string();
    let mut branches = Vec::new();
    for (index, label) in ["a", "b"].iter().enumerate() {
        let (directory, branch) = &found[*label];
        let branch = branch.clone().unwrap();
        assert_ne!(directory, &repo);
        assert!(std::path::Path::new(directory).is_dir(), "{directory}");
        assert_eq!(result["pair"][index]["branch"], json!(branch));
        assert!(git(&flow.f.repo, &["branch", "--list", &branch]).contains(&branch));
        branches.push(branch);
    }
    branches.sort();
    assert_eq!(
        branches,
        [format!("cyber/run-{id}/1"), format!("cyber/run-{id}/2")]
    );
    assert_ne!(found["a"].0, found["b"].0);
    // A lone writer and `isolation: "none"` work in the owner's checkout.
    for label in ["alone", "c", "d"] {
        assert_eq!(found[label], (repo.clone(), None), "{label}");
    }
    assert!(result["alone"].get("branch").is_none());
    // Run worktrees are kept after the run ends, until archive or prune.
    let listed = git(&flow.f.repo, &["worktree", "list"]);
    assert!(listed.contains(&format!("run-{id}-1")), "{listed}");
    assert!(listed.contains(&format!("run-{id}-2")), "{listed}");
    flow.runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_workflow_tool_starts_an_owned_run_and_obeys_workflow_run_rules() {
    let flow = Flow::new(vec![text("tool child answer")], false);
    flow.f
        .set_config(json!({"permissions": {"workflow.run": {"blocked": "deny"}}}));
    let owner = flow.session("bypass").await;
    let script = "export const meta = { name: 'tooled', description: 'From a tool', phases: [{ title: 'One' }] };\nexport default async () => (await agent('child')).text;\n";
    let mut invocation = flow.f.invocation(
        "bypass",
        "workflow",
        json!({"script": script, "args": {}, "budget": {"max_agents": 2}, "max_concurrent": 1}),
    );
    invocation.session_id = owner.clone();
    let output = support::ok(
        flow.f
            .host
            .execute(
                invocation.clone(),
                tokio_util::sync::CancellationToken::new(),
            )
            .await,
    );
    let mut handle: Value = serde_json::from_str(&output).unwrap();
    let run_id = handle["run_id"].as_str().unwrap().to_owned();
    handle["run_id"] = json!("<run>");
    support::golden(&flow.f, "workflow", &handle.to_string());
    let run = flow.runtime.workflow_run(&run_id).unwrap().unwrap();
    assert_eq!(run.session_id, owner);
    assert!(run.continue_owner && !run.interactive);
    for _ in 0..1000 {
        if flow.runtime.workflow_run(&run_id).unwrap().unwrap().status != WorkflowRunStatus::Running
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let run = flow.runtime.workflow_run(&run_id).unwrap().unwrap();
    assert_eq!(run.result, Some(json!("tool child answer")), "{run:?}");

    // A deny rule for the workflow name refuses it even in bypass; nothing starts.
    invocation.input["script"] = json!(script.replace("'tooled'", "'blocked'"));
    let denied = flow
        .f
        .host
        .execute(
            invocation.clone(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(&denied, ToolOutcome::Failed(message) if message.contains("Permission denied")),
        "{denied:?}"
    );
    // A workflow agent cannot start a run of its own.
    let child = flow.runtime.workflow_agents(&run_id).unwrap()[0]
        .session_id
        .clone()
        .unwrap();
    invocation.session_id = child;
    invocation.input["script"] = json!(script);
    let nested = flow
        .f
        .host
        .execute(invocation, tokio_util::sync::CancellationToken::new())
        .await;
    assert!(
        matches!(&nested, ToolOutcome::Failed(message) if message.contains("Workflow agents cannot start")),
        "{nested:?}"
    );
    assert_eq!(flow.runtime.workflow_runs(None).unwrap().len(), 1);
    flow.runtime.shutdown().await;
}

/// Answers in pairs: each stream waits until another one is open beside it, so the two
/// agents of a pair overlap in time.
#[derive(Default)]
struct Pairs {
    arrived: AtomicUsize,
}

impl Adapter for Pairs {
    fn stream(&self, _request: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>> {
        let pair = (self.arrived.fetch_add(1, Ordering::SeqCst) + 1).div_ceil(2) * 2;
        Box::pin(async move {
            for _ in 0..500 {
                if self.arrived.load(Ordering::SeqCst) >= pair {
                    return Ok(answer("paired"));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(LlmError::new(
                ErrorKind::InvalidRequest,
                "no other agent overlapped",
            ))
        })
    }
}

/// A script awaiting one agent on `test/pairs` with `options`.
fn paired(label: &str, options: &str) -> String {
    format!(
        "export default () => agent('{label} writer', {{ label: '{label}', model: 'test/pairs'{options} }});\n"
    )
}

/// `workflows` → Write isolation default across runs: "Writers of two runs isolated",
/// `isolation: "none"` still opting out beside another run's writer, and "Single writer
/// in place" once no other run has an edit-capable agent in the Location.
#[tokio::test(flavor = "multi_thread")]
async fn writers_of_two_runs_in_one_location_are_isolated_and_none_still_opts_out() {
    let pairs = Arc::new(Pairs::default());
    let flow = flow_with(
        committed_checkout(),
        vec![text("lone answer")],
        "test/pairs",
        Arc::clone(&pairs) as Arc<dyn Adapter>,
    );
    let owner = flow.session("default").await;
    let repo = flow.f.repo.display().to_string();

    let a = start(&flow, &owner, &paired("a", "")).await;
    eventually("a in flight", || pairs.arrived.load(Ordering::SeqCst) == 1).await;
    let b = start(&flow, &owner, &paired("b", "")).await;
    let (a, b) = (settled(&flow, &a).await, settled(&flow, &b).await);
    assert_eq!(a.status, WorkflowRunStatus::Completed, "{a:?}");
    assert_eq!(b.status, WorkflowRunStatus::Completed, "{b:?}");
    // The first writer stays in the owner's checkout; the one that starts beside it,
    // in another run, gets its own worktree.
    assert_eq!(checkouts(&flow, &a).await["a"], (repo.clone(), None));
    let (directory, branch) = checkouts(&flow, &b).await["b"].clone();
    let id = b.id.strip_prefix("run_").unwrap().to_ascii_lowercase();
    assert_eq!(
        branch.as_deref(),
        Some(format!("cyber/run-{id}/1").as_str())
    );
    assert_ne!(directory, repo);
    assert!(std::path::Path::new(&directory).is_dir(), "{directory}");
    assert_eq!(b.result.unwrap()["branch"], json!(branch));

    // `isolation: "none"` works in place even beside another run's writer.
    let c = start(&flow, &owner, &paired("c", "")).await;
    eventually("c in flight", || pairs.arrived.load(Ordering::SeqCst) == 3).await;
    let d = start(&flow, &owner, &paired("d", ", isolation: 'none'")).await;
    for (id, label) in [(c, "c"), (d, "d")] {
        let run = settled(&flow, &id).await;
        assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
        assert_eq!(checkouts(&flow, &run).await[label], (repo.clone(), None));
    }

    // With no other run's writer in flight, a lone writer stays in place.
    let lone = run(
        &flow,
        &owner,
        "export default () => agent('lone writer', { label: 'lone' });\n",
    )
    .await;
    assert_eq!(lone.status, WorkflowRunStatus::Completed, "{lone:?}");
    assert_eq!(checkouts(&flow, &lone).await["lone"], (repo, None));
    flow.runtime.shutdown().await;
}

/// Each stream waits for one [`Gated::open`], oldest first; a dropped stream leaves the
/// queue.
struct Gated {
    arrived: AtomicUsize,
    gate: tokio::sync::Semaphore,
}

impl Default for Gated {
    fn default() -> Self {
        Self {
            arrived: AtomicUsize::new(0),
            gate: tokio::sync::Semaphore::new(0),
        }
    }
}

impl Adapter for Gated {
    fn stream(&self, _request: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>> {
        self.arrived.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            self.gate.acquire().await.unwrap().forget();
            Ok(answer("gated answer"))
        })
    }
}

/// How many times each message ID is in `entries`.
fn delivered(entries: &[Entry], ids: &[String]) -> Vec<usize> {
    ids.iter()
        .map(|id| entries.iter().filter(|entry| entry.id() == id).count())
        .collect()
}

/// Start `name` through the owner's `workflow` tool; its one agent waits at the gate.
async fn start_by_tool(flow: &Flow, owner: &str, name: &str) -> String {
    let script = format!(
        "export const meta = {{ name: '{name}', description: 'Gated' }};\nexport default async () => (await agent('{name} task', {{ model: 'test/gate' }})).text;\n"
    );
    let mut invocation =
        flow.f
            .invocation("bypass", "workflow", json!({"script": script, "args": {}}));
    invocation.session_id = owner.into();
    let output = support::ok(
        flow.f
            .host
            .execute(invocation, tokio_util::sync::CancellationToken::new())
            .await,
    );
    let handle: Value = serde_json::from_str(&output).unwrap();
    handle["run_id"].as_str().unwrap().to_owned()
}

/// `workflows` → Concurrent runs, "Two runs from one Session": both run, the owner can be
/// prompted meanwhile, and stopping one leaves the other running with its handback.
#[tokio::test(flavor = "multi_thread")]
async fn two_runs_of_one_session_run_together_and_stop_independently() {
    let gated = Arc::new(Gated::default());
    let flow = flow_with(
        Fixture::new(),
        (0..6).map(|n| text(&format!("owner answer {n}"))).collect(),
        "test/gate",
        Arc::clone(&gated) as Arc<dyn Adapter>,
    );
    let owner = flow.session("bypass").await;
    let review = start_by_tool(&flow, &owner, "review").await;
    eventually("review's agent at the gate", || {
        gated.arrived.load(Ordering::SeqCst) == 1
    })
    .await;
    let plan = start_by_tool(&flow, &owner, "plan").await;
    eventually("plan's agent at the gate", || {
        gated.arrived.load(Ordering::SeqCst) == 2
    })
    .await;

    // The owner answers a prompt while both runs execute.
    flow.runtime
        .admit(
            &owner,
            Admission::text("how are the runs going?", Delivery::Queue),
        )
        .await
        .unwrap();
    flow.runtime.downgrade().wait_idle(&owner).await.unwrap();
    let transcript = format!("{:?}", flow.runtime.state(&owner).await.unwrap().entries);
    assert!(transcript.contains("text: \"owner answer"), "{transcript}");
    for id in [&review, &plan] {
        assert_eq!(
            flow.runtime.workflow_run(id).unwrap().unwrap().status,
            WorkflowRunStatus::Running
        );
    }

    let stopped = flow.runtime.stop_workflow_run(&review).await.unwrap();
    assert_eq!(stopped.status, WorkflowRunStatus::Stopped);
    assert_eq!(
        flow.runtime.workflow_run(&plan).unwrap().unwrap().status,
        WorkflowRunStatus::Running
    );
    gated.gate.add_permits(1);
    let plan_run = settled(&flow, &plan).await;
    assert_eq!(
        plan_run.status,
        WorkflowRunStatus::Completed,
        "{plan_run:?}"
    );
    assert_eq!(plan_run.result, Some(json!("gated answer")));
    assert_eq!(gated.arrived.load(Ordering::SeqCst), 2);

    // Each run hands its summary back to the owner exactly once.
    let ids = [handback_message_id(&review), handback_message_id(&plan)];
    for _ in 0..1000 {
        let state = flow.runtime.state(&owner).await.unwrap();
        if delivered(&state.entries, &ids) == [1, 1] && !flow.runtime.is_running(&owner) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let state = flow.runtime.state(&owner).await.unwrap();
    assert_eq!(
        delivered(&state.entries, &ids),
        [1, 1],
        "{:?}",
        state.entries
    );
    let summary = state
        .entries
        .iter()
        .find(|entry| entry.id() == ids[1])
        .unwrap();
    assert!(
        format!("{summary:?}").contains("gated answer"),
        "{summary:?}"
    );
    flow.runtime.shutdown().await;
}

/// `workflows` → Direct read-only tools on the built-in host: `tools.glob`, `tools.grep`
/// and `tools.read` return structured data from the owner's Location under the built-in
/// checks, and an ask in a non-interactive run is refused for want of an approver.
#[tokio::test(flavor = "multi_thread")]
async fn direct_tools_return_structured_results_and_refuse_unattended_asks() {
    let flow = Flow::new(Vec::new(), false);
    std::fs::create_dir_all(flow.f.repo.join("src")).unwrap();
    std::fs::write(flow.f.repo.join("src/b.rs"), "fn b() {}\n// TODO b\n").unwrap();
    std::fs::write(flow.f.repo.join("src/a.rs"), "// TODO a\nfn a() {}\n").unwrap();
    let outside = tempfile::tempdir().unwrap();
    let owner = flow.session("default").await;
    let source = format!(
        "export const meta = {{ name: 'scan', description: 'Scan', tools: ['glob', 'grep', 'read'] }};\n\
         export default async () => {{\n\
           const files = await tools.glob({{ pattern: 'src/*.rs' }});\n\
           const todos = await tools.grep({{ pattern: 'TODO', include: '*.rs' }});\n\
           const page = await tools.read({{ path: 'src/a.rs', limit: 1 }});\n\
           let refused = null;\n\
           try {{ await tools.glob({{ pattern: '*', path: {:?} }}); }} catch (e) {{ refused = `${{e.kind}}: ${{e.message}}`; }}\n\
           return {{ files, todos, page, refused }};\n\
         }};\n",
        outside.path().display().to_string()
    );
    let run = flow
        .runtime
        .start_workflow_run(StartWorkflowRun {
            module: WorkflowModule::inline(source).unwrap(),
            args: json!({}),
            session_id: owner,
            limits: Limits::default(),
            tiers: BTreeMap::new(),
            budget: cyber_core::budget::RunBudget::resolve(&json!({}), &[]).unwrap(),
            share: Default::default(),
            starter: Default::default(),
            tools: Vec::new(),
        })
        .await
        .unwrap();
    let run = settled(&flow, &run.id).await;
    assert_eq!(run.status, WorkflowRunStatus::Completed, "{run:?}");
    let result = run.result.unwrap();
    assert_eq!(
        result["files"],
        json!({"paths": ["src/a.rs", "src/b.rs"], "truncated": false})
    );
    assert_eq!(
        result["todos"]["matches"],
        json!([
            {"path": "src/a.rs", "line": 1, "text": "// TODO a"},
            {"path": "src/b.rs", "line": 2, "text": "// TODO b"},
        ])
    );
    assert_eq!(
        result["page"],
        json!({"content": "// TODO a\n", "truncated": true, "next_offset": 2})
    );
    assert_eq!(
        result["refused"],
        "PermissionDenied: No interactive approver for workflow run"
    );
}
