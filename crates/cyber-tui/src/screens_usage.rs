//! The Settings Usage tab (`tui` → Settings Usage tab, Usage limit meters), drawn by the
//! real `view::draw` from Sessions a production App ran with a local scripted model, and
//! loaded through the runner's own `perform` and `apply`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, buffer, render, row_of};
use crate::settings::Tab;

fn typed(app: &mut App, text: &str) -> Vec<Action> {
    for c in text.chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
}

fn row(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

/// Run what the tab asks for through `perform` and the runner's `apply`.
async fn load(server: &Server, app: &mut App, actions: Vec<Action>) {
    for action in actions {
        let mut session = app.session.clone();
        session.directory = server.root().display().to_string();
        let msg = crate::perform::perform_owned(&server.client, &session, action, None).await;
        crate::runner::apply(app, msg, &mut Default::default());
    }
}

/// What differs between runs: the durations and the reset time and zone.
fn stabilize(app: &mut App) {
    let cost = app.cost.snapshot_mut().expect("usage loaded");
    let breakdown = cost
        .breakdown
        .as_mut()
        .expect("the server sends a breakdown");
    breakdown.api_duration_ms = 11_300;
    breakdown.wall_duration_ms = Some(12_060_000);
    for budget in &mut breakdown.budgets {
        if budget.reset_ms.is_some() {
            // Midnight UTC on 2026-10-17.
            budget.reset_ms = Some(1_792_195_200_000);
            budget.zone = Some("UTC".into());
        }
    }
}

/// A Session on `main` whose title came from `small`, through `/usage`.
async fn usage_app(main: Value, small: Value, extra: Value) -> (Server, App) {
    let mut config = json!({ "small_model": "test/small" });
    if let (Some(config), Value::Object(extra)) = (config.as_object_mut(), extra) {
        config.extend(extra);
    }
    let server = Server::start_models(json!({ "main": main, "small": small }), config, false).await;
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let actions = typed(&mut app, "/usage");
    assert!(matches!(app.overlay, Overlay::Settings));
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
    stabilize(&mut app);
    (server, app)
}

fn priced(input: f64, output: f64) -> Value {
    json!({ "cost": { "input": input, "output": output } })
}

/// The value of the `label:` summary row.
fn value(buf: &Buffer, label: &str) -> String {
    let y = row_of(buf, label);
    row(buf, y)
        .trim()
        .strip_prefix(label)
        .unwrap()
        .trim()
        .to_owned()
}

#[tokio::test]
async fn two_models_at_80_and_120_columns_and_without_color() {
    let (_server, mut app) = usage_app(priced(10.0, 20.0), priced(1.0, 2.0), json!({})).await;
    let opts = Opts {
        legend: &["Total cost:", "main:"],
        ..Opts::default()
    };
    let buf = buffer(&mut app, 120, 40, &opts);
    assert_eq!(value(&buf, "Total cost:"), "$0.0013");
    assert_eq!(value(&buf, "Total duration (API):"), "11s");
    assert_eq!(value(&buf, "Total duration (wall):"), "3h 21m 0s");
    // The Turn model cost more than the title model, so it is listed first.
    let main = row_of(&buf, "main:");
    let small = row_of(&buf, "small:");
    assert!(main < small, "main at {main}, small at {small}");
    assert!(
        row(&buf, main).contains("100 input, 10 output, 0 cache read, 0 cache write ($0.0012)")
    );
    assert!(row(&buf, small).contains("($0.0001)"));
    row_of(&buf, "Cache hit rate");
    for (width, height) in [(80, 24), (120, 40)] {
        insta::assert_snapshot!(
            format!("usage_two_models_{width}x{height}"),
            render(&mut app, width, height, &opts)
        );
    }
    let plain = Opts {
        no_color: true,
        legend: &[" Usage "],
        ..Opts::default()
    };
    insta::assert_snapshot!("usage_no_color_80x24", render(&mut app, 80, 24, &plain));

    // `/cost` opens the same tab at the Session section.
    app.settings.scroll = 3;
    app.overlay = Overlay::None;
    let actions = typed(&mut app, "/cost");
    assert_eq!(app.settings.tab, Tab::Usage);
    assert_eq!(app.settings.scroll, 0);
    assert!(matches!(
        &actions[..],
        [
            Action::LoadCost { .. },
            Action::LoadLimits { .. },
            Action::LoadInsights { .. }
        ]
    ));
}

#[tokio::test]
async fn an_unpriced_session_shows_a_dash_not_zero() {
    let (_server, mut app) = usage_app(json!({}), json!({}), json!({})).await;
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    assert_eq!(value(&buf, "Total cost:"), "—");
    assert!(row(&buf, row_of(&buf, "main:")).contains("cache write (—)"));
    assert!(!row(&buf, row_of(&buf, "Total cost:")).contains("$0.00"));
}

#[tokio::test]
async fn partial_pricing_is_a_lower_bound_with_the_unpriced_count() {
    let (_server, mut app) = usage_app(priced(10.0, 20.0), json!({}), json!({})).await;
    let buf = buffer(&mut app, 120, 40, &Opts::default());
    assert_eq!(value(&buf, "Total cost:"), "≥ $0.0012 · 1 unpriced calls");
    assert!(row(&buf, row_of(&buf, "small:")).contains("(—)"));
}

#[tokio::test]
async fn no_budget_means_no_limits_section() {
    let (_server, mut app) = usage_app(priced(10.0, 20.0), priced(1.0, 2.0), json!({})).await;
    // The scripted gateway sends no usage window headers, so no provider limit either.
    assert_eq!(app.settings.limits.data.data, Some(Vec::new()));
    let shown = render(&mut app, 120, 40, &Opts::default());
    assert!(!shown.contains("% used"), "{shown}");
    assert!(!shown.contains("budget"), "{shown}");
    let rows = crate::settings::usage::lines(&app, 116);
    assert!(!rows.iter().any(|r| r.to_string().contains("% used")));
}

#[tokio::test]
async fn a_daily_budget_at_85_percent_is_a_warning_with_its_reset_time() {
    use cyber_store::{Expected, NewEvent};
    let mut config = json!({ "small_model": "test/small" });
    config["budgets"] =
        json!({ "daily": { "max_cost_usd": 20.0 }, "session": { "max_turns": 10 } });
    let server = Server::start_models(
        json!({ "main": priced(10.0, 20.0), "small": priced(1.0, 2.0) }),
        config,
        false,
    )
    .await;
    // $17.00 of today's spending came from another Session, through the projector.
    let other = server.session().await;
    server
        .host
        .store
        .append(
            &other,
            Expected::Any,
            vec![NewEvent::new(
                "usage.recorded.1",
                json!({"provider":"test","model":"test/main","purpose":"discarded_title",
                    "call_id":null,"duration_ms":10,
                    "usage":{"input":1,"output":1,"reasoning":0,"cache_read":0,"cache_write":0},
                    "cost":17.0}),
            )],
        )
        .unwrap();
    let id = server.answered_session().await;
    let mut app = server.app(&id).await;
    let actions = typed(&mut app, "/usage");
    load(&server, &mut app, actions).await;
    stabilize(&mut app);
    let opts = Opts {
        legend: &["█"],
        ..Opts::default()
    };
    let buf = buffer(&mut app, 120, 40, &opts);
    let label = row_of(&buf, "Daily budget ($20.00)");
    let meter = label + 1;
    assert!(
        row(&buf, meter).contains("85% used"),
        "{}",
        row(&buf, meter)
    );
    let x = row(&buf, meter).find('█').unwrap() as u16;
    assert_eq!(buf[(x, meter)].fg, app.theme.warning);
    assert!(row(&buf, meter + 1).contains("Resets 12:00am (UTC)"));
    // The Session's own Turn budget is below 80%.
    let turns = row_of(&buf, "Session budget (10 Turns)") + 1;
    assert!(row(&buf, turns).contains("10% used"));
    let x = row(&buf, turns).find('█').unwrap() as u16;
    assert_eq!(buf[(x, turns)].fg, app.theme.meter_fill);
    insta::assert_snapshot!(
        "usage_daily_budget_120x40",
        render(&mut app, 120, 40, &opts)
    );
}

/// A finished call of a child of `root` with `prompt` prompt tokens, a minute ago.
fn child_call(server: &Server, root: &str, id: &str, agent: &str, prompt: i64) {
    let (root, id, agent) = (root.to_owned(), id.to_owned(), agent.to_owned());
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        - 60_000;
    server
        .host
        .store
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO usage_step (event_id, session_id, root_id, parent_id, agent, purpose,
                    provider, model, started_ms, ended_ms, input_tokens, output_tokens,
                    reasoning_tokens, cache_read_tokens, cache_write_tokens, prompt_tokens, cost,
                    directory)
                 VALUES (?1, ?2, ?3, ?3, ?4, 'turn', 'test', 'main', ?5, ?5, ?6, 0, 0, 0, 0, ?6,
                    1.0, '/elsewhere')",
                rusqlite::params![
                    format!("evt_{id}"),
                    format!("ses_{id}"),
                    root,
                    agent,
                    at,
                    prompt
                ],
            )?;
            Ok(())
        })
        .unwrap();
}

