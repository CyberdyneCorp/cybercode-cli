//! `/export` (`tui` → Export command) against a production App: the command writes the
//! default `<title-slug>-<short-id>.html` in the Location through the runner's own
//! `perform`, and a second export onto that file asks before replacing it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, App, Overlay};
use crate::export::default_name;
use crate::screens::{Opts, Server, render};
use cyber_client::export::Format;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter))
}

async fn run(server: &Server, app: &mut App, actions: Vec<Action>) {
    assert!(
        matches!(actions.as_slice(), [Action::Export(_)]),
        "{actions:?}"
    );
    for action in actions {
        let msg = crate::perform::perform(&server.client, &app.session, action).await;
        crate::runner::apply(app, msg, &mut Default::default());
    }
}

#[tokio::test]
async fn export_writes_the_default_page_and_asks_before_replacing_it() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    // The fixture's Location is a name only; export into a real directory.
    let location = tempfile::tempdir().unwrap();
    app.session.directory = location.path().to_string_lossy().into_owned();
    let path = std::path::Path::new(&app.session.directory).join(default_name(
        &app.session.title,
        &app.session.id,
        Format::Html,
    ));

    let actions = typed(&mut app, "/export");
    run(&server, &mut app, actions).await;
    let page = std::fs::read_to_string(&path).unwrap();
    assert!(page.starts_with("<!doctype html>\n"), "{page}");
    assert!(page.contains("Content-Security-Policy"), "{page}");
    let toast = app
        .toast
        .as_ref()
        .map(|(text, _)| text.clone())
        .unwrap_or_default();
    assert!(toast.contains(&path.display().to_string()), "{toast}");
    assert!(toast.contains("full conversation"), "{toast}");

    // The file exists now: the next export asks, and n keeps it.
    std::fs::write(&path, "mine").unwrap();
    let actions = typed(&mut app, "/export");
    run(&server, &mut app, actions).await;
    assert!(matches!(app.overlay, Overlay::ConfirmExport(_)));
    let screen = render(&mut app, 120, 40, &Opts::default());
    assert!(screen.contains("Overwrite export?"), "{screen}");
    assert!(app.on_key(key(KeyCode::Char('n'))).is_empty());
    assert!(matches!(app.overlay, Overlay::None));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");

    // y replaces it.
    let actions = typed(&mut app, "/export");
    run(&server, &mut app, actions).await;
    let actions = app.on_key(key(KeyCode::Char('y')));
    run(&server, &mut app, actions).await;
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .starts_with("<!doctype html>")
    );

    // `/export md notes.md` names the format and the file.
    let actions = typed(&mut app, "/export md notes.md");
    run(&server, &mut app, actions).await;
    let notes = std::path::Path::new(&app.session.directory).join("notes.md");
    assert!(std::fs::read_to_string(notes).unwrap().starts_with("# "));
}
