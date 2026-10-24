//! The Workflow monitor (`tui` → Workflow monitor), drawn by the real `view::draw` from
//! a production App: runs started over HTTP, agents held per request by the scripted
//! model's gates, the monitor's reads and actions performed through `perform` and the
//! runner's `apply`, and its live updates fed from the real instance event stream
//! through the runner's own `server_event`.

use std::collections::HashSet;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay};
use crate::screens::{Opts, Server, buffer, cells, render};

/// Children answer at once, unless their prompt holds `GATE-<label>`: then they wait for
/// one opening of the gate, oldest first.
pub(crate) fn answer(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({ "content": "Agent thread" });
    }
    let last = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .map(Value::to_string)
        .unwrap_or_default();
    match last.find("GATE-") {
        Some(at) => {
            let label: String = last[at + 5..]
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            json!({ "gate": label })
        }
        None => json!({ "content": "agent answer" }),
    }
}

/// `count` agents of `phase` whose prompts are `prompt(i)`, labeled `label i`.
pub(crate) fn fan(phase: &str, label: &str, prompts: &[String]) -> String {
    let list: Vec<String> = prompts.iter().map(|p| format!("'{p}'")).collect();
    format!(
        "phase('{phase}');\n  await parallel([{}].map((p, i) => () => agent(p, {{ label: '{label} ' + (i + 1) }})));\n",
        list.join(", ")
    )
}

pub(crate) fn gates(prefix: &str, n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("GATE-{prefix}{i}")).collect()
}

pub(crate) fn plain(prefix: &str, n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("{prefix} {i}")).collect()
}

/// A script named `name` with declared `phases` whose body is `body`.
pub(crate) fn script(name: &str, phases: &[&str], body: &str) -> String {
    let phases: Vec<String> = phases
        .iter()
        .map(|p| format!("{{ title: '{p}' }}"))
        .collect();
    format!(
        "export const meta = {{ name: '{name}', description: 'Monitor', phases: [{}] }};\nexport default async () => {{\n  {body}  return 'ok';\n}};\n",
        phases.join(", ")
    )
}

pub(crate) struct Fixture {
    pub(crate) server: Server,
    pub(crate) owner: String,
    pub(crate) events: futures::stream::BoxStream<'static, cyber_client::Event>,
}

impl Fixture {
    pub(crate) async fn new(workflows: Value) -> Self {
        Self::with_reply(answer, workflows).await
    }

    /// A server whose scripted model answers with `reply`.
    pub(crate) async fn with_reply(
        reply: crate::screens::scripted_model::Reply,
        workflows: Value,
    ) -> Self {
        let server = Server::start_reply(
            reply,
            json!({ "main": {} }),
            // The scripted model holds whole responses at its gates, so warm-up would hold
            // every fan-out behind its first agent.
            json!({ "workflows": with_warmup_off(workflows) }),
            false,
        )
        .await;
        let events = server.client.events(true).await.unwrap().boxed();
        let owner = server.session().await;
        Self {
            server,
            owner,
            events,
        }
    }

    pub(crate) async fn start(&self, script: &str, extra: Value) -> String {
        let mut body = json!({ "session_id": self.owner, "script": script });
        if let (Some(body), Value::Object(extra)) = (body.as_object_mut(), extra) {
            body.extend(extra);
        }
        let started = self
            .server
            .client
            .post("/workflows/runs", body)
            .await
            .unwrap();
        // Distinct start times keep the list's order fixed.
        tokio::time::sleep(Duration::from_millis(5)).await;
        started["data"]["id"].as_str().unwrap().to_owned()
    }

    pub(crate) async fn run(&self, id: &str) -> Value {
        self.server
            .client
            .get(&format!("/workflows/runs/{id}"))
            .await
            .unwrap()["data"]
            .take()
    }