/// `What's contributing to your usage?` from the production `GET /usage/insights`: two
/// subagent groups that used most of the tree at a large context, then `w`.
#[tokio::test]
async fn contributing_factors_and_subagents_from_the_server() {
    let server = Server::start_models(
        json!({ "main": priced(10.0, 20.0), "small": priced(1.0, 2.0) }),
        json!({ "small_model": "test/small" }),
        false,
    )
    .await;
    let id = server.answered_session().await;
    child_call(&server, &id, "explore1", "explore", 200_000);
    child_call(&server, &id, "review1", "review", 1_000);
    let mut app = server.app(&id).await;
    let actions = typed(&mut app, "/usage");
    load(&server, &mut app, actions).await;
    stabilize(&mut app);
    let data = app
        .settings
        .insights
        .data
        .data
        .clone()
        .expect("insights loaded");
    assert_eq!(data.basis, "cost");
    let lines: Vec<String> = crate::settings::usage::lines(&app, 116)
        .iter()
        .map(ToString::to_string)
        .collect();
    let has = |text: &str| lines.iter().any(|l| l.contains(text));
    assert!(has("What's contributing to your usage?"));
    assert!(has("Approximate: Sessions recorded by this server only"));
    assert!(has("Last 24h · These are independent characteristics"));
    // Both children cost $2 of a tree that cost about $2.0013.
    assert!(
        has("100% of your usage came from subagent-heavy sessions"),
        "{lines:#?}"
    );
    assert!(
        has("50% of your usage came from requests above 150k context"),
        "{lines:#?}"
    );
    assert!(has("  Long contexts cost more even when cached."));
    assert!(!has("8+ hours"));
    let explore = lines.iter().position(|l| l.starts_with("explore")).unwrap();
    let review = lines.iter().position(|l| l.starts_with("review")).unwrap();
    assert_eq!(review, explore + 1);
    assert!(
        lines[explore - 1].starts_with("Subagents") && lines[explore - 1].ends_with("% of cost")
    );
    assert!(lines[explore].ends_with("50%"));

    // The section at the end of the tab, with and without color.
    app.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    buffer(&mut app, 120, 40, &Opts::default());
    app.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    let opts = Opts {
        legend: &["of your usage came from", "Subagents"],
        ..Opts::default()
    };
    let shown = render(&mut app, 120, 40, &opts);
    assert!(shown.contains("d day · w week"), "{shown}");
    insta::assert_snapshot!("usage_insights_120x40", shown);
    let plain = Opts {
        no_color: true,
        legend: &["of your usage came from"],
        ..Opts::default()
    };
    insta::assert_snapshot!(
        "usage_insights_no_color_120x40",
        render(&mut app, 120, 40, &plain)
    );

    // `w` reloads only the factors, for the last 7 days.
    let actions = app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
    assert!(
        matches!(&actions[..], [Action::LoadInsights { window: "7d", .. }]),
        "{actions:?}"
    );
    load(&server, &mut app, actions).await;
    let lines: Vec<String> = crate::settings::usage::lines(&app, 116)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(lines.iter().any(|l| l.starts_with("Last 7 days · ")));
    assert_eq!(
        app.settings.insights.data.data.as_ref().unwrap().window,
        "7d"
    );
}

