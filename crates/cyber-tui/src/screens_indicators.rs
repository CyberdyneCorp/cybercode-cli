//! Workflow run indicators and notifications (`tui` → Workflow run indicators, Workflow
//! run notifications), from a production App attached to a real server: runs started
//! over HTTP or by the owner's `workflow` tool, agents held per request by the scripted
//! model's gates, the App's reads performed through `perform` and the runner's `apply`,
//! and the instance event stream fed through the runner's own `server_event`.

use std::time::Duration;

use crossterm::event::KeyCode;
use futures::StreamExt;
use serde_json::{Value, json};

use crate::app::{App, Overlay};
use crate::screens::Opts;
use crate::screens_workflows::{Fixture, fan, gates, plain, script, text};

impl Fixture {
    /// The owner's TUI once the stream's `server.connected` read the runs.
    async fn attach(&mut self) -> App {
        let mut app = self.server.load(&self.owner).await;
        app.home = Some("/Users/leo".into());
        self.pump(&mut app, "the runs are read", |app| {
            app.watch.runs.data.is_some() && !app.watch.runs.loading
        })
        .await;
        app
    }

    /// Feed the events that arrive within `quiet` of each other.
    async fn settle(&mut self, app: &mut App, quiet: Duration) {
        while let Ok(Some(event)) = tokio::time::timeout(quiet, self.events.next()).await {
            let actions = crate::runner::server_event(app, &event);
            self.drive(app, actions).await;
        }
    }

    /// Feed instance events until `run_id`'s `kind` event was fed.
    async fn fed(&mut self, app: &mut App, kind: &str, run_id: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let Ok(Some(event)) = tokio::time::timeout(left, self.events.next()).await else {
                panic!("no {kind} of {run_id}");
            };
            let actions = crate::runner::server_event(app, &event);
            self.drive(app, actions).await;
            if event.kind == kind && event.session_id.as_deref() == Some(run_id) {
                return;
            }
        }
    }

    /// The stream dropped and reopened: its `server.connected` resyncs.
    async fn reconnect(&mut self, app: &mut App) {
        self.events = self.server.client.events(true).await.unwrap().boxed();
        let first = self.events.next().await.unwrap();
        assert_eq!(first.kind, "server.connected");
        let actions = crate::runner::server_event(app, &first);
        self.drive(app, actions).await;
    }

    /// Wait until the runs' agents counts read `done/total`.
    async fn counted(&mut self, app: &mut App, name: &str, done: u64, total: u64) {
        self.pump(app, &format!("{name} counts {done}/{total}"), |app| {
            app.watch.runs().iter().any(|run| {
                run.name == name && run.agents.completed == done && run.agents.total == total
            })
        })
        .await;
    }
}

/// A run of `done` agents that answer, then `held` that wait at their gates.
fn held_after(name: &str, done: usize, held: usize) -> String {
    let body = format!(
        "{}  {}",
        fan("Scan", "scan", &plain(&format!("{name} scan"), done)),
        fan("Verify", "verify", &gates(name, held))
    );
    script(name, &["Scan", "Verify"], &body)
}

fn footer(buf: &ratatui::buffer::Buffer) -> String {
    text(buf).lines().last().unwrap_or_default().to_owned()
}

