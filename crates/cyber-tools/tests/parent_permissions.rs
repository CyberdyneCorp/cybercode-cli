//! Child execution cannot widen deny rules from its durable parent chain.
mod support;

use cyber_server::runtime::CreateSession;
use serde_json::{Value, json};
use support::flow::{Flow, call, text};

async fn session(
    flow: &Flow,
    parent: Option<String>,
    agent: &str,
    mode: &str,
    rules: Value,
) -> String {
    flow.runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: parent,
            agent: Some(agent.into()),
            mode: Some(mode.into()),
            rules: Some(rules),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}

async fn refused_write(flow: &Flow, child: &str) {
    flow.prompt(child, "write child.txt").await;
    flow.settle(child).await;
    assert!(!flow.f.repo.join("child.txt").exists());
    let state = flow.runtime.state(child).await.unwrap();
    let result = serde_json::to_string(&state.calls).unwrap();
    assert!(
        result.contains("denied") || result.contains("Not pre-approved"),
        "{result}"
    );
}

fn write_flow() -> Flow {
    Flow::new(
        vec![
            call(
                "write",
                "write",
                json!({"path":"child.txt","content":"forbidden"}),
            ),
            text("done"),
        ],
        false,
    )
}

#[tokio::test]
async fn child_allow_and_bypass_cannot_widen_parent_session_denies() {
    let flow = write_flow();
    let parent = session(
        &flow,
        None,
        "build",
        "default",
        json!([{ "action":"edit", "resource":"*", "effect":"deny" }]),
    )
    .await;
    let child = session(
        &flow,
        Some(parent),
        "general",
        "bypass",
        json!([{ "action":"edit", "resource":"*", "effect":"allow" }]),
    )
    .await;
    refused_write(&flow, &child).await;
}

#[tokio::test]
async fn a_nested_child_retains_grandparent_denies() {
    let flow = write_flow();
    let grandparent = session(&flow, None, "build", "default", json!({"edit":"deny"})).await;
    let parent = session(
        &flow,
        Some(grandparent),
        "general",
        "bypass",
        json!({"edit":"allow"}),
    )
    .await;
    let child = session(
        &flow,
        Some(parent),
        "general",
        "bypass",
        json!({"edit":"allow"}),
    )
    .await;
    refused_write(&flow, &child).await;
}

#[tokio::test]
async fn parent_profile_denies_remain_child_ceilings() {
    let flow = write_flow();
    flow.f.set_config(json!({"agents":{
        "build":{"permissions":{"edit":"deny"}},
        "general":{"permissions":{"edit":"allow"}}
    }}));
    let parent = session(&flow, None, "build", "default", Value::Null).await;
    let child = session(&flow, Some(parent), "general", "bypass", Value::Null).await;
    refused_write(&flow, &child).await;
}

#[tokio::test]
async fn parent_allows_do_not_preapprove_the_child() {
    let flow = write_flow();
    let parent = session(&flow, None, "build", "default", json!({"edit":"allow"})).await;
    let child = session(&flow, Some(parent), "general", "dont-ask", Value::Null).await;
    refused_write(&flow, &child).await;
}

#[tokio::test]
async fn missing_parent_is_refused_before_session_creation() {
    let flow = Flow::new(vec![], false);
    let result = flow
        .runtime
        .create_session(CreateSession {
            id: Some("ses_child".into()),
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some("ses_missing".into()),
            ..Default::default()
        })
        .await;
    assert!(result.is_err());
    assert!(
        flow.f
            .store
            .read_events("ses_child", -1, 10)
            .unwrap()
            .events
            .is_empty()
    );
}

#[tokio::test]
async fn a_session_cannot_be_its_own_parent() {
    let flow = Flow::new(vec![], false);
    let result = flow
        .runtime
        .create_session(CreateSession {
            id: Some("ses_child".into()),
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some("ses_child".into()),
            ..Default::default()
        })
        .await;
    assert!(result.is_err());
    assert!(
        flow.f
            .store
            .read_events("ses_child", -1, 10)
            .unwrap()
            .events
            .is_empty()
    );
}

#[tokio::test]
async fn a_root_session_can_still_execute_its_own_allow() {
    let flow = write_flow();
    let root = session(&flow, None, "general", "dont-ask", json!({"edit":"allow"})).await;
    flow.prompt(&root, "write child.txt").await;
    flow.settle(&root).await;
    assert_eq!(flow.f.read("child.txt"), "forbidden");
}

#[tokio::test]
async fn unavailable_parent_profile_refuses_child_dispatch() {
    let flow = write_flow();
    let parent = session(&flow, None, "build", "default", Value::Null).await;
    let child = session(
        &flow,
        Some(parent),
        "general",
        "bypass",
        json!({"edit":"allow"}),
    )
    .await;
    flow.f
        .set_config(json!({"agents":{"build":{"disabled":true}}}));
    flow.prompt(&child, "write child.txt").await;
    flow.settle(&child).await;
    assert!(!flow.f.repo.join("child.txt").exists());
    let result = serde_json::to_string(&flow.runtime.state(&child).await.unwrap().calls).unwrap();
    assert!(
        result.contains("Unknown agent") && result.contains("build"),
        "{result}"
    );
}

fn append_legacy(
    flow: &Flow,
    mut info: cyber_server::runtime::SessionInfo,
    id: &str,
    parent: &str,
) {
    info.id = id.into();
    info.parent_id = Some(parent.into());
    info.rules = json!({"edit":"allow"});
    info.agent = "general".into();
    info.mode = "bypass".into();
    flow.f
        .store
        .append(
            id,
            cyber_store::Expected::Seq(-1),
            vec![cyber_store::NewEvent::new(
                "session.created.1",
                json!({
                    "info":info,"history":[],"calls":[],"forked_from":null
                }),
            )],
        )
        .unwrap();
}

#[tokio::test]
async fn restored_ancestry_cycle_refuses_dispatch_without_looping() {
    let flow = write_flow();
    let root = session(&flow, None, "build", "default", Value::Null).await;
    let info = flow.runtime.state(&root).await.unwrap().info;
    append_legacy(&flow, info.clone(), "ses_a", "ses_b");
    append_legacy(&flow, info.clone(), "ses_b", "ses_a");
    append_legacy(&flow, info, "ses_child", "ses_a");
    flow.prompt("ses_child", "write child.txt").await;
    flow.settle("ses_child").await;
    assert!(!flow.f.repo.join("child.txt").exists());
    let result =
        serde_json::to_string(&flow.runtime.state("ses_child").await.unwrap().calls).unwrap();
    assert!(result.contains("ancestry cycle"), "{result}");
}

#[tokio::test]
async fn restored_missing_parent_refuses_dispatch() {
    let flow = write_flow();
    let root = session(&flow, None, "build", "default", Value::Null).await;
    let info = flow.runtime.state(&root).await.unwrap().info;
    append_legacy(&flow, info, "ses_child", "ses_missing");
    flow.prompt("ses_child", "write child.txt").await;
    flow.settle("ses_child").await;
    assert!(!flow.f.repo.join("child.txt").exists());
    let result =
        serde_json::to_string(&flow.runtime.state("ses_child").await.unwrap().calls).unwrap();
    assert!(result.contains("ses_missing"), "{result}");
}
