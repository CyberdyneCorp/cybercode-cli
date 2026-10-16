//! The theme switcher (`tui` → Theme switcher), drawn by the real `view::draw` over a
//! Session a production App ran with a local scripted model, and started the way `cyber`
//! starts the TUI so a kept theme is read back after a restart.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, buffer, cells, render, row_of};
use crate::switcher::Filter;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.completion = None;
    app.on_key(key(KeyCode::Enter))
}

fn row(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

fn highlighted(app: &App) -> String {
    let Overlay::Themes(s) = &app.overlay else {
        panic!("the switcher is not open")
    };
    s.current(app.theme_filter).unwrap().name.clone()
}

/// Move down to `name`, as the user would.
fn highlight(app: &mut App, name: &str) {
    for _ in 0..40 {
        if highlighted(app) == name {
            return;
        }
        app.on_key(key(KeyCode::Down));
    }
    panic!("{name} is not in the list");
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

#[tokio::test]
async fn snapshots_with_two_highlighted_themes_and_without_color() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    typed(&mut app, "/theme");
    assert!(matches!(app.overlay, Overlay::Themes(_)));
    assert_eq!(highlighted(&app), "cyber");

    // Moving applies the theme to the whole TUI at once.
    highlight(&mut app, "synthwave-84");
    assert_eq!(app.theme.name, "synthwave-84");
    let opts = Opts {
        theme: "synthwave-84",
        legend: &["❯ synthwave-84", "+ an added line", "Contrast:", "DARK"],
        ..Opts::default()
    };
    let buf = buffer(&mut app, 124, 36, &opts);
    let y = row_of(&buf, "❯ synthwave-84");
    assert!(row(&buf, y).contains("●"));
    let current = row_of(&buf, "  cyber");
    assert!(
        row(&buf, current).contains("current"),
        "{}",
        row(&buf, current)
    );
    row_of(&buf, "Preview · synthwave-84");
    row_of(&buf, "✓ success");
    row_of(&buf, "▣ Build · main · 11.3s");
    assert!(row(&buf, row_of(&buf, "Contrast: text")).contains("AAA"));
    let added = row_of(&buf, "+ an added line");
    let x = row(&buf, added).find('+').unwrap() as u16;
    assert_eq!(buf[(x, added)].bg, app.theme.diff_added_bg);
    // The swatch is drawn on the theme's own background.
    let dot = row(&buf, y).chars().position(|c| c == '●').unwrap() as u16;
    assert_eq!(buf[(dot, y)].bg, app.theme.background);
    assert_eq!(buf[(dot, y)].fg, app.theme.accent);
    row_of(&buf, "LIGHT");
    insta::assert_snapshot!(
        "switcher_synthwave_124x36",
        render(&mut app, 124, 36, &opts)
    );

    highlight(&mut app, "github-light");
    let light = Opts {
        theme: "github-light",
        legend: &["❯ github-light", "Contrast:"],
        ..Opts::default()
    };
    insta::assert_snapshot!(
        "switcher_github_light_124x36",
        render(&mut app, 124, 36, &light)
    );

    let plain = Opts {
        theme: "github-light",
        no_color: true,
        legend: &["❯ github-light"],
        ..Opts::default()
    };
    let buf = buffer(&mut app, 124, 36, &plain);
    for cell in cells(&buf) {
        assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset), "{cell:?}");
    }
    let y = row_of(&buf, "❯ github-light");
    assert!(!row(&buf, y).contains('●'), "no swatches without color");
    let x = row(&buf, y).chars().position(|c| c == '❯').unwrap() as u16;
    assert!(buf[(x, y)].modifier.contains(Modifier::REVERSED));
    row_of(&buf, "Colors are disabled");
    insta::assert_snapshot!(
        "switcher_no_color_124x36",
        render(&mut app, 124, 36, &plain)
    );
}

#[tokio::test]
async fn esc_reverts_without_persisting_and_enter_persists_across_a_restart() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let store = crate::store::LocalStore::new(&server.root().join("state"));
    let mut app = started(&server, &id).await;
    assert_eq!(app.theme.name, "cyber");

    typed(&mut app, "/theme");
    highlight(&mut app, "synthwave-84");
    assert_eq!(app.theme.name, "synthwave-84");
    assert!(app.on_key(key(KeyCode::Esc)).is_empty());
    assert!(matches!(app.overlay, Overlay::None));
    assert_eq!(app.theme.name, "cyber");
    assert!(store.get("prefs").is_none_or(|p| p.get("theme").is_none()));
    assert_eq!(started(&server, &id).await.theme.name, "cyber");

    typed(&mut app, "/theme");
    // Typing searches; Backspace widens again.
    for c in "rose".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert_eq!(highlighted(&app), "rose-pine");
    assert_eq!(app.theme.name, "rose-pine");
    let actions = app.on_key(key(KeyCode::Enter));
    assert_eq!(actions, [Action::SaveTheme("rose-pine".into())]);
    for action in actions {
        crate::runner::persist(&store, action);
    }
    let restarted = started(&server, &id).await;
    assert_eq!(restarted.theme.name, "rose-pine");
    assert_eq!(
        crate::settings::config::current(&restarted, "theme"),
        ("rose-pine".into(), crate::prefs::Origin::Saved)
    );
}