    pub(crate) async fn wait_run(
        &self,
        what: &str,
        id: &str,
        done: impl Fn(&Value) -> bool,
    ) -> Value {
        for _ in 0..6000 {
            let run = self.run(id).await;
            if done(&run) {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{what}: {}", self.run(id).await);
    }

    pub(crate) async fn held(&self, what: &str, n: usize) {
        for _ in 0..6000 {
            if self.server.model.held().len() == n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{what}: held {:?}", self.server.model.held());
    }

    /// The owner's TUI with `/workflows` open and its first reads applied.
    pub(crate) async fn monitor(&self) -> App {
        let mut app = self.server.load(&self.owner).await;
        app.utc_offset_minutes = Some(0);
        app.home = Some("/Users/leo".into());
        type_line(&mut app, "/workflows");
        let actions = press(&mut app, KeyCode::Enter);
        assert!(matches!(app.overlay, Overlay::Workflows));
        self.drive(&mut app, actions).await;
        app
    }

    /// Perform `actions` as the runner does, until nothing follows.
    pub(crate) async fn drive(&self, app: &mut App, mut actions: Vec<Action>) {
        while !actions.is_empty() {
            let action = actions.remove(0);
            if matches!(action, Action::EditFile(_) | Action::Copy(_)) {
                continue;
            }
            let msg =
                crate::perform::perform_owned(&self.server.client, &app.session, action, None)
                    .await;
            actions.extend(crate::runner::apply(app, msg, &mut Default::default()));
        }
    }

    /// Feed instance events to the App through the runner until `done` holds.
    /// The deadline only bounds a failure: some tests settle 40 sequential agents, each a
    /// full provider round trip, which takes well over 10 s on Windows runners.
    pub(crate) async fn pump(&mut self, app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        while !done(app) {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !left.is_zero(),
                "{what}: {}",
                text(&self.show(app, 120, 40))
            );
            if let Ok(Some(event)) = tokio::time::timeout(left, self.events.next()).await {
                let actions = crate::runner::server_event(app, &event);
                self.drive(app, actions).await;
            }
        }
    }

    /// Draw as the runner does, without the stabilization snapshots need.
    pub(crate) fn show(&self, app: &mut App, width: u16, height: u16) -> Buffer {
        buffer(app, width, height, &Opts::default())
    }
}

fn type_line(app: &mut App, line: &str) {
    for c in line.chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.completion = None;
}

pub(crate) fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
}

pub(crate) fn key(app: &mut App, c: char) -> Vec<Action> {
    press(app, KeyCode::Char(c))
}

pub(crate) fn text(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
                + "\n"
        })
        .collect()
}

/// The screen row naming `name` in the run list.
pub(crate) fn row(buf: &Buffer, name: &str) -> String {
    text(buf)
        .lines()
        .find(|line| line.contains(&format!(" {name} ")))
        .unwrap_or_else(|| panic!("{name} is not listed:\n{}", text(buf)))
        .to_owned()
}

pub(crate) fn select(app: &mut App, name: &str) {
    let index = app
        .workflows
        .summaries()
        .iter()
        .position(|run| run.name == name)
        .unwrap_or_else(|| panic!("{name} is not listed"));
    app.workflows.selected = index;
}

/// What differs between runs: the temporary directory, times, spend and IDs.
pub(crate) fn stabilized(app: &mut App, render_it: impl FnOnce(&mut App) -> String) -> String {
    let saved = (
        app.session.directory.clone(),
        app.workflows.directory.clone(),
        app.workflows.runs.data.clone(),
        app.workflows.run.clone(),
    );
    if let Some(detail) = app
        .workflows
        .run
        .as_mut()
        .and_then(|v| v.detail.data.as_mut())
    {
        detail.summary.id = "run_01JA2BCDEFGHK3".into();
        detail.summary.elapsed_ms = 724_000;
        detail.summary.cost_usd = 0.82;
        // Agents of one `parallel()` start in either order; within a phase the view lists
        // them by start, so order them by label before their IDs are numbered.
        detail.agents.sort_by(|a, b| a.label.cmp(&b.label));
        for (i, agent) in detail.agents.iter_mut().enumerate() {
            agent.id = format!("agt_01JA2B{:02}", i + 1);
            agent.duration_ms = Some(38_000 + 1_000 * i as u64);
            agent.tokens = 9_800 + 1_100 * i as u64;
        }
    }
    app.session.directory = "/Users/leo/work/app".into();
    app.workflows.directory = "/Users/leo/work/app".into();
    app.workflows.read_at = None;
    for (i, run) in app.workflows.runs.data.iter_mut().flatten().enumerate() {
        run.elapsed_ms = 61_000 * (i as u64 + 1) + 4_000;
        run.tokens = 1_200 * run.agents.completed;
        run.cost_usd = 0.41 * run.agents.completed as f64;
    }
    let shown = render_it(app);
    app.session.directory = saved.0;
    app.workflows.directory = saved.1;
    app.workflows.runs.data = saved.2;
    app.workflows.run = saved.3;
    shown
}

