//! add-parallel-workflow-runs 11.4: the TUI driven against the server of 11.1, drawn by
//! the real `view::draw` on a `TestBackend`. A production App runs A (12 agents) and B
//! (2) started over HTTP and C (6, `max_concurrent: 2`) started by the `workflow` tool
//! of the TUI's own Session S; every agent waits at a gate of the scripted model. The
//! monitor's reads and actions go through `perform` and the runner's `apply`, and its
//! live updates come from the real instance event stream through `server_event`. The
//! snapshots are the acceptance evidence of `/workflows`, `p`, `x` and `r`.

use serde_json::{Value, json};

use crate::app::App;
use crate::screens::{Opts, render};
use crate::screens_workflows::{Fixture, key, row, select, stabilized, text};

/// `workflows.max_running_runs: 3` as in 11.1, and `max_concurrent: 3` instead of 4: the
/// server caps the limit at the core count and the macOS runners have 3 cores, so 3
/// keeps the recorded counts the same on every machine.
const SLOTS: usize = 3;

fn gated(name: &str, run: char, n: usize) -> String {
    let prompts: Vec<String> = (1..=n).map(|i| format!("'GATE-{run}{i}'")).collect();
    format!(
        "export const meta = {{ name: '{name}', description: 'Gated fan-out' }};\nexport default async () => (await parallel([{}].map((p) => () => agent(p)))).length;\n",
        prompts.join(", ")
    )
}

/// S starts C through its `workflow` tool and acknowledges what comes back; agents wait
/// at their gates.
fn reply(body: &Value) -> Value {
    let last = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .cloned()
        .unwrap_or_default();
    let text = last.to_string();
    if body.to_string().contains("Write a short title") {
        return json!({ "content": "Session S" });
    }
    if last["role"] == "tool" || text.contains("Workflow run handback") {
        return json!({ "content": "noted" });
    }
    if text.contains("start the C workflow") {
        let arguments =
            json!({ "script": gated("rc", 'c', 6), "args": {}, "max_concurrent": 2 }).to_string();
        return json!({"tool_calls": [{"index": 0, "id": "call_c", "type": "function",
            "function": {"name": "workflow", "arguments": arguments}}]});
    }
    crate::screens_workflows::answer(body)
}