#[tokio::test]
async fn theme_cycle_moves_through_the_dark_filter_and_announces_it() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    // Tab in the switcher sets the filter that `theme_cycle` follows.
    typed(&mut app, "/theme");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.theme_filter, Filter::Dark);
    let buf = buffer(&mut app, 124, 36, &Opts::default());
    let y = row_of(&buf, " Dark ");
    let x = row(&buf, y).find(" Dark ").unwrap();
    let x = row(&buf, y)[..x].chars().count() as u16;
    assert!(
        buf[(x + 1, y)].bg == app.theme.selection
            || buf[(x + 1, y)].modifier.contains(Modifier::REVERSED)
    );
    app.on_key(key(KeyCode::Esc));
    let dark: Vec<String> = {
        typed(&mut app, "/theme");
        let Overlay::Themes(s) = &app.overlay else {
            panic!()
        };
        let names = s
            .visible(Filter::Dark)
            .iter()
            .map(|&i| s.rows[i].name.clone())
            .collect();
        app.on_key(key(KeyCode::Esc));
        names
    };
    assert_eq!(dark[0], "cyber");
    let ctrl_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
    let mut saved = Vec::new();
    for _ in 0..2 {
        app.on_key(ctrl_x);
        saved.extend(app.on_key(key(KeyCode::Char('t'))));
    }
    assert!(matches!(app.overlay, Overlay::None), "no switcher opens");
    assert_eq!(app.theme.name, dark[2]);
    assert_eq!(saved.last(), Some(&Action::SaveTheme(dark[2].clone())));
    let name: &'static str = Box::leak(dark[2].clone().into_boxed_str());
    let opts = Opts {
        theme: name,
        ..Opts::default()
    };
    let buf = buffer(&mut app, 100, 30, &opts);
    let footer = row(&buf, 29);
    assert!(footer.contains(&format!("Theme: {}", dark[2])), "{footer}");
    // The announcement lasts 2 seconds.
    let later = Opts {
        theme: name,
        clock_ms: 2_500,
        ..Opts::default()
    };
    app.announcement = app
        .announcement
        .take()
        .map(|(text, _)| (text, app.started()));
    let buf = buffer(&mut app, 100, 30, &later);
    assert!(!row(&buf, 29).contains("Theme:"));
}

#[tokio::test]
async fn p_writes_the_project_and_the_config_row_and_palette_open_the_switcher() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let root = server.root();
    // A custom theme with too little contrast is listed as `custom` with a warning.
    std::fs::create_dir_all(root.join("config/themes")).unwrap();
    std::fs::write(
        root.join("config/themes/dim.json"),
        r##"{"theme":{"background":"#202020","text":"#505050","accent":"#606060"}}"##,
    )
    .unwrap();
    let mut app = started(&server, &id).await;

    // The Config tab's theme row opens the switcher; Esc goes back to the tab.
    typed(&mut app, "/config");
    assert_eq!(app.settings.config_row, 0);
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.overlay, Overlay::Themes(_)));
    highlight(&mut app, "dim");
    let opts = Opts {
        theme: "dim",
        ..Opts::default()
    };
    let buf = buffer(&mut app, 124, 36, &opts);
    let y = row_of(&buf, "❯ dim");
    assert!(row(&buf, y).contains("custom"));
    let warning = row_of(&buf, "low contrast");
    let x = row(&buf, warning).find("low contrast").unwrap();
    let x = row(&buf, warning)[..x].chars().count() as u16;
    assert_eq!(buf[(x, warning)].fg, app.theme.warning);
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.overlay, Overlay::Settings));
    app.on_key(key(KeyCode::Esc));

    // The palette's `Theme…` opens it too; `p` writes the project's tui.jsonc.
    app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    for c in "theme…".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.overlay, Overlay::Themes(_)));
    highlight(&mut app, "kanagawa");
    let actions = app.on_key(key(KeyCode::Char('p')));
    assert_eq!(actions, [Action::SaveProjectTheme("kanagawa".into())]);
    crate::runner::save_project_theme(&mut app, "kanagawa");
    let file = root.join(".cyber/tui.jsonc");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written["theme"], "kanagawa");
    let restarted = started(&server, &id).await;
    assert_eq!(restarted.theme.name, "kanagawa");
    assert_eq!(
        crate::settings::config::current(&restarted, "theme").1,
        crate::prefs::Origin::Project
    );
    // A tui.jsonc with comments is never rewritten.
    std::fs::write(&file, "// mine\n{ \"sidebar\": \"auto\" }\n").unwrap();
    crate::runner::save_project_theme(&mut app, "matrix");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "// mine\n{ \"sidebar\": \"auto\" }\n"
    );
    assert!(app.toast.as_ref().unwrap().0.contains("has comments"));
}

#[tokio::test]
async fn a_permission_request_closes_the_switcher_and_restores_the_theme() {
    let server = Server::start().await;
    let id = server.session().await;
    let mut app = server.app(&id).await;
    typed(&mut app, "/theme");
    highlight(&mut app, "matrix");
    server.prompt(&id, "WRITE the notes").await;
    for _ in 0..500 {
        let pending = server
            .client
            .get(&format!("/permissions/requests?session_id={id}"))
            .await
            .unwrap();
        if pending["data"].as_array().is_some_and(|a| !a.is_empty()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let msg = crate::perform::snapshot(&server.client, &id).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    assert!(matches!(app.overlay, Overlay::Permission(_)));
    assert_eq!(app.theme.name, "cyber");
}