/// "Two runs in the sidebar", then the section, field and progress feed go when the
/// last run ends.
#[tokio::test]
async fn two_runs_in_the_sidebar_and_the_section_goes_with_the_last_run() {
    let mut f = Fixture::new(json!({})).await;
    let mut app = f.attach().await;
    let review = f
        .start(&held_after("review", 0, 2), json!({ "max_concurrent": 1 }))
        .await;
    let audit = f
        .start(&held_after("audit", 31, 69), json!({ "max_concurrent": 1 }))
        .await;
    f.counted(&mut app, "review", 0, 2).await;
    f.counted(&mut app, "audit", 31, 100).await;
    let buf = f.show(&mut app, 160, 48);
    let shown = text(&buf);
    let lines: Vec<&str> = shown.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.contains("Workflows"))
        .unwrap_or_else(|| panic!("no Workflows section:\n{shown}"));
    assert!(
        lines[at + 1].contains("audit") && lines[at + 1].contains(" 31/100"),
        "{shown}"
    );
    let bar: String = lines[at + 1]
        .chars()
        .map(|c| if c == '▄' { '▀' } else { c })
        .collect();
    assert!(bar.contains("▀▀····"), "a 6-cell bar at 31%: {shown}");
    assert!(
        lines[at + 2].contains("review") && lines[at + 2].contains(" 0/2"),
        "{shown}"
    );
    assert!(footer(&buf).contains("⚙ 2 workflows ·"), "{}", footer(&buf));
    assert!(
        shown.contains("2 workflows 31/102 agents · 30%"),
        "the progress line sums both runs:\n{shown}"
    );
    // Enter on a workflow row opens that run in the monitor.
    app.focus_sidebar();
    let index = app
        .sidebar
        .rows()
        .iter()
        .position(|row| matches!(row, crate::sidebar::Row::Workflow(0)))
        .unwrap();
    app.sidebar.select_focus(index);
    let actions = app.on_key(crossterm::event::KeyEvent::new(
        KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ));
    f.drive(&mut app, actions).await;
    assert!(matches!(app.overlay, Overlay::Workflows));
    assert_eq!(
        app.workflows.run.as_ref().map(|v| v.run_id.as_str()),
        Some(audit.as_str())
    );
    app.on_key(crossterm::event::KeyEvent::new(
        KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    ));
    app.on_key(crossterm::event::KeyEvent::new(
        KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    ));
    assert!(matches!(app.overlay, Overlay::None));

    for id in [&audit, &review] {
        f.server
            .client
            .post(&format!("/workflows/runs/{id}/stop"), json!({}))
            .await
            .unwrap();
    }
    f.pump(&mut app, "no run is active", |app| {
        app.watch.indicated().next().is_none()
    })
    .await;
    let buf = f.show(&mut app, 160, 48);
    let shown = text(&buf);
    assert!(!shown.contains("Workflows"), "{shown}");
    assert!(!footer(&buf).contains('⚙'), "{}", footer(&buf));
    assert!(app.activity().is_none(), "{:?}", app.activity());
}

/// "Aggregate progress": 30 of 100 and 12 of 30 agents done.
#[tokio::test]
async fn the_progress_line_shows_the_sessions_runs_together() {
    let mut f = Fixture::new(json!({})).await;
    let mut app = f.attach().await;
    f.start(&held_after("audit", 30, 70), json!({ "max_concurrent": 1 }))
        .await;
    f.start(
        &held_after("review", 12, 18),
        json!({ "max_concurrent": 1 }),
    )
    .await;
    f.counted(&mut app, "audit", 30, 100).await;
    f.counted(&mut app, "review", 12, 30).await;
    let width = 110;
    let buf = crate::screens::buffer(
        &mut app,
        width,
        24,
        &Opts {
            reduced_motion: false,
            ..Opts::default()
        },
    );
    let shown = text(&buf);
    let line = shown
        .lines()
        .find(|l| l.contains("agents ·"))
        .unwrap_or_else(|| panic!("no progress line:\n{shown}"));
    assert!(line.ends_with("2 workflows 42/130 agents · 32%"), "{line}");
    // The trail covers 32% of the bar: the half blocks between the leading space and the
    // cat, which is followed by its sparkle cell.
    let label = "  2 workflows 42/130 agents · 32%".chars().count();
    let room = width as usize - 1 - crate::view::progress::CAT_WIDTH - 1 - label - 1;
    let trail = line
        .chars()
        .skip(1)
        .take_while(|&c| matches!(c, '▀' | '▄'))
        .count();
    assert_eq!(
        trail,
        (42.0 / 130.0 * room as f64).round() as usize,
        "{line}"
    );
}

/// "Queued count in the footer": 8 runs running and 2 queued.
#[tokio::test]
async fn the_footer_counts_the_queued_runs() {
    let mut f = Fixture::new(json!({ "max_running_runs": 8 })).await;
    let mut app = f.attach().await;
    for i in 0..10 {
        let body = fan("Work", "w", &gates(&format!("q{i}x"), 1));
        f.start(&script(&format!("run{i}"), &["Work"], &body), json!({}))
            .await;
    }
    f.pump(&mut app, "8 running and 2 queued", |app| {
        let queued = app
            .watch
            .indicated()
            .filter(|r| r.status == "queued")
            .count();
        queued == 2 && app.watch.indicated().count() == 10
    })
    .await;
    let buf = f.show(&mut app, 120, 40);
    assert!(
        footer(&buf).contains("⚙ 10 workflows · 2 queued"),
        "{}",
        footer(&buf)
    );
}