/// Four runs of one Location, as the monitor lists them: `migrate` queued behind the
/// two run slots, `audit` running with 2 of its 6 verify agents in flight and 4 waiting
/// for an agent slot, `docs` paused with its 2 agents still settling, and `plan`
/// completed.
async fn four_runs() -> Fixture {
    let f = Fixture::new(json!({ "max_running_runs": 2, "max_concurrent": 4 })).await;
    let plan = f
        .start(
            &script(
                "plan",
                &["Draft"],
                &fan("Draft", "draft", &plain("draft", 2)),
            ),
            json!({}),
        )
        .await;
    f.wait_run("plan completes", &plan, |r| r["status"] == "completed")
        .await;
    let docs = f
        .start(
            &script("docs", &["Draft"], &fan("Draft", "draft", &gates("d", 2))),
            json!({}),
        )
        .await;
    f.held("docs agents in flight", 2).await;
    f.server
        .client
        .post(&format!("/workflows/runs/{docs}/pause"), json!({}))
        .await
        .unwrap();
    let body = format!(
        "{}  {}",
        fan("Scan", "scan", &plain("scan", 2)),
        fan("Verify", "verify finding", &gates("v", 6))
    );
    let audit = f
        .start(
            &script("audit", &["Scan", "Verify", "Report"], &body),
            json!({ "budget": { "max_cost_usd": 10 } }),
        )
        .await;
    f.held("audit agents in flight", 4).await;
    f.wait_run("audit queues 4 agents", &audit, |r| {
        r["agents"]["queued"] == 4
    })
    .await;
    let migrate = f
        .start(
            &script("migrate", &[], &fan("Move", "move", &gates("m", 1))),
            json!({}),
        )
        .await;
    assert_eq!(f.run(&migrate).await["status"], "queued");
    f
}

#[tokio::test]
async fn the_monitor_lists_four_runs_from_the_production_server() {
    let f = four_runs().await;
    let mut app = f.monitor().await;
    let names: Vec<&str> = app
        .workflows
        .summaries()
        .iter()
        .map(|run| run.name.as_str())
        .collect();
    assert_eq!(names, ["migrate", "audit", "docs", "plan"]);
    select(&mut app, "audit");
    let buf = f.show(&mut app, 120, 40);
    assert!(
        row(&buf, "docs").contains("paused (2 settling)"),
        "{}",
        text(&buf)
    );
    assert!(row(&buf, "migrate").contains("#1 in queue"));
    assert!(row(&buf, "audit").contains("2/2/4/0"));
    assert!(row(&buf, "audit").contains(" / $10.00"));
    assert!(row(&buf, "plan").contains("completed"));
    for (width, height) in [(160, 48), (120, 40), (80, 24), (50, 20)] {
        let shown = stabilized(&mut app, |app| render(app, width, height, &Opts::default()));
        insta::assert_snapshot!(format!("workflows_{width}x{height}"), shown);
    }
    let no_color = Opts {
        no_color: true,
        ..Opts::default()
    };
    let buf = buffer(&mut app, 120, 40, &no_color);
    for cell in cells(&buf) {
        assert_eq!((cell.fg, cell.bg), (Color::Reset, Color::Reset), "{cell:?}");
    }
    let shown = stabilized(&mut app, |app| render(app, 120, 40, &no_color));
    insta::assert_snapshot!("workflows_no_color_120x40", shown);
    // The run view: agents by phase, the queued calls in the current phase.
    let actions = press(&mut app, KeyCode::Enter);
    f.drive(&mut app, actions).await;
    let shown = stabilized(&mut app, |app| render(app, 120, 40, &Opts::default()));
    assert!(
        shown.contains("▼ Verify       0/6  · 2 running · 4 queued"),
        "{shown}"
    );
    insta::assert_snapshot!("workflow_run_120x40", shown);
    press(&mut app, KeyCode::Esc);
    // Screen readers hear each run as one sentence.
    app.screen_reader = true;
    let spoken = text(&f.show(&mut app, 120, 40));
    assert!(
        spoken.contains("migrate, queued, number 1 in queue"),
        "{spoken}"
    );
    assert!(
        spoken.contains(
            "docs, paused, 2 agents settling, phase Draft, 0 of 2 agents done, 2 running, 0 queued, 0 tokens"
        ),
        "{spoken}"
    );
}

