//! Parent-scoped names through actual agent dispatch and durable child history.
mod support;
use cyber_server::runtime::{ToolHost, ToolOutcome};
use serde_json::{Value, json};
use support::flow::{Flow, text};
use tokio_util::sync::CancellationToken;

async fn invoke(flow: &Flow, parent: &str, input: Value) -> ToolOutcome {
    let mut inv = flow.f.invocation("default", "agent", input);
    inv.session_id = parent.into();
    flow.f.host.execute(inv, CancellationToken::new()).await
}

#[tokio::test]
async fn foreground_children_get_distinct_durable_names_and_unchanged_titles() {
    let flow = Flow::new(vec![text("first"), text("second")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    for name in ["general", "general-2"] {
        let output = support::ok(invoke(&flow, &parent, json!({"prompt":"inspect"})).await);
        let result: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(result["name"], name);
        let child = flow
            .runtime
            .state(result["id"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&child.info).unwrap()["subagent_name"],
            name
        );
        assert_eq!(child.info.title, "Run focused subagent task (@general)");
    }
}

#[tokio::test]
async fn caller_names_are_parent_scoped_and_duplicates_create_no_child() {
    let flow = Flow::new(vec![text("first"), text("second")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let input = json!({"prompt":"inspect","name":"inspection"});
    let first = support::ok(invoke(&flow, &parent, input.clone()).await);
    assert_eq!(
        serde_json::from_str::<Value>(&first).unwrap()["name"],
        "inspection"
    );
    let before = flow.main.requests().len();
    let sessions = flow
        .runtime
        .list(&Default::default())
        .unwrap()
        .sessions
        .len();
    let duplicate = invoke(&flow, &parent, input.clone()).await;
    assert!(support::failed(duplicate).contains("name already"));
    assert_eq!(flow.main.requests().len(), before);
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        sessions
    );
    let other = flow.session("default").await;
    let second = support::ok(invoke(&flow, &other, input).await);
    assert_eq!(
        serde_json::from_str::<Value>(&second).unwrap()["name"],
        "inspection"
    );
}

#[tokio::test]
async fn foreground_and_background_share_the_default_name_namespace() {
    let flow = Flow::new(
        vec![text("first"), text("second"), text("notice handled")],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let foreground = support::ok(invoke(&flow, &parent, json!({"prompt":"inspect"})).await);
    assert_eq!(
        serde_json::from_str::<Value>(&foreground).unwrap()["name"],
        "general"
    );
    let background = support::ok(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"inspect","background":true}),
        )
        .await,
    );
    let ack: Value = serde_json::from_str(&background).unwrap();
    assert_eq!(ack["name"], "general-2");
    let child = flow
        .runtime
        .state(ack["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(child.info.title, "Run focused subagent task (@general)");
    assert_eq!(child.info.subagent_name.as_deref(), Some("general-2"));
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
}
