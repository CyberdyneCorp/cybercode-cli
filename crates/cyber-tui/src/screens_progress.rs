//! The progress line above the composer (`tui` → Nyan progress indicator), drawn by
//! the real `view::draw` for Sessions a production App runs with a local scripted
//! model: a held Turn, a compaction from the server's own events, and the workflow-run
//! hook in known-progress mode.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::style::Color;
use serde_json::{Value, json};

use crate::app::App;
use crate::prefs::Origin;
use crate::progress::{FRAME, MAX_FPS, WorkflowProgress};
use crate::screens::{Opts, Server, buffer, render, row_of};

fn row(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

/// 12.4 s into the Turn, as in the "Unknown progress" scenario.
const AT: u64 = 12_400;

fn moving(clock_ms: u64) -> Opts {
    Opts {
        reduced_motion: false,
        clock_ms,
        legend: &["▀", "Thinking", "agents", "interrupt", ",------,", "~|_("],
        ..Opts::default()
    }
}

/// A Session whose Turn the scripted model holds, loaded as the runner loads it.
async fn held(server: &Server) -> (String, App) {
    let id = server.answered_session().await;
    server.prompt(&id, "HOLD on").await;
    for _ in 0..1000 {
        if server.host.runtime.is_running(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut app = server.app(&id).await;
    assert!(app.session.running, "the server reports the Turn running");
    // The Turn was first seen at the start of the UI's clock.
    buffer(&mut app, 110, 24, &moving(0));
    (id, app)
}

fn spinner(app: &mut App, value: &str) {
    app.prefs.set("spinner", value, Origin::Saved);
}

/// The next frame, `FRAME` after the last.
fn next_frame(app: &mut App, clock_ms: u64) {
    app.clock = Some(app.started() + Duration::from_millis(clock_ms));
    assert!(app.advance_frame(), "a frame is due at {clock_ms} ms");
}

#[tokio::test]
async fn nyan_frames_0_and_1_scroll_the_trail_at_110_columns() {
    let server = Server::start().await;
    let (_id, mut app) = held(&server).await;
    assert_eq!(app.spinner(), crate::progress::Spinner::Nyan, "the default");
    assert_eq!(app.progress.frame, 0);
    let first = buffer(&mut app, 110, 24, &moving(AT));
    let y = row_of(&first, "Thinking · 12.4s · esc interrupt");
    let line = row(&first, y);
    assert!(line.starts_with(" ▀▀▀"), "{line}");
    assert!(line.contains("▀~[,,_,,]:3  Thinking"), "{line}");
    // The trail fills the width: label, then one free column.
    assert_eq!(line.trim_end().chars().count(), 109, "{line}");
    // Two bands per cell, and the composer's info line has no second hint.
    let cell = &first[(1, y)];
    assert_ne!(cell.fg, cell.bg);
    let hints = (0..24)
        .filter(|&y| row(&first, y).contains("interrupt"))
        .count();
    assert_eq!(hints, 1);
    insta::assert_snapshot!("nyan_frame0_110x24", render(&mut app, 110, 24, &moving(AT)));

    next_frame(&mut app, AT + FRAME.as_millis() as u64);
    let second = buffer(&mut app, 110, 24, &moving(AT + 84));
    // The hues moved one cell toward the cat.
    assert_eq!(second[(2, y)].fg, first[(1, y)].fg);
    assert_eq!(second[(2, y)].bg, first[(1, y)].bg);
    insta::assert_snapshot!(
        "nyan_frame1_110x24",
        render(&mut app, 110, 24, &moving(AT + 84))
    );
    server.model.release();
}

#[tokio::test]
async fn nyan_tall_frames_0_and_1_and_the_short_terminal_fallback() {
    let server = Server::start().await;
    let (_id, mut app) = held(&server).await;
    spinner(&mut app, "nyan-tall");
    let opts = moving(AT);
    let buf = buffer(&mut app, 110, 32, &opts);
    let label = row_of(&buf, "Thinking · 12.4s");
    for (offset, art) in [(1, ",------,"), (0, "| /\\_/\\"), (-1, "~|_( ^ .^)")] {
        assert!(
            row(&buf, (label as i32 - offset) as u16).contains(art),
            "{art}"
        );
    }
    assert!(row(&buf, label + 2).contains("\"\"  \"\""));
    insta::assert_snapshot!("nyan_tall_frame0_110x32", render(&mut app, 110, 32, &opts));
    next_frame(&mut app, AT + 84);
    let opts = moving(AT + 84);
    let moved = buffer(&mut app, 110, 32, &opts);
    assert_ne!(
        (1..40)
            .map(|x| moved[(x, label - 1)].fg)
            .collect::<Vec<_>>(),
        (1..40).map(|x| buf[(x, label - 1)].fg).collect::<Vec<_>>(),
        "the wave moved"
    );
    insta::assert_snapshot!("nyan_tall_frame1_110x32", render(&mut app, 110, 32, &opts));
    // Below 30 rows it is the one-row nyan.
    let short = buffer(&mut app, 110, 29, &opts);
    let y = row_of(&short, "Thinking");
    assert!(row(&short, y).contains("▀~[,,_,,]:3"));
    assert!(!(0..29).any(|y| row(&short, y).contains(",------,")));
    server.model.release();
}

#[tokio::test]
async fn known_workflow_progress_draws_70_percent_of_the_trail() {
    let server = Server::start().await;
    let (_id, mut app) = held(&server).await;
    app.progress.set_workflow(Some(WorkflowProgress {
        label: "fanout".into(),
        done: 7,
        total: 10,
    }));
    let opts = moving(AT);
    let buf = buffer(&mut app, 110, 24, &opts);
    let y = row_of(&buf, "fanout 7/10 agents · 70%");
    let line = row(&buf, y);
    let trail = line.chars().filter(|&c| c == '▀').count();
    let rest = line.chars().filter(|&c| c == '·').count() - 1;
    assert_eq!(
        trail,
        ((trail + rest) as f64 * 0.7).round() as usize,
        "{line}"
    );
    assert!(line.contains("▀~[,,_,,]:3···"), "{line}");
    let x = line
        .find("7/10")
        .map(|b| line[..b].chars().count())
        .unwrap() as u16;
    assert_eq!(buf[(x, y)].fg, app.theme.success);
    insta::assert_snapshot!("nyan_known_70_110x24", render(&mut app, 110, 24, &opts));
    // A workflow name is untrusted: no escape sequence reaches the terminal.
    app.progress.set_workflow(Some(WorkflowProgress {
        label: "evil\u{1b}]52;c;SGk=\u{7}".into(),
        done: 1,
        total: 4,
    }));
    let buf = buffer(&mut app, 110, 24, &opts);
    row_of(&buf, "evil]52;c;SGk= 1/4 agents · 25%");
    server.model.release();
}

#[tokio::test]
async fn compaction_from_the_servers_events_shows_compacting() {
    // Keeping one Turn, two Turns leave history to compact.
    let keep = json!({ "compaction": { "keep": { "turns": 1 } } });
    let server = Server::start_config(json!({}), keep, false).await;
    let id = server.answered_session().await;
    server.prompt(&id, "again").await;
    for _ in 0..1000 {
        if !server.host.runtime.is_running(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut app = server.app(&id).await;
    server
        .client
        .post(&format!("/sessions/{id}/compact"), json!({}))
        .await
        .unwrap();
    let mut events: Vec<Value> = Vec::new();
    for _ in 0..1000 {
        let page = server
            .client
            .get(&format!("/sessions/{id}/history?limit=500"))
            .await
            .unwrap();
        events = page["data"].as_array().cloned().unwrap_or_default();
        if events.iter().any(|e| {
            e["type"]
                .as_str()
                .is_some_and(|k| k.starts_with("session.compaction.completed"))
        }) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let kinds: Vec<&str> = events.iter().filter_map(|e| e["type"].as_str()).collect();
    let started = kinds
        .iter()
        .position(|k| k.starts_with("session.compaction.started"))
        .unwrap_or_else(|| panic!("{kinds:?}"));
    for event in &events[..=started] {
        app.on_event(event["type"].as_str().unwrap(), &event["data"]);
    }
    let buf = buffer(&mut app, 110, 24, &Opts::default());
    let line = row(&buf, row_of(&buf, "compacting · 0.0s"));
    assert!(line.contains("[,,_,,]:3  compacting"), "{line}");
    let x = line.find("compacting").unwrap() as u16;
    assert_eq!(buf[(x, row_of(&buf, "compacting"))].fg, app.theme.warning);
    for event in &events[started + 1..] {
        app.on_event(event["type"].as_str().unwrap(), &event["data"]);
    }
    app.session.running = false;
    let buf = buffer(&mut app, 110, 24, &Opts::default());
    assert!(!(0..24).any(|y| row(&buf, y).contains("compacting")));
}

#[tokio::test]
async fn no_color_sixteen_colors_reduced_motion_and_screen_readers() {
    let server = Server::start().await;
    let (_id, mut app) = held(&server).await;
    // NO_COLOR: an `=-` trail and no colored cell.
    let plain = Opts {
        no_color: true,
        ..moving(AT)
    };
    let buf = buffer(&mut app, 110, 24, &plain);
    let y = row_of(&buf, "Thinking · 12.4s");
    assert!(row(&buf, y).starts_with(" =-=-=-=-"), "{}", row(&buf, y));
    let line = row(&buf, y);
    let (trail, _) = line[1..].split_once("~[,,_,,]:3  Thinking").unwrap();
    assert!(trail.chars().all(|c| c == '=' || c == '-'), "{line}");
    assert!((0..110).all(|x| (buf[(x, y)].fg, buf[(x, y)].bg) == (Color::Reset, Color::Reset)));
    insta::assert_snapshot!("nyan_no_color_110x24", render(&mut app, 110, 24, &plain));

    // 16 colors: the bands are ANSI colors.
    let ansi = Opts {
        depth: crate::theme::Depth::Ansi16,
        ..moving(AT)
    };
    let buf = buffer(&mut app, 110, 24, &ansi);
    let y = row_of(&buf, "Thinking");
    for x in 1..13 {
        let cell = &buf[(x, y)];
        for color in [cell.fg, cell.bg] {
            assert!(
                !matches!(color, Color::Rgb(..) | Color::Indexed(_)),
                "{color:?}"
            );
        }
    }

    // Reduced motion: the static cat, and no frame is ever scheduled.
    let still = Opts {
        reduced_motion: true,
        ..moving(AT)
    };
    let before = buffer(&mut app, 110, 24, &still);
    assert!(row(&before, row_of(&before, "Thinking")).starts_with(" [,,_,,]:3  Thinking · 12.4s"));
    assert_eq!(app.next_frame(), None);
    let frame = app.progress.frame;
    for ms in (AT..AT + 2_000).step_by(10) {
        app.clock = Some(app.started() + Duration::from_millis(ms));
        assert!(!app.advance_frame());
    }
    assert_eq!(app.progress.frame, frame);
    assert!(!(0..24).any(|y| row(&before, y).contains('▀')));

    // Screen readers: `Working, 10 seconds`, never the trail.
    app.screen_reader = true;
    let buf = buffer(&mut app, 110, 24, &still);
    let shown: String = (0..24).map(|y| row(&buf, y) + "\n").collect();
    assert!(
        shown.contains(" Working, 10 seconds\n") || shown.contains("Working, 10 seconds "),
        "{shown}"
    );
    assert!(
        !shown.contains("[,,_,,]") && !shown.contains('▀'),
        "{shown}"
    );
    let later = buffer(
        &mut app,
        110,
        24,
        &Opts {
            clock_ms: 19_900,
            ..still
        },
    );
    row_of(&later, "Working, 10 seconds");
    let later = buffer(
        &mut app,
        110,
        24,
        &Opts {
            clock_ms: 20_000,
            ..still
        },
    );
    row_of(&later, "Working, 20 seconds");
    server.model.release();
}

#[tokio::test]
async fn frames_run_at_most_12_per_second_and_stop_unfocused_or_off_screen() {
    let server = Server::start().await;
    let (_id, mut app) = held(&server).await;
    let opts = moving(0);
    buffer(&mut app, 110, 24, &opts);
    let start = app.started();
    let mut frames = Vec::new();
    for ms in 0..3_000u64 {
        app.clock = Some(start + Duration::from_millis(ms));
        if app.next_frame().is_some_and(|due| due <= app.now()) {
            assert!(app.advance_frame());
            frames.push(ms);
        }
    }
    assert!(frames.len() as u64 >= 3 * MAX_FPS - 3, "{frames:?}");
    for (i, first) in frames.iter().enumerate() {
        let within = frames[i..]
            .iter()
            .take_while(|&&at| at < first + 1000)
            .count();
        assert!(within as u64 <= MAX_FPS, "{within} frames from {first} ms");
    }
    // Unfocused: nothing is scheduled; focus back resumes.
    app.focused = false;
    assert_eq!(app.next_frame(), None);
    app.focused = true;
    assert!(app.next_frame().is_some());
    // Behind the full-screen Settings panel the indicator is off screen.
    app.overlay = crate::app::Overlay::Settings;
    buffer(&mut app, 110, 24, &opts);
    app.overlay = crate::app::Overlay::None;
    assert_eq!(app.next_frame(), None, "the last frame did not draw it");
    buffer(&mut app, 110, 24, &opts);
    assert!(app.next_frame().is_some());
    // `off` draws no line and schedules nothing; the info line keeps the hint.
    spinner(&mut app, "off");
    let buf = buffer(&mut app, 110, 24, &opts);
    assert_eq!(app.next_frame(), None);
    assert!(!(0..24).any(|y| row(&buf, y).contains("Thinking")));
    row_of(&buf, "default   esc interrupt");
    // `braille` and `dots` are one glyph per frame.
    spinner(&mut app, "braille");
    let buf = buffer(&mut app, 110, 24, &opts);
    let glyph = row(&buf, row_of(&buf, "Thinking")).chars().nth(1).unwrap();
    assert!(('⠀'..='⣿').contains(&glyph), "{glyph}");
    spinner(&mut app, "dots");
    let buf = buffer(&mut app, 110, 24, &opts);
    assert!(row(&buf, row_of(&buf, "Thinking")).contains("·"));
    server.model.release();
}

/// `tui.spinner` from `tui.jsonc` through the runner's own startup; an unknown value is
/// reported and the default stays.
#[tokio::test]
async fn tui_spinner_is_read_at_startup() {
    let server = Server::start().await;
    let id = server.answered_session().await;
    let root = server.root();
    let config = root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let opts = crate::TuiOptions {
        client: server.client.clone(),
        directory: root.display().to_string(),
        state_dir: root.join("state"),
        config_dir: config.clone(),
        start: crate::Start::Resume(id.clone()),
        fork: false,
        model: None,
        agent: None,
        mode: None,
        prompt: None,
    };
    let store = crate::store::LocalStore::new(&opts.state_dir);
    let env = |_: &str| None;
    for (value, expected) in [
        ("nyan-tall", crate::progress::Spinner::NyanTall),
        ("off", crate::progress::Spinner::Off),
        ("rainbow", crate::progress::Spinner::Nyan),
    ] {
        std::fs::write(
            config.join("tui.jsonc"),
            format!("{{ \"spinner\": \"{value}\" }}"),
        )
        .unwrap();
        let (app, _) = crate::runner::start(&server.client, &store, &opts, &env)
            .await
            .unwrap();
        assert_eq!(app.spinner(), expected, "{value}");
        let warned = app
            .themes
            .warnings
            .iter()
            .any(|w| w.contains("tui.spinner"));
        assert_eq!(warned, value == "rainbow", "{:?}", app.themes.warnings);
    }
}