/// 2026-10-16T04:00:00Z, the fixed wall clock of the limit snapshots.
const LIMITS_NOW: i64 = 1_792_123_200_000;

/// The provider-reported windows from the production `GET /usage/limits`, after the
/// scripted provider sent plan headers: the daily view, `w` for the weekly one, then the
/// reading age and `NO_COLOR`.
#[tokio::test]
async fn provider_reported_windows_with_reset_age_and_day_week_views() {
    let server = Server::start_models(
        json!({ "main": priced(10.0, 20.0), "small": priced(1.0, 2.0) }),
        json!({ "small_model": "test/small" }),
        false,
    )
    .await;
    let id = server.answered("PLAN LIMITS please").await;
    let mut app = server.app(&id).await;
    let actions = typed(&mut app, "/usage");
    load(&server, &mut app, actions).await;
    stabilize(&mut app);
    let state = &mut app.settings.limits;
    let readings = state.data.data.as_mut().expect("limits loaded");
    let windows: Vec<(&str, &str, &str)> = readings
        .iter()
        .map(|r| (r.provider.as_str(), r.window.as_str(), r.period.as_str()))
        .collect();
    assert_eq!(
        windows,
        [("test", "5h", "daily"), ("test", "7d", "weekly")],
        "{readings:?}"
    );
    // A reading 12 minutes old, and reset times in UTC.
    for r in readings.iter_mut() {
        r.observed_ms = LIMITS_NOW - 12 * 60_000;
    }
    state.clock_ms = Some(LIMITS_NOW);
    state.zone = Some("UTC".into());

    let text = |app: &App| -> Vec<String> {
        crate::settings::usage::lines(app, 116)
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    let rows = text(&app);
    let label = rows
        .iter()
        .position(|r| r == "test · 5-hour limit")
        .unwrap_or_else(|| panic!("{rows:#?}"));
    assert!(
        rows[label + 1].ends_with(" 85% used"),
        "{}",
        rows[label + 1]
    );
    assert_eq!(
        rows[label + 2],
        "Resets 6:00am (UTC) · Reading from 12m ago"
    );
    assert!(
        !rows.iter().any(|r| r.contains("Weekly limit")),
        "{rows:#?}"
    );

    // Scroll the meter to the top of the tab and draw it.
    app.settings.scroll = label as u16;
    let opts = Opts {
        legend: &["█"],
        ..Opts::default()
    };
    let buf = buffer(&mut app, 120, 40, &opts);
    let meter = row_of(&buf, "test · 5-hour limit") + 1;
    let x = row(&buf, meter).find('█').unwrap() as u16;
    let x = row(&buf, meter)[..x as usize].chars().count() as u16;
    assert_eq!(buf[(x, meter)].fg, app.theme.warning, "85% is a warning");
    insta::assert_snapshot!(
        "usage_limits_daily_120x40",
        render(&mut app, 120, 40, &opts)
    );
    let plain = Opts {
        no_color: true,
        legend: &["█"],
        ..Opts::default()
    };
    insta::assert_snapshot!(
        "usage_limits_no_color_80x24",
        render(&mut app, 80, 24, &plain)
    );

    // `w` shows the weekly window of a provider that reports both, with its date.
    let actions = app.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
    load(&server, &mut app, actions).await;
    let rows = text(&app);
    let label = rows
        .iter()
        .position(|r| r == "test · Weekly limit")
        .unwrap_or_else(|| panic!("{rows:#?}"));
    assert!(rows[label + 1].ends_with(" 30% used"));
    assert_eq!(
        rows[label + 2],
        "Resets Oct 23, 4:00am (UTC) · Reading from 12m ago"
    );
    assert!(!rows.iter().any(|r| r.contains("5-hour limit")));
    app.settings.scroll = label as u16;
    insta::assert_snapshot!(
        "usage_limits_weekly_120x40",
        render(&mut app, 120, 40, &opts)
    );

    // `d` goes back; a fresh reading shows no age.
    app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    for r in app.settings.limits.data.data.as_mut().unwrap() {
        r.observed_ms = LIMITS_NOW - 60_000;
    }
    let rows = text(&app);
    assert!(
        rows.contains(&"Resets 6:00am (UTC)".to_owned()),
        "{rows:#?}"
    );
}

/// A ChatGPT-plan usage limit, as the provider answers it: 429 `usage_limit_reached`
/// with the reset time 40 minutes ahead.
fn usage_limit(_: &Value) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    json!({ "fail": 429, "error": {
        "type": "usage_limit_reached", "message": "The usage limit has been reached",
        "plan_type": "plus", "resets_at": now + 40 * 60
    }})
}

