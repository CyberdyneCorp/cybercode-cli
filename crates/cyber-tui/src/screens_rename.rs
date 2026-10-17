//! `/rename` (`session-runtime` → Fork, rename, archive, delete) against a production
//! App: the command and its dialog send the rename through the runner's own `perform`,
//! and the server's `session.renamed.1` event retitles the header and sidebar.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, render};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter))
}

/// Perform `actions` and feed this Session's events until its rename arrives.
async fn renamed(
    server: &Server,
    app: &mut App,
    events: &mut (impl futures::Stream<Item = cyber_client::Event> + Unpin),
    actions: Vec<Action>,
) {
    assert!(
        matches!(actions.as_slice(), [Action::RenameSession { .. }]),
        "{actions:?}"
    );
    for action in actions {
        let msg = crate::perform::perform(&server.client, &app.session, action).await;
        crate::runner::apply(app, msg, &mut Default::default());
    }
    loop {
        let event = tokio::time::timeout(Duration::from_secs(10), events.next())
            .await
            .expect("no rename event")
            .unwrap();
        crate::runner::server_event(app, &event);
        if event.kind == "session.renamed.1" && event.session_id.as_deref() == Some(&app.session.id)
        {
            return;
        }
    }
}

#[tokio::test]
async fn rename_retitles_the_session_from_the_command_and_the_dialog() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let mut events = Box::pin(server.client.events(true).await.unwrap());

    let actions = typed(&mut app, "/rename Auth refactor");
    renamed(&server, &mut app, &mut events, actions).await;
    assert_eq!(app.session.title, "Auth refactor");
    let screen = render(&mut app, 160, 48, &Opts::default());
    assert!(screen.contains("Auth refactor"), "{screen}");
    let stored = server.client.get(&format!("/sessions/{id}")).await.unwrap();
    assert_eq!(stored["data"]["title"], "Auth refactor");

    // A bare `/rename` opens the dialog prefilled with the title; Esc keeps it.
    assert!(typed(&mut app, "/rename").is_empty());
    assert!(matches!(&app.overlay, Overlay::Rename(t) if t == "Auth refactor"));
    let screen = render(&mut app, 120, 40, &Opts::default());
    assert!(screen.contains("Rename session"), "{screen}");
    assert!(screen.contains("Enter save · Esc cancel"), "{screen}");
    assert!(app.on_key(key(KeyCode::Esc)).is_empty());
    assert!(matches!(app.overlay, Overlay::None));

    // Edit in the dialog: clear, type, Enter.
    typed(&mut app, "/rename");
    app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.on_paste("Login\nfix");
    let actions = app.on_key(key(KeyCode::Enter));
    renamed(&server, &mut app, &mut events, actions).await;
    assert_eq!(app.session.title, "Login fix");
    assert!(render(&mut app, 160, 48, &Opts::default()).contains("Login fix"));

    // An empty title is refused locally and nothing is sent.
    typed(&mut app, "/rename");
    app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert!(app.on_key(key(KeyCode::Enter)).is_empty());
    assert_eq!(app.session.title, "Login fix");
}

#[test]
fn rename_is_offered_in_the_palette_and_help() {
    let mut app = App::new(Default::default(), Vec::new(), "cyber");
    let entries = crate::palette::entries(&app.keymap, &[]);
    assert!(
        entries.iter().any(|e| e.name == "Rename session"),
        "palette"
    );
    assert!(entries.iter().any(|e| e.name == "/rename"), "command");
    app.overlay = Overlay::Help;
    let screen = render(&mut app, 120, 48, &Opts::default());
    assert!(screen.contains("/rename [title]"), "{screen}");
}
