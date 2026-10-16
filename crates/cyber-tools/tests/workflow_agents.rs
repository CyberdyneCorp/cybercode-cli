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

async fn run(flow: &Flow, owner: &str, body: &str) -> WorkflowRun {
    let started = flow
        .runtime
        .start_workflow_run(StartWorkflowRun {
            module: module(body),
            args: json!({}),
            session_id: owner.into(),
            limits: Limits::default(),
            tiers: BTreeMap::new(),
            budget: cyber_core::budget::RunBudget::resolve(&json!({}), &[]).unwrap(),
            starter: Default::default(),
        })
        .await
        .unwrap();
    for _ in 0..1000 {
        let run = flow.runtime.workflow_run(&started.id).unwrap().unwrap();
        if run.status != WorkflowRunStatus::Running {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {} did not settle", started.id);
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

struct WithRendezvous {
    inner: Arc<dyn ModelResolver>,
    adapter: Arc<Rendezvous>,
}

impl ModelResolver for WithRendezvous {
    fn resolve(&self, model_ref: &str) -> Result<ResolvedModel, String> {
        let mut resolved = self.inner.resolve("test/main")?;
        if model_ref == "test/rendezvous" {
            resolved.adapter = Arc::clone(&self.adapter) as Arc<dyn Adapter>;
            resolved.model = "rendezvous".into();
        }
        Ok(resolved)
    }

    fn role(&self, role: cyber_llm::catalog::ModelRole) -> Option<String> {
        self.inner.role(role)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn workflow_agents_are_not_limited_by_agents_max_concurrent() {
    let adapter = Arc::new(Rendezvous {
        expected: 3,
        arrived: AtomicUsize::new(0),
    });
    let shared = Arc::clone(&adapter);
    let flow = Flow::with_resolver_factory(
        Fixture::new(),
        Vec::new(),
        false,
        Arc::new(NoSnapshots),
        Vec::new(),
        move |inner| {
            Arc::new(WithRendezvous {
                inner,
                adapter: shared,
            })
        },
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
    let fixture = support::Fixture::native_git();
    git(&fixture.repo, &["init", "--quiet", "-b", "main"]);
    fixture.write("tracked.txt", "baseline\n");
    git(&fixture.repo, &["add", "tracked.txt"]);
    git(&fixture.repo, &["commit", "--quiet", "-m", "initial"]);
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
        json!({"script": script, "args": {}, "budget": {"max_agents": 2}}),
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
