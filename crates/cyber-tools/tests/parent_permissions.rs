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
    // Keep this regression on dispatch; memory context checks ancestor authority earlier.
    flow.f
        .set_config(json!({"memory":{"enabled":false},"agents":{"build":{"disabled":true}}}));
    flow.prompt(&child, "write child.txt").await;
    flow.settle(&child).await;
    assert!(!flow.f.repo.join("child.txt").exists());
    let result = serde_json::to_string(&flow.runtime.state(&child).await.unwrap().calls).unwrap();
    assert!(
        result.contains("Unknown agent") && result.contains("build"),
        "{result}"
    );
}

fn restore_corrupt_parent(
    flow: &Flow,
    mut info: cyber_server::runtime::SessionInfo,
    id: &str,
    parent: &str,
) {
    info.id = id.into();
    info.parent_id = None;
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
    let id = id.to_owned();
    let parent = parent.to_owned();
    // Restore corruption directly; current admission must not create invalid ancestry.
    flow.f.store.transaction(move |tx| {
        tx.execute("UPDATE session SET parent_id=?1 WHERE id=?2", rusqlite::params![parent,id])?;
        tx.execute("UPDATE event SET data=json_set(data,'$.info.parent_id',?1) WHERE aggregate_id=?2 AND type='session.created.1'", rusqlite::params![parent,id])?;
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn restored_ancestry_cycle_refuses_dispatch_without_looping() {
    let flow = write_flow();
    let root = session(&flow, None, "build", "default", Value::Null).await;
    let info = flow.runtime.state(&root).await.unwrap().info;
    restore_corrupt_parent(&flow, info.clone(), "ses_a", "ses_b");
    restore_corrupt_parent(&flow, info.clone(), "ses_b", "ses_a");
    restore_corrupt_parent(&flow, info, "ses_child", "ses_a");
    let error = flow
        .runtime
        .admit(
            "ses_child",
            cyber_server::runtime::Admission::text(
                "write child.txt",
                cyber_server::runtime::Delivery::Queue,
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        cyber_server::runtime::RuntimeError::Corrupt(_)
    ));
    assert!(!flow.runtime.is_running("ses_child"));
    assert!(flow.main.requests().is_empty());
    assert!(!flow.f.repo.join("child.txt").exists());
    assert!(
        flow.runtime
            .state("ses_child")
            .await
            .unwrap()
            .calls
            .is_empty()
    );
}

#[tokio::test]
async fn restored_missing_parent_refuses_dispatch() {
    let flow = write_flow();
    let root = session(&flow, None, "build", "default", Value::Null).await;
    let info = flow.runtime.state(&root).await.unwrap().info;
    restore_corrupt_parent(&flow, info, "ses_child", "ses_missing");
    let error = flow
        .runtime
        .admit(
            "ses_child",
            cyber_server::runtime::Admission::text(
                "write child.txt",
                cyber_server::runtime::Delivery::Queue,
            ),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        cyber_server::runtime::RuntimeError::Corrupt(_)
    ));
    assert!(!flow.runtime.is_running("ses_child"));
    assert!(flow.main.requests().is_empty());
    assert!(!flow.f.repo.join("child.txt").exists());
    assert!(
        flow.runtime
            .state("ses_child")
            .await
            .unwrap()
            .calls
            .is_empty()
    );
}

#[tokio::test]
async fn agent_pinning_preserves_an_active_parent_profile_deny() {
    use cyber_server::runtime::PermissionReply;
    let flow = Flow::new(
        vec![
            call("parent_read", "read", json!({"path":".env"})),
            call(
                "child_write",
                "write",
                json!({"path":"child.txt","content":"forbidden"}),
            ),
            text("child done"),
            text("parent done"),
        ],
        true,
    );
    flow.f.set_config(json!({"agents": {
        "build": {"permissions":{"edit":"deny"}},
        "general": {"permissions":{"edit":"allow"}}
    }}));
    flow.f.write(".env", "secret");
    let parent = session(&flow, None, "build", "default", Value::Null).await;
    flow.prompt(&parent, "read .env").await;
    let pending = flow.pending(&parent).await;
    flow.runtime.switch_agent(&parent, "general").await.unwrap();
    let child = session(
        &flow,
        Some(parent.clone()),
        "general",
        "bypass",
        json!({"edit":"allow"}),
    )
    .await;
    flow.prompt(&child, "write child.txt").await;
    flow.settle(&child).await;
    let result = serde_json::to_string(&flow.runtime.state(&child).await.unwrap().calls).unwrap();
    flow.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    flow.settle(&parent).await;
    assert!(
        !flow.f.repo.join("child.txt").exists(),
        "pending parent agent selection widened child authority"
    );
    assert!(result.contains("denied"), "{result}");
}