/// The sidebar's Workflows rows, from the section header to the blank row after them.
fn sidebar_runs(buf: &ratatui::buffer::Buffer) -> String {
    let shown = text(buf);
    let lines: Vec<&str> = shown.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.contains("Workflows"))
        .unwrap_or_else(|| panic!("no Workflows section:\n{shown}"));
    let byte = lines[at].find("Workflows").unwrap();
    let col = lines[at][..byte].chars().count();
    lines[at..]
        .iter()
        .map(|l| {
            l.chars()
                .skip(col)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .take_while(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A queued run's sidebar row shows its place in the queue, as the monitor does, not
/// an empty bar and `0/0`.
#[tokio::test]
async fn a_queued_run_shows_its_queue_position_in_the_sidebar() {
    let mut f = Fixture::new(json!({ "max_running_runs": 1 })).await;
    let mut app = f.attach().await;
    let body = fan("Work", "w", &gates("lead", 1));
    f.start(&script("review", &["Work"], &body), json!({}))
        .await;
    f.held("review's agent at its gate", 1).await;
    for name in ["audit", "deploy"] {
        let body = fan("Work", "w", &gates(name, 1));
        f.start(&script(name, &["Work"], &body), json!({})).await;
    }
    f.pump(&mut app, "two queued runs", |app| {
        let queued: Vec<Option<u64>> = app
            .watch
            .indicated()
            .filter(|r| r.status == "queued")
            .map(|r| r.queue_position)
            .collect();
        queued.len() == 2 && queued.iter().all(Option::is_some)
    })
    .await;
    f.counted(&mut app, "review", 0, 1).await;
    let rows = sidebar_runs(&f.show(&mut app, 160, 48));
    assert!(!rows.contains("0/0"), "{rows}");
    insta::assert_snapshot!("sidebar_queued_runs_160x48", rows);
}

/// Children answer at once; the owner starts `audit` with its `workflow` tool.
fn tool_reply(body: &Value) -> Value {
    let last = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .cloned()
        .unwrap_or_default();
    let text = last.to_string();
    if body.to_string().contains("Write a short title") {
        return json!({ "content": "Owner" });
    }
    if last["role"] == "tool" || text.contains("Workflow run handback") {
        return json!({ "content": "noted" });
    }
    if text.contains("start the audit workflow") {
        let audit = script("audit", &["Scan"], &fan("Scan", "scan", &plain("scan", 2)));
        let arguments = json!({ "script": audit, "args": {} }).to_string();
        return json!({"tool_calls": [{"index": 0, "id": "call_audit", "type": "function",
            "function": {"name": "workflow", "arguments": arguments}}]});
    }
    crate::screens_workflows::answer(body)
}

/// A bypass Session, so its `workflow` tool runs without asking.
async fn bypass_owner(f: &mut Fixture) {
    let created = f
        .server
        .client
        .post(
            "/sessions",
            json!({ "model": "test/main", "mode": "bypass" }),
        )
        .await
        .unwrap();
    f.owner = created["data"]["id"].as_str().unwrap().to_owned();
}

/// "Background run finishes": the transcript line and the same text as an OSC 9
/// desktop notification while the terminal is unfocused; a reconnect's resync does not
/// show it again.
#[tokio::test]
async fn a_background_run_ends_in_the_transcript_and_a_desktop_notification_once() {
    let mut f = Fixture::with_reply(tool_reply, json!({})).await;
    bypass_owner(&mut f).await;
    let mut app = f.attach().await;
    app.focused = false;
    f.server
        .client
        .post(
            &format!("/sessions/{}/prompt", f.owner),
            json!({ "parts": [{ "type": "text", "text": "please start the audit workflow" }] }),
        )
        .await
        .unwrap();
    f.pump(&mut app, "the run's notice", |app| {
        !app.watch.notices.is_empty()
    })
    .await;
    let run = app.watch.notices[0].clone();
    let id = f
        .server
        .client
        .get("/workflows/runs?directory=all")
        .await
        .unwrap()["data"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let summary = f.run(&id).await;
    let expected = format!(
        "✓ audit completed · 2 agents · ${:.2} · {}s",
        summary["cost_usd"].as_f64().unwrap(),
        summary["elapsed_ms"].as_u64().unwrap() / 1000
    );
    assert_eq!(run.text, expected);
    assert_eq!(run.session_id, f.owner);
    let workflow_notices = |app: &App| -> Vec<String> {
        let idle = "\x07\x1b]9;cyber: the session is idle\x07";
        app.attention_log
            .iter()
            .filter(|s| *s != idle)
            .cloned()
            .collect()
    };
    assert_eq!(
        workflow_notices(&app),
        [format!("\x07\x1b]9;{expected}\x07")]
    );
    assert!(app.toast.is_none(), "{:?}", app.toast);
    f.settle(&mut app, Duration::from_millis(300)).await;
    let shown = text(&f.show(&mut app, 120, 40));
    assert!(shown.contains(&format!("  {expected}")), "{shown}");

    f.reconnect(&mut app).await;
    f.settle(&mut app, Duration::from_millis(300)).await;
    assert_eq!(app.watch.notices.len(), 1, "{:?}", app.watch.notices);
    assert_eq!(workflow_notices(&app).len(), 1, "{:?}", app.attention_log);
}

/// A run that ends while the stream is down is noticed by the reconnect's resync, once;
/// a second reconnect shows nothing new.
#[tokio::test]
async fn an_end_missed_while_disconnected_is_noticed_once_after_reconnecting() {
    let mut f = Fixture::new(json!({})).await;
    let mut app = f.attach().await;
    let body = fan("Work", "w", &gates("m", 1));
    let run = f
        .start(&script("migrate", &["Work"], &body), json!({}))
        .await;
    f.held("the agent at its gate", 1).await;
    f.pump(&mut app, "the run is shown", |app| {
        app.watch.indicated().count() == 1
    })
    .await;
    // The stream drops: nothing more is fed until it reopens.
    f.server
        .client
        .post(&format!("/workflows/runs/{run}/stop"), json!({}))
        .await
        .unwrap();
    f.wait_run("migrate stops", &run, |r| r["status"] == "stopped")
        .await;
    f.reconnect(&mut app).await;
    f.settle(&mut app, Duration::from_millis(300)).await;
    let texts: Vec<&str> = app.watch.notices.iter().map(|n| n.text.as_str()).collect();
    assert_eq!(texts, ["■ migrate stopped"]);
    f.reconnect(&mut app).await;
    f.settle(&mut app, Duration::from_millis(300)).await;
    assert_eq!(app.watch.notices.len(), 1, "{:?}", app.watch.notices);
    assert!(
        app.attention_log.is_empty(),
        "focused: no desktop notification"
    );
}

fn failing(name: &str) -> String {
    format!(
        "export const meta = {{ name: '{name}', description: 'Fails' }};\nexport default async () => {{ throw new Error('boom'); }};\n"
    )
}

/// "Run of another Session": with `own`, a failure of a run another Session started
/// shows nothing here, and `/workflows` lists it; `location` shows it as a toast, and
/// `off` shows nothing at all.
#[tokio::test]
async fn another_sessions_run_notifies_only_when_the_location_is_asked_for() {
    let mut f = Fixture::new(json!({})).await;
    let mut app = f.attach().await;
    app.focused = false;
    let other = f.server.session().await;
    let start = |name: &str| json!({ "session_id": other, "script": failing(name) });
    let started = f
        .server
        .client
        .post("/workflows/runs", start("review"))
        .await
        .unwrap();
    let review = started["data"]["id"].as_str().unwrap().to_owned();
    f.fed(&mut app, "workflow.run.failed.1", &review).await;
    assert!(app.watch.notices.is_empty() && app.toast.is_none() && app.attention_log.is_empty());
    let actions = app.open_workflows();
    f.drive(&mut app, actions).await;
    let listed = text(&f.show(&mut app, 120, 40));
    assert!(
        listed.contains("review") && listed.contains("failed"),
        "{listed}"
    );
    app.overlay = Overlay::None;

    app.prefs.set(
        "workflow_notifications",
        "location",
        crate::prefs::Origin::Saved,
    );
    let started = f
        .server
        .client
        .post("/workflows/runs", start("scan"))
        .await
        .unwrap();
    let scan = started["data"]["id"].as_str().unwrap().to_owned();
    f.pump(&mut app, "the toast", |app| app.toast.is_some())
        .await;
    let toast = app.toast.as_ref().unwrap().0.clone();
    assert!(toast.starts_with("✗ scan failed: "), "{toast}");
    assert!(toast.contains("boom"), "{toast}");
    assert!(
        app.watch.notices.is_empty(),
        "not this Session's transcript"
    );
    assert_eq!(app.attention_log, [format!("\x07\x1b]9;{toast}\x07")]);
    assert_eq!(f.run(&scan).await["status"], "failed");

    app.prefs
        .set("workflow_notifications", "off", crate::prefs::Origin::Saved);
    app.toast = None;
    let started = f
        .server
        .client
        .post("/workflows/runs", start("plan"))
        .await
        .unwrap();
    let plan = started["data"]["id"].as_str().unwrap().to_owned();
    f.fed(&mut app, "workflow.run.failed.1", &plan).await;
    assert!(app.toast.is_none(), "{:?}", app.toast);
    assert_eq!(app.attention_log.len(), 1);
}

/// A run of another Session that ends before the TUI's first read of it still belongs to
/// the Location: the read lists only active runs, so the run's own directory decides.
#[tokio::test]
async fn another_sessions_run_that_ends_before_it_is_read_notifies_the_location() {
    let mut f = Fixture::new(json!({})).await;
    let mut app = f.attach().await;
    app.prefs.set(
        "workflow_notifications",
        "location",
        crate::prefs::Origin::Saved,
    );
    let other = f.server.session().await;
    let started = f
        .server
        .client
        .post(
            "/workflows/runs",
            json!({ "session_id": other, "script": failing("lint") }),
        )
        .await
        .unwrap();
    let lint = started["data"]["id"].as_str().unwrap().to_owned();
    // Nothing is fed before the run has failed: the started event's read misses it.
    f.wait_run("lint fails", &lint, |r| r["status"] == "failed")
        .await;
    f.pump(&mut app, "the toast", |app| app.toast.is_some())
        .await;
    let toast = &app.toast.as_ref().unwrap().0;
    assert!(toast.starts_with("✗ lint failed: "), "{toast}");
    assert!(app.watch.notices.is_empty(), "{:?}", app.watch.notices);
}

/// The server pauses its running runs as it shuts down: the pause's event is the
/// restart notice, in the owner's transcript.
#[tokio::test]
async fn a_shutdown_pause_is_noticed_as_paused_by_restart() {
    let mut f = Fixture::new(json!({})).await;
    let mut app = f.attach().await;
    let body = fan("Work", "w", &gates("d", 1));
    f.start(&script("docs", &["Work"], &body), json!({})).await;
    f.held("the agent at its gate", 1).await;
    f.pump(&mut app, "the run is shown", |app| {
        app.watch.indicated().count() == 1
    })
    .await;
    let runtime = f.server.host.runtime.clone();
    tokio::spawn(async move { runtime.shutdown().await });
    f.pump(&mut app, "the restart notice", |app| {
        !app.watch.notices.is_empty()
    })
    .await;
    assert_eq!(app.watch.notices[0].text, "⏸ docs paused by restart");
    f.server.model.open_gate();
}

/// A restart's pause recorded while the stream was down is noticed by the resync, from
/// the run's `interrupted` flag.
#[tokio::test]
async fn a_restart_pause_missed_while_disconnected_is_noticed_after_reconnecting() {
    let mut f = Fixture::new(json!({})).await;
    let mut app = f.attach().await;
    let body = fan("Work", "w", &gates("e", 1));
    let run = f.start(&script("edit", &["Work"], &body), json!({})).await;
    f.held("the agent at its gate", 1).await;
    f.pump(&mut app, "the run is shown", |app| {
        app.watch.indicated().count() == 1
    })
    .await;
    let runtime = f.server.host.runtime.clone();
    tokio::spawn(async move { runtime.shutdown().await });
    let paused = f
        .wait_run("edit pauses", &run, |r| r["status"] == "paused")
        .await;
    assert_eq!(paused["interrupted"], true, "{paused}");
    // A stopped runtime serves reads but no new event stream, so the reconnect's
    // `server.connected` is fed as the restarted server would send it.
    let connected = cyber_client::Event {
        kind: "server.connected".into(),
        data: json!({}),
        seq: None,
        session_id: None,
    };
    let actions = crate::runner::server_event(&mut app, &connected);
    f.drive(&mut app, actions).await;
    f.settle(&mut app, Duration::from_millis(300)).await;
    let texts: Vec<&str> = app.watch.notices.iter().map(|n| n.text.as_str()).collect();
    assert_eq!(texts, ["⏸ edit paused by restart"]);
    f.server.model.open_gate();
}
