//! Tool-event hook effects (`hooks` → Decision schema): context the host recorded
//! for a call commits with its settlement and reaches the next request; a stop
//! request ends the Drain after the current Turn with a durable reason.
mod support;
use cyber_server::runtime::*;
use serde_json::Value;
use support::{Harness, Setup, text, tools, transcript};

fn admit(text: &str) -> Admission {
    Admission::text(text, Delivery::Queue)
}

fn kinds(h: &Harness, id: &str) -> Vec<(String, Value)> {
    h.store
        .read_events(id, -1, 200)
        .unwrap()
        .events
        .into_iter()
        .map(|e| (e.kind, e.data))
        .collect()
}

fn system_texts(state: &SessionState) -> Vec<String> {
    state
        .entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::System { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn hook_context_commits_with_the_settlement_and_replays_once() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "read", "{}")]), text("done")],
        )],
        ..Setup::default()
    });
    h.tools.hook_effects.lock().unwrap().insert(
        "c1".into(),
        HookEffects {
            context: Some("[hook ctx] lint is clean".into()),
            stop: None,
            rewrite: None,
        },
    );
    let id = h.session().await;
    h.runtime.admit(&id, admit("read it")).await.unwrap();
    h.settle(&id).await;

    let requests = h.models.requests("test/main");
    assert_eq!(requests.len(), 2);
    assert!(!transcript(&requests[0]).contains("lint is clean"));
    let second = transcript(&requests[1]);
    assert_eq!(second.matches("[hook ctx] lint is clean").count(), 1);

    let events = kinds(&h, &id);
    let settled = events
        .iter()
        .position(|(kind, data)| kind == "session.tool.settled.1" && data["call_id"] == "c1")
        .expect("settled");
    let (kind, data) = &events[settled + 1];
    assert_eq!(kind, "session.system.added.1");
    assert_eq!(data["reason"], "hook_context");
    assert_eq!(data["text"], "[hook ctx] lint is clean");
    assert!(
        h.tools.hook_effects.lock().unwrap().is_empty(),
        "taken once"
    );

    // Recreating the runtime replays the committed context without admitting it again.
    let restarted = h.restart();
    let state = restarted.state(&id).await.unwrap();
    assert_eq!(system_texts(&state), ["[hook ctx] lint is clean"]);
    restarted.shutdown().await;
}

#[tokio::test]
async fn hook_stop_ends_the_drain_after_the_turn_with_a_durable_reason() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c1", "read", "{}"), ("c2", "clock", "{}")]),
                text("never requested"),
            ],
        )],
        ..Setup::default()
    });
    h.tools.hook_effects.lock().unwrap().insert(
        "c1".into(),
        HookEffects {
            context: Some("[hook ctx] noted".into()),
            stop: Some("[hook guard] policy stop".into()),
            rewrite: None,
        },
    );
    let mut live = h.runtime.subscribe();
    let id = h.session().await;
    h.runtime.admit(&id, admit("read it")).await.unwrap();
    h.settle(&id).await;

    assert_eq!(h.models.requests("test/main").len(), 1);
    let state = h.state(&id).await;
    // The rest of the Turn still ran: the stop applies after the current Turn.
    assert_eq!(state.calls["c1"].status, CallStatus::Ok);
    assert_eq!(state.calls["c2"].status, CallStatus::Ok);
    assert_eq!(
        system_texts(&state),
        ["[hook ctx] noted", "[hook guard] policy stop"]
    );
    let stop = kinds(&h, &id)
        .into_iter()
        .find(|(kind, data)| kind == "session.system.added.1" && data["reason"] == "hook_stop");
    assert!(stop.is_some(), "the stop reason is durable");
    let mut noticed = Vec::new();
    while let Ok(event) = live.try_recv() {
        if let LiveEvent::HookNotice {
            hook_id, message, ..
        } = event
        {
            noticed.push((hook_id, message));
        }
    }
    assert_eq!(noticed, [("guard".to_string(), "policy stop".to_string())]);
    assert!(!h.runtime.is_running(&id));
}

/// `builtin-tools` → apply_patch through the shell: the host's notes on a call land on
/// its settlement and call record, and the step's calls learn the model's output budget.
#[tokio::test]
async fn call_metadata_commits_with_the_settlement() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "read", "{}")]), text("done")],
        )],
        ..Setup::default()
    });
    h.tools.call_metadata.lock().unwrap().insert(
        "c1".into(),
        serde_json::json!({"intercepted": "apply_patch"}),
    );
    let id = h.session().await;
    h.runtime.admit(&id, admit("read it")).await.unwrap();
    h.settle(&id).await;
    let events = kinds(&h, &id);
    let (_, settled) = events
        .iter()
        .find(|(kind, data)| kind == "session.tool.settled.1" && data["call_id"] == "c1")
        .expect("settled");
    assert_eq!(
        settled["metadata"],
        serde_json::json!({"intercepted": "apply_patch"})
    );
    let state = h.runtime.state(&id).await.unwrap();
    assert_eq!(
        state.calls["c1"].metadata,
        Some(serde_json::json!({"intercepted": "apply_patch"}))
    );
    assert!(
        h.tools.call_metadata.lock().unwrap().is_empty(),
        "taken once"
    );
    let executed = h.tools.executed.lock().unwrap();
    assert_eq!(
        executed[0].tool_output,
        cyber_llm::catalog::ToolOutput::default(),
        "the Turn model's output budget rides on the invocation"
    );
}
