//! The Settings panel (`tui` → Settings panel, Settings Status tab, Settings Config tab),
//! drawn by the real `view::draw` from Sessions a production App ran with a local
//! scripted model, and loaded through the runner's own `perform` and `apply`.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use serde_json::json;

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, buffer, render, row_of};
use crate::settings::Tab;
use crate::settings::status::Section;

fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
    press(app, KeyCode::Enter)
}

fn row(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

/// Run what a tab asks for through `perform` and the runner's `apply`.
async fn load(server: &Server, app: &mut App, actions: Vec<Action>) {
    for action in actions {
        let mut session = app.session.clone();
        session.directory = server.root().display().to_string();
        let msg = crate::perform::perform_owned(&server.client, &session, action, None).await;
        crate::runner::apply(app, msg, &mut Default::default());
    }
}

/// What differs between runs: the Session ID, the temporary directory and the version.
fn stabilize_status(app: &mut App) {
    app.client_version = "1.0.0".into();
    let status = app.settings.status.data.as_mut().expect("status loaded");
    if let Section::Ready(health) = &mut status.health {
        health.version = Some("1.0.0".into());
    }
    if let Section::Ready(session) = &mut status.session {
        session.id = "ses_settings".into();
        session.directory = "/Users/leo/work/app".into();
    }
}

/// `test/main` with a display name, a price and a 116-token window.
async fn priced_server() -> Server {
    Server::start_with(json!({
        "name": "Amini Coder",
        "limits": { "context": 116, "output": 8 },
        "cost": { "input": 1.0, "output": 2.0 },
    }))
    .await
}

#[tokio::test]
async fn status_and_config_tabs_at_80_and_120_columns_and_without_color() {
    let server = priced_server().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let msg = crate::perform::catalog(&server.client).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());

    let actions = typed(&mut app, "/status");
    assert!(matches!(app.overlay, Overlay::Settings));
    assert_eq!(app.settings.tab, Tab::Status);
    let buf = buffer(&mut app, 80, 24, &Opts::default());
    row_of(&buf, "Loading…");
    load(&server, &mut app, actions).await;
    stabilize_status(&mut app);
    let opts = Opts {
        legend: &["Settings", "Context:"],
        ..Opts::default()
    };
    for (width, height) in [(80, 24), (120, 40)] {
        let shown = render(&mut app, width, height, &opts);
        assert!(shown.contains("Amini Coder (test)"), "{shown}");
        assert!(shown.contains("100 / 116 tokens (86%)"), "{shown}");
        assert!(shown.contains("not a git repository"), "{shown}");
        insta::assert_snapshot!(format!("status_{width}x{height}"), shown);
    }
    let plain = Opts {
        no_color: true,
        legend: &["Settings"],
        ..Opts::default()
    };
    insta::assert_snapshot!("status_no_color_80x24", render(&mut app, 80, 24, &plain));

    // `2` jumps to Config; it loads nothing from the server.
    assert!(press(&mut app, KeyCode::Char('2')).is_empty());
    assert_eq!(app.settings.tab, Tab::Config);
    let opts = Opts {
        legend: &["Settings", "› theme"],
        ..Opts::default()
    };
    for (width, height) in [(80, 24), (120, 40)] {
        let shown = render(&mut app, width, height, &opts);
        assert!(
            shown.contains(crate::settings::config::CORE_POINTER),
            "{shown}"
        );
        insta::assert_snapshot!(format!("config_{width}x{height}"), shown);
    }
    let plain = Opts {
        no_color: true,
        legend: &["› theme"],
        ..Opts::default()
    };
    let buf = buffer(&mut app, 80, 24, &plain);
    let y = row_of(&buf, "› theme");
    assert!(
        buf[(4, y)]
            .modifier
            .contains(ratatui::style::Modifier::REVERSED)
    );
    insta::assert_snapshot!("config_no_color_80x24", render(&mut app, 80, 24, &plain));

    // Tab cycles, Shift+Tab goes back, Esc closes and `/settings` reopens the last tab.
    let actions = press(&mut app, KeyCode::Tab);
    assert_eq!(app.settings.tab, Tab::Usage);
    assert!(
        matches!(
            &actions[..],
            [
                Action::LoadCost { .. },
                Action::LoadLimits { .. },
                Action::LoadInsights { .. }
            ]
        ),
        "{actions:?}"
    );
    load(&server, &mut app, actions).await;
    let buf = buffer(&mut app, 80, 24, &Opts::default());
    row_of(&buf, "Cache hit rate");
    app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.settings.tab, Tab::Config);
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.overlay, Overlay::None));
    assert!(typed(&mut app, "/settings").is_empty());
    assert_eq!(app.settings.tab, Tab::Config);
}

