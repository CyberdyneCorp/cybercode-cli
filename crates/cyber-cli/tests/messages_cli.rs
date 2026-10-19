//! `cyber messages` and `cyber exec --accept-messages` (`cross-session-messaging` →
//! Non-interactive sessions, Messaging CLI) through the real binary and the background
//! server it starts, with a loopback scripted model. Children end on their own, never by
//! signals, so the tests also run on Windows.

use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    /// A repository whose global default model is `model`.
    fn new(model: &scripted_model::ScriptedModel) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        let config = root.join("cyber-home/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("cyber.jsonc"), model.config().to_string()).unwrap();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
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
            .current_dir(self.root.join("repo"));
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command
    }

    fn cyber(&self, args: &[&str]) -> Output {
        self.command(args).stdin(Stdio::null()).output().unwrap()
    }

    /// `cyber` with `stdin` piped in.
    fn cyber_with_stdin(&self, args: &[&str], stdin: &str) -> Output {
        use std::io::Write;
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// A `cyber exec` run in the background.
    fn exec(&self, args: &[&str]) -> Child {
        self.command(&[&["exec"], args].concat())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cyber(&["service", "stop"]);
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn exits(output: &Output, code: i32) -> String {
    let text = text(output);
    assert_eq!(output.status.code(), Some(code), "{text}");
    text
}

/// A Turn whose prompt says `GATED` waits at the model for an opening; every other request
/// (a message's Turn among them) answers `ok`.
fn reply(body: &Value) -> Value {
    let last = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .map(Value::to_string)
        .unwrap_or_default();
    if last.contains("GATED") {
        json!({"gate": "work"})
    } else {
        json!({"content": "ok"})
    }
}

/// Wait until the scripted model holds `count` gated requests.
fn held(model: &scripted_model::ScriptedModel, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while model.held().len() < count {
        assert!(
            Instant::now() < deadline,
            "the exec run never reached the model"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn finished(child: Child) -> Output {
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    output
}

/// Task 5.4, scenario "Exec session receives a message": a running `cyber exec` (held at
/// a gated model request) refuses a `cyber messages send` with `non_interactive` (exit 1,
/// the reason on stderr); a run with `--accept-messages` gets it `delivered` (exit 0) and
/// reads it; a reused Session keeps its policy with a warning.
#[test]
fn an_exec_session_refuses_messages_unless_it_accepts_them() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);

    let run = env.exec(&["--name", "nightly", "GATED fix the build"]);
    held(&model, 1);
    let refused = env.cyber(&["messages", "send", "nightly", "are you done?"]);
    let stderr = exits(&refused, 1);
    assert!(stderr.contains("non_interactive"), "{stderr}");
    let refused_json = env.cyber(&[
        "messages",
        "send",
        "nightly",
        "are you done?",
        "--format",
        "json",
    ]);
    exits(&refused_json, 1);
    let result: Value = serde_json::from_slice(&refused_json.stdout).unwrap();
    assert_eq!(
        (&result["outcome"], &result["reason"]),
        (&json!("refused"), &json!("non_interactive")),
        "{result}"
    );
    model.open_gate();
    finished(run);
    assert!(
        !model
            .requests()
            .iter()
            .any(|r| r.to_string().contains("are you done?")),
        "a refused message never reaches the model"
    );

    let run = env.exec(&["--accept-messages", "--name", "open", "GATED fix the tests"]);
    held(&model, 1);
    let delivered = env.cyber_with_stdin(
        &["messages", "send", "open", "-"],
        "the API moved to 8080\n",
    );
    let stdout = exits(&delivered, 0);
    assert!(
        stdout.starts_with("msgx_") && stdout.contains(" delivered to open (ses_"),
        "{stdout}"
    );
    model.open_gate();
    finished(run);
    let read = model
        .requests()
        .iter()
        .any(|r| r.to_string().contains("the API moved to 8080"));
    assert!(read, "the accepted message is read by the exec Session");

    // A reused Session keeps its policy and says so.
    let reused = env.cyber(&["exec", "--session", "nightly", "--accept-messages", "go on"]);
    let output = exits(&reused, 0);
    assert!(
        output.contains("--accept-messages cannot change the reused Session"),
        "{output}"
    );
    let still = env.cyber(&["messages", "send", "nightly", "and now?"]);
    assert!(exits(&still, 1).contains("non_interactive"));

    // Regression: a fork is a Session exec creates, so it takes this run's policy rather
    // than its source's: a fork of the accepting `open` refuses, without the warning.
    let run = env.exec(&["--session", "open", "--fork", "GATED fork the tests"]);
    held(&model, 1);
    let forked = env.cyber(&["messages", "send", "open (fork #1)", "still there?"]);
    assert!(exits(&forked, 1).contains("non_interactive"));
    model.open_gate();
    let output = text(&finished(run));
    assert!(!output.contains("keeps its messaging policy"), "{output}");
    // And `--accept-messages` opens a fork of the refusing `nightly`.
    exits(
        &env.cyber(&[
            "exec",
            "--session",
            "nightly",
            "--fork",
            "--accept-messages",
            "go",
        ]),
        0,
    );
    let accepted = env.cyber(&["messages", "send", "nightly (fork #1)", "ready?"]);
    assert!(exits(&accepted, 0).contains(" delivered to "));

    // Usage errors: an unknown address and an oversized text exit 2, a remote one 6.
    exits(&env.cyber(&["messages", "send", "nobody", "hi"]), 2);
    let big = "x".repeat(20 * 1024);
    exits(&env.cyber(&["messages", "send", "open", &big]), 2);
    exits(&env.cyber(&["messages", "send", "api@laptop", "hi"]), 6);
}

/// The scripted model of the cost report: `DELEGATE` starts a background subagent,
/// `SEND_NOW` messages `worker` with `send_message`, and every other request answers `ok`.
fn attribution_reply(body: &Value) -> Value {
    let last = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .map(Value::to_string)
        .unwrap_or_default();
    let call = |id: &str, name: &str, arguments: Value| {
        json!({"tool_calls":[{"index":0,"id":id,"type":"function",
            "function":{"name":name,"arguments":arguments.to_string()}}]})
    };
    if last.contains("DELEGATE") {
        call(
            "call_agent",
            "agent",
            json!({"prompt":"tidy the changelog","description":"Tidy the changelog file","background":true}),
        )
    } else if last.contains("SEND_NOW") {
        call(
            "call_send",
            "send_message",
            json!({"to":"worker","text":"the schema changed"}),
        )
    } else {
        json!({"content": "ok"})
    }
}

/// `cyber stats --by origin --format json`, once every origin in `wanted` has a Turn.
fn origins(env: &Env, wanted: &[&str]) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let output = env.cyber(&["stats", "--by", "origin", "--format", "json"]);
        exits(&output, 0);
        let stats: Value = serde_json::from_slice(&output.stdout).unwrap();
        let origins = stats["origins"].as_array().cloned().unwrap_or_default();
        let has = |name: &str| origins.iter().any(|o| o["origin"] == name);
        if wanted.iter().all(|name| has(name)) {
            return origins;
        }
        assert!(Instant::now() < deadline, "origins never appeared: {stats}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Task 9.3, scenario "Cost report": a user prompt, a message sent by another Session's
/// `send_message`, a message sent with `cyber messages send` and a background Job handback
/// each spend one scripted Turn, and `cyber stats --by origin` lists `user`, `session`,
/// `client` and `job` separately with their tokens (every scripted step reports 100 input
/// and 10 output tokens).
#[test]
fn stats_separate_message_driven_usage() {
    let model = scripted_model::ScriptedModel::start(attribution_reply);
    let env = Env::new(&model);
    let mut config = model.config();
    config["permissions"] = json!({"agent": "allow"});
    std::fs::write(
        env.root.join("cyber-home/config/cyber.jsonc"),
        config.to_string(),
    )
    .unwrap();

    // The user's prompt starts a background subagent whose handback starts a Job Turn.
    let worker = env.exec(&["--accept-messages", "--name", "worker", "DELEGATE now"]);
    finished(worker);
    origins(&env, &["user", "job"]);
    // Another Session's agent messages the worker, which wakes to read it.
    finished(env.exec(&["--name", "sender", "SEND_NOW please"]));
    origins(&env, &["session"]);
    // A client message from the CLI.
    exits(
        &env.cyber(&["messages", "send", "worker", "deploy at noon"]),
        0,
    );
    let origins = origins(&env, &["client"]);

    let entry = |name: &str| {
        origins
            .iter()
            .find(|o| o["origin"] == name)
            .unwrap_or_else(|| panic!("no {name} in {origins:?}"))
            .clone()
    };
    for name in ["session", "client", "job"] {
        let o = entry(name);
        assert_eq!(o["turns"], 1, "{o}");
        assert_eq!(
            (
                &o["tokens"]["input"],
                &o["tokens"]["output"],
                &o["total_tokens"]
            ),
            (&json!(100), &json!(10), &json!(110)),
            "{o}"
        );
    }
    let user = entry("user");
    assert!(user["turns"].as_u64().unwrap() >= 4, "{user}");
    let requests = model.requests();
    let read = |text: &str| requests.iter().any(|r| r.to_string().contains(text));
    assert!(read("the schema changed") && read("deploy at noon"));
    assert!(read("Background subagent handback"));

    let table = env.cyber(&["stats", "--by", "origin"]);
    let table = exits(&table, 0);
    for name in ["Origin", "user", "session", "client", "job"] {
        assert!(
            table.lines().any(|line| line.starts_with(name)),
            "{name}: {table}"
        );
    }
}

/// `cyber messages list --format json` of `args`, as an array.
fn listed(env: &Env, args: &[&str]) -> Vec<Value> {
    let output = env.cyber(&[&["messages", "list"], args, &["--format", "json"]].concat());
    exits(&output, 0);
    serde_json::from_slice::<Value>(&output.stdout)
        .unwrap()
        .as_array()
        .cloned()
        .unwrap()
}

/// The ID of the Location's most recently updated Session.
fn newest(env: &Env) -> Value {
    let output = env.cyber(&["sessions", "list", "--limit", "1", "--format", "json"]);
    exits(&output, 0);
    serde_json::from_slice::<Value>(&output.stdout).unwrap()[0]["id"].clone()
}

/// Wait until `session`'s history holds a record whose text is `text`.
fn recorded(env: &Env, session: &str, text: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let records = listed(env, &["--session", session]);
        if let Some(record) = records.iter().find(|r| r["text"] == text) {
            return record.clone();
        }
        assert!(
            Instant::now() < deadline,
            "{text} never recorded: {records:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Tasks 11.1 and 11.2 through the real binary and server: `cyber messages send` from
/// stdin and `--from` (with `--notify-when-idle`), an ambiguous name (exit 2 with the
/// candidates), an oversized text (exit 2), `cyber messages list` as a history table and
/// JSON (with `--direction`, `--limit` and the Location's latest Session by default),
/// `--reachable`, and `cyber peers` as an unavailable capability.
#[test]
fn messages_are_sent_and_listed_from_the_cli() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    for name in ["alpha", "beta", "twin", "twin"] {
        finished(env.exec(&["--accept-messages", "--name", name, "hello"]));
    }

    sends(&env);
    history(&env);
    reachable(&env);

    // Direct peers are not here yet.
    let peers = exits(&env.cyber(&["peers", "list"]), 2);
    assert!(peers.contains("add-messaging-direct-peers"), "{peers}");
}

/// `cyber messages send` from stdin, with `--from` and `--notify-when-idle`, to an
/// ambiguous name and with an oversized text.
fn sends(env: &Env) {
    // From stdin, as the `client:cli` client; alpha wakes to read it.
    let sent = env.cyber_with_stdin(
        &["messages", "send", "alpha", "-"],
        "the API moved to 8080\nport details follow\n",
    );
    let stdout = exits(&sent, 0);
    assert!(stdout.contains(" delivered to alpha (ses_"), "{stdout}");
    let received = recorded(env, "alpha", "the API moved to 8080\nport details follow");
    assert_eq!(
        (
            &received["direction"],
            &received["from"],
            &received["origin_kind"]
        ),
        (&json!("in"), &json!("client:cli"), &json!("client")),
        "{received}"
    );

    // As beta, which is notified once alpha goes idle again.
    let as_beta = env.cyber(&[
        "messages",
        "send",
        "alpha",
        "rebase on main",
        "--from",
        "beta",
        "--notify-when-idle",
    ]);
    let stdout = exits(&as_beta, 0);
    assert!(
        stdout.contains("notifies the sender when the target goes idle"),
        "{stdout}"
    );
    let out = recorded(env, "beta", "rebase on main");
    assert_eq!(
        (&out["direction"], &out["to"]),
        (&json!("out"), &json!("alpha")),
        "{out}"
    );
    // `--notify-when-idle` needs a sending Session.
    exits(
        &env.cyber(&["messages", "send", "alpha", "hi", "--notify-when-idle"]),
        2,
    );

    // An ambiguous name lists its candidates; an oversized text is a usage error.
    let ambiguous = exits(&env.cyber(&["messages", "send", "twin", "hi"]), 2);
    let twins = listed(env, &["--reachable"])
        .into_iter()
        .filter(|s| s["name"] == "twin")
        .collect::<Vec<_>>();
    assert_eq!(twins.len(), 2, "{twins:?}");
    for twin in &twins {
        let id = twin["session_id"].as_str().unwrap();
        assert!(ambiguous.contains(id), "{ambiguous}");
        assert_eq!(
            twin["address"], twin["session_id"],
            "a shared name is shown by ID"
        );
    }
    let big = "x".repeat(16 * 1024 + 1);
    assert!(exits(&env.cyber(&["messages", "send", "alpha", &big]), 2).contains("16384"));
}

/// `cyber messages list` as a table and as JSON, with its filters and default Session.
fn history(env: &Env) {
    // The history table: time, direction, peer, outcome, size, first line.
    let table = exits(&env.cyber(&["messages", "list", "--session", "alpha"]), 0);
    let line = table
        .lines()
        .find(|l| l.contains("the API moved to 8080"))
        .unwrap_or_else(|| panic!("{table}"));
    assert!(
        line.contains("  in   client:cli")
            && line.contains("delivered")
            && line.contains("    41  "),
        "{line}"
    );
    assert!(
        !table.contains("port details follow"),
        "only the first line: {table}"
    );
    assert!(
        table
            .lines()
            .any(|l| l.contains("rebase on main") && l.contains("  in "))
    );
    // Filters: only sent records, one at a time.
    let sent = listed(
        env,
        &["--session", "beta", "--direction", "out", "--limit", "1"],
    );
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(listed(env, &["--session", "beta", "--direction", "in"]).is_empty());
    // Without --session: the Location's most recently updated Session (read between two
    // equal answers of `cyber sessions list`, since beta's watch notice may still move it).
    let (newest, latest) = loop {
        let before = newest(env);
        let latest = listed(env, &[]);
        if newest(env) == before {
            break (before, latest);
        }
    };
    assert!(!latest.is_empty());
    assert!(
        latest.iter().all(|r| r["session_id"] == newest),
        "{latest:?}"
    );
    // An unknown Session fails; a bad direction is a usage error.
    exits(&env.cyber(&["messages", "list", "--session", "nobody"]), 1);
    exits(
        &env.cyber(&["messages", "list", "--direction", "sideways"]),
        2,
    );
}

/// `cyber messages list --reachable`, as a client and as a Session.
fn reachable(env: &Env) {
    // The reachable listing, as beta: alpha but not beta itself.
    let reachable = listed(env, &["--reachable", "--from", "beta"]);
    assert!(
        reachable
            .iter()
            .any(|s| s["address"] == "alpha" && s["canReceive"] == true)
    );
    assert!(
        !reachable.iter().any(|s| s["name"] == "beta"),
        "{reachable:?}"
    );
    let table = exits(&env.cyber(&["messages", "list", "--reachable"]), 0);
    assert!(
        table
            .lines()
            .any(|l| l.starts_with("alpha ") && l.contains("open")),
        "{table}"
    );
}