#[tokio::test]
async fn pausing_a_run_shows_its_settling_agents_until_they_finish() {
    let mut f = Fixture::new(json!({ "max_concurrent": 8 })).await;
    let body = format!(
        "{}  await agent('after the fan-out');\n",
        fan("Verify", "verify", &gates("v", 6))
    );
    let run = f
        .start(&script("audit", &["Verify"], &body), json!({}))
        .await;
    f.held("6 agents in flight", 6).await;
    let mut app = f.monitor().await;
    select(&mut app, "audit");
    let actions = key(&mut app, 'p');
    f.drive(&mut app, actions).await;
    let buf = f.show(&mut app, 120, 40);
    assert!(
        row(&buf, "audit").contains("paused (6 settling)"),
        "{}",
        text(&buf)
    );
    for _ in 0..6 {
        f.server.model.open_gate();
    }
    f.pump(&mut app, "the agents settle", |app| {
        app.workflows
            .current()
            .is_some_and(|r| r.status == "paused" && r.settling() == 0)
    })
    .await;
    let buf = f.show(&mut app, 120, 40);
    let line = row(&buf, "audit");
    assert!(
        line.contains("paused ") && !line.contains("settling"),
        "{line}"
    );
    assert!(line.contains("6/0/0/0"), "{line}");
    // No new agent started after the pause.
    let run = f.run(&run).await;
    assert_eq!(run["agents"]["total"], 6, "{run}");
    assert_eq!(f.server.model.starts().len(), 6);
}

#[tokio::test]
async fn pausing_one_of_several_runs_leaves_the_other_rows_unchanged() {
    let mut f = Fixture::new(json!({ "max_running_runs": 3, "max_concurrent": 8 })).await;
    for name in ["a", "b", "c"] {
        let body = fan("Work", "work", &gates(name, 2));
        f.start(&script(&format!("run{name}"), &["Work"], &body), json!({}))
            .await;
        f.held(
            "agents in flight",
            2 * (name.as_bytes()[0] - b'a' + 1) as usize,
        )
        .await;
    }
    f.start(
        &script("rund", &["Work"], &fan("Work", "work", &gates("d", 1))),
        json!({}),
    )
    .await;
    let mut app = f.monitor().await;
    assert_eq!(app.workflows.summaries().len(), 4);
    let before = f.show(&mut app, 120, 40);
    assert!(row(&before, "rund").contains("#1 in queue"));
    // The first opening lets runa's first agent finish: its row follows live.
    f.server.model.open_gate();
    f.pump(&mut app, "runa counts one agent done", |app| {
        let runs = app.workflows.summaries();
        runs.iter()
            .any(|r| r.name == "runa" && r.agents.completed == 1)
    })
    .await;
    let live = f.show(&mut app, 120, 40);
    assert!(row(&live, "runa").contains("1/1/0/0"), "{}", text(&live));
    select(&mut app, "runb");
    let actions = key(&mut app, 'p');
    f.drive(&mut app, actions).await;
    let after = f.show(&mut app, 120, 40);
    assert!(
        row(&after, "runb").contains("paused (2 settling)"),
        "{}",
        text(&after)
    );
    // Rows compare without the selection marker and the elapsed time, which counts on.
    // The panel's right border goes first, so the last token is the elapsed time.
    let strip = |line: String| {
        let line = line.replace('›', " ");
        let line = line.trim_end().trim_end_matches('│').trim_end();
        line.rsplit_once(' ').unwrap().0.to_owned()
    };
    for name in ["runa", "runc", "rund"] {
        let stripped = strip(row(&after, name));
        assert!(
            !stripped.ends_with('s'),
            "elapsed time left in {stripped:?}"
        );
        assert_eq!(stripped, strip(row(&live, name)), "{name}");
    }
}

