//! Parallel Workflow Runs end to end (`workflows` → Concurrent runs, Fair agent
//! scheduling, Run queue, Recovery of several runs; add-parallel-workflow-runs 11.1–11.3)
//! with the real binary: the `cyber` CLI starts the background `cyber serve` service,
//! runs are started through `cyber workflows run`, `POST /api/v1/workflows/runs` and the
//! `workflow` tool of a Session, and every agent's model request waits at a gate of the
//! local scripted model, so each step is driven by a gate opening and observed through the
//! provider log, the HTTP API and the CLI. Nothing reaches the internet or a live model.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::Duration;

use serde_json::{Value, json};

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

use scripted_model::ScriptedModel;

/// An isolated `HOME` and `CYBER_HOME`, the working directory of every command.
struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    /// A fresh environment whose global configuration names `model` and has
    /// `workflows` as its `workflows` settings.
    fn new(model: &ScriptedModel, workflows: Value) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let config = root.join("cyber-home").join("config");
        std::fs::create_dir_all(&config).unwrap();
        let mut global = model.config();
        global["workflows"] = workflows;
        std::fs::write(config.join("cyber.jsonc"), global.to_string()).unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        Self { _dir: dir, root }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(&self.root);
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        // The owners are `bypass` Sessions; no OS sandbox admits them on Windows, so the
        // service it starts vouches for its isolation instead.
        #[cfg(windows)]
        command.env("CYBER_CONTAINER", "1");
        command
    }

    /// Run the CLI and return its stdout, asserting it exits 0.
    fn cyber(&self, args: &[&str]) -> String {
        let output: Output = self.command(args).output().unwrap();
        let (out, err) = (
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        );
        assert!(output.status.success(), "cyber {args:?}: {out}{err}");
        out
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.root.join(name), text).unwrap();
    }

    /// The registered service's PID.
    fn service_pid(&self) -> u32 {
        let path = self.root.join("cyber-home/state/server.json");
        let registration: Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        u32::try_from(registration["pid"].as_u64().unwrap()).unwrap()
    }
}

/// The HTTP API of the registered `cyber serve`, for the request Location `root`.
struct Api {
    rt: tokio::runtime::Runtime,
    client: cyber_client::Client,
}

impl Api {
    /// Start the background service through the CLI and attach to it.
    fn start(env: &Env) -> Self {
        env.cyber(&["workflows", "runs"]);
        let state = env.root.join("cyber-home/state");
        let registration: Value =
            serde_json::from_str(&std::fs::read_to_string(state.join("server.json")).unwrap())
                .unwrap();
        let password = std::fs::read_to_string(state.join("password")).unwrap();
        let client = cyber_client::Client::http(
            registration["url"].as_str().unwrap(),
            Some(password.trim().to_owned()),
        )
        .at(&env.root.display().to_string());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        Self { rt, client }
    }

    fn get(&self, path: &str) -> Value {
        self.rt
            .block_on(self.client.get(path))
            .unwrap_or_else(|e| panic!("GET {path}: {e}"))
    }

    fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        self.rt
            .block_on(self.client.post(path, body))
            .map_err(|e| e.to_string())
    }

    fn run(&self, id: &str) -> Value {
        self.get(&format!("/workflows/runs/{id}"))["data"].take()
    }

    fn status(&self, id: &str) -> String {
        self.run(id)["status"].as_str().unwrap().to_owned()
    }

    fn session(&self, body: Value) -> String {
        self.post("/sessions", body).unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn start_run(&self, session: &str, script: &str) -> Value {
        let body = json!({"session_id": session, "script": script});
        self.post("/workflows/runs", body).unwrap()["data"].take()
    }

    fn prompt(&self, session: &str, text: &str) {
        let body = json!({"parts": [{"type": "text", "text": text}]});
        self.post(&format!("/sessions/{session}/prompt"), body)
            .unwrap();
    }

    /// Pause, stop or resume `run`, retrying a resume refused while agents still exit.
    fn control(&self, run: &str, action: &str) -> Value {
        let path = format!("/workflows/runs/{run}/{action}");
        for _ in 0..500 {
            match self.post(&path, json!({})) {
                Ok(mut answer) => return answer["data"].take(),
                Err(e) if e.contains(cyber_server::runtime::AGENTS_EXITING) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => panic!("{action} {run}: {e}"),
            }
        }
        panic!("{action} {run} stayed refused");
    }

    /// How often each message ID was admitted to `session` (its inbox rows: a run
    /// started over HTTP queues its summary without resuming the owner, and a promoted
    /// row stays listed) and how often it is in the owner's model-visible history.
    fn delivered(&self, session: &str, ids: &[String]) -> (Vec<usize>, Vec<usize>) {
        let inbox = self.get(&format!("/sessions/{session}/inbox"));
        let history = self.get(&format!("/sessions/{session}/messages?limit=200"));
        let count = |rows: &Value, key: &str| -> Vec<usize> {
            let rows = rows["data"].as_array().unwrap();
            ids.iter()
                .map(|id| rows.iter().filter(|row| row[key] == id.as_str()).count())
                .collect()
        };
        (count(&inbox, "message_id"), count(&history, "id"))
    }

    fn ended(&self, id: &str) -> Value {
        wait_until(&format!("{id} ends"), || {
            let run = self.run(id);
            let status = run["status"].as_str().unwrap_or_default();
            (!["queued", "running", "waiting_input"].contains(&status)).then_some(run)
        })
    }
}

/// Stop the service when the test ends, however it ends.
struct Service<'a>(&'a Env);

impl Drop for Service<'_> {
    fn drop(&mut self) {
        let _ = self.0.command(&["service", "stop"]).output();
    }
}