impl Fixture {
    /// The three runs of 11.1, all running, with A holding every agent slot.
    async fn three_runs(&mut self) -> [String; 3] {
        let other = self.server.session().await;
        let start = |script: String| json!({ "session_id": other, "script": script });
        let client = &self.server.client;
        let a = client
            .post("/workflows/runs", start(gated("ra", 'a', 12)))
            .await
            .unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.held("A holds every slot", SLOTS).await;
        let b = client
            .post("/workflows/runs", start(gated("rb", 'b', 2)))
            .await
            .unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.wait_run("B queues", &b, |r| r["agents"]["queued"] == 2)
            .await;
        let created = client
            .post(
                "/sessions",
                json!({ "model": "test/main", "mode": "bypass" }),
            )
            .await
            .unwrap();
        self.owner = created["data"]["id"].as_str().unwrap().to_owned();
        self.server
            .prompt(&self.owner, "please start the C workflow")
            .await;
        let c = loop {
            let runs = client
                .get(&format!("/workflows/runs?session_id={}", self.owner))
                .await
                .unwrap();
            if let Some(id) = runs["data"][0]["id"].as_str() {
                break id.to_owned();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        self.wait_run("C queues", &c, |r| r["agents"]["queued"] == 6)
            .await;
        [a, b, c]
    }

    async fn status(&self, id: &str) -> String {
        self.run(id).await["status"].as_str().unwrap().to_owned()
    }

    /// Open gates one at a time until `runs` all ended; the TUI reads the events later.
    async fn drain(&self, runs: &[&String]) {
        let model = &self.server.model;
        let passed = || model.starts().len() - model.held().len();
        loop {
            let mut active = false;
            for id in runs {
                active |= ["queued", "running"].contains(&self.status(id).await.as_str());
            }
            if !active {
                return;
            }
            if model.held().is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                continue;
            }
            let before = passed();
            model.open_gate();
            while passed() == before {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }
    }
}

fn snapshot(app: &mut App) -> String {
    stabilized(app, |app| render(app, 120, 40, &Opts::default()))
}

/// The monitor rows of A, B and C: status and done/running/queued/failed counts.
fn rows(app: &mut App) -> [String; 3] {
    let buf = crate::screens::buffer(app, 120, 40, &Opts::default());
    ["ra", "rb", "rc"].map(|name| row(&buf, name))
}

/// Each run's `(status, done/running/queued/failed)` as `ra`, `rb` and `rc`.
type Want = [(&'static str, [u64; 4]); 3];

/// Whether the monitor lists the runs as `want` and the footer's indicators caught up.
fn shows(app: &App, want: &Want) -> bool {
    let listed = ["ra", "rb", "rc"]
        .iter()
        .zip(want)
        .all(|(name, (status, n))| {
            app.workflows.summaries().iter().any(|r| {
                let a = &r.agents;
                r.name == *name
                    && r.status == *status
                    && [a.completed, a.running, a.queued, a.failed] == *n
            })
        });
    let active = want
        .iter()
        .filter(|(status, _)| ["running", "queued"].contains(status))
        .count();
    listed && !app.watch.runs.loading && app.watch.indicated().count() == active
}

impl Fixture {
    /// Follow the stream until the screen shows `want`.
    async fn shown(&mut self, app: &mut App, what: &str, want: Want) {
        self.pump(app, what, |app| shows(app, &want)).await;
    }
}

#[tokio::test]
async fn the_monitor_controls_the_selected_run_of_three_and_notices_their_ends() {
    let workflows = json!({ "max_concurrent": SLOTS, "max_running_runs": 3 });
    let mut f = Fixture::with_reply(reply, workflows).await;
    let [a, b, c] = f.three_runs().await;
    let mut app = f.monitor().await;
    let names: Vec<&str> = app
        .workflows
        .summaries()
        .iter()
        .map(|run| run.name.as_str())
        .collect();
    assert_eq!(names, ["rc", "rb", "ra"]);
    let a_held = ("running", [0, 3, 9, 0]);
    f.shown(
        &mut app,
        "three runs with live counts",
        [a_held, ("running", [0, 0, 2, 0]), ("running", [0, 0, 6, 0])],
    )
    .await;
    insta::assert_snapshot!("parallel_three_runs_120x40", snapshot(&mut app));

    // `p` pauses the selected run only: B's queued calls leave.
    select(&mut app, "rb");
    let actions = key(&mut app, 'p');
    f.drive(&mut app, actions).await;
    f.shown(
        &mut app,
        "B paused",
        [a_held, ("paused", [0; 4]), ("running", [0, 0, 6, 0])],
    )
    .await;
    assert_eq!(f.status(&b).await, "paused");
    for id in [&a, &c] {
        assert_eq!(f.status(id).await, "running");
    }
    insta::assert_snapshot!("parallel_p_pauses_b_120x40", snapshot(&mut app));

    // `x` (confirmed) stops the selected run only; S's own transcript notices the end.
    select(&mut app, "rc");
    assert!(key(&mut app, 'x').is_empty());
    let actions = key(&mut app, 'y');
    f.drive(&mut app, actions).await;
    f.shown(
        &mut app,
        "C stopped",
        [a_held, ("paused", [0; 4]), ("stopped", [0; 4])],
    )
    .await;
    assert_eq!(f.status(&c).await, "stopped");
    assert_eq!(f.status(&a).await, "running");
    assert_eq!(f.status(&b).await, "paused");
    insta::assert_snapshot!("parallel_x_stops_c_120x40", snapshot(&mut app));
    f.pump(&mut app, "C's end notice", |app| {
        app.watch.notices.iter().any(|n| n.text == "■ rc stopped")
    })
    .await;

    // `r` resumes the selected run only: B queues again behind A's slots.
    select(&mut app, "rb");
    let actions = key(&mut app, 'r');
    f.drive(&mut app, actions).await;
    f.shown(
        &mut app,
        "B running again",
        [a_held, ("running", [0, 0, 2, 0]), ("stopped", [0; 4])],
    )
    .await;
    assert_eq!(f.status(&b).await, "running");
    assert_eq!(f.status(&c).await, "stopped");
    assert_eq!(f.status(&a).await, "running");
    insta::assert_snapshot!("parallel_r_resumes_b_120x40", snapshot(&mut app));

    // The runs end: B, which this TUI acted on, ends in a toast; C's stop stays in S's
    // transcript.
    f.drain(&[&a, &b]).await;
    f.pump(&mut app, "B's end notice", |app| {
        app.toast
            .as_ref()
            .is_some_and(|(t, _)| t.starts_with("✓ rb completed · 2 agents · $"))
    })
    .await;
    f.shown(
        &mut app,
        "every run ended",
        [
            ("completed", [12, 0, 0, 0]),
            ("completed", [2, 0, 0, 0]),
            ("stopped", [0; 4]),
        ],
    )
    .await;
    app.toast = None;
    insta::assert_snapshot!("parallel_ended_120x40", snapshot(&mut app));
    let [ra, rb, rc] = rows(&mut app);
    assert!(
        ra.contains("12/0/0/0") && rb.contains("2/0/0/0"),
        "{ra}\n{rb}"
    );
    assert!(rc.contains("stopped"), "{rc}");
    app.overlay = crate::app::Overlay::None;
    let shown = text(&f.show(&mut app, 120, 40));
    assert!(shown.contains("■ rc stopped"), "{shown}");
}