#[tokio::test]
async fn an_agent_transcript_streams_and_esc_returns_to_the_run_with_it_selected() {
    let mut f = Fixture::new(json!({ "max_concurrent": 8 })).await;
    let body = format!(
        "{}  {}",
        fan("Scan", "scan", &plain("scan", 1)),
        fan("Verify", "verify finding", &gates("v", 2))
    );
    f.start(&script("audit", &["Scan", "Verify"], &body), json!({}))
        .await;
    f.held("verify agents in flight", 2).await;
    let mut app = f.monitor().await;
    select(&mut app, "audit");
    let actions = press(&mut app, KeyCode::Enter);
    f.drive(&mut app, actions).await;
    let tree = text(&f.show(&mut app, 120, 40));
    assert!(tree.contains("▼ Scan"), "{tree}");
    assert!(tree.contains("1/1"), "{tree}");
    assert!(
        tree.contains("▼ Verify") && tree.contains("2 running"),
        "{tree}"
    );
    // The first verify agent: Down past the scan agent.
    press(&mut app, KeyCode::Down);
    let view = app.workflows.run.as_ref().unwrap();
    let detail = view.detail.data.as_ref().unwrap();
    let agent = detail.agents[detail.order()[view.selected]].clone();
    assert_eq!(agent.label.as_deref(), Some("verify finding 1"));
    assert_eq!(agent.status, "running");
    let actions = key(&mut app, 't');
    f.drive(&mut app, actions).await;
    assert!(
        matches!(app.overlay, Overlay::None),
        "{:?}",
        app.workflows.refusals
    );
    let child = f
        .server
        .client
        .get(&format!("/sessions/{}", app.session.id))
        .await
        .unwrap();
    assert_eq!(child["data"]["subagent_name"], agent.id.as_str(), "{child}");
    assert_eq!(app.session.parent_id.as_deref(), Some(f.owner.as_str()));
    let transcript = text(&f.show(&mut app, 120, 40));
    assert!(transcript.contains("GATE-v1"), "{transcript}");
    // The agent answers while its transcript is open; the gate lets the oldest request
    // through, which may be the other agent's.
    f.server.model.open_gate();
    f.server.model.open_gate();
    f.pump(&mut app, "the agent's answer streams in", |app| {
        app.items
            .iter()
            .any(|item| format!("{item:?}").contains("v1 done"))
    })
    .await;
    assert!(text(&f.show(&mut app, 120, 40)).contains("v1 done"));
    let actions = press(&mut app, KeyCode::Esc);
    assert!(matches!(app.overlay, Overlay::Workflows));
    f.drive(&mut app, actions).await;
    assert_eq!(app.session.id, f.owner);
    let view = app.workflows.run.as_ref().expect("the run view");
    let detail = view.detail.data.as_ref().unwrap();
    let selected = &detail.agents[detail.order()[view.selected]];
    assert_eq!(selected.id, agent.id, "the agent stays selected");
    assert_eq!(selected.status, "completed");
    press(&mut app, KeyCode::Esc);
    assert!(app.workflows.run.is_none(), "Esc returns to the list");
}

/// Regression: the transcript of a running agent of a run another Session owns (a CLI or
/// HTTP run) was looked up among the TUI Session's children and reported "no transcript
/// yet"; it is a child of the run's owner.
#[tokio::test]
async fn the_transcript_of_an_agent_of_another_sessions_run_opens() {
    let f = Fixture::new(json!({})).await;
    let other = f.server.session().await;
    let body = fan("Verify", "v", &gates("v", 1));
    f.start(
        &script("audit", &["Verify"], &body),
        json!({ "session_id": other }),
    )
    .await;
    f.held("agent in flight", 1).await;
    let mut app = f.monitor().await;
    select(&mut app, "audit");
    let actions = press(&mut app, KeyCode::Enter);
    f.drive(&mut app, actions).await;
    let actions = key(&mut app, 't');
    f.drive(&mut app, actions).await;
    assert!(
        matches!(app.overlay, Overlay::None),
        "{:?}",
        app.workflows.refusals
    );
    assert_eq!(app.session.parent_id.as_deref(), Some(other.as_str()));
    let transcript = text(&f.show(&mut app, 120, 40));
    assert!(transcript.contains("GATE-v1"), "{transcript}");
    let actions = press(&mut app, KeyCode::Esc);
    f.drive(&mut app, actions).await;
    assert_eq!(app.session.id, f.owner, "Esc returns to the TUI's Session");
    assert!(app.workflows.run.is_some(), "and to the run view");
    f.server.model.open_gate();
}