/// Poll `check` until it returns a value. Polling only observes; every ordering the
/// tests assert comes from a gate opening.
fn wait_until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    for _ in 0..3000 {
        if let Some(value) = check() {
            return value;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{what} did not happen");
}

fn last_message(body: &Value) -> Value {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default()
}

/// The `n` gated agents of run letter `run`, as one parallel fan-out named `name`.
fn fan_out(name: &str, run: char, n: usize) -> String {
    let prompts: Vec<String> = (1..=n).map(|i| format!("'GATE-{run}{i}'")).collect();
    format!(
        "export const meta = {{ name: '{name}', description: 'Gated fan-out' }};\nexport default async () => (await parallel([{}].map((p) => () => agent(p)))).length;\n",
        prompts.join(", ")
    )
}

/// The owner Sessions start C through their `workflow` tool and answer anything else;
/// an agent whose task names `GATE-<label>` waits at that gate, and an interrupted child
/// that continues answers at once.
fn reply(body: &Value) -> Value {
    let whole = body.to_string();
    if whole.contains("Write a short title") {
        return json!({"content": "Owner"});
    }
    if whole.contains("was interrupted") {
        return json!({"content": "continued"});
    }
    let last = last_message(body);
    if last["role"] == "tool" {
        return json!({"content": "started"});
    }
    let text = last.to_string();
    if text.contains("Workflow run handback") {
        return json!({"content": "handback received"});
    }
    if text.contains("start the C workflow") {
        let arguments =
            json!({"script": fan_out("rc", 'c', 6), "args": {}, "max_concurrent": 2}).to_string();
        return json!({"tool_calls": [{"index": 0, "id": "call_c", "type": "function",
            "function": {"name": "workflow", "arguments": arguments}}]});
    }
    match text.find("GATE-") {
        Some(at) => {
            let label: String = text[at + 5..]
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            json!({"gate": label})
        }
        None => json!({"content": "agent answer"}),
    }
}

/// The run letter of each gated request still waiting, oldest first.
fn held(model: &ScriptedModel) -> String {
    model
        .held()
        .iter()
        .filter_map(|l| l.chars().next())
        .collect()
}

fn started(model: &ScriptedModel) -> String {
    model
        .starts()
        .iter()
        .filter_map(|(l, _)| l.chars().next())
        .collect()
}

/// Gated requests that have passed their gate.
fn passed(model: &ScriptedModel) -> usize {
    model.starts().len() - model.held().len()
}

/// Let the oldest gated request answer and wait until it has.
fn open_one(model: &ScriptedModel) {
    let before = passed(model);
    assert!(!model.held().is_empty(), "no request waits at a gate");
    model.open_gate();
    wait_until("the opened request answers", || {
        (passed(model) > before).then_some(())
    });
}

/// The fair scheduler's rule, as the spec states it ("Fair agent scheduling"), over the
/// scripted model's gates: a free slot goes to the eligible run that least recently
/// received one (never first, then by when it became eligible), and gates open oldest
/// first. It predicts, step by step, which run's request reaches the provider next.
#[derive(Default)]
struct Expected {
    capacity: usize,
    tick: u64,
    runs: BTreeMap<char, Queue>,
    /// Gated requests by run letter, oldest first; false once its agent was cancelled.
    held: VecDeque<(char, bool)>,
    starts: String,
}

#[derive(Default)]
struct Queue {
    queued: usize,
    running: usize,
    share: usize,
    last_grant: Option<u64>,
    eligible_since: u64,
    eligible: bool,
    suspended: bool,
}

impl Queue {
    fn refresh(&mut self, tick: &mut u64) {
        let eligible = !self.suspended && self.queued > 0 && self.running < self.share;
        if eligible && !self.eligible {
            *tick += 1;
            self.eligible_since = *tick;
        }
        self.eligible = eligible;
    }
}

impl Expected {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            ..Self::default()
        }
    }

    /// Run `run` issues `calls` agent calls with `share`.
    fn queue(&mut self, run: char, calls: usize, share: usize) {
        let share = share.min(self.capacity);
        let queue = Queue {
            queued: calls,
            share,
            ..Queue::default()
        };
        self.runs.insert(run, queue);
        self.dispatch();
    }

    fn open(&mut self) {
        let (run, live) = self.held.pop_front().expect("a held request");
        if live {
            self.runs.get_mut(&run).unwrap().running -= 1;
        }
        self.dispatch();
    }

    fn pause(&mut self, run: char) {
        let queue = self.runs.get_mut(&run).unwrap();
        queue.suspended = true;
        queue.queued = 0;
        self.dispatch();
    }

    /// Stop `run`: its agents are cancelled and their slots freed at once, while their
    /// requests still wait at the gate.
    fn stop(&mut self, run: char) {
        let queue = self.runs.get_mut(&run).unwrap();
        queue.running = 0;
        for request in self.held.iter_mut().filter(|(r, _)| *r == run) {
            request.1 = false;
        }
        self.pause(run);
    }

    fn dispatch(&mut self) {
        while self.runs.values().map(|q| q.running).sum::<usize>() < self.capacity {
            let tick = &mut self.tick;
            let next = self
                .runs
                .iter_mut()
                .filter_map(|(id, queue)| {
                    queue.refresh(tick);
                    queue
                        .eligible
                        .then_some((queue.last_grant, queue.eligible_since, *id))
                })
                .min();
            let Some((_, _, run)) = next else {
                return;
            };
            self.tick += 1;
            let queue = self.runs.get_mut(&run).unwrap();
            queue.queued -= 1;
            queue.running += 1;
            queue.last_grant = Some(self.tick);
            queue.refresh(&mut self.tick);
            self.held.push_back((run, true));
            self.starts.push(run);
        }
    }

    fn held(&self) -> String {
        self.held.iter().map(|(run, _)| *run).collect()
    }

    fn running(&self, run: char) -> usize {
        self.runs.get(&run).map_or(0, |q| q.running)
    }

    fn queued(&self, run: char) -> usize {
        self.runs.get(&run).map_or(0, |q| q.queued)
    }

    /// Wait until the provider shows exactly the predicted gate queue and start log.
    fn observed(&self, model: &ScriptedModel) {
        let want = (self.held(), self.starts.clone());
        for _ in 0..3000 {
            if (held(model), started(model)) == want {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "expected held {:?} and starts {:?}, the provider shows {:?} and {:?}",
            want.0,
            want.1,
            held(model),
            started(model)
        );
    }

    /// Open the oldest gate in the provider and in the prediction, and compare.
    fn step(&mut self, model: &ScriptedModel) {
        model.open_gate();
        self.open();
        self.observed(model);
    }
}

