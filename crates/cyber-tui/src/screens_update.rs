//! Update notices (`tui` → Update notices, `installation-upgrade` → Minimum supported
//! version) against a production App: the server's own `installation.*` events reach the
//! TUI over the real instance stream and go through the runner's `server_event`; a TUI
//! that attaches after the check reads `GET /service/update` through `perform` and the
//! runner's `apply`. The view is the real `view::draw`.

use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;

use crate::app::{Action, App};
use crate::screens::{Opts, Server, render};
use cyber_server::runtime::{InstallationNotice, UpdateStatus};

type Events = BoxStream<'static, cyber_client::Event>;

async fn events(server: &Server) -> (Events, cyber_client::Event) {
    let mut events = server.client.events(true).await.unwrap().boxed();
    let first = events.next().await.unwrap();
    assert_eq!(first.kind, "server.connected");
    (events, first)
}

/// Run what the App asks for through `perform` and the runner's `apply`, as the runner
/// does; clipboard and editor actions have no server side.
async fn drive(server: &Server, app: &mut App, mut actions: Vec<Action>) {
    while !actions.is_empty() {
        let action = actions.remove(0);
        if matches!(action, Action::EditFile(_) | Action::Copy(_)) {
            continue;
        }
        let msg = crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
    }
}

/// Feed instance events to the App until one of `kind` (for `session`, when given).
async fn fed(
    server: &Server,
    app: &mut App,
    events: &mut Events,
    kind: &str,
    session: Option<&str>,
) -> cyber_client::Event {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let event = events.next().await.expect("the stream is open");
            let actions = crate::runner::server_event(app, &event);
            drive(server, app, actions).await;
            if event.kind == kind && (session.is_none() || event.session_id.as_deref() == session) {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {kind}"))
}

const AVAILABLE: &str = "cyber 1.0.1 is available — run \"cyber upgrade\"";

/// "Update notice during a run": the toast appears while the Turn is held, and the Drain
/// goes on to answer.
#[tokio::test]
async fn update_toast_during_a_run_keeps_the_drain() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let (mut events, _) = events(&server).await;
    server.prompt(&id, "HOLD on").await;
    for _ in 0..1000 {
        if server.host.runtime.is_running(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut app = server.app(&id).await;
    assert!(app.session.running, "the Turn is running");

    server.host.runtime.publish_installation(
        InstallationNotice {
            current: "1.0.0".into(),
            latest: "1.0.1".into(),
            kind: "patch".into(),
            notes_url: None,
        },
        false,
    );
    fed(
        &server,
        &mut app,
        &mut events,
        "installation.update_available.1",
        None,
    )
    .await;
    assert_eq!(app.toast.as_ref().map(|(t, _)| t.as_str()), Some(AVAILABLE));
    let screen = render(&mut app, 120, 30, &Opts::default());
    assert!(screen.contains(AVAILABLE), "{screen}");
    assert!(app.session.running, "the notice does not stop the run");
    assert!(server.host.runtime.is_running(&id));

    server.model.release();
    fed(&server, &mut app, &mut events, "session.idle", Some(&id)).await;
    assert!(!app.session.running);
    let screen = render(&mut app, 120, 30, &Opts::default());
    assert!(
        screen.contains("Memoization"),
        "the Turn answered: {screen}"
    );
}

/// "Attaching after the check": the toast is read from `GET /service/update` once on
/// attach, not again on a reconnect; an unsupported version also gets the banner.
#[tokio::test]
async fn toast_from_endpoint_after_attach() {
    let server = Server::start().await;
    server.host.runtime.set_update_status(UpdateStatus {
        current: "1.0.0".into(),
        channel: "stable".into(),
        method: Some("script".into()),
        latest: Some("1.1.0".into()),
        kind: Some("minor".into()),
        notes_url: None,
        min_supported: Some("1.0.5".into()),
        unsupported: true,
        updated: false,
        checked_at_ms: Some(1),
    });
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    assert!(app.toast.is_none());
    let (_events, connected) = events(&server).await;
    let actions = crate::runner::server_event(&mut app, &connected);
    assert!(actions.contains(&Action::LoadUpdate), "{actions:?}");
    drive(&server, &mut app, actions).await;
    let toast = "cyber 1.1.0 is available — run \"cyber upgrade\"";
    assert_eq!(app.toast.as_ref().map(|(t, _)| t.as_str()), Some(toast));
    let warning =
        "warning: cyber 1.0.0 is no longer supported (minimum 1.0.5); run \"cyber upgrade\"";
    let screen = render(&mut app, 120, 30, &Opts::default());
    assert!(screen.contains(toast), "{screen}");
    assert!(screen.contains(warning), "{screen}");

    // A reconnect re-reads the check but shows the same notice only once.
    app.toast = None;
    let actions = crate::runner::server_event(&mut app, &connected);
    drive(&server, &mut app, actions).await;
    assert!(app.toast.is_none(), "{:?}", app.toast);
    let screen = render(&mut app, 120, 30, &Opts::default());
    assert!(screen.contains(warning), "the banner stays: {screen}");

    // After a silent upgrade the live event names the restart.
    app.update_status(Some(serde_json::json!({
        "current": "1.0.0", "latest": "1.0.1", "kind": "patch", "updated": true,
    })));
    assert_eq!(
        app.toast.as_ref().map(|(t, _)| t.as_str()),
        Some("cyber updated to 1.0.1; restart cyber to use it")
    );
}
