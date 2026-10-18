//! The sidebar's Context bar (`tui` → Session sidebar): the context window meter from a
//! production App over HTTP, its states in light, dark and `NO_COLOR`, the threshold
//! colors, agreement with the footer and the live `session.usage` update.

use serde_json::json;

use crate::model::ContextMeter;
use crate::screens::{Opts, Server, buffer, render, row_of};

/// A 128K window that auto-compacts at 85% (108.8K).
fn meter(used_tokens: u64) -> ContextMeter {
    ContextMeter {
        used_tokens,
        window: Some(128_000),
        compact_at: Some(108_800),
        measured: true,
        compacted: false,
    }
}

/// `test/main` with a 400K window, an 8K output limit and a price.
async fn server() -> Server {
    Server::start_with(json!({
        "name": "Demo Sonnet",
        "limits": { "context": 400_000, "output": 8_000 },
        "cost": { "input": 1.0, "output": 2.0 },
    }))
    .await
}

/// The App for `id`, with the model catalog loaded so the model's display name shows.
async fn app(server: &Server, id: &str) -> crate::app::App {
    let mut app = server.app(id).await;
    let msg = crate::perform::catalog(&server.client).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    app
}

fn sidebar_text(screen: &str) -> String {
    screen
        .lines()
        .map(|line| line.chars().skip(110).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn context_bar_states_render_in_light_dark_and_no_color() {
    let server = server().await;
    let id = server.answered_session().await;
    let mut app = app(&server, &id).await;
    let unknown = ContextMeter {
        window: None,
        compact_at: None,
        ..meter(29_470)
    };
    let before = ContextMeter {
        used_tokens: 0,
        window: Some(400_000),
        compact_at: Some(340_000),
        measured: false,
        compacted: false,
    };
    let states = [
        (
            "early",
            meter(28_160),
            "28.2K / 128K tokens",
            "│ auto-compact at 85%",
        ),
        (
            "warning",
            meter(99_840),
            "99.8K / 128K tokens",
            "│ auto-compact at 85%",
        ),
        (
            "pending",
            meter(116_480),
            "116.5K / 128K tokens",
            "auto-compact pending",
        ),
        (
            "before",
            before,
            "0 / 400K tokens · Demo Sonnet",
            "│ auto-compact at 85%",
        ),
        (
            "unknown",
            unknown,
            "29,470 tokens · window unknown",
            "spent",
        ),
    ];
    for (state, meter, tokens, compaction) in states {
        app.session.context = Some(meter);
        for (variant, opts) in [
            (
                "dark",
                Opts {
                    legend: &["Context "],
                    ..Opts::default()
                },
            ),
            (
                "light",
                Opts {
                    light: true,
                    legend: &["Context "],
                    ..Opts::default()
                },
            ),
            (
                "no_color",
                Opts {
                    no_color: true,
                    ..Opts::default()
                },
            ),
        ] {
            let screen = render(&mut app, 160, 48, &opts);
            let side = sidebar_text(&screen);
            assert!(side.contains(tokens), "{state} {variant}: {side}");
            assert!(side.contains(compaction), "{state} {variant}: {side}");
            if state == "unknown" {
                assert!(!side.contains('%'), "no percent without a window: {side}");
            } else if opts.no_color {
                assert!(side.contains("[") && side.contains('|'), "{side}");
            }
            insta::assert_snapshot!(format!("context_{state}_{variant}_160x48"), screen);
        }
    }
}

#[tokio::test]
async fn bar_colors_follow_70_percent_and_the_compaction_point() {
    let server = server().await;
    let id = server.answered_session().await;
    let mut app = app(&server, &id).await;
    let t = app.theme.clone();
    for (percent, compact, color) in [
        (69.0, Some(85.0), t.success),
        (70.0, Some(85.0), t.warning),
        (84.0, Some(85.0), t.warning),
        (85.0, Some(85.0), t.error),
        (89.0, None, t.warning),
        (90.0, None, t.error),
    ] {
        assert_eq!(
            crate::view::sidebar::context_color(&t, percent, compact),
            color,
            "{percent} {compact:?}"
        );
    }
    for (used, color) in [(28_160, t.success), (99_840, t.warning), (116_480, t.error)] {
        app.session.context = Some(meter(used));
        let buf = buffer(&mut app, 160, 48, &Opts::default());
        let y = row_of(&buf, "Context ");
        let x = (110..buf.area.width)
            .find(|&x| buf[(x, y)].symbol() == "█")
            .expect("a filled cell");
        assert_eq!(buf[(x, y)].fg, color, "{used}");
        // The marker sits at the auto-compaction point.
        assert!(
            (110..buf.area.width).any(|x| buf[(x, y)].symbol() == "│"),
            "marker at 85%"
        );
    }
    // Off: no marker, and the line says so.
    app.session.context = Some(ContextMeter {
        compact_at: None,
        ..meter(28_160)
    });
    let screen = render(&mut app, 160, 48, &Opts::default());
    assert!(
        sidebar_text(&screen).contains("auto-compact off"),
        "{screen}"
    );
    let buf = buffer(&mut app, 160, 48, &Opts::default());
    let y = row_of(&buf, "Context ");
    assert!(!(110..buf.area.width).any(|x| buf[(x, y)].symbol() == "│"));
}

#[tokio::test]
async fn the_bar_and_the_footer_show_the_same_numbers() {
    let server = server().await;
    let id = server.answered_session().await;
    let mut app = app(&server, &id).await;
    let measured = app.session.context.expect("the server measured the reply");
    assert!(measured.measured, "{measured:?}");
    for used in [28_160, 99_840, 116_480] {
        app.session.context = Some(meter(used));
        let screen = render(&mut app, 160, 48, &Opts::default());
        let footer = screen.lines().last().unwrap().to_owned();
        let tokens = crate::view::footer::compact(used);
        let percent = format!("{:.0}%", used as f64 * 100.0 / 128_000.0);
        assert!(
            footer.contains(&format!("{tokens} ({percent})")),
            "{footer}"
        );
        let side = sidebar_text(&screen);
        assert!(side.contains(&format!("{tokens} / 128K tokens")), "{side}");
        assert!(side.contains(&format!(" {percent}")), "{side}");
    }
}

#[tokio::test]
async fn the_window_shows_before_the_first_reply_and_usage_events_fill_the_bar() {
    let server = server().await;
    let id = server.session().await;
    let mut app = app(&server, &id).await;
    // The real server reports the current model's window before any step.
    let meter = app.session.context.expect("the window is known up front");
    assert_eq!(
        (meter.used_tokens, meter.window, meter.measured),
        (0, Some(400_000), false)
    );
    let screen = render(&mut app, 160, 48, &Opts::default());
    let side = sidebar_text(&screen);
    assert!(side.contains("0 / 400K tokens · Demo Sonnet"), "{side}");
    // 400K minus the 20K compaction buffer.
    assert!(side.contains("│ auto-compact at 95%"), "{side}");
    // The footer and Status tab wait for a measured value.
    assert!(!screen.lines().last().unwrap().contains("(0%)"), "{screen}");

    app.on_event(
        "session.usage",
        &json!({ "context": {
            "used_tokens": 88_000, "window": 400_000, "compact_at": 340_000,
            "measured": true, "compacted": false,
        } }),
    );
    let screen = render(&mut app, 160, 48, &Opts::default());
    let side = sidebar_text(&screen);
    assert!(side.contains("88.0K / 400K tokens"), "{side}");
    assert!(side.contains(" 22%"), "{side}");
    assert!(
        screen.lines().last().unwrap().contains("88.0K (22%)"),
        "{screen}"
    );
}

#[test]
fn the_screen_reader_sentence_reads_the_meter() {
    assert_eq!(
        crate::sidebar::context_sentence(&ContextMeter {
            used_tokens: 29_500,
            ..meter(0)
        }),
        "Context 23 percent used, 29.5 thousand of 128 thousand tokens, auto-compact at 85 percent"
    );
    assert_eq!(
        crate::sidebar::context_sentence(&ContextMeter {
            window: None,
            compact_at: None,
            ..meter(500)
        }),
        "Context 500 tokens, window unknown"
    );
}