/// The run IDs and owners of the three runs of 11.1.
struct Three {
    a: String,
    b: String,
    c: String,
    owner_a: String,
    owner_b: String,
    owner_s: String,
}

/// 11.1: A (12 agents) from the real binary, B (2) over HTTP and C (6, `max_concurrent:
/// 2`) from Session S's `workflow` tool start together, before any gate opens, and are
/// all listed running with their queued counts.
fn start_three(env: &Env, api: &Api, model: &ScriptedModel, expected: &mut Expected) -> Three {
    env.write("ra.js", &fan_out("ra", 'a', 12));
    let owner_b = api.session(json!({"title": "B owner"}));
    let owner_s = api.session(json!({"title": "Session S", "mode": "bypass"}));
    let a = env.cyber(&["workflows", "run", "ra.js"]).trim().to_owned();
    expected.queue('a', 12, usize::MAX);
    expected.observed(model);
    let b = api.start_run(&owner_b, &fan_out("rb", 'b', 2))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_until("B queues its 2 agents", || {
        (api.run(&b)["agents"]["queued"] == 2).then_some(())
    });
    expected.queue('b', 2, usize::MAX);
    api.prompt(&owner_s, "please start the C workflow");
    let c = wait_until("C is started by S's workflow tool", || {
        let runs = api.get(&format!("/workflows/runs?session_id={owner_s}"));
        runs["data"][0]["id"].as_str().map(str::to_owned)
    });
    wait_until("C queues its 6 agents", || {
        (api.run(&c)["agents"]["queued"] == 6).then_some(())
    });
    expected.queue('c', 6, 2);
    expected.observed(model);
    let owner_a = api.run(&a)["session_id"].as_str().unwrap().to_owned();

    let listed = api.get("/workflows/runs?limit=200");
    let runs = listed["data"].as_array().unwrap();
    let shown = |id: &str| runs.iter().find(|r| r["id"] == id).cloned().unwrap();
    // "Together" is checked by events, not by wall-clock spacing (starting the real binary
    // and a model Turn takes seconds on Windows runners): no gate has opened yet, so every
    // run is running with nothing settled while the other two have started.
    for (id, queued, share) in [
        (&a, 12 - expected.capacity, expected.capacity),
        (&b, 2, expected.capacity),
        (&c, 6, 2),
    ] {
        let run = shown(id);
        assert_eq!(run["status"], "running", "{run}");
        assert_eq!(run["agents"]["queued"], queued, "{run}");
        assert_eq!(run["agents"]["completed"], 0, "{run}");
        assert_eq!(run["share"], share, "{run}");
    }
    Three {
        a,
        b,
        c,
        owner_a,
        owner_b,
        owner_s,
    }
}

/// C never runs more than its share of 2 agents.
fn check_share(api: &Api, expected: &Expected, c: &str) {
    assert!(expected.running('c') <= 2);
    let run = api.run(c);
    assert!(run["agents"]["running"].as_u64().unwrap() <= 2, "{run}");
}

/// The 11.1/11.2 scenario: the three runs, the provider and the prediction.
struct Scenario<'a> {
    api: &'a Api,
    model: &'a ScriptedModel,
    expected: Expected,
    runs: Three,
}

