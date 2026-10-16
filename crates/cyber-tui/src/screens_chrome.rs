//! The composer, status footer, Turn footer marker and command palette (`tui` →
//! Composer appearance, Status line, Turn footer marker, Command palette,
//! Keybindings), drawn by the real `view::draw` from Sessions a production App ran with
//! a local scripted model.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::model::Item;
use crate::screens::{Opts, Server, buffer, render, row_of, stabilize};

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, modifiers))
}

fn typed(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c), KeyModifiers::NONE);
    }
}

fn row(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

/// Wait until `id` is idle with `assistants` assistant entries that all ended.
async fn settled(server: &Server, id: &str, assistants: usize) -> Value {
    for _ in 0..1000 {
        let page = server
            .client
            .get(&format!("/sessions/{id}/messages?order=asc&limit=200"))
            .await
            .unwrap();
        let ended = page["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["kind"] == "assistant" && m["ended_ms"].is_i64())
            .count();
        if ended >= assistants && !server.host.runtime.is_running(id) {
            return page;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} did not settle");
}

/// Run `action` the way the runner's dispatch does, against the real directory.
async fn perform(server: &Server, app: &App, action: Action) -> crate::perform::Msg {
    let mut session = app.session.clone();
    session.directory = server.root().display().to_string();
    crate::perform::perform_owned(&server.client, &session, action, None)
        .await
        .unwrap()
}

/// A completed, an interrupted (Esc while the model streams) and a failed run.
async fn three_runs(server: &Server) -> String {
    let id = server.answered_session().await;
    server.prompt(&id, "HOLD on").await;
    let mut app = server.app(&id).await;
    for _ in 0..1000 {
        if server.host.runtime.is_running(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    app.session.running = true;
    let actions = press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(actions, vec![Action::Interrupt]);
    perform(server, &app, Action::Interrupt).await;
    settled(server, &id, 2).await;
    server.model.release();
    server.prompt(&id, "FAIL now").await;
    settled(server, &id, 3).await;
    id
}

#[tokio::test]
async fn markers_for_completed_interrupted_and_failed_runs_come_from_durable_messages() {
    let server = Server::start().await;
    let id = three_runs(&server).await;
    let page = settled(&server, &id, 3).await;
    let mut app = server.load(&id).await;
    let assistants: Vec<&Value> = page["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["kind"] == "assistant")
        .collect();
    assert_eq!(assistants[1]["error_kind"], "interrupted");
    assert!(assistants[2]["error"].is_string(), "{page:#}");
    assert_ne!(assistants[2]["error_kind"], "interrupted");

    // The markers the reloaded state yields span the server's own step times.
    let markers = crate::turns::markers(&app.items, false);
    assert_eq!(markers.len(), 3);
    for (marker, step) in markers.iter().zip(&assistants) {
        let took = step["ended_ms"].as_i64().unwrap() - step["started_ms"].as_i64().unwrap();
        assert_eq!(marker.duration_ms, took);
        assert_eq!(marker.agent, "build");
    }
    // A fresh load (another client, or after a restart) builds the same markers.
    let again = server.load(&id).await;
    assert_eq!(crate::turns::markers(&again.items, false).len(), 3);

    stabilize(&mut app);
    let opts = Opts {
        legend: &["▣"],
        ..Opts::default()
    };
    let shown = render(&mut app, 120, 40, &opts);
    assert!(shown.contains("▣ Build · main · 11.3s\n"), "{shown}");
    assert!(
        shown.contains("▣ Build · main · 11.3s · interrupted"),
        "{shown}"
    );
    assert!(shown.contains("▣ Build · main · 11.3s · failed"), "{shown}");
    insta::assert_snapshot!("turn_markers_120x40", shown);

    app.timestamps = true;
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    assert!(row(&buf, row_of(&buf, "· failed")).contains("· 11.3s · 14:02 · failed"));

    app.screen_reader = true;
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    row_of(
        &buf,
        "Turn finished: agent Build, model main, 11.3s, 14:02, interrupted",
    );
}

#[tokio::test]
async fn the_running_run_has_no_marker_until_it_ends() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    app.session.running = true;
    let buf = buffer(&mut app, 100, 30, &Opts::default());
    assert!(!(0..30).any(|y| row(&buf, y).contains('▣')));
    app.session.running = false;
    let buf = buffer(&mut app, 100, 30, &Opts::default());
    row_of(&buf, "▣ Build · main · 11.3s");
}

/// `test/main` with a display name, a price and a 116-token window, so the server's
/// meter reads 100 of 116 tokens (86%).
async fn priced_server() -> Server {
    Server::start_with(json!({
        "name": "Amini Coder",
        "limits": { "context": 116, "output": 8 },
        "cost": { "input": 1.0, "output": 2.0 },
    }))
    .await
}

#[tokio::test]
async fn footer_shows_cwd_context_cost_and_the_hint_at_60_and_120_columns() {
    let server = priced_server().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    // The footer alone: no docked sidebar beside it.
    app.sidebar.preference = crate::sidebar::Preference::Hide;
    let meter = app.session.context.expect("the server reports the meter");
    assert_eq!((meter.used_tokens, meter.window), (100, Some(116)));
    let msg = crate::perform::catalog(&server.client).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    for width in [60, 120] {
        let opts = Opts {
            legend: &["ctrl+p commands"],
            ..Opts::default()
        };
        let shown = render(&mut app, width, 20, &opts);
        let footer = shown.lines().nth(19).unwrap().to_owned();
        assert!(
            footer.ends_with("100 (86%) · $0.00  ctrl+p commands"),
            "{footer}"
        );
        assert!(footer.starts_with(" /Users/"), "{footer}");
        insta::assert_snapshot!(format!("footer_{width}x20"), shown);
    }
    // Above 85% the context field is drawn in the warning color.
    let buf = buffer(&mut app, 120, 20, &Opts::default());
    let x = (0..120).find(|&x| buf[(x, 19)].symbol() == "(").unwrap();
    assert_eq!(buf[(x, 19)].fg, app.theme.warning);
    let cost = (0..120).find(|&x| buf[(x, 19)].symbol() == "$").unwrap();
    assert_eq!(buf[(cost, 19)].fg, app.theme.text_muted);

    // The live `session.usage` event moves the meter without a refresh.
    app.on_event(
        "session.usage",
        &json!({ "context": { "used_tokens": 50, "window": 116, "compact_at": 108 } }),
    );
    let buf = buffer(&mut app, 120, 20, &Opts::default());
    assert!(row(&buf, 19).contains("50 (43%) · $0.00"));
}

#[tokio::test]
async fn a_toast_replaces_the_left_side_and_an_unpriced_model_shows_a_dash() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    // The footer alone: no docked sidebar beside it.
    app.sidebar.preference = crate::sidebar::Preference::Hide;
    let buf = buffer(&mut app, 120, 20, &Opts::default());
    let footer = row(&buf, 19);
    assert!(
        footer.trim_end().ends_with("100 · —  ctrl+p commands"),
        "{footer}"
    );
    app.toast("Copied 3 lines (pbcopy)");
    let buf = buffer(&mut app, 120, 20, &Opts::default());
    assert!(row(&buf, 19).starts_with(" Copied 3 lines (pbcopy)"));
    assert!(!row(&buf, 19).contains("/Users/leo"));
    let later = Opts {
        clock_ms: 6_500,
        ..Opts::default()
    };
    let buf = buffer(&mut app, 120, 20, &later);
    assert!(row(&buf, 19).starts_with(" /Users/leo/work/app"));
}

#[tokio::test]
async fn composer_modes_recolor_the_bar_and_the_info_line() {
    let server = priced_server().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let msg = crate::perform::catalog(&server.client).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    let t = app.theme.clone();
    let bar = |buf: &Buffer| buf[(0, buf.area.height - 3)].fg;

    let buf = buffer(&mut app, 80, 24, &Opts::default());
    let info = row(&buf, 22);
    assert!(
        info.starts_with("┃ Build · Amini Coder test · default"),
        "{info}"
    );
    assert!(row(&buf, 20).contains("Ask anything… \"/\" commands · \"@\" files · \"!\" shell"));
    assert_eq!(bar(&buf), t.accent);
    let opts = Opts {
        legend: &["┃"],
        ..Opts::default()
    };
    insta::assert_snapshot!("composer_prompt_80x24", render(&mut app, 80, 24, &opts));

    typed(&mut app, "!ls");
    let buf = buffer(&mut app, 80, 24, &Opts::default());
    assert_eq!(bar(&buf), t.warning);
    assert!(row(&buf, 22).starts_with("┃ shell"));

    app.composer.set_text("/help");
    app.completion = None;
    let buf = buffer(&mut app, 80, 24, &Opts::default());
    assert_eq!(bar(&buf), t.info);
    assert!(row(&buf, 22).starts_with("┃ command"));

    app.composer.set_text("one\ntwo\nthree");
    app.session.running = true;
    let opts = Opts {
        legend: &["esc interrupt"],
        ..Opts::default()
    };
    let shown = render(&mut app, 80, 24, &opts);
    assert!(
        shown.contains("┃ Build · Amini Coder test · default   … esc interrupt"),
        "{shown}"
    );
    insta::assert_snapshot!("composer_running_80x24", shown);
    // Ten lines show the last eight, with the cursor on the last.
    app.composer.set_text(
        &(1..=10)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let buf = buffer(&mut app, 80, 30, &Opts::default());
    row_of(&buf, "┃ line 10");
    assert!(!(0..30).any(|y| row(&buf, y).starts_with("┃ line 2")));
}

#[tokio::test]
async fn the_palette_opens_settings_usage_and_runs_commands() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let msg = crate::perform::commands(&server.client).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    press(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert!(matches!(app.overlay, Overlay::Palette(_)));
    let opts = Opts {
        legend: &["Settings: Usage"],
        ..Opts::default()
    };
    let shown = render(&mut app, 100, 30, &opts);
    assert!(shown.contains("/model"), "{shown}");
    assert!(shown.contains("ctrl+x m"), "{shown}");
    insta::assert_snapshot!("palette_100x30", shown);

    typed(&mut app, "usage");
    let actions = press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(app.overlay, Overlay::Settings));
    assert_eq!(app.settings.tab, crate::settings::Tab::Usage);
    let [
        Action::LoadCost { .. },
        Action::LoadLimits { .. },
        Action::LoadInsights { .. },
    ] = &actions[..]
    else {
        panic!("{actions:?}")
    };
    for load in actions {
        let msg = perform(&server, &app, load).await;
        crate::runner::apply(&mut app, Ok(msg), &mut Default::default());
    }
    let buf = buffer(&mut app, 100, 30, &Opts::default());
    row_of(&buf, "Input");

    // Esc closes without side effects; `/` then Tab opens it too.
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL);
    typed(&mut app, "zzz");
    assert!(press(&mut app, KeyCode::Esc, KeyModifiers::NONE).is_empty());
    assert!(matches!(app.overlay, Overlay::None));
    typed(&mut app, "/");
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert!(matches!(app.overlay, Overlay::Palette(_)));
    assert_eq!(app.composer.text(), "");
    typed(&mut app, "/model");
    assert_eq!(
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE),
        vec![Action::LoadModels]
    );
}

/// `tui.keybinds` read by the runner's own startup.
#[tokio::test]
async fn a_rebound_palette_key_updates_the_footer_hint_and_disabled_keys_are_hidden() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let root = server.root();
    let config = root.join("config");
    std::fs::write(
        config.join("tui.jsonc"),
        r#"{ "keybinds": { "command_palette": "ctrl+k", "help": "none" },
             "statusline": { "fields": ["cwd", "agent", "context", "cost"] },
             "sidebar": "hide" }"#,
    )
    .unwrap();
    let opts = crate::TuiOptions {
        client: server.client.clone(),
        directory: root.display().to_string(),
        state_dir: root.join("state"),
        config_dir: config,
        start: crate::Start::Resume(id.clone()),
        fork: false,
        model: None,
        agent: None,
        mode: None,
        prompt: None,
    };
    let store = crate::store::LocalStore::new(&opts.state_dir);
    let (mut app, _) = crate::runner::start(&server.client, &store, &opts, &|_: &str| None)
        .await
        .unwrap();
    let msg = crate::perform::snapshot(&server.client, &id).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    stabilize(&mut app);
    let buf = buffer(&mut app, 120, 20, &Opts::default());
    let footer = row(&buf, 19);
    assert!(
        footer
            .trim_end()
            .ends_with("Build · 100 · —  ctrl+k commands"),
        "{footer}"
    );

    assert!(press(&mut app, KeyCode::Char('p'), KeyModifiers::CONTROL).is_empty());
    assert!(matches!(app.overlay, Overlay::None));
    press(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
    let Overlay::Palette(palette) = &app.overlay else {
        panic!("ctrl+k opens the palette")
    };
    let help = palette.entries.iter().find(|e| e.name == "/help").unwrap();
    assert_eq!(help.key, None, "a disabled binding is never shown");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    app.composer.set_text("/help");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    row_of(&buf, "ctrl+x m");
    assert!(!(0..40).any(|y| row(&buf, y).contains("ctrl+x h")));
}

/// Regression: `tui.keybinds.composer` was accepted but never read, so disabling
/// `history_previous` left Up recalling history.
#[tokio::test]
async fn a_disabled_composer_history_binding_leaves_up_moving_the_cursor() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let root = server.root();
    let config = root.join("config");
    std::fs::write(
        config.join("tui.jsonc"),
        r#"{ "keybinds": { "composer": { "history_previous": "none", "newline": "alt+j" } } }"#,
    )
    .unwrap();
    let opts = crate::TuiOptions {
        client: server.client.clone(),
        directory: root.display().to_string(),
        state_dir: root.join("state"),
        config_dir: config,
        start: crate::Start::Resume(id.clone()),
        fork: false,
        model: None,
        agent: None,
        mode: None,
        prompt: None,
    };
    let store = crate::store::LocalStore::new(&opts.state_dir);
    let (mut app, _) = crate::runner::start(&server.client, &store, &opts, &|_: &str| None)
        .await
        .unwrap();
    app.composer = crate::composer::Composer::new(vec!["earlier prompt".into()]);
    typed(&mut app, "a");
    press(&mut app, KeyCode::Char('j'), KeyModifiers::CONTROL);
    assert_eq!(
        app.composer.text(),
        "a",
        "ctrl+j is no longer the newline key"
    );
    press(&mut app, KeyCode::Char('j'), KeyModifiers::ALT);
    typed(&mut app, "b");
    press(&mut app, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(app.composer.cursor(), (0, 1), "Up moves the cursor");
    press(&mut app, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(app.composer.text(), "a\nb", "Up never recalls history");

    app.keymap = crate::keys::Keymap::default();
    press(&mut app, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(
        app.composer.text(),
        "earlier prompt",
        "the default recalls history"
    );
}

#[test]
fn a_streaming_answer_is_not_a_finished_run() {
    let items = vec![Item::User {
        id: "u".into(),
        text: "hi".into(),
        promoted_ms: Some(1),
    }];
    assert!(crate::turns::markers(&items, false).is_empty());
}
