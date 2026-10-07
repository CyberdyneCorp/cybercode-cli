//! Durable schema contracts, terminal replay and runtime-owned result dispatch.
mod support;

use cyber_server::runtime::{Admission, CreateSession, Delivery, StructuredSchema};
use serde_json::{Value, json};
use support::{Harness, Setup, tools};

async fn child(h: &Harness, schema: Value) -> String {
    let parent = h.session().await;
    h.runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent),
            title: Some("Structured child".into()),
            output_schema: Some(StructuredSchema::new(schema).unwrap()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn a_pending_contract_is_restored_before_inference() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("result", "return_result", "{\"ok\":true}")])],
        )],
        ..Setup::default()
    });
    let schema = json!({"type":"object","required":["ok"],"properties":{"ok":{"const":true}}});
    let id = child(&h, schema.clone()).await;
    let mut admission = Admission::text("inspect", Delivery::Queue);
    admission.resume = false;
    h.runtime.admit(&id, admission).await.unwrap();
    h.runtime.shutdown().await;
    let restarted = h.restart();
    restarted.wake(&id).await.unwrap();
    restarted.wait_idle(&id).await;
    assert_eq!(
        restarted.state(&id).await.unwrap().structured_result(),
        Some(&json!({"ok":true}))
    );
    let requests = h.models.adapters["test/main"].requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .tools
            .iter()
            .any(|tool| tool.name == "return_result" && tool.input_schema == schema)
    );
}

#[tokio::test]
async fn a_completed_result_remains_terminal_after_restart() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("result", "return_result", "null")])],
        )],
        ..Setup::default()
    });
    let id = child(&h, json!({"type":"null"})).await;
    h.runtime
        .admit(&id, Admission::text("inspect", Delivery::Queue))
        .await
        .unwrap();
    h.settle(&id).await;
    h.runtime.shutdown().await;
    let restarted = h.restart();
    assert_eq!(
        restarted.state(&id).await.unwrap().structured_result(),
        Some(&Value::Null)
    );
    restarted.resume(&id).await.unwrap();
    restarted.wait_idle(&id).await;
    assert_eq!(h.models.adapters["test/main"].requests().len(), 1);
    assert_eq!(
        restarted.state(&id).await.unwrap().calls["result"].structured_output,
        Some(Value::Null)
    );
}

#[tokio::test]
async fn a_caller_without_a_contract_cannot_use_return_result() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("result", "return_result", "{}")]),
                support::text("done"),
            ],
        )],
        ..Setup::default()
    });
    let id = h.session().await;
    h.runtime
        .admit(&id, Admission::text("inspect", Delivery::Queue))
        .await
        .unwrap();
    h.settle(&id).await;
    let state = h.state(&id).await;
    assert!(
        state.calls["result"]
            .output
            .as_deref()
            .unwrap()
            .contains("Unknown tool: return_result")
    );
    assert!(state.structured_result().is_none());
    assert_eq!(state.calls["result"].attempt, 0);
}

#[test]
fn a_client_cannot_register_the_runtime_result_name() {
    let tools = cyber_server::http::remote_tools::RemoteTools::default();
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let result = tools.register(tools.owner(), tx, &json!({"name":"return_result"}), &[]);
    assert!(result.unwrap_err().contains("runtime-owned"));
    assert!(tools.definitions().is_empty());
}

#[tokio::test]
async fn conversation_rewind_removes_a_terminal_result_but_keeps_its_schema() {
    use cyber_server::runtime::RevertTarget;
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("first", "return_result", "{\"ok\":true}")]),
                tools(&[("second", "return_result", "{\"ok\":false}")]),
            ],
        )],
        ..Setup::default()
    });
    let id = child(
        &h,
        json!({"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"}}}),
    )
    .await;
    let receipt = h
        .runtime
        .admit(&id, Admission::text("inspect", Delivery::Queue))
        .await
        .unwrap();
    h.settle(&id).await;
    assert!(h.state(&id).await.structured_result().is_some());
    h.runtime
        .revert_stage(&id, &receipt.message_id, RevertTarget::Conversation)
        .await
        .unwrap();
    h.runtime.revert_commit(&id).await.unwrap();
    assert!(h.state(&id).await.structured_result().is_none());
    h.runtime
        .admit(&id, Admission::text("inspect again", Delivery::Queue))
        .await
        .unwrap();
    h.settle(&id).await;
    assert_eq!(
        h.state(&id).await.structured_result(),
        Some(&json!({"ok":false}))
    );
    assert_eq!(h.models.adapters["test/main"].requests().len(), 2);
}