#[tokio::test]
async fn a_different_server_version_is_a_warning_with_restart_the_service() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let actions = app.open_settings(Some(Tab::Status));
    load(&server, &mut app, actions).await;
    app.client_version = "0.9.1".into();
    if let Some(status) = app.settings.status.data.as_mut()
        && let Section::Ready(health) = &mut status.health
    {
        health.version = Some("0.9.0".into());
    }
    let buf = buffer(&mut app, 100, 30, &Opts::default());
    let y = row_of(&buf, "0.9.0 · restart the service");
    assert!(row(&buf, y - 1).contains("Client:        0.9.1"));
    let x = row(&buf, y).find("0.9.0").unwrap() as u16;
    assert_eq!(buf[(x, y)].fg, app.theme.warning);
    assert_eq!(buf[(x, y - 1)].fg, app.theme.warning);

    // A failed source says so and the panel stays open; `r` loads again.
    app.settings.status.data.as_mut().unwrap().mcp = Section::Failed("connection reset".into());
    let buf = buffer(&mut app, 100, 30, &Opts::default());
    row_of(
        &buf,
        "Could not load MCP servers: connection reset · r to retry",
    );
    let actions = press(&mut app, KeyCode::Char('r'));
    assert!(matches!(&actions[..], [Action::LoadStatus { .. }]));
    load(&server, &mut app, actions).await;
    assert!(matches!(
        app.settings.status.data.as_ref().unwrap().mcp,
        Section::Ready(_)
    ));
}

/// Start the TUI the way `cyber` does, for `id`, with the state in `state`.
async fn started(server: &Server, id: &str) -> App {
    let root = server.root();
    let opts = crate::TuiOptions {
        client: server.client.clone(),
        directory: root.display().to_string(),
        state_dir: root.join("state"),
        config_dir: root.join("config"),
        start: crate::Start::Resume(id.to_owned()),
        fork: false,
        model: None,
        agent: None,
        mode: None,
        prompt: None,
    };
    let store = crate::store::LocalStore::new(&opts.state_dir);
    let (app, _) = crate::runner::start(&server.client, &store, &opts, &|_: &str| None)
        .await
        .unwrap();
    app
}

fn origin(app: &App, key: &str) -> (String, crate::prefs::Origin) {
    crate::settings::config::current(app, key)
}

#[tokio::test]
async fn a_sidebar_hidden_from_config_persists_until_tui_jsonc_is_edited_later() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let store = crate::store::LocalStore::new(&server.root().join("state"));
    let mut app = started(&server, &id).await;
    assert!(app.sidebar.docked(160));
    typed(&mut app, "/config");
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    let mut saved = Vec::new();
    while origin(&app, "sidebar").0 != "hide" {
        saved.extend(press(&mut app, KeyCode::Enter));
    }
    // Applied at once, and saved through the runner.
    assert!(!app.sidebar.docked(160));
    assert_eq!(
        saved.last(),
        Some(&Action::SavePreference {
            key: "sidebar",
            value: json!("hide"),
        })
    );
    for action in saved {
        crate::runner::persist(&store, action);
    }
    let buf = buffer(&mut app, 120, 30, &Opts::default());
    let y = row_of(&buf, "› sidebar");
    assert!(row(&buf, y).contains("hide"));
    assert!(row(&buf, y).trim_end().ends_with("saved"));

    let app = started(&server, &id).await;
    assert_eq!(app.sidebar.preference, crate::sidebar::Preference::Hide);
    assert!(!app.sidebar.docked(160));
    assert_eq!(
        origin(&app, "sidebar"),
        ("hide".into(), crate::prefs::Origin::Saved)
    );

    // An edit of `tui.jsonc` made after the save wins.
    let file = server.root().join("config/tui.jsonc");
    std::fs::write(&file, "{ \"sidebar\": \"auto\" }").unwrap();
    let later = std::time::SystemTime::now() + Duration::from_secs(60);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let app = started(&server, &id).await;
    assert!(app.sidebar.docked(160));
    assert_eq!(
        origin(&app, "sidebar"),
        ("auto".into(), crate::prefs::Origin::Global)
    );
}

#[tokio::test]
async fn a_permission_request_closes_the_panel_and_shows_its_prompt() {
    let server = Server::start().await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    let actions = app.open_settings(Some(Tab::Usage));
    load(&server, &mut app, actions).await;
    assert!(matches!(app.overlay, Overlay::Settings));
    // In `default` mode the model's `write` asks; the Drain keeps running meanwhile.
    server.prompt(&id, "WRITE the notes").await;
    let mut asked = false;
    for _ in 0..500 {
        let pending = server
            .client
            .get(&format!("/permissions/requests?session_id={id}"))
            .await
            .unwrap();
        if pending["data"].as_array().is_some_and(|a| !a.is_empty()) {
            asked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(asked, "the write asked for permission");
    let msg = crate::perform::snapshot(&server.client, &id).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    assert!(matches!(app.overlay, Overlay::Permission(_)));
    let buf = buffer(&mut app, 100, 30, &Opts::default());
    row_of(&buf, "Allow once");
    assert!(!row(&buf, 1).contains("Settings"));
}
