//! `workflows` → Durable agent results and resume, on the production host: an unsettled
//! agent whose child Session was never created (the server stopped between recording the
//! agent's start and spawning its child) resumes by spawning a new child, not by failing
//! with `Subagent not found`.
mod support;

use cyber_server::runtime::{
    CreateSession, StartWorkflowRun, ToolHost, TurnContext, WorkflowAgentRequest,
};
use serde_json::Value;
use support::flow::{Flow, text};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn resuming_an_agent_that_never_got_a_child_spawns_one() {
    let flow = Flow::new(vec![text("fresh answer")], true);
    let owner = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    // A recorded workflow start precedes agent-start recovery in production. Keep
    // that durable budget owner rather than inventing a run ID with no evidence.
    let run=flow.runtime.start_workflow_run(StartWorkflowRun{
        module:cyber_workflow::WorkflowModule::inline("export const meta={name:'recover-agent',description:'Agent resume fixture'}; export default async()=>null;").unwrap(),
        args:serde_json::json!({}),session_id:owner.clone(),limits:Default::default(),tiers:Default::default(),budget:cyber_core::budget::RunBudget::resolve(&serde_json::json!({}),&[]).unwrap(),share:Default::default(),starter:Default::default(),tools:vec![],
    }).await.unwrap().id;
    let slot = flow
        .runtime
        .workflow_scheduler()
        .enqueue(&run, usize::MAX)
        .await
        .unwrap();
    let request = WorkflowAgentRequest {
        run_id: run,
        agent_id: "agt_missing".into(),
        call_key: "key:1".into(),
        owner: TurnContext {
            session_id: owner.clone(),
            directory: flow.f.repo.display().to_string(),
            agent: "build".into(),
            mode: "bypass".into(),
            prefers_apply_patch: false,
            rules: Value::Null,
            worktree: Default::default(),
        },
        prompt: "the interrupted task".into(),
        agent: None,
        model: None,
        schema: None,
        label: None,
        resume: true,
        worktree: None,
        goal: None,
        slot,
    };
    let output = flow
        .f
        .host
        .spawn_workflow_agent(request, CancellationToken::new())
        .await
        .unwrap_or_else(|error| panic!("{error:?}"));
    assert_eq!(output.text, "fresh answer");
    let child = flow
        .runtime
        .resolve_subagent(&owner, "agt_missing")
        .await
        .unwrap();
    assert_eq!(child.id, output.session_id);
    flow.runtime.shutdown().await;
}
