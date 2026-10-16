//! The Settings Stats tab (`tui` → Settings Stats tab, Accessibility), drawn by the real
//! `view::draw` from `GET /usage/stats` of a production App: one Session answered by a
//! local scripted model, plus a year of earlier Turns in its ledger. Replies go through
//! the runner's own `perform` and `apply`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, buffer, render, row_of};
use crate::settings::Tab;
use crate::settings::stats::{Period, View};
use crate::settings::usage::count;

const DAY: i64 = 86_400_000;
/// Snapshots show the period as if it ended on this day.
const TODAY: &str = "2026-10-16";

fn key(app: &mut App, c: char) -> Vec<Action> {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        key(app, c);
    }
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}

fn row(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// Earlier Turns of `root` on most of the last 340 days, with varying tokens, written
/// into the ledger the way the projector writes them.
fn earlier_turns(server: &Server, root: &str) {
    let root = root.to_owned();
    let directory = server.root().display().to_string();
    let now = now_ms();
    server
        .host
        .store
        .transaction(move |tx| {
            for days_ago in 1..340i64 {
                // Idle on some days, and never on the day before yesterday.
                if days_ago % 9 == 4 || days_ago % 13 == 0 || days_ago == 2 {
                    continue;
                }
                let tokens = 1_000 * (1 + (days_ago * 37) % 23);
                let at = now - days_ago * DAY;
                tx.execute(
                    "INSERT INTO usage_step (event_id, session_id, root_id, agent, purpose,
                        provider, model, started_ms, ended_ms, input_tokens, output_tokens,
                        reasoning_tokens, cache_read_tokens, cache_write_tokens, prompt_tokens,
                        cost, directory)
                     VALUES (?1, ?2, ?2, 'build', 'turn', 'test', ?3, ?4, ?4, ?5, ?6, 0, 0, 0,
                        ?5, 0.01, ?7)",
                    rusqlite::params![
                        format!("evt_earlier_{days_ago}"),
                        root,
                        if days_ago % 3 == 0 { "small" } else { "main" },
                        at,
                        tokens,
                        tokens / 10,
                        directory,
                    ],
                )
                .map_err(cyber_store::StoreError::from)?;
            }
            Ok(())
        })
        .unwrap();
}

/// Run what the tab asks for through `perform` and the runner's `apply`.
async fn load(server: &Server, app: &mut App, actions: Vec<Action>) {
    for action in actions {
        assert!(matches!(action, Action::LoadStats { .. }), "{action:?}");
        let mut session = app.session.clone();
        session.directory = server.root().display().to_string();
        let msg = crate::perform::perform_owned(&server.client, &session, action, None).await;
        crate::runner::apply(app, msg, &mut Default::default());
    }
}

/// What differs between runs: the dates (shifted to end on [`TODAY`]) and the longest
/// Session's wall time.
fn stabilize(app: &mut App) {
    let stats = app.settings.stats.data.data.as_mut().expect("stats loaded");
    let today: jiff::civil::Date = TODAY.parse().unwrap();
    let last: jiff::civil::Date = stats.days.last().unwrap().date.parse().unwrap();
    let shift = |date: &str| {
        let date: jiff::civil::Date = date.parse().unwrap();
        let span = last.until(date).unwrap();
        today.checked_add(span).unwrap().to_string()
    };
    for day in &mut stats.days {
        day.date = shift(&day.date);
    }
    stats.most_active_day = stats.most_active_day.as_deref().map(shift);
    if let Some(longest) = stats.longest_session.as_mut() {
        longest.duration_ms = 339 * DAY as u64 + 7_380_000;
    }
}

/// A production App with a year of Turns, and the TUI on its Stats tab.
async fn stats_app() -> (Server, App) {
    let priced = |input: f64, output: f64| json!({ "cost": { "input": input, "output": output } });
    let server = Server::start_models(
        json!({ "main": priced(10.0, 20.0), "small": priced(1.0, 2.0) }),
        json!({ "small_model": "test/small" }),
        false,
    )
    .await;
    let id = server.answered_session().await;
    earlier_turns(&server, &id);
    let mut app = server.app(&id).await;
    app.settings.stats.zone = Some("UTC".into());
    let actions = typed(&mut app, "/stats");
    assert!(matches!(app.overlay, Overlay::Settings));
    assert_eq!(app.settings.tab, Tab::Stats);
    load(&server, &mut app, actions).await;
    (server, app)
}

