//! The accessibility pass (`tui` → Accessibility) over the new surfaces, drawn by the real
//! `view::draw` from a production App with a local scripted model: screen-reader output
//! (the Turn marker sentence, no sidebar, the heatmap sentence) and a keyboard-only
//! walkthrough of the palette, the sidebar focus, every Settings tab and the theme
//! switcher, where the focused element is always in reverse video or marked `›`/`❯`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, buffer, row_of};
use crate::settings::Tab;

fn key(app: &mut App, code: KeyCode) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl(app: &mut App, c: char) -> Vec<Action> {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn chars(app: &mut App, text: &str) {
    for c in text.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn row(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

/// Run what a key asked for through `perform` and the runner's `apply`.
async fn run(server: &Server, app: &mut App, actions: Vec<Action>) {
    for action in actions {
        let mut session = app.session.clone();
        if session.directory.is_empty() {
            session.directory = server.root().display().to_string();
        }
        let msg = crate::perform::perform_owned(&server.client, &session, action, None).await;
        crate::runner::apply(app, msg, &mut Default::default());
    }
}

const PLAIN: Opts = Opts {
    theme: "cyber",
    no_color: true,
    depth: crate::theme::Depth::None,
    light: false,
    reduced_motion: true,
    clock_ms: 0,
    legend: &[],
};

/// Under `NO_COLOR`, the row holding `needle` shows focus with reverse video or a marker.
fn focused(app: &mut App, needle: &str) {
    let buf = buffer(app, 160, 48, &PLAIN);
    let y = row_of(&buf, needle);
    let text = row(&buf, y);
    let reversed = (0..160).any(|x| buf[(x, y)].modifier.contains(Modifier::REVERSED));
    assert!(
        reversed || text.contains('›') || text.contains('❯'),
        "no focus marker on {needle:?}: {text}"
    );
}

#[tokio::test]
async fn screen_reader_mode_states_the_marker_drops_the_sidebar_and_describes_the_heatmap() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let buf = buffer(&mut app, 160, 48, &Opts::default());
    row_of(&buf, "Context");
    assert!(app.sidebar.docked(160));

    app.screen_reader = true;
    let buf = buffer(&mut app, 160, 48, &Opts::default());
    let shown: String = (0..48).map(|y| row(&buf, y) + "\n").collect();
    assert!(
        !shown.contains("Context"),
        "the sidebar is not drawn: {shown}"
    );
    assert!(!shown.contains("▣"), "{shown}");
    row_of(&buf, "Turn finished: agent Build, model main, 11.3s");
    // Focusing the sidebar explains itself instead of moving focus into nothing.
    ctrl(&mut app, 'x');
    key(&mut app, KeyCode::Char('e'));
    assert!(app.sidebar.focus.is_none());
    assert!(app.toast.as_ref().unwrap().0.contains("screen-reader"));

    // The Stats tab is one sentence, with no heatmap grid.
    app.completion = None;
    chars(&mut app, "/stats");
    app.completion = None;
    let actions = key(&mut app, KeyCode::Enter);
    run(&server, &mut app, actions).await;
    assert_eq!(app.settings.tab, Tab::Stats);
    let lines: Vec<String> = crate::view::settings_lines(&app, 150)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        lines.iter().any(|l| l.starts_with("Active 1 of 1 days")),
        "{lines:#?}"
    );
    assert!(
        lines.iter().all(|l| !l.contains('░') && !l.contains("Mon")),
        "{lines:#?}"
    );
}

#[tokio::test]
async fn keyboard_only_walkthrough_of_the_palette_sidebar_tabs_and_switcher() {
    // A Turn that wrote a file, so the sidebar has a Modified files row to focus.
    let server = Server::start_config(serde_json::json!({}), serde_json::json!({}), true).await;
    let work = server.root().join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    git(&work, &["commit", "-q", "--allow-empty", "-m", "start"]);
    let created = server
        .client
        .at(&work.display().to_string())
        .post(
            "/sessions",
            serde_json::json!({ "model": "test/main", "mode": "accept-edits" }),
        )
        .await
        .unwrap();
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    server.prompt(&id, "WRITE the notes").await;
    server.wait_answered(&id).await;
    let mut app = server.app(&id).await;
    assert!(!app.sidebar.rows().is_empty(), "a Modified files row");
    buffer(&mut app, 160, 48, &PLAIN);

    // Palette: open, move, filter, run.
    ctrl(&mut app, 'p');
    assert!(matches!(app.overlay, Overlay::Palette(_)));
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Up);
    chars(&mut app, "settings: usage");
    focused(&mut app, "Settings: Usage");
    let actions = key(&mut app, KeyCode::Enter);
    assert_eq!(app.settings.tab, Tab::Usage);
    run(&server, &mut app, actions).await;

    // Every tab by number, Tab, Shift+Tab and the arrows; the active tab is reversed.
    for (n, tab) in Tab::ALL.into_iter().enumerate() {
        let actions = key(&mut app, KeyCode::Char(char::from(b'1' + n as u8)));
        run(&server, &mut app, actions).await;
        assert_eq!(app.settings.tab, tab);
        focused(&mut app, &format!(" {} ", tab.label()));
    }
    let actions = key(&mut app, KeyCode::Tab);
    run(&server, &mut app, actions).await;
    assert_eq!(app.settings.tab, Tab::Status);
    let actions = app.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    run(&server, &mut app, actions).await;
    assert_eq!(app.settings.tab, Tab::Stats);
    // Stats: period, view and scope keys, then refresh.
    for c in ['p', 's', 'v'] {
        let actions = key(&mut app, KeyCode::Char(c));
        run(&server, &mut app, actions).await;
    }
    focused(&mut app, " Models ");
    assert!(matches!(
        &key(&mut app, KeyCode::Char('r'))[..],
        [Action::LoadStats { .. }]
    ));
    key(&mut app, KeyCode::Left);
    // Usage: day and week, and scrolling to the end and back.
    assert_eq!(app.settings.tab, Tab::Usage);
    assert!(matches!(
        &key(&mut app, KeyCode::Char('w'))[..],
        [Action::LoadInsights { window: "7d", .. }]
    ));
    key(&mut app, KeyCode::End);
    key(&mut app, KeyCode::Home);
    assert_eq!(app.settings.scroll, 0);
    // Config: rows by arrow, the selected one marked.
    key(&mut app, KeyCode::Left);
    assert_eq!(app.settings.tab, Tab::Config);
    key(&mut app, KeyCode::Down);
    focused(&mut app, "› theme_mode");
    key(&mut app, KeyCode::Up);
    // Enter on `theme` opens the switcher: filter, search, move, revert.
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.overlay, Overlay::Themes(_)));
    key(&mut app, KeyCode::Tab);
    focused(&mut app, " Dark ");
    chars(&mut app, "kana");
    assert_eq!(app.theme.name, "kanagawa");
    // (Drawing with the harness re-applies its own theme; the switcher keeps its state.)
    focused(&mut app, "❯ kanagawa");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.overlay, Overlay::Settings));
    assert_eq!(app.theme.name, "cyber");
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.overlay, Overlay::None));

    // Sidebar: focus it, walk its rows, collapse a section, and come back.
    ctrl(&mut app, 'x');
    key(&mut app, KeyCode::Char('e'));
    assert_eq!(app.sidebar.focus, Some(0));
    assert!(app.sidebar.focused().is_some());
    let buf = buffer(&mut app, 160, 48, &PLAIN);
    let side = 160 - crate::view::layout::sidebar_width(160);
    let marked = (0..48).any(|y| {
        (side..160).any(|x| {
            buf[(x, y)].modifier.contains(Modifier::REVERSED) || buf[(x, y)].symbol() == "›"
        })
    });
    assert!(marked, "the focused sidebar row is marked");
    key(&mut app, KeyCode::Down);
    assert_eq!(app.sidebar.focus, Some(1));
    key(&mut app, KeyCode::Up);
    key(&mut app, KeyCode::Enter);
    assert!(app.sidebar.focus.is_some());
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Esc);
    assert!(app.sidebar.focus.is_none());
    // Typing reaches the composer again.
    chars(&mut app, "hi");
    assert_eq!(app.composer.text(), "hi");
}
