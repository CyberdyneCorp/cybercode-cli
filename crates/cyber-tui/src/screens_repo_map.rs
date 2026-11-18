//! `/map [<path>]` (`tui` → Repository map view) against a production App and server:
//! the view opens on the index status and the focused map, adds nothing to the
//! transcript, and a disabled map shows a message instead of a view.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, render};

fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
    press(app, KeyCode::Enter)
}

/// Perform the map requests through the real server, at its real directory; the App
/// shows a stable one for snapshots.
async fn drive(server: &Server, app: &mut App, mut actions: Vec<Action>) {
    let shown = std::mem::replace(
        &mut app.session.directory,
        server.root().display().to_string(),
    );
    while !actions.is_empty() {
        let action = actions.remove(0);
        if !matches!(action, Action::RepoMap(_)) {
            continue;
        }
        let msg = crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
    }
    app.session.directory = shown;
}

fn source(server: &Server) {
    let root = server.root();
    std::fs::create_dir_all(root.join("src/auth")).unwrap();
    std::fs::write(
        root.join("src/auth/session.rs"),
        "pub struct SessionStore;\npub fn open_session() -> SessionStore { SessionStore }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "fn main() { let _ = auth::session::open_session(); }\n",
    )
    .unwrap();
}

#[tokio::test]
async fn inspect_the_map() {
    let server = Server::start().await;
    source(&server);
    let id = server.session().await;
    let mut app = server.app(&id).await;
    let before = format!("{:?}", app.session.seq);
    let actions = typed(&mut app, "/map src/auth");
    drive(&server, &mut app, actions).await;
    let Overlay::RepoMap(view) = &app.overlay else {
        panic!("no map view: {:?}", app.toast);
    };
    assert_eq!(view.focus.as_deref(), Some("src/auth"));
    assert!(view.header.contains("files indexed"), "{}", view.header);
    assert!(view.header.contains("index complete"), "{}", view.header);
    assert_eq!(view.rows[0].path.as_deref(), Some("src/auth/session.rs"));
    let screen = render(&mut app, 120, 30, &Opts::default());
    assert!(screen.contains("Repository map · src/auth"), "{screen}");
    assert!(
        screen.contains("pub fn open_session() -> SessionStore"),
        "{screen}"
    );
    assert!(screen.contains("r rebuild"), "{screen}");
    assert_eq!(
        format!("{:?}", app.session.seq),
        before,
        "the transcript is unchanged"
    );
    // `r` rebuilds and reloads; Enter opens the selected path in the file explorer.
    let actions = press(&mut app, KeyCode::Char('r'));
    drive(&server, &mut app, actions).await;
    assert!(matches!(app.overlay, Overlay::RepoMap(_)));
    let actions = press(&mut app, KeyCode::Enter);
    assert!(matches!(app.overlay, Overlay::Files));
    assert_eq!(app.explorer.filter, "src/auth/session.rs");
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::SearchFiles(path) if path == "src/auth/session.rs"))
    );
    app.overlay = Overlay::None;
    let actions = typed(&mut app, "/map");
    drive(&server, &mut app, actions).await;
    assert!(matches!(app.overlay, Overlay::RepoMap(_)));
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.overlay, Overlay::None));
}

#[tokio::test]
async fn map_disabled() {
    let server =
        Server::start_config(json!({}), json!({"repo_map": {"enabled": false}}), false).await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    let actions = typed(&mut app, "/map");
    drive(&server, &mut app, actions).await;
    assert!(matches!(app.overlay, Overlay::None));
    let toast = app.toast.as_ref().map(|(text, _)| text.as_str());
    assert_eq!(toast, Some("Repository map is disabled (repo_map.enabled)"));
}