#[tokio::test]
async fn stop_asks_first_and_the_other_keys_use_the_public_routes() {
    let mut f = Fixture::new(json!({})).await;
    let body = format!(
        "log('scanning', 2, {{ found: 1 }});\n  {}",
        fan("Verify", "v", &gates("v", 1))
    );
    let run = f
        .start(&script("audit", &["Verify"], &body), json!({}))
        .await;
    f.held("agent in flight", 1).await;
    let mut app = f.monitor().await;
    assert!(key(&mut app, 'x').is_empty());
    let prompt = text(&f.show(&mut app, 120, 40));
    assert!(prompt.contains("Stop audit? y stop"), "{prompt}");
    assert!(key(&mut app, 'n').is_empty(), "any other key cancels");
    assert!(!text(&f.show(&mut app, 120, 40)).contains("Stop audit?"));
    assert_eq!(f.run(&run).await["status"], "running");

    // `l` shows the run's log, `s` opens its script file, `a` lists every Location.
    let actions = key(&mut app, 'l');
    f.drive(&mut app, actions).await;
    let log = text(&f.show(&mut app, 120, 40));
    assert!(
        log.contains("audit · log") && log.contains("scanning 2 {\"found\":1}"),
        "{log}"
    );
    press(&mut app, KeyCode::Esc);
    let script_file = f.run(&run).await["script_file"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(key(&mut app, 's'), [Action::EditFile(script_file)]);
    let actions = key(&mut app, 'a');
    f.drive(&mut app, actions).await;
    assert!(text(&f.show(&mut app, 120, 40)).contains("Workflows · all Locations"));
    assert_eq!(app.workflows.summaries().len(), 1);

    key(&mut app, 'x');
    let actions = key(&mut app, 'y');
    f.drive(&mut app, actions).await;
    f.wait_run("audit stops", &run, |r| r["status"] == "stopped")
        .await;
    f.pump(&mut app, "the row shows the stop", |app| {
        app.workflows
            .current()
            .is_some_and(|r| r.status == "stopped")
    })
    .await;
    // A refusal shows on the run's row: a stopped run cannot be paused.
    let actions = key(&mut app, 'p');
    f.drive(&mut app, actions).await;
    let refused = app
        .workflows
        .refusals
        .get(&run)
        .cloned()
        .expect("a refusal");
    let shown = text(&f.show(&mut app, 120, 40));
    let lines: Vec<&str> = shown.lines().collect();
    let at = lines.iter().position(|l| l.contains(" audit ")).unwrap();
    assert!(lines[at + 1].contains(&format!("⚠ {refused}")), "{shown}");
    // `R` with nothing interrupted says so.
    let actions = key(&mut app, 'R');
    f.drive(&mut app, actions).await;
    assert_eq!(
        app.toast.as_ref().map(|(t, _)| t.as_str()),
        Some("No interrupted run to resume")
    );
}

#[tokio::test]
async fn resuming_a_changed_script_offers_a_new_revision() {
    let f = Fixture::new(json!({})).await;
    let body = format!(
        "{}  await agent('after');\n",
        fan("Verify", "v", &gates("v", 1))
    );
    let run = f
        .start(&script("audit", &["Verify"], &body), json!({}))
        .await;
    f.held("agent in flight", 1).await;
    f.server
        .client
        .post(&format!("/workflows/runs/{run}/pause"), json!({}))
        .await
        .unwrap();
    f.server.model.open_gate();
    let paused = f
        .wait_run("audit settles paused", &run, |r| {
            r["status"] == "paused" && r["agents"]["running"] == 0
        })
        .await;
    let file = paused["script_file"].as_str().unwrap();
    let changed = script(
        "audit",
        &["Verify"],
        &fan("Verify", "v", &plain("changed", 1)),
    );
    std::fs::write(file, changed).unwrap();
    let mut app = f.monitor().await;
    let actions = key(&mut app, 'r');
    f.drive(&mut app, actions).await;
    let prompt = text(&f.show(&mut app, 120, 40));
    assert!(
        prompt.contains("Script changed — start revision 2? y yes"),
        "{prompt}"
    );
    let actions = key(&mut app, 'y');
    f.drive(&mut app, actions).await;
    let resumed = f
        .wait_run("revision 2 completes", &run, |r| r["status"] == "completed")
        .await;
    assert_eq!(resumed["revision"], 2, "{resumed}");
    assert!(
        app.workflows.refusals.is_empty(),
        "{:?}",
        app.workflows.refusals
    );
}

#[tokio::test]
async fn at_most_the_selected_running_row_animates_among_twenty() {
    let f = Fixture::new(json!({ "max_running_runs": 1 })).await;
    for i in 0..20 {
        let body = fan("Work", "w", &gates(&format!("r{i}x"), 1));
        f.start(&script(&format!("run{i:02}"), &["Work"], &body), json!({}))
            .await;
    }
    f.held("the running run's agent", 1).await;
    let mut app = f.monitor().await;
    assert_eq!(app.workflows.summaries().len(), 20);
    let running = app
        .workflows
        .summaries()
        .iter()
        .position(|run| run.status == "running")
        .expect("one run is running");
    app.workflows.selected = running;
    let moving = |clock_ms| Opts {
        reduced_motion: false,
        clock_ms,
        ..Opts::default()
    };
    let start = app.started();
    let mut frames = Vec::new();
    let mut changed_rows = HashSet::new();
    let mut last = buffer(&mut app, 160, 48, &moving(0));
    for ms in (0..2_000).step_by(4) {
        app.clock = Some(start + Duration::from_millis(ms));
        if app.advance_frame() {
            frames.push(ms);
            let next = buffer(&mut app, 160, 48, &moving(ms));
            for y in 0..48 {
                if (0..160).any(|x| next[(x, y)] != last[(x, y)]) {
                    changed_rows.insert(y);
                }
            }
            last = next;
        }
    }
    assert!(frames.len() >= 20, "{frames:?}");
    for (i, first) in frames.iter().enumerate() {
        let in_second = frames[i..].iter().take_while(|&&at| at < first + 1000);
        assert!(in_second.count() as u64 <= crate::progress::MAX_FPS);
    }
    let name = app.workflows.summaries()[running].name.clone();
    let rows: Vec<u16> = changed_rows.into_iter().collect();
    assert_eq!(rows.len(), 1, "only one row moves: {rows:?}");
    let line: String = (0..160).map(|x| last[(x, rows[0])].symbol()).collect();
    assert!(line.contains(&name), "{line}");
    // A queued row selected: nothing animates.
    app.workflows.selected = (running + 1) % 20;
    buffer(&mut app, 160, 48, &moving(2_000));
    assert_eq!(app.next_frame(), None);
}

fn with_warmup_off(mut workflows: Value) -> Value {
    if !workflows.is_object() {
        workflows = json!({});
    }
    workflows["prompt_cache_warmup"] = json!(false);
    workflows
}

fn ctrl_u(app: &mut App) {
    app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
}

fn select_entry(app: &mut App, name: &str) {
    let library = &mut app.workflows.library;
    library.selected = library
        .listed()
        .iter()
        .position(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} is not in the library"));
}

/// `workflows` → Saved and bundled workflows: Tab shows the Location's library with origin
/// badges; Enter opens the args form prefilled from the defaults, which refuses args the
/// schema rejects and starts the run for the Session; `o` shows the source.
#[tokio::test]
async fn the_library_tab_lists_the_library_and_starts_a_workflow_from_its_form() {
    let f = Fixture::new(json!({})).await;
    let mut app = f.monitor().await;
    let project = std::path::Path::new(&app.session.directory).join(".cyber/workflows");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("release.js"),
        script("release", &["Ship"], "return 1;"),
    )
    .unwrap();
    let actions = press(&mut app, KeyCode::Tab);
    f.drive(&mut app, actions).await;
    assert!(app.workflows.library_tab);
    let names: Vec<&str> = app
        .workflows
        .library
        .listed()
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "audit", "migrate", "plan", "release", "research", "review", "solve"
        ]
    );
    for (width, height) in [(120, 40), (80, 24)] {
        let shown = stabilized(&mut app, |app| render(app, width, height, &Opts::default()));
        insta::assert_snapshot!(format!("workflow_library_{width}x{height}"), shown);
    }

    select_entry(&mut app, "research");
    press(&mut app, KeyCode::Enter);
    let form = app.workflows.library.form.clone().unwrap();
    assert_eq!(
        form.text,
        r#"{"sources":["repo","docs"],"angles":3,"max_sources":12}"#
    );
    let shown = stabilized(&mut app, |app| render(app, 120, 40, &Opts::default()));
    insta::assert_snapshot!("workflow_library_form_120x40", shown);
    ctrl_u(&mut app);
    type_line(&mut app, r#"{"question":"q","angles":9}"#);
    assert!(press(&mut app, KeyCode::Enter).is_empty());
    let error = app
        .workflows
        .library
        .form
        .as_ref()
        .unwrap()
        .error
        .clone()
        .unwrap();
    assert!(error.starts_with("args.angles: "), "{error}");
    ctrl_u(&mut app);
    type_line(
        &mut app,
        r#"{"question":"q","sources":["repo"],"angles":1}"#,
    );
    let actions = press(&mut app, KeyCode::Enter);
    assert!(matches!(
        actions.as_slice(),
        [Action::Workflow(crate::workflows::Request::StartRun { .. })]
    ));
    f.drive(&mut app, actions).await;
    assert!(!app.workflows.library_tab);
    assert!(
        app.toast
            .as_ref()
            .unwrap()
            .0
            .starts_with("Started research")
    );
    assert!(
        app.workflows
            .summaries()
            .iter()
            .any(|run| run.name == "research")
    );

    let actions = press(&mut app, KeyCode::Tab);
    f.drive(&mut app, actions).await;
    select_entry(&mut app, "review");
    key(&mut app, 'o');
    let source = app.workflows.library.source.as_ref().unwrap();
    assert!(
        source.lines[0].starts_with("export const meta"),
        "{:?}",
        source.lines[0]
    );
    press(&mut app, KeyCode::Esc);
    assert!(app.workflows.library.source.is_none() && app.workflows.library_tab);
}

