//! Profile prompts use the durable Context Epoch and safe-boundary updates.

mod support;

use serde_json::json;
use support::flow::{Flow, text};

#[tokio::test]
async fn agent_prompt_starts_the_request_and_changes_without_rewriting_the_prefix() {
    let flow = Flow::new(vec![text("first"), text("second")], false);
    flow.f
        .set_config(json!({"agents":{"build":{"system":"Initial agent instructions."}}}));
    let id = flow.session("default").await;
    flow.prompt(&id, "one").await;
    flow.settle(&id).await;
    let first = flow.main.requests()[0].system.clone();
    assert_eq!(first[0], "Initial agent instructions.");
    flow.f
        .set_config(json!({"agents":{"build":{"system":"Changed agent instructions."}}}));
    flow.prompt(&id, "two").await;
    flow.settle(&id).await;
    let requests = flow.main.requests();
    assert_eq!(requests[1].system, first);
    let messages = serde_json::to_string(&requests[1].messages).unwrap();
    assert!(
        messages.contains("Changed agent instructions."),
        "{messages}"
    );
    let state = flow.runtime.state(&id).await.unwrap();
    assert_eq!(
        state.epoch.unwrap().snapshot["core/agent"],
        "Changed agent instructions."
    );
    let page = flow.f.store.read_events(&id, -1, 500).unwrap();
    assert!(page.events.len() < 500);
    let events = page.events;
    let replayed = cyber_server::runtime::SessionState::replay(&events).unwrap();
    assert_eq!(
        replayed.epoch.unwrap().system_prefix.as_deref(),
        Some("Initial agent instructions.")
    );
}

#[tokio::test]
async fn removing_an_agent_prompt_emits_a_removal_and_new_sessions_use_the_base_prompt() {
    let flow = Flow::new(vec![text("first"), text("second"), text("third")], false);
    flow.f
        .set_config(json!({"agents":{"build":{"system":"Original profile prompt."}}}));
    let id = flow.session("default").await;
    flow.prompt(&id, "one").await;
    flow.settle(&id).await;
    flow.f.set_config(json!({}));
    flow.prompt(&id, "two").await;
    flow.settle(&id).await;
    let requests = flow.main.requests();
    assert_eq!(requests[1].system, requests[0].system);
    let messages = serde_json::to_string(&requests[1].messages).unwrap();
    assert!(
        messages.contains("core/agent context no longer applies"),
        "{messages}"
    );
    assert!(
        messages.contains("The default agent instructions now apply"),
        "{messages}"
    );
    assert!(
        !flow
            .runtime
            .state(&id)
            .await
            .unwrap()
            .epoch
            .unwrap()
            .snapshot
            .contains_key("core/agent")
    );
    let fresh = flow.session("default").await;
    flow.prompt(&fresh, "three").await;
    flow.settle(&fresh).await;
    assert!(flow.main.requests()[2].system[0].starts_with("You are Cyber Code"));
}

#[tokio::test]
async fn unavailable_initial_profile_keeps_the_prompt_retryable() {
    let flow = Flow::new(vec![text("recovered")], false);
    flow.f.set_config(json!({"agents":{"build":{"steps":0}}}));
    let id = flow.session("default").await;
    flow.prompt(&id, "keep this prompt").await;
    flow.settle(&id).await;
    assert!(flow.main.requests().is_empty());
    let state = flow.runtime.state(&id).await.unwrap();
    assert!(state.epoch.is_none());
    assert_eq!(
        state.inbox[0].status,
        cyber_server::runtime::InputStatus::Pending
    );
    flow.f
        .set_config(json!({"agents":{"build":{"system":"Recovered profile."}}}));
    flow.runtime.wake(&id).await.unwrap();
    flow.settle(&id).await;
    assert_eq!(flow.main.requests()[0].system[0], "Recovered profile.");
}

#[tokio::test]
async fn unavailable_existing_profile_retains_context_and_pauses_until_repaired() {
    let flow = Flow::new(vec![text("first"), text("second")], false);
    flow.f
        .set_config(json!({"agents":{"build":{"system":"Keep these instructions."}}}));
    let id = flow.session("default").await;
    flow.prompt(&id, "one").await;
    flow.settle(&id).await;
    flow.f.set_config(json!({"agents":{"build":{"steps":0}}}));
    flow.prompt(&id, "two").await;
    flow.settle(&id).await;
    assert_eq!(flow.main.requests().len(), 1);
    let state = flow.runtime.state(&id).await.unwrap();
    assert_eq!(
        state.epoch.unwrap().snapshot["core/agent"],
        "Keep these instructions."
    );
    assert_eq!(
        state.inbox[1].status,
        cyber_server::runtime::InputStatus::Pending
    );
    flow.f.set_config(json!({"agents":{"build":{
        "system":"Keep these instructions.", "request":{"body":{"temperature":0.4}}
    }}}));
    flow.runtime.wake(&id).await.unwrap();
    flow.settle(&id).await;
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].system, requests[0].system);
    assert_eq!(requests[1].body["temperature"], 0.4);
}
