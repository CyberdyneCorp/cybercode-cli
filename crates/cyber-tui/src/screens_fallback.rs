//! The model fallback in the TUI (`provider-catalog` → Model fallback chain): the footer
//! shows the active fallback until it ends, and the event explains the switch.

use serde_json::json;

use crate::screens::{Opts, Server, render};

#[tokio::test]
async fn the_footer_shows_an_active_fallback_and_the_event_explains_it() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let soon = crate::loops::now_ms() + 600_000;

    let event = cyber_client::Event {
        kind: "session.model.fallback.1".into(),
        data: json!({"from": "test/main", "to": "test/backup", "kind": "provider_internal",
            "reason": "529 overloaded", "started_ms": 0, "until_ms": soon}),
        seq: Some(99),
        session_id: Some(id.clone()),
    };
    crate::runner::server_event(&mut app, &event);
    let toast = app.toast.as_ref().map(|(text, _)| text.clone());
    assert_eq!(
        toast.as_deref(),
        Some("test/main failed (provider_internal); continuing on test/backup")
    );

    app.toast = None;
    app.session.fallback = Some(("test/backup".into(), soon));
    let screen = render(&mut app, 120, 40, &Opts::default());
    assert!(screen.contains("fallback: test/backup"), "{screen}");

    app.session.fallback = Some(("test/backup".into(), crate::loops::now_ms() - 1));
    let screen = render(&mut app, 120, 40, &Opts::default());
    assert!(
        !screen.contains("fallback:"),
        "an expired fallback is not shown: {screen}"
    );
}

#[test]
fn the_session_view_carries_the_fallback() {
    let session = crate::model::Session::parse(&json!({
        "id": "ses_1",
        "fallback": {"from": "test/main", "to": "test/backup", "kind": "authentication",
            "reason": "invalid key", "started_ms": 1, "until_ms": 2}
    }));
    assert_eq!(session.fallback, Some(("test/backup".into(), 2)));
    assert_eq!(
        crate::model::Session::parse(&json!({"id": "ses_1"})).fallback,
        None
    );
}