#[tokio::test]
async fn a_second_click_opens_the_run_view() {
    use crate::screens_mouse::{cell_of, click};
    let f = Fixture::new(json!({})).await;
    for name in ["plan", "docs"] {
        let run = f
            .start(
                &script(name, &["Draft"], &fan("Draft", "draft", &plain("draft", 2))),
                json!({}),
            )
            .await;
        f.wait_run("the run completes", &run, |r| r["status"] == "completed")
            .await;
    }
    let mut app = f.monitor().await;
    let selected = app.workflows.selected;
    let other = 1 - selected;
    let name = app.workflows.summaries()[other].name.clone();
    let at = cell_of(&f.show(&mut app, 120, 40), &format!(" {name} "));
    assert!(click(&mut app, at).is_empty(), "the first click selects");
    assert_eq!(app.workflows.selected, other);
    assert!(app.workflows.run.is_none());
    let actions = click(&mut app, at);
    assert!(!actions.is_empty());
    f.drive(&mut app, actions).await;
    let view = app.workflows.run.as_ref().expect("the run view");
    assert_eq!(
        view.detail.data.as_ref().unwrap().summary.name,
        name,
        "the clicked run"
    );

    // An agent row: the second click opens its transcript.
    let at = cell_of(&f.show(&mut app, 120, 40), "draft 2");
    assert!(click(&mut app, at).is_empty());
    let view = app.workflows.run.as_ref().unwrap();
    let detail = view.detail.data.as_ref().unwrap();
    let agent = detail.agents[detail.order()[view.selected]].clone();
    assert_eq!(agent.label.as_deref(), Some("draft 2"));
    let actions = click(&mut app, at);
    f.drive(&mut app, actions).await;
    assert!(
        matches!(app.overlay, Overlay::None),
        "{:?}",
        app.workflows.refusals
    );
    assert_eq!(app.session.parent_id.as_deref(), Some(f.owner.as_str()));
}
