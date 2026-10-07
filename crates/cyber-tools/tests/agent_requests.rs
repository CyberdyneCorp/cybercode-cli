//! Agent request options reach the actual runtime provider request.
mod support;

use serde_json::json;
use support::flow::{Flow, text};

#[tokio::test]
async fn selected_agent_request_options_reach_the_model() {
    let flow = Flow::new(vec![text("done")], false);
    flow.f.set_config(json!({"agents":{"build":{"request":{
        "headers":{"X-Agent":"build"},
        "body":{"temperature":0.2,"vendor":{"mode":"careful"},"apiKey":"must-not-send", "items":[{"api_key":"also-secret","keep":true}]}
    }}}}));
    let id = flow.session("default").await;
    flow.prompt(&id, "test").await;
    flow.settle(&id).await;
    let request = &flow.main.requests()[0];
    assert_eq!(request.body["temperature"], 0.2);
    assert_eq!(request.body["vendor"]["mode"], "careful");
    assert_eq!(request.body["items"][0]["keep"], true);
    assert!(request.body.get("apiKey").is_none());
    assert!(request.body["items"][0].get("api_key").is_none());
    assert!(
        request
            .headers
            .contains(&("X-Agent".into(), "build".into()))
    );
    let events = flow.f.store.read_events(&id, -1, 500).unwrap();
    assert!(
        !serde_json::to_string(&events.events)
            .unwrap()
            .contains("must-not-send")
    );
}

#[tokio::test]
async fn agent_request_changes_and_removal_apply_at_later_safe_boundaries() {
    let flow = Flow::new(vec![text("first"), text("second"), text("third")], false);
    flow.f
        .set_config(json!({"agents":{"build":{"request":{"body":{"temperature":0.2}}}}}));
    let id = flow.session("default").await;
    flow.prompt(&id, "one").await;
    flow.settle(&id).await;
    flow.f
        .set_config(json!({"agents":{"build":{"request":{"body":{"temperature":0.8}}}}}));
    flow.prompt(&id, "two").await;
    flow.settle(&id).await;
    flow.f.set_config(json!({}));
    flow.prompt(&id, "three").await;
    flow.settle(&id).await;
    let requests = flow.main.requests();
    assert_eq!(requests[0].body["temperature"], 0.2);
    assert_eq!(requests[1].body["temperature"], 0.8);
    assert!(requests[2].body.is_null());
}