/// `provider-catalog` → "Subscription limit reached": with no fallback model, the TUI
/// shows when the limit resets, in the failed answer and in the status toast.
#[tokio::test]
async fn a_usage_limit_shows_when_it_resets() {
    use cyber_server::runtime::LiveEvent;
    let server = Server::start_reply(usage_limit, json!({ "main": {} }), json!({}), false).await;
    let mut live = server.host.runtime.subscribe();
    let id = server.session().await;
    server.prompt(&id, "hello").await;
    let event = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            match live.recv().await.unwrap() {
                LiveEvent::Error {
                    session_id,
                    kind,
                    message,
                } if session_id == id => break json!({ "kind": kind, "message": message }),
                _ => {}
            }
        }
    })
    .await
    .expect("the Turn fails");
    assert_eq!(event["kind"], "quota_exceeded", "{event}");
    server.host.runtime.wait_idle(&id).await;

    let mut app = server.load(&id).await;
    // `HH:MM`, or `YYYY-MM-DD HH:MM` when the reset falls after midnight.
    let reset = |shown: &str| {
        let at = shown.find("usage limit reached; resets at ")? + 31;
        let after: Vec<u8> = shown[at..].bytes().take(16).collect();
        let clock = |w: &[u8]| w[2] == b':' && [0, 1, 3, 4].iter().all(|&i| w[i].is_ascii_digit());
        after.windows(5).any(clock).then_some(())
    };
    let shown = render(&mut app, 100, 30, &Opts::default());
    assert!(reset(&shown).is_some(), "the failed answer: {shown}");

    app.on_event("session.error", &event);
    let shown = render(&mut app, 100, 30, &Opts::default());
    let toast = shown
        .lines()
        .find(|l| l.contains("error: QuotaExceeded: usage limit reached"))
        .unwrap_or_else(|| panic!("the status toast: {shown}"));
    assert!(reset(toast).is_some(), "{toast}");
    // `tui` → Resume at usage limit reset: the notice offers the command.
    let hint = app
        .toast
        .as_ref()
        .map(|(text, _)| text.clone())
        .unwrap_or_default();
    assert!(
        hint.ends_with(" · /resume-at-reset to continue then"),
        "{hint}"
    );

    for c in "/resume-at-reset".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let actions = app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(actions, vec![Action::ResumeAtReset]);
    let msg = crate::perform::perform(&server.client, &app.session, Action::ResumeAtReset).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    let toast = app
        .toast
        .as_ref()
        .map(|(text, _)| text.clone())
        .unwrap_or_default();
    assert!(toast.starts_with("resuming at "), "{toast}");
    // The scheduled wait holds the Session; the footer shows when it resumes.
    let mut app = server.load(&id).await;
    let shown = render(&mut app, 120, 30, &Opts::default());
    assert!(shown.contains("⏸ usage limit · resumes "), "{shown}");
    // A second request finds the Session running.
    let msg = crate::perform::perform(&server.client, &app.session, Action::ResumeAtReset).await;
    crate::runner::apply(&mut app, msg, &mut Default::default());
    let refused = app
        .toast
        .as_ref()
        .map(|(text, _)| text.clone())
        .unwrap_or_default();
    assert!(refused.contains("SessionBusyError"), "{refused}");
    server.host.runtime.interrupt(&id).await.unwrap();
}
