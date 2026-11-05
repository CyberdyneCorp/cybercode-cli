//! `/language` and the Config tab's language row (`localization` → Language selection)
//! against a production App and server: a choice is saved as the global `language`,
//! and the interface switches at once.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyber_core::locale::Locale;

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

/// Perform the language requests at the server's real directory; the App shows a
/// stable one for snapshots.
async fn drive(server: &Server, app: &mut App, mut actions: Vec<Action>) {
    let shown = std::mem::replace(
        &mut app.session.directory,
        server.root().display().to_string(),
    );
    while !actions.is_empty() {
        let action = actions.remove(0);
        if !matches!(action, Action::Language(_)) {
            continue;
        }
        let msg = crate::perform::perform_owned(&server.client, &app.session, action, None).await;
        actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
    }
    app.session.directory = shown;
}

fn global_config(server: &Server) -> String {
    let dir = server.root().join("config");
    ["cyber.jsonc", "cyber.json"]
        .iter()
        .find_map(|name| std::fs::read_to_string(dir.join(name)).ok())
        .unwrap()
}

#[tokio::test]
async fn the_command_picks_saves_and_switches_the_interface() {
    let server = Server::start().await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    app.session.title = "New session".into();

    let actions = typed(&mut app, "/language");
    assert!(matches!(app.overlay, Overlay::Language(_)));
    drive(&server, &mut app, actions).await;
    let picker = render(&mut app, 100, 30, &Opts::default());
    assert!(
        picker.contains("Português (Brasil)") && picker.contains("current"),
        "{picker}"
    );
    insta::assert_snapshot!("language_picker", picker);

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    let actions = press(&mut app, KeyCode::Enter);
    assert!(matches!(app.overlay, Overlay::None));
    drive(&server, &mut app, actions).await;
    assert_eq!(app.locale, Locale::PtBr, "{:?}", app.toast);
    assert!(global_config(&server).contains("\"language\": \"pt-BR\""));
    let screen = render(&mut app, 100, 30, &Opts::default());
    assert!(screen.contains("Idioma: Português (Brasil)"), "{screen}");
    assert!(screen.contains("Pergunte algo"), "{screen}");

    let actions = typed(&mut app, "/language FR");
    drive(&server, &mut app, actions).await;
    assert_eq!(app.locale, Locale::Fr);
    assert!(global_config(&server).contains("\"language\": \"fr\""));

    let actions = typed(&mut app, "/language klingon");
    assert!(actions.is_empty());
    let toast = app.toast.as_ref().map(|(text, _)| text.clone());
    assert_eq!(
        toast.as_deref(),
        Some("language : attendu auto, en, pt-BR, es ou fr, reçu klingon")
    );
}

#[tokio::test]
async fn the_config_row_opens_the_picker_and_returns_to_settings() {
    let server = Server::start().await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    let actions = typed(&mut app, "/config");
    drive(&server, &mut app, actions).await;
    let tab = render(&mut app, 120, 40, &Opts::default());
    let row = tab
        .lines()
        .find(|l| l.contains("language  "))
        .unwrap_or_default()
        .to_owned();
    assert!(
        row.contains("auto (English)") && row.contains("default"),
        "{tab}"
    );

    press(&mut app, KeyCode::End);
    let actions = press(&mut app, KeyCode::Enter);
    assert!(matches!(app.overlay, Overlay::Language(_)));
    drive(&server, &mut app, actions).await;
    for _ in 0..3 {
        press(&mut app, KeyCode::Down);
    }
    let actions = press(&mut app, KeyCode::Enter);
    assert!(
        matches!(app.overlay, Overlay::Settings),
        "Enter returns to Settings"
    );
    drive(&server, &mut app, actions).await;
    assert_eq!(app.locale, Locale::Es, "{:?}", app.toast);
    let tab = render(&mut app, 120, 40, &Opts::default());
    let row = tab
        .lines()
        .find(|l| l.contains("language  "))
        .unwrap_or_default()
        .to_owned();
    assert!(row.contains("Español") && row.contains("config"), "{tab}");

    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Esc);
    assert!(
        matches!(app.overlay, Overlay::Settings),
        "Esc returns to Settings"
    );
}
