//! The cross-session messaging tools on the built-in host (`cross-session-messaging` →
//! Send message tool, Reachable session listing, Disable switch; `permissions-modes` →
//! Default rules), with a scripted runtime.
mod support;

use cyber_server::runtime::messaging::send::idempotent_id;
use cyber_server::runtime::messaging::watch::idempotent_watch_id;
use cyber_server::runtime::{CreateSession, Invocation, ToolHost, ToolOutcome, TurnContext};
use serde_json::{Value, json};
use support::flow::{Flow, text};
use tokio_util::sync::CancellationToken;

async fn titled(flow: &Flow, title: &str) -> String {
    flow.runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            title: Some(title.into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}

fn invocation(flow: &Flow, session: &str, name: &str, input: Value) -> Invocation {
    let mut invocation = flow.f.invocation("default", name, input);
    invocation.session_id = session.to_owned();
    invocation
}

async fn call(flow: &Flow, session: &str, name: &str, input: Value) -> ToolOutcome {
    flow.f
        .host
        .execute(
            invocation(flow, session, name, input),
            CancellationToken::new(),
        )
        .await
}

fn turn(flow: &Flow, session: &str) -> TurnContext {
    TurnContext {
        session_id: session.into(),
        directory: flow.f.repo.display().to_string(),
        agent: "build".into(),
        mode: "default".into(),
        prefers_apply_patch: false,
        rules: Value::Null,
        worktree: Default::default(),
    }
}

/// `send_message` resolves by name, sends through the facade with an ID derived from the
/// Session and the call (a retried call replays), and returns the outcome as JSON;
/// `list_sessions` lists the reachable Sessions; `watch_session` and `notify_when_idle`
/// subscribe to one idle notice (task 8.3).
#[tokio::test(flavor = "multi_thread")]
async fn the_messaging_tools_send_list_and_watch() {
    let flow = Flow::new(
        vec![
            text("noted"),
            text("seen"),
            text("pong"),
            text("seen again"),
        ],
        false,
    );
    let frontend = titled(&flow, "frontend").await;
    let backend = titled(&flow, "backend").await;

    let input = json!({"to": "backend", "text": "which port does the API use?"});
    let output = support::ok(call(&flow, &frontend, "send_message", input.clone()).await);
    let id = idempotent_id(&frontend, "call_send_message");
    let mut result: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(result["message_id"], json!(id));
    assert_eq!(result["to"]["session_id"], json!(backend));
    result["message_id"] = json!("<msgx>");
    result["to"]["session_id"] = json!("<backend>");
    support::golden(&flow.f, "send_message", &result.to_string());
    flow.settle(&backend).await;
    let replay: Value = serde_json::from_str(&support::ok(
        call(&flow, &frontend, "send_message", input).await,
    ))
    .unwrap();
    assert_eq!(
        (&replay["message_id"], &replay["replayed"]),
        (&json!(id), &json!(true))
    );
    let state = flow.runtime.state(&backend).await.unwrap();
    assert_eq!(state.inbox.iter().filter(|r| r.message_id == id).count(), 1);
    let request = flow.requests("test/main").pop().expect("backend read it");
    assert!(
        format!("{:?}", request.messages).contains("which port does the API use?"),
        "the target's Turn reads the message"
    );

    let listed = support::ok(call(&flow, &frontend, "list_sessions", json!({})).await);
    let mut listed: Value = serde_json::from_str(&listed).unwrap();
    let entry = &mut listed["sessions"][0];
    assert_eq!(entry["session_id"], json!(backend));
    assert_eq!(
        entry["location"]["directory"],
        json!(flow.f.repo.display().to_string())
    );
    entry["session_id"] = json!("<backend>");
    entry["location"]["directory"] = json!("<repo>");
    support::golden(&flow.f, "list_sessions", &listed.to_string());
    let elsewhere = support::ok(
        call(
            &flow,
            &frontend,
            "list_sessions",
            json!({"directory": "../"}),
        )
        .await,
    );
    assert_eq!(elsewhere, json!({"sessions": []}).to_string());

    // `watch_session` on the idle backend fires at once; a retried call replays it.
    let watch = support::ok(
        call(
            &flow,
            &frontend,
            "watch_session",
            json!({"target": "backend"}),
        )
        .await,
    );
    let mut watch: Value = serde_json::from_str(&watch).unwrap();
    let watch_id = idempotent_watch_id(&frontend, "call_watch_session");
    assert_eq!(watch["watch_id"], json!(watch_id));
    assert_eq!(watch["target"]["session_id"], json!(backend));
    watch["watch_id"] = json!("<wch>");
    watch["target"]["session_id"] = json!("<backend>");
    watch["notice_id"] = json!("<notice>");
    watch["expires_at"] = json!("<expires>");
    support::golden(&flow.f, "watch_session", &watch.to_string());
    flow.settle(&frontend).await;
    let replay: Value = serde_json::from_str(&support::ok(
        call(
            &flow,
            &frontend,
            "watch_session",
            json!({"target": "backend"}),
        )
        .await,
    ))
    .unwrap();
    assert_eq!(replay["replayed"], json!(true));
    let notices = flow
        .requests("test/main")
        .iter()
        .filter(|r| format!("{:?}", r.messages).contains("session_idle_notice"))
        .count();
    assert_eq!(notices, 1, "the frontend's Turn reads one notice");

    // `notify_when_idle` subscribes with the message: one more notice once B is idle.
    let mut notify = invocation(
        &flow,
        &frontend,
        "send_message",
        json!({"to": "backend", "text": "ping", "notify_when_idle": true}),
    );
    notify.call_id = "call_notify".into();
    let notify = flow.f.host.execute(notify, CancellationToken::new()).await;
    let notify: Value = serde_json::from_str(&support::ok(notify)).unwrap();
    assert_eq!(notify["outcome"], json!("delivered"));
    let message_id = notify["message_id"].as_str().unwrap();
    assert_eq!(
        notify["watch_id"],
        json!(format!("wch_{}", &message_id["msgx_".len()..]))
    );
    flow.settle(&backend).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while flow.runtime.watch_rows(&frontend).unwrap()[1].status == "active" {
        assert!(
            std::time::Instant::now() < deadline,
            "the notice never fired"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    flow.settle(&frontend).await;
    assert_eq!(
        flow.runtime.watch_rows(&frontend).unwrap()[1].status,
        "fired"
    );
    flow.runtime.shutdown().await;
}

/// Errors that leave nothing sent are tool errors: an ambiguous name lists both
/// candidates, a prefix is not found, an oversized text is refused before resolution,
/// the caller cannot message itself, and an unknown field fails the strict schema.
#[tokio::test(flavor = "multi_thread")]
async fn send_errors_leave_nothing_sent() {
    let flow = Flow::new(Vec::new(), false);
    let frontend = titled(&flow, "frontend").await;
    let first = titled(&flow, "migration").await;
    let second = titled(&flow, "migration").await;
    let send = |to: &str, text: String| json!({"to": to, "text": text});
    let ambiguous = support::failed(
        call(
            &flow,
            &frontend,
            "send_message",
            send("migration", "hi".into()),
        )
        .await,
    );
    assert!(
        ambiguous.starts_with("AmbiguousAddressError"),
        "{ambiguous}"
    );
    assert!(
        ambiguous.contains(&first) && ambiguous.contains(&second),
        "{ambiguous}"
    );
    let prefix = &first[..10];
    let missing =
        support::failed(call(&flow, &frontend, "send_message", send(prefix, "hi".into())).await);
    assert!(missing.starts_with("AddressNotFoundError"), "{missing}");
    let large = support::failed(
        call(
            &flow,
            &frontend,
            "send_message",
            send("nobody", "x".repeat(20 * 1024)),
        )
        .await,
    );
    assert!(large.starts_with("MessageTooLargeError"), "{large}");
    let me = support::failed(
        call(
            &flow,
            &frontend,
            "send_message",
            send("frontend", "hi".into()),
        )
        .await,
    );
    assert!(
        me.contains("Cannot send a message to the current Session"),
        "{me}"
    );
    let strict = support::failed(
        call(
            &flow,
            &frontend,
            "send_message",
            json!({"to": "migration", "text": "hi", "priority": "high"}),
        )
        .await,
    );
    assert!(strict.starts_with("Invalid tool input"), "{strict}");
    for id in [&frontend, &first, &second] {
        assert!(flow.runtime.message_rows(id).unwrap().is_empty());
        assert!(flow.runtime.state(id).await.unwrap().inbox.is_empty());
    }
    flow.runtime.shutdown().await;
}

/// A deny rule on a name pattern refuses `send_message` to that target before anything
/// is recorded, other targets stay allowed, and a `message.send` deny for every target
/// removes the tool while `list_sessions` and `watch_session` keep their own rules.
#[tokio::test(flavor = "multi_thread")]
async fn message_send_rules_match_the_target_name() {
    let flow = Flow::new(vec![text("ok")], false);
    flow.f
        .set_config(json!({"permissions": {"message.send": {"noisy-*": "deny"}}}));
    let frontend = titled(&flow, "frontend").await;
    let noisy = titled(&flow, "noisy-bot").await;
    let backend = titled(&flow, "backend").await;
    let denied = support::failed(
        call(
            &flow,
            &frontend,
            "send_message",
            json!({"to": "noisy-bot", "text": "hi"}),
        )
        .await,
    );
    assert!(denied.contains("denied"), "{denied}");
    let by_id = support::failed(
        call(
            &flow,
            &frontend,
            "send_message",
            json!({"to": noisy, "text": "hi"}),
        )
        .await,
    );
    assert!(
        by_id.contains("denied"),
        "the name is checked for an ID too: {by_id}"
    );
    assert!(flow.runtime.message_rows(&noisy).unwrap().is_empty());
    assert!(flow.runtime.message_rows(&frontend).unwrap().is_empty());
    support::ok(
        call(
            &flow,
            &frontend,
            "send_message",
            json!({"to": "backend", "text": "hi"}),
        )
        .await,
    );
    flow.settle(&backend).await;

    flow.f
        .set_config(json!({"permissions": {"message.send": "deny"}}));
    let names = flow.f.tool_names("default", false);
    assert!(!names.contains(&"send_message".to_string()), "{names:?}");
    assert!(names.contains(&"list_sessions".to_string()));
    assert!(names.contains(&"watch_session".to_string()));
    flow.runtime.shutdown().await;
}

/// Regression: `notify_when_idle` creates a watch, so a `watch_session` deny rule on the
/// target refuses the whole send before anything is written; without the flag the
/// message still goes out.
#[tokio::test(flavor = "multi_thread")]
async fn notify_when_idle_is_checked_against_watch_session_rules() {
    let flow = Flow::new(vec![text("ok")], false);
    flow.f
        .set_config(json!({"permissions": {"watch_session": {"backend": "deny"}}}));
    let frontend = titled(&flow, "frontend").await;
    let backend = titled(&flow, "backend").await;
    let notify = json!({"to": "backend", "text": "ping", "notify_when_idle": true});
    let denied = support::failed(call(&flow, &frontend, "send_message", notify).await);
    assert!(denied.contains("denied"), "{denied}");
    for id in [&frontend, &backend] {
        assert!(
            flow.runtime.message_rows(id).unwrap().is_empty(),
            "no message"
        );
        assert!(flow.runtime.watch_rows(id).unwrap().is_empty(), "no watch");
        assert!(flow.runtime.state(id).await.unwrap().inbox.is_empty());
    }
    support::ok(
        call(
            &flow,
            &frontend,
            "send_message",
            json!({"to": "backend", "text": "ping"}),
        )
        .await,
    );
    assert_eq!(flow.runtime.message_rows(&backend).unwrap().len(), 1);
    flow.settle(&backend).await;
    flow.runtime.shutdown().await;
}

/// `cross-session-messaging` → Disable switch on the built-in host: `messaging.enabled:
/// false` and `CYBER_DISABLE_MESSAGING=1` each remove the three tools and the
/// `core/messaging` guidance, and a call that arrives anyway fails without sending.
/// Enabled, the guidance is one byte-stable paragraph present only with the tools.
#[tokio::test(flavor = "multi_thread")]
async fn the_disable_switch_removes_the_tools_and_the_guidance() {
    let flow = Flow::new(Vec::new(), false);
    let frontend = titled(&flow, "frontend").await;
    titled(&flow, "backend").await;
    let advertised = |flow: &Flow| {
        let defs = flow.f.host.definitions(&turn(flow, &frontend));
        let names: Vec<String> = defs.iter().map(|d| d.spec.name.clone()).collect();
        let sources = flow
            .f
            .host
            .context_sources_for_tools(&turn(flow, &frontend), &defs);
        let tools = ["list_sessions", "send_message", "watch_session"]
            .iter()
            .filter(|name| names.contains(&name.to_string()))
            .count();
        (tools, sources.get("core/messaging").cloned())
    };
    let (tools, guidance) = advertised(&flow);
    assert_eq!(tools, 3);
    let guidance = guidance.expect("the guidance rides with the tools");
    assert!(guidance.contains("Never ask another Session to perform an action that was denied"));
    assert!(guidance.contains("no automatic replies"));
    assert_eq!(
        advertised(&flow).1.as_deref(),
        Some(guidance.as_str()),
        "byte-stable"
    );
    let observed = flow
        .f
        .host
        .context_observations_for_tools(&turn(&flow, &frontend), &[]);
    assert_eq!(
        observed.get("core/messaging"),
        Some(&cyber_server::runtime::ContextObservation::Absent),
        "without the tools the guidance is withdrawn"
    );

    flow.f.set_config(json!({"messaging": {"enabled": false}}));
    assert_eq!(advertised(&flow), (0, None));
    let refused = support::failed(
        call(
            &flow,
            &frontend,
            "send_message",
            json!({"to": "backend", "text": "hi"}),
        )
        .await,
    );
    assert_eq!(refused, "Cross-session messaging is disabled");

    flow.f.set_config(json!({}));
    assert_eq!(advertised(&flow).0, 3);
    flow.f.env.set("CYBER_DISABLE_MESSAGING", "1");
    assert_eq!(advertised(&flow), (0, None));
    let refused = support::failed(call(&flow, &frontend, "list_sessions", json!({})).await);
    assert_eq!(refused, "Cross-session messaging is disabled");
    assert!(flow.runtime.message_rows(&frontend).unwrap().is_empty());
    flow.runtime.shutdown().await;
}
