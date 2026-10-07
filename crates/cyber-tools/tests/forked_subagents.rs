//! Fork execution through the real agent tool and immutable copied context.
mod support;
use cyber_server::runtime::ToolHost;
use serde_json::{Value, json};
use support::flow::{Flow, call, text};
use tokio_util::sync::CancellationToken;

async fn invoke(flow: &Flow, parent: &str, input: Value) -> Value {
    let mut inv = flow.f.invocation("default", "agent", input);
    inv.session_id = parent.into();
    let info = flow.runtime.state(parent).await.unwrap().info;
    inv.agent = info.agent;
    inv.mode = info.mode;
    inv.rules = info.rules;
    let output = support::ok(flow.f.host.execute(inv, CancellationToken::new()).await);
    serde_json::from_str(&output).unwrap()
}

#[tokio::test]
async fn a_fork_inherits_the_parent_agent_model_history_and_task() {
    let flow = Flow::new(vec![text("parent answer"), text("child findings")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "original objective").await;
    flow.settle(&parent).await;
    let source = flow.runtime.state(&parent).await.unwrap();
    let result = invoke(
        &flow,
        &parent,
        json!({"prompt":"try another approach","fork":true}),
    )
    .await;
    let child = flow
        .runtime
        .state(result["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(child.info.parent_id.as_deref(), Some(parent.as_str()));
    assert_eq!(child.info.agent, "build");
    assert_eq!(child.info.model, source.info.model);
    assert_eq!(
        child.task.objective.as_ref().unwrap().text,
        "original objective"
    );
    assert_eq!(
        child.task.objective.as_ref().unwrap().message_id,
        child.entries[0].id()
    );
    assert_eq!(result["name"], "build");
    assert_eq!(result["text"], "child findings");
    assert_eq!(child.entries.len(), 4);
    assert_ne!(child.entries[0].id(), source.entries[0].id());
    assert_eq!(child.totals.steps, 1);
    assert!(format!("{:?}", flow.main.requests()[1].messages).contains("parent answer"));
    assert_eq!(
        flow.runtime.state(&parent).await.unwrap().entries,
        source.entries
    );
}

#[tokio::test]
async fn inherited_model_is_not_replaced_by_a_new_profile_default() {
    let flow = Flow::new(vec![text("parent answer"), text("child findings")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "original objective").await;
    flow.settle(&parent).await;
    flow.f.set_config(
        json!({"permissions":{"agent":"allow"},"agents":{"build":{"model":"test/missing"}}}),
    );
    let result = invoke(
        &flow,
        &parent,
        json!({"prompt":"try another approach","fork":true}),
    )
    .await;
    assert_eq!(result["text"], "child findings");
    assert_eq!(flow.main.requests()[1].model, "main");
}

#[tokio::test]
async fn a_fork_supports_explicit_agent_selection_without_changing_its_parent() {
    let flow = Flow::new(vec![text("parent answer"), text("child findings")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "original objective").await;
    flow.settle(&parent).await;
    let result = invoke(
        &flow,
        &parent,
        json!({"prompt":"try another approach","fork":true,"agent":"general","model":"test/main"}),
    )
    .await;
    assert_eq!(
        flow.runtime
            .state(result["id"].as_str().unwrap())
            .await
            .unwrap()
            .info
            .agent,
        "general"
    );
    assert_eq!(
        flow.runtime.state(&parent).await.unwrap().info.agent,
        "build"
    );
    assert!(format!("{:?}", flow.main.requests()[1].messages).contains("parent answer"));
}

#[tokio::test]
async fn a_fork_of_the_executing_agent_call_does_not_reexecute_its_copied_call() {
    let flow = Flow::new(
        vec![
            call(
                "fork",
                "agent",
                json!({"prompt":"try another approach","fork":true}),
            ),
            text("child findings"),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "original objective").await;
    flow.settle(&parent).await;
    let result: Value = serde_json::from_str(&flow.output(&parent, "fork").await).unwrap();
    let child = flow
        .runtime
        .state(result["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(result["text"], "child findings");
    assert_eq!(child.calls.len(), 1);
    let copied = child.calls.values().next().unwrap();
    assert_ne!(copied.call_id, "fork");
    assert!(copied.output.as_ref().unwrap().contains("source call"));
    assert_eq!(flow.main.requests().len(), 3);
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        2
    );
}

#[tokio::test]
async fn a_background_fork_allows_the_parent_to_continue_independently() {
    let flow = Flow::new(
        vec![
            text("parent answer"),
            call("read", "read", json!({"path":".env"})),
            text("parent continues"),
            text("child findings"),
            text("notice handled"),
        ],
        true,
    );
    flow.f.write(".env", "private intermediate output");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "original objective").await;
    flow.settle(&parent).await;
    let ack = invoke(
        &flow,
        &parent,
        json!({"prompt":"try another approach","fork":true,"background":true}),
    )
    .await;
    assert_eq!(ack["state"], "running");
    let request = flow.pending(&parent).await;
    flow.prompt(&parent, "continue the primary task").await;
    flow.settle(&parent).await;
    assert!(flow.runtime.is_running(ack["id"].as_str().unwrap()));
    flow.runtime
        .reply_permission(&request.id, cyber_server::runtime::PermissionReply::Once)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if flow
                .runtime
                .job(ack["job_id"].as_str().unwrap())
                .unwrap()
                .notified
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    flow.runtime.wait_idle(&parent).await;
    let job = flow.runtime.job(ack["job_id"].as_str().unwrap()).unwrap();
    assert_eq!(job.result.unwrap()["text"], "child findings");
    assert_eq!(job.tokens, 215);
    assert_eq!(flow.main.requests().len(), 5);
}

#[tokio::test]
async fn forked_children_support_structured_results() {
    let flow = Flow::new(
        vec![
            call(
                "fork",
                "agent",
                json!({"prompt":"try another approach","fork":true,"output_schema":{"type":"integer"}}),
            ),
            call("result", "return_result", json!(1)),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "original objective").await;
    flow.settle(&parent).await;
    let state = flow.runtime.state(&parent).await.unwrap();
    assert_eq!(state.calls["fork"].structured_output, Some(json!(1)));
    assert_eq!(flow.main.requests().len(), 3);
}