/// The route's own answer, as `cyber stats --format json` prints it.
async fn route(server: &Server, period: &str) -> Value {
    server
        .client
        .get(&format!("/usage/stats?period={period}&tz=UTC"))
        .await
        .unwrap()["data"]
        .clone()
}

/// The value after `label:` on its row.
fn value(buf: &Buffer, label: &str) -> String {
    let text = row(buf, row_of(buf, label));
    let (_, rest) = text.split_once(label).unwrap();
    rest.trim()
        .split("  ")
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

#[tokio::test]
async fn overview_heatmap_at_70_120_and_160_columns_and_without_color() {
    let (_server, mut app) = stats_app().await;
    stabilize(&mut app);
    let opts = Opts {
        legend: &["Less", "All time", "Favorite model:"],
        ..Opts::default()
    };
    for (width, height) in [(70, 40), (120, 40), (160, 48)] {
        let buf = buffer(&mut app, width, height, &opts);
        // Every week fits: the newest column (today, a Friday) is drawn, nothing clipped.
        let fri = row(&buf, row_of(&buf, "Fri "));
        assert!(fri.trim_end().chars().count() < width as usize - 2, "{fri}");
        insta::assert_snapshot!(
            format!("stats_overview_{width}x{height}"),
            render(&mut app, width, height, &opts)
        );
    }
    // At 50 columns the most recent 42 weeks fit, today's column included.
    let buf = buffer(&mut app, 50, 30, &Opts::default());
    let fri = row(&buf, row_of(&buf, "Fri "));
    let cells = fri.trim_end().chars().count() - 2 - crate::settings::heatmap::LABEL;
    assert_eq!(cells, 42, "{fri:?}");
    let plain = Opts {
        no_color: true,
        legend: &["Less", "All time"],
        ..Opts::default()
    };
    let buf = buffer(&mut app, 120, 40, &plain);
    let grid: String = (row_of(&buf, "Mon ") - 1..row_of(&buf, "Fri ") + 2)
        .map(|y| row(&buf, y))
        .collect();
    for glyph in ["·", "░", "▒", "▓", "█"] {
        assert!(grid.contains(glyph), "level {glyph} missing:\n{grid}");
    }
    insta::assert_snapshot!(
        "stats_overview_no_color_120x40",
        render(&mut app, 120, 40, &plain)
    );
}

#[tokio::test]
async fn the_period_key_recomputes_every_metric_for_the_last_7_days() {
    let (server, mut app) = stats_app().await;
    let actions = key(&mut app, 'p');
    assert_eq!(app.settings.stats.period, Period::Week);
    assert!(
        matches!(
            &actions[..],
            [Action::LoadStats {
                period: "7d",
                project: false,
                ..
            }]
        ),
        "{actions:?}"
    );
    load(&server, &mut app, actions).await;
    stabilize(&mut app);
    let expected = route(&server, "7d").await;
    let stats = app.settings.stats.data.data.as_ref().unwrap();
    assert_eq!(stats.days.len(), 7);
    let opts = Opts {
        legend: &["Last 7 days"],
        ..Opts::default()
    };
    let buf = buffer(&mut app, 120, 40, &opts);
    assert_eq!(
        value(&buf, "Active days:"),
        format!("{}/7", expected["active_days"])
    );
    assert_eq!(
        value(&buf, "Total tokens:"),
        count(expected["totals"]["total_tokens"].as_u64().unwrap())
    );
    // `Last 7 days` is the bold accent label; `All time` is muted.
    let periods = row_of(&buf, "Last 7 days");
    let x = row(&buf, periods).find("Last 7 days").unwrap() as u16;
    let all = &buf[(2, periods)];
    let week = &buf[(x, periods)];
    assert!(week.modifier.contains(ratatui::style::Modifier::BOLD));
    assert_eq!(week.fg, app.theme.accent);
    assert_ne!(all.fg, app.theme.accent);
    // A 7-day heatmap is two week columns.
    let mon = row(&buf, row_of(&buf, "Mon "));
    assert_eq!(mon.trim_end().chars().count(), 2 + 4 + 2, "{mon:?}");
    // Then the 30-day period, then back to all time.
    let actions = key(&mut app, 'p');
    assert!(matches!(
        &actions[..],
        [Action::LoadStats { period: "30d", .. }]
    ));
    let actions = key(&mut app, 'p');
    assert!(matches!(
        &actions[..],
        [Action::LoadStats { period: "all", .. }]
    ));
}

#[tokio::test]
async fn the_tab_and_the_cli_route_show_the_same_numbers() {
    let (server, mut app) = stats_app().await;
    key(&mut app, 'p');
    let actions = key(&mut app, 'p');
    load(&server, &mut app, actions).await;
    assert_eq!(app.settings.stats.period, Period::Month);
    let json = route(&server, "30d").await;
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    assert_eq!(
        value(&buf, "Total tokens:"),
        count(json["totals"]["total_tokens"].as_u64().unwrap())
    );
    assert_eq!(
        value(&buf, "Sessions:"),
        json["totals"]["sessions"].to_string()
    );
    assert_eq!(
        value(&buf, "Current streak:"),
        format!("{} days", json["current_streak"])
    );
    assert_eq!(
        value(&buf, "Longest streak:"),
        format!("{} days", json["longest_streak"])
    );
    assert_eq!(
        value(&buf, "Active days:"),
        format!("{}/{}", json["active_days"], json["period_days"])
    );
    assert_eq!(json["current_streak"], 2, "today and yesterday: {json}");
}

#[tokio::test]
async fn models_scope_and_the_screen_reader_sentence() {
    let (server, mut app) = stats_app().await;
    stabilize(&mut app);
    assert!(key(&mut app, 'v').is_empty());
    assert_eq!(app.settings.stats.view, View::Models);
    let opts = Opts {
        legend: &["%"],
        ..Opts::default()
    };
    let buf = buffer(&mut app, 120, 40, &opts);
    let main = row_of(&buf, "main");
    let small = row_of(&buf, "small");
    assert!(main < small, "sorted by tokens");
    assert!(row(&buf, main + 1).contains("█"), "{}", row(&buf, main + 1));
    insta::assert_snapshot!("stats_models_120x40", render(&mut app, 120, 40, &opts));

    // `s` asks for this project only.
    let actions = key(&mut app, 's');
    assert!(
        matches!(&actions[..], [Action::LoadStats { project: true, .. }]),
        "{actions:?}"
    );
    load(&server, &mut app, actions).await;
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    row_of(&buf, "This project");

    // Screen readers get one sentence and no grid.
    key(&mut app, 'v');
    app.screen_reader = true;
    stabilize(&mut app);
    let shown = render(&mut app, 120, 40, &Opts::default());
    assert!(shown.contains("Active "), "{shown}");
    assert!(shown.contains(" days; most active "), "{shown}");
    // Today and yesterday had Turns, the day before did not.
    assert!(shown.contains("; current streak 2 days"), "{shown}");
    assert!(!shown.contains("Mon "), "{shown}");
    assert!(!shown.contains("Less"), "{shown}");
}

#[tokio::test]
async fn a_failed_load_says_so_and_keeps_the_panel() {
    let (_server, mut app) = stats_app().await;
    let generation = app.settings.stats.data.start();
    app.settings
        .stats
        .data
        .apply(generation, Err("Invalid usage statistics".into()));
    let buf = buffer(&mut app, 100, 30, &Opts::default());
    row_of(
        &buf,
        "Could not load stats: Invalid usage statistics · r to retry",
    );
    assert!(matches!(app.overlay, Overlay::Settings));
    // Malformed replies are refused whole.
    let bad = json!({ "days": [{ "date": "not a date", "total_tokens": 0, "turns": 0 }] });
    assert!(crate::settings::stats::Stats::parse(&bad).is_err());
}