impl Scenario<'_> {
    /// Open the oldest gate and compare with the prediction; C stays within its share.
    fn step(&mut self) {
        self.expected.step(self.model);
        check_share(self.api, &self.expected, &self.runs.c);
    }

    /// 11.1: three releases, one at a time. B, never granted, is first; then C; then A.
    fn fair_releases(&mut self) {
        for _ in 0..3 {
            self.step();
        }
        let grants = &started(self.model)[self.expected.capacity..];
        assert_eq!(
            grants, "bca",
            "B and C are served ahead of A's queued calls"
        );
    }

    /// 11.2: pause B. Its queued call leaves and its running agent finishes, while A and
    /// C keep their queues and go on being served.
    fn pause_b(&mut self) {
        let (api, b) = (self.api, self.runs.b.clone());
        let b = b.as_str();
        let before = (self.expected.queued('a'), self.expected.queued('c'));
        let paused = api.control(b, "pause");
        assert_eq!(paused["status"], "paused", "{paused}");
        self.expected.pause('b');
        wait_until("B's queued call leaves", || {
            (api.run(b)["agents"]["queued"] == 0).then_some(())
        });
        for (id, queued) in [(&self.runs.a, before.0), (&self.runs.c, before.1)] {
            let run = api.run(id);
            assert_eq!(run["status"], "running", "{run}");
            assert_eq!(run["agents"]["queued"], queued, "{run}");
        }
        while self.expected.running('b') > 0 {
            self.step();
        }
        let settled = wait_until("B's running agent finishes", || {
            let run = api.run(b);
            (run["agents"]["running"] == 0).then_some(run)
        });
        assert_eq!(settled["status"], "paused", "{settled}");
        assert_eq!(settled["agents"]["completed"], 1, "{settled}");
        assert_eq!(started(self.model).matches('b').count(), 1);
    }

    /// Stop C: its agents are cancelled, and A gets the freed slots without any gate
    /// opening. Then A alone is served until its queue is empty.
    fn stop_c(&mut self) {
        let stopped = self.api.control(&self.runs.c, "stop");
        assert_eq!(stopped["status"], "stopped", "{stopped}");
        self.expected.stop('c');
        self.expected.observed(self.model);
        let run = self.api.run(&self.runs.c);
        let detail = run["agents_detail"].as_array().unwrap();
        assert!(!detail.is_empty(), "{run}");
        assert!(detail.iter().all(|a| a["status"] == "cancelled"), "{run}");
        while self.expected.queued('a') > 0 {
            self.expected.step(self.model);
        }
    }

    /// Resume B and C, which with A hold the three run slots, so a fourth run D queues;
    /// D is dispatched when A ends. Returns D and its owner.
    fn resume_and_queue_d(&mut self) -> (String, String) {
        let api = self.api;
        for id in [&self.runs.b, &self.runs.c] {
            let resumed = api.control(id, "resume");
            assert_eq!(resumed["status"], "running", "{resumed}");
        }
        let owner_d = api.session(json!({"title": "D owner"}));
        let d = api.start_run(&owner_d, &fan_out("rd", 'd', 1));
        assert_eq!(d["status"], "queued", "{d}");
        assert_eq!(d["queue_position"], 1, "{d}");
        let d = d["id"].as_str().unwrap().to_owned();
        // A's last agents are the oldest at the gate, with B's and C's behind them.
        let held = &self.expected.held;
        let a_left = held.iter().rposition(|(run, _)| *run == 'a').unwrap() + 1;
        for _ in 0..a_left {
            assert_eq!(api.status(&d), "queued");
            assert!(!started(self.model).contains('d'));
            open_one(self.model);
        }
        assert_eq!(api.ended(&self.runs.a)["status"], "completed");
        wait_until("D is dispatched when A ends", || {
            (api.status(&d) != "queued").then_some(())
        });
        for id in [&self.runs.b, &self.runs.c] {
            assert_eq!(api.status(id), "running", "only A's end dispatches D");
        }
        drain(api, self.model, &[&self.runs.b, &self.runs.c, &d]);
        for (id, agents) in [
            (&self.runs.a, 12),
            (&self.runs.b, 2),
            (&self.runs.c, 6),
            (&d, 1),
        ] {
            let run = api.ended(id);
            assert_eq!(run["status"], "completed", "{run}");
            assert_eq!(run["agents"]["completed"], agents, "{run}");
        }
        (d, owner_d)
    }

    /// One summary per owner and attempt: none for B's pause, one for its resumed
    /// attempt, and S's for C's stopped attempt and its resumed one. A run started over
    /// HTTP queues its summary; S's tool-started run resumes S, so its summaries reach
    /// S's history too.
    fn summaries_once(&self, d: &str, owner_d: &str) {
        use cyber_server::runtime::{attempt_handback_message_id as attempt, handback_message_id};
        let runs = &self.runs;
        let owners = [
            (runs.owner_a.as_str(), vec![handback_message_id(&runs.a)]),
            (
                &runs.owner_b,
                vec![handback_message_id(&runs.b), attempt(&runs.b, 1)],
            ),
            (
                &runs.owner_s,
                vec![handback_message_id(&runs.c), attempt(&runs.c, 1)],
            ),
            (owner_d, vec![handback_message_id(d)]),
        ];
        let wanted = [
            (vec![1], vec![0]),
            (vec![0, 1], vec![0, 0]),
            (vec![1, 1], vec![1, 1]),
            (vec![1], vec![0]),
        ];
        for ((owner, ids), want) in owners.iter().zip(wanted) {
            self.delivered_exactly(owner, ids, &want);
        }
    }

    /// Wait until `owner` received `want` of `ids`, never more.
    fn delivered_exactly(&self, owner: &str, ids: &[String], want: &(Vec<usize>, Vec<usize>)) {
        let at_most = |seen: &[usize]| seen.iter().zip(&want.0).all(|(n, m)| n <= m);
        let mut counts = (Vec::new(), Vec::new());
        for _ in 0..3000 {
            counts = self.api.delivered(owner, ids);
            assert!(
                at_most(&counts.0) && at_most(&counts.1),
                "{owner}: {counts:?}"
            );
            if counts == *want {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(counts, *want, "summaries {ids:?} in {owner}");
    }
}

/// add-parallel-workflow-runs 11.1 and 11.2, through the real binary and `cyber serve`
/// with `workflows.max_concurrent: 4` (reduced to the core count) and `max_running_runs:
/// 3`: the provider's start log follows the least-recently-granted order gate by gate,
/// and pausing B, stopping C, resuming both and queueing a fourth run each act on that
/// run only, and every attempt's summary reaches its owner once.
#[test]
fn three_runs_share_the_slots_fairly_and_are_controlled_independently() {
    let model = ScriptedModel::start(reply);
    let env = Env::new(&model, json!({"max_concurrent": 4, "max_running_runs": 3}));
    let _service = Service(&env);
    let api = Api::start(&env);
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    let capacity = 4.min(cores);
    assert!(
        capacity >= 3,
        "the scenario needs 3 agent slots; {cores} cores"
    );
    let mut expected = Expected::new(capacity);
    let runs = start_three(&env, &api, &model, &mut expected);
    let mut scenario = Scenario {
        api: &api,
        model: &model,
        expected,
        runs,
    };
    scenario.fair_releases();
    scenario.pause_b();
    scenario.stop_c();
    let (d, owner_d) = scenario.resume_and_queue_d();
    scenario.summaries_once(&d, &owner_d);
    model.release();
}

/// Open gates one at a time until every run of `ids` ended.
fn drain(api: &Api, model: &ScriptedModel, ids: &[&String]) {
    while ids
        .iter()
        .any(|id| ["queued", "running"].contains(&api.status(id).as_str()))
    {
        if model.held().is_empty() {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        open_one(model);
    }
}

/// Kill the registered `cyber serve` at once, as a crash does: SIGKILL on Unix,
/// TerminateProcess (`taskkill /F`) on Windows. Neither lets the server pause its runs.
fn kill_service(env: &Env) {
    let pid = env.service_pid().to_string();
    #[cfg(unix)]
    let killed = Command::new("kill").args(["-KILL", &pid]).output().unwrap();
    #[cfg(windows)]
    let killed = Command::new("taskkill")
        .args(["/F", "/PID", &pid])
        .output()
        .unwrap();
    assert!(killed.status.success(), "{killed:?}");
    wait_until("the killed service stops answering", || {
        let registration = env.root.join("cyber-home/state/server.json");
        let url = serde_json::from_str::<Value>(&std::fs::read_to_string(registration).ok()?)
            .ok()?["url"]
            .as_str()?
            .to_owned();
        let address = url.trim_start_matches("http://").trim_end_matches('/');
        std::net::TcpStream::connect(address).is_err().then_some(())
    });
}

/// 11.3's runs: A (`ka1`, then `ka2` and `ka3` together) and B (`kb1`, then the ungated
/// `kb2`) from the CLI, each with agents held at the gate, and the queued Q.
const CRASH_A: &str = "export const meta = { name: 'ka', description: 'One, then two' };\nexport default async () => {\n  await agent('GATE-ka1');\n  const [two, three] = await parallel([() => agent('GATE-ka2'), () => agent('GATE-ka3')]);\n  return [two.text, three.text];\n};\n";
const CRASH_B: &str = "export const meta = { name: 'kb', description: 'Held, then plain' };\nexport default async () => {\n  await agent('GATE-kb1');\n  return (await agent('kb2 plain')).text;\n};\n";
const CRASH_Q: &str = "export const meta = { name: 'kq', description: 'Queued' };\nexport default async () => (await agent('kq plain')).text;\n";

/// Requests whose body names `needle`, and those continuing an interrupted child.
fn requests(model: &ScriptedModel, needle: &str) -> (usize, usize) {
    let bodies: Vec<String> = model
        .requests()
        .iter()
        .map(Value::to_string)
        .filter(|body| body.contains(needle))
        .collect();
    let continued = bodies
        .iter()
        .filter(|body| body.contains("was interrupted"))
        .count();
    (bodies.len(), continued)
}

/// Start A, B and Q, and kill the service with A's `ka2` and `ka3` and B's `kb1` held
/// at the gate (A's `ka1` answered before) and Q queued.
fn crash_mid_flight(env: &Env, model: &ScriptedModel) -> [String; 3] {
    env.write("ka.js", CRASH_A);
    env.write("kb.js", CRASH_B);
    env.write("kq.js", CRASH_Q);
    let a = env.cyber(&["workflows", "run", "ka.js"]).trim().to_owned();
    wait_until("ka1 at the gate", || {
        (model.held() == ["ka1"]).then_some(())
    });
    model.open_gate();
    wait_until("ka2 and ka3 at the gate", || {
        let mut held = model.held();
        held.sort();
        (held == ["ka2", "ka3"]).then_some(())
    });
    let b = env.cyber(&["workflows", "run", "kb.js"]).trim().to_owned();
    wait_until("kb1 at the gate", || {
        (model.held().len() == 3 && model.held()[2] == "kb1").then_some(())
    });
    let q = env.cyber(&["workflows", "run", "kq.js", "--format", "json"]);
    let q: Value = serde_json::from_str(&q).unwrap();
    assert_eq!(q["status"], "queued", "{q}");
    let q = q["id"].as_str().unwrap().to_owned();
    kill_service(env);
    [a, b, q]
}

fn show(env: &Env, id: &str) -> Value {
    serde_json::from_str(&env.cyber(&["workflows", "show", id, "--format", "json"])).unwrap()
}

fn ended_via_cli(env: &Env, id: &str) -> Value {
    wait_until(&format!("{id} ends"), || {
        let run = show(env, id);
        let status = run["status"].as_str().unwrap_or_default().to_owned();
        (!["queued", "running", "waiting_input"].contains(&status.as_str())).then_some(run)
    })
}

/// After the recovery, every run completes with the agents of an uninterrupted run, and
/// each interrupted child continued its own Session once while settled agents made no
/// new model call.
fn completed_as_uninterrupted(env: &Env, model: &ScriptedModel, [a, b, q]: &[String; 3]) {
    for (id, agents) in [(a, 3), (b, 2), (q, 1)] {
        let run = ended_via_cli(env, id);
        assert_eq!(run["status"], "completed", "{run}");
        assert_eq!(run["agents"]["total"], agents, "{run}");
        assert_eq!(run["agents"]["completed"], agents, "{run}");
    }
    assert_eq!(requests(model, "GATE-ka1"), (1, 0), "a settled agent");
    for label in ["GATE-ka2", "GATE-ka3", "GATE-kb1"] {
        assert_eq!(requests(model, label), (2, 1), "{label}");
    }
    assert_eq!(requests(model, "kb2 plain"), (1, 0));
    assert_eq!(requests(model, "kq plain"), (1, 0));
}

/// The runs a crash interrupted are paused with their in-flight agents `interrupted`;
/// the queued run was dispatched by the restart.
fn recovered_paused(env: &Env, [a, b, _]: &[String; 3]) {
    for id in [a, b] {
        let run = show(env, id);
        assert_eq!(run["status"], "paused", "{run}");
        let reason = run["reason"].as_str().unwrap_or_default();
        assert!(reason.contains("restarted"), "{run}");
        let detail = run["agents_detail"].as_array().unwrap();
        assert!(
            detail.iter().any(|agent| agent["status"] == "interrupted"),
            "{run}"
        );
    }
}

/// add-parallel-workflow-runs 11.3 with the real `cyber serve`: the service is killed
/// with A and B mid-flight and Q queued. The restart pauses A and B (`interrupted`) and
/// dispatches Q; `cyber workflows resume --interrupted` resumes both, each interrupted
/// child Session continues once, and every run completes with its full agent count.
#[test]
fn a_killed_server_recovers_its_runs_and_resume_interrupted_completes_them() {
    let model = ScriptedModel::start(reply);
    let env = Env::new(&model, json!({"max_concurrent": 4, "max_running_runs": 2}));
    let _service = Service(&env);
    let runs = crash_mid_flight(&env, &model);
    // The next command starts a new service, whose recovery pauses A and B.
    let restarted = ended_via_cli(&env, &runs[2]);
    assert_eq!(restarted["status"], "completed", "{restarted}");
    recovered_paused(&env, &runs);
    let resumed = env.cyber(&["workflows", "resume", "--interrupted"]);
    let lines: Vec<String> = runs[..2].iter().map(|id| format!("resumed {id}")).collect();
    assert_eq!(resumed.trim(), lines.join("\n"), "{resumed}");
    completed_as_uninterrupted(&env, &model, &runs);
    model.release();
}

/// 11.3 again with `workflows.resume_after_restart: true`: the restart resumes A and B
/// itself, with no manual step.
#[test]
fn a_killed_server_resumes_its_runs_after_restart_when_configured() {
    let model = ScriptedModel::start(reply);
    let workflows =
        json!({"max_concurrent": 4, "max_running_runs": 2, "resume_after_restart": true});
    let env = Env::new(&model, workflows);
    let _service = Service(&env);
    let runs = crash_mid_flight(&env, &model);
    completed_as_uninterrupted(&env, &model, &runs);
    let none = env.cyber(&["workflows", "resume", "--interrupted"]);
    assert!(none.contains("No interrupted Workflow Runs."), "{none}");
    model.release();
}
