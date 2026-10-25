//! OpenTelemetry export on the production server (`observability-costs` → OpenTelemetry
//! configuration, OpenTelemetry export, Telemetry privacy): `run_server_until` over the
//! production App, the local scripted OpenAI-compatible provider and an in-process OTLP
//! HTTP collector. Nothing here reaches the network. Counters are exported when the
//! server stops, so each test stops its server before it reads the collector.

#[path = "support/otlp_receiver.rs"]
mod otlp_receiver;
#[path = "support/scripted_model.rs"]
mod scripted_model;
#[path = "support/mode_app.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use cyber_app::ServeOptions;
use otlp_receiver::{Receiver, Silent, attribute, is_child, log_attributes, log_body};
use serde_json::{Value, json};
use support::{ModeApp, Setup};
use tokio::sync::Notify;

/// Structured logs of this test process, written under one temporary directory.
fn logs() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    let dir = DIR.get_or_init(|| tempfile::tempdir().unwrap()).path();
    cyber_core::log::init(dir, cyber_core::log::Level::Info, false);
    dir
}

/// The log lines of this process containing `text`.
fn logged(text: &str) -> Vec<String> {
    std::fs::read_dir(logs())
        .unwrap()
        .flatten()
        .filter_map(|file| std::fs::read_to_string(file.path()).ok())
        .flat_map(|log| log.lines().map(str::to_string).collect::<Vec<_>>())
        .filter(|line| line.contains(text))
        .collect()
}

/// The global configuration that exports to `receiver`, merged with `extra`'s
/// `telemetry` section and its other keys.
fn exporting(receiver: &Receiver, extra: Value) -> Value {
    exporting_to(&receiver.url, extra)
}

fn exporting_to(endpoint: &str, extra: Value) -> Value {
    let mut config = extra;
    let telemetry = config["telemetry"].take();
    let mut otel = json!({"enabled": true, "endpoint": endpoint});
    if let Some(more) = telemetry["otel"].as_object() {
        otel.as_object_mut().unwrap().extend(more.clone());
    }
    let mut section = telemetry.as_object().cloned().unwrap_or_default();
    section.insert("otel".into(), otel);
    config["telemetry"] = Value::Object(section);
    config
}

/// How a test server is set up.
struct Options {
    config: Value,
    reply: scripted_model::Reply,
    /// A database file shared with a later server (a restart).
    database: Option<PathBuf>,
    /// An account signed in before the server starts.
    tokens: Option<cyber_account::TokenSet>,
    /// The scripted provider's models with their prices.
    models: Option<Value>,
    /// The root of an earlier server, whose checkout its Sessions are in (a restart).
    temp: Option<tempfile::TempDir>,
    /// Permission asks wait for a reply instead of being denied.
    interactive: bool,
    /// How long the built App runs before the server starts: the restart recovery it
    /// began has settled by then.
    serve_after: Duration,
}

impl Options {
    fn new(config: Value) -> Self {
        Self {
            config,
            reply: support::prompted_call,
            database: None,
            tokens: None,
            models: None,
            temp: None,
            interactive: false,
            serve_after: Duration::ZERO,
        }
    }
}

/// The production server on a temporary root, stopped by `stop`.
struct Served {
    temp: tempfile::TempDir,
    repo: PathBuf,
    base: String,
    runtime: cyber_server::runtime::Runtime,
    store: Arc<cyber_store::Store>,
    stop: Arc<Notify>,
    task: tokio::task::JoinHandle<Result<(), String>>,
    model: scripted_model::ScriptedModel,
}

impl Served {
    async fn start(config: Value) -> Self {
        Self::open(Options::new(config)).await
    }

    async fn open(options: Options) -> Self {
        logs();
        let temp = options.temp.unwrap_or_else(|| tempfile::tempdir().unwrap());
        let root = temp.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git/hooks")).unwrap();
        let model = scripted_model::ScriptedModel::start(options.reply);
        let setup = Setup {
            config: options.config,
            sandbox_policy: support::BASH_SANDBOX,
            database: options.database,
            models: options.models,
            interactive: options.interactive,
            served: true,
            ..Setup::default()
        };
        let app = ModeApp::build(&root, &repo, &model, &setup).await;
        tokio::time::sleep(options.serve_after).await;
        let paths = app.paths.clone();
        if let Some(tokens) = options.tokens {
            // The file store, which the production account store reads without a vault.
            cyber_account::AccountStore::with_vault(paths.data.clone(), None)
                .save(tokens)
                .await
                .unwrap();
        }
        let runtime = app.runtime.clone();
        let store = Arc::clone(&app.store);
        let stop = Arc::new(Notify::new());
        let serve = ServeOptions {
            port: Some(0),
            socket: cfg!(unix).then(|| root.join("s.sock")),
            register: true,
            ..ServeOptions::default()
        };
        let task = tokio::spawn(cyber_app::run_server_until(
            app,
            serve,
            |_| {},
            Arc::clone(&stop),
        ));
        let registration = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(registration) = cyber_app::read_registration(&paths) {
                    break registration;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the server registers");
        Self {
            temp,
            repo,
            base: registration.url,
            runtime,
            store,
            stop,
            task,
            model,
        }
    }

    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut request = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .request(method, format!("{}/api/v1{path}", self.base))
            .basic_auth("cyber", Some(support::PASSWORD))
            .header("x-cyber-directory", self.repo.display().to_string());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn session(&self) -> String {
        let (status, created) = self
            .send("POST", "/sessions", Some(json!({"mode": "default"})))
            .await;
        assert_eq!(status, 201, "{created}");
        created["data"]["id"].as_str().unwrap().to_string()
    }

    /// Prompt Session `id` and wait for its Drain to settle.
    async fn run(&self, id: &str, text: &str) {
        self.prompt(id, text).await;
        tokio::time::timeout(Duration::from_secs(60), self.runtime.wait_idle(id))
            .await
            .expect("the Drain settles");
    }

    async fn prompt(&self, id: &str, text: &str) {
        let (status, body) = self
            .send(
                "POST",
                &format!("/sessions/{id}/prompt"),
                Some(json!({"parts": [{"type": "text", "text": text}], "delivery": "queue"})),
            )
            .await;
        assert_eq!(status, 202, "{body}");
    }

    /// The durable events of Session `id`.
    fn events(&self, id: &str) -> Vec<cyber_store::StoredEvent> {
        self.store.read_events(id, -1, 500).unwrap().events
    }

    /// Stop the server; its root is returned for a restart.
    async fn stop(self) -> tempfile::TempDir {
        self.stop.notify_one();
        tokio::time::timeout(Duration::from_secs(20), self.task)
            .await
            .expect("the server stops")
            .unwrap()
            .unwrap();
        self.temp
    }
}

/// Invalid settings keep export off with the reason in the status route and one `warn`
/// line, and the server still starts and runs Turns. Header values are never logged.
#[tokio::test(flavor = "multi_thread")]
async fn invalid_settings_keep_export_off_and_server_serves() {
    let served = Served::start(json!({"telemetry": {"otel": {
        "enabled": true,
        "endpoint": "http://collector.lan:4318",
        "headers": {"authorization": "Bearer s3cr3t-t0ken"},
    }}}))
    .await;

    let (status, body) = served.send("GET", "/telemetry", None).await;
    assert_eq!(status, 200, "{body}");
    let telemetry = &body["data"];
    assert_eq!(telemetry["enabled"], false, "{telemetry}");
    let reason = telemetry["reason"].as_str().unwrap();
    assert!(
        reason.contains("headers need an https or loopback endpoint"),
        "{reason}"
    );
    assert!(!body.to_string().contains("s3cr3t"), "{body}");
    assert_eq!(telemetry["restart_required"], false);
    let warned = logged("telemetry export off");
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(warned[0].contains("\"level\":\"warn\""), "{warned:?}");
    assert!(warned[0].contains("headers need an https"), "{warned:?}");
    assert!(
        logged("s3cr3t").is_empty(),
        "header values are never logged"
    );

    let id = served.session().await;
    served.run(&id, "hello").await;
    let (status, messages) = served
        .send("GET", &format!("/sessions/{id}/messages"), None)
        .await;
    assert_eq!(status, 200, "{messages}");
    assert!(messages.to_string().contains("\"ok\""), "{messages}");
    served.stop().await;
}

/// `observability-costs` → "Subagent under its Turn": a Turn under its session span, the
/// `agent` tool call under the Turn, and the subagent's session span under the parent's
/// Turn, all in one trace.
#[tokio::test(flavor = "multi_thread")]
async fn turn_tool_and_subagent_spans_are_parented() {
    let receiver = Receiver::start().await;
    let served = Served::start(exporting(
        &receiver,
        json!({"permissions": {"agent": "allow"}}),
    ))
    .await;
    let id = served.session().await;
    // The child runs one tool call of its own.
    let call = json!({"prompt": "TOOL<<list_sessions>>{}", "description": "Run the child task"});
    served.run(&id, &format!("TOOL<<agent>>{call}")).await;
    served.stop().await;

    let kind = |span: &_| attribute(span, "cyber.session.kind").unwrap_or_default();
    let of = |span: &_| attribute(span, "cyber.session.id").unwrap_or_default();
    let sessions = receiver.named("cyber.session");
    let turns = receiver.named("cyber.turn");
    let root = sessions
        .iter()
        .find(|s| of(s) == id)
        .expect("root session span");
    let turn = turns.iter().find(|t| of(t) == id).expect("root Turn span");
    let tool = receiver
        .named("cyber.tool")
        .into_iter()
        .find(|t| attribute(t, "cyber.tool.name").as_deref() == Some("agent"))
        .expect("agent tool span");
    let child = sessions
        .iter()
        .find(|s| kind(s) == "subagent")
        .expect("subagent session span");
    let child_turn = turns.iter().find(|t| of(t) == of(child)).unwrap();
    assert!(
        root.parent_span_id.is_empty(),
        "a root Session starts a trace"
    );
    assert!(is_child(turn, root));
    assert!(is_child(&tool, turn));
    assert!(
        is_child(child, turn),
        "the subagent is under its parent's Turn"
    );
    assert!(is_child(child_turn, child));
    let child_tool = receiver
        .named("cyber.tool")
        .into_iter()
        .find(|t| of(t) == of(child))
        .expect("the child's tool span");
    assert!(is_child(&child_tool, child_turn));
    assert_eq!(child_tool.trace_id, root.trace_id, "one trace");
    assert_eq!(
        attribute(&tool, "cyber.tool.outcome").as_deref(),
        Some("ok")
    );
    for span in receiver.spans() {
        assert_eq!(attribute(&span, "cyber.interrupted"), None, "{}", span.name);
    }
}

const SCRIPT: &str = r#"export const meta = { name: 'solo', description: 'One agent' };
export default async function () {
  phase('Only');
  const result = await agent('alpha task', { label: 'alpha' });
  return { text: result.text };
}
"#;

/// `observability-costs` → "Traces for a workflow": the run span is the root of its own
/// trace, the agent span is under it and the agent's child Session under the agent.
#[tokio::test(flavor = "multi_thread")]
async fn workflow_run_is_the_root_of_its_trace() {
    let receiver = Receiver::start().await;
    let served = Served::start(exporting(&receiver, json!({}))).await;
    let owner = served.session().await;
    let (status, started) = served
        .send(
            "POST",
            "/workflows/runs",
            Some(json!({"session_id": owner, "script": SCRIPT})),
        )
        .await;
    assert!(status < 300, "{started}");
    let run_id = started["data"]["id"].as_str().unwrap().to_string();
    let mut settled = Value::Null;
    for _ in 0..600 {
        let (_, run) = served
            .send("GET", &format!("/workflows/runs/{run_id}"), None)
            .await;
        if run["data"]["status"] != "running" && run["data"]["status"] != "queued" {
            settled = run;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(settled["data"]["status"], "completed", "{settled}");
    served.stop().await;

    let run = receiver
        .named("cyber.workflow.run")
        .pop()
        .expect("run span");
    let agent = receiver
        .named("cyber.workflow.agent")
        .pop()
        .expect("agent span");
    let child = receiver
        .named("cyber.session")
        .into_iter()
        .find(|s| attribute(s, "cyber.session.kind").as_deref() == Some("workflow_agent"))
        .expect("the agent's session span");
    assert!(run.parent_span_id.is_empty(), "the run is a trace root");
    assert_eq!(attribute(&run, "cyber.workflow.run_id"), Some(run_id));
    assert_eq!(
        attribute(&run, "cyber.workflow.name").as_deref(),
        Some("solo")
    );
    assert_eq!(
        attribute(&run, "cyber.workflow.status").as_deref(),
        Some("completed")
    );
    assert!(is_child(&agent, &run));
    assert!(is_child(&child, &agent));
    assert_eq!(
        attribute(&agent, "cyber.workflow.phase").as_deref(),
        Some("Only")
    );
}

/// `observability-costs` → "Metrics for a Turn": Turn, token, cost and tool counters
/// with bounded attributes and no ids.
#[tokio::test(flavor = "multi_thread")]
async fn turn_metrics_are_exported() {
    let receiver = Receiver::start().await;
    let served = Served::open(Options {
        models: Some(json!({"main": {"cost": {"input": 10000.0, "output": 0.0}}})),
        ..Options::new(exporting(&receiver, json!({})))
    })
    .await;
    let id = served.session().await;
    // A `read` of a file that does not exist: the call fails.
    served
        .run(&id, "TOOL<<read>>{\"path\": \"missing.txt\"}")
        .await;
    served.stop().await;

    assert_eq!(
        receiver.total("cyber.session.count", &[("cyber.session.kind", "root")]),
        1.0
    );
    assert_eq!(
        receiver.total("cyber.turn.count", &[("outcome", "completed")]),
        1.0
    );
    assert_eq!(
        receiver.total(
            "cyber.tool.calls",
            &[("cyber.tool.name", "read"), ("outcome", "error")]
        ),
        1.0,
        "{:?}",
        receiver.points()
    );
    assert_eq!(
        receiver.total("cyber.tool.errors", &[("cyber.tool.name", "read")]),
        1.0
    );
    for class in ["input", "output"] {
        assert!(
            receiver.total("cyber.token.usage", &[("type", class)]) > 0.0,
            "{class}"
        );
    }
    assert!(
        receiver.total("cyber.token.usage", &[("cyber.provider", "test")]) > 0.0,
        "{:?}",
        receiver.points()
    );
    assert!(
        receiver.total("cyber.cost.usage", &[("cyber.model", "main")]) > 0.0,
        "{:?}",
        receiver.points()
    );
    assert!(receiver.total("cyber.active_time", &[]) >= 0.0);
    for point in receiver.points() {
        for (key, value) in &point.attributes {
            assert!(!key.ends_with(".id"), "{point:?}");
            assert!(!value.contains(&id), "{point:?}");
        }
    }
}

/// `observability-costs` → "Default omits content": no prompt, output, tool input or tool
/// output text anywhere in the exported payloads.
#[tokio::test(flavor = "multi_thread")]
async fn default_export_has_no_content() {
    let receiver = Receiver::start().await;
    let served = Served::start(exporting(&receiver, json!({}))).await;
    std::fs::write(served.repo.join("notes.txt"), "private-file-contents-7731").unwrap();
    let id = served.session().await;
    served
        .run(
            &id,
            "PRIVATE-PROMPT-4412 TOOL<<read>>{\"path\": \"notes.txt\"}",
        )
        .await;
    served.stop().await;

    let tool = receiver.named("cyber.tool").pop().expect("tool span");
    assert_eq!(attribute(&tool, "cyber.tool.name").as_deref(), Some("read"));
    let raw = receiver.raw();
    for content in [
        "PRIVATE-PROMPT-4412",
        "private-file-contents-7731",
        "notes.txt",
    ] {
        assert!(!raw.contains(content), "{content} was exported");
    }
    for span in receiver.spans() {
        for key in [
            "cyber.prompt",
            "cyber.response",
            "cyber.tool.input",
            "cyber.tool.output",
        ] {
            assert_eq!(attribute(&span, key), None, "{key} on {}", span.name);
        }
    }
}

/// `observability-costs` → "Prompt logging redacts secrets": with `log_prompts` the
/// content is exported, credentials and the configured header value become `***`, and
/// the header itself still reaches the collector.
#[tokio::test(flavor = "multi_thread")]
async fn log_prompts_redacts_secrets() {
    let receiver = Receiver::start().await;
    let header = "hdr-secret-5521";
    let served = Served::start(exporting(
        &receiver,
        json!({"telemetry": {"log_prompts": true, "otel": {"headers": {"x-collector-key": header}}}}),
    ))
    .await;
    let id = served.session().await;
    let prompt = format!("deploy with ghp_abcdefghijklmnop and sk-abcdefghijklmnop and {header}");
    served.run(&id, &prompt).await;
    cyber_core::log::warn(
        "probe",
        &format!("Could not truncate /work/out.log: {header}"),
        json!({}),
    );
    served.stop().await;

    let turn = receiver.named("cyber.turn").pop().expect("Turn span");
    assert_eq!(
        attribute(&turn, "cyber.prompt").as_deref(),
        Some("deploy with *** and *** and ***")
    );
    assert_eq!(attribute(&turn, "cyber.response").as_deref(), Some("ok"));
    // The message of a log record is content: exported, redacted, with `log_prompts`.
    assert!(
        receiver
            .log_records()
            .iter()
            .any(|record| log_body(record) == "Could not truncate /work/out.log: ***"),
        "the probe line is exported redacted"
    );
    let raw = receiver.raw();
    for secret in ["ghp_abcdefghijklmnop", "sk-abcdefghijklmnop", header] {
        assert!(!raw.contains(secret), "{secret} was exported");
    }
    assert!(
        receiver
            .header("x-collector-key")
            .iter()
            .all(|v| v == header)
    );
    assert!(!receiver.header("x-collector-key").is_empty());
}

/// Hook spans carry the outcome and decision of their receipt under the tool call, and
/// hook IO never reaches telemetry, even with `log_hook_io` and `log_prompts` on.
#[tokio::test(flavor = "multi_thread")]
async fn hook_spans_carry_outcome_without_io() {
    let receiver = Receiver::start().await;
    let served = Served::start(exporting(
        &receiver,
        json!({
            "hooks": {"PreToolUse": [{"hooks": [
                {"type": "command", "command": "echo hook-io-9917", "id": "guard"},
            ]}]},
            "telemetry": {"log_hook_io": true, "log_prompts": true},
        }),
    ))
    .await;
    std::fs::write(served.repo.join("notes.txt"), "notes").unwrap();
    let id = served.session().await;
    served
        .run(&id, "TOOL<<read>>{\"path\": \"notes.txt\"}")
        .await;
    served.stop().await;

    let hook = receiver.named("cyber.hook").pop().expect("hook span");
    let tool = receiver.named("cyber.tool").pop().expect("tool span");
    assert!(is_child(&hook, &tool), "the hook is under its tool call");
    assert_eq!(attribute(&hook, "cyber.hook.id").as_deref(), Some("guard"));
    assert_eq!(
        attribute(&hook, "cyber.hook.event").as_deref(),
        Some("PreToolUse")
    );
    assert_eq!(
        attribute(&hook, "cyber.hook.outcome").as_deref(),
        Some("ok")
    );
    assert_eq!(
        attribute(&hook, "cyber.hook.scope").as_deref(),
        Some("global")
    );
    assert!(hook.end_time_unix_nano >= hook.start_time_unix_nano);
    assert!(
        !receiver.raw().contains("hook-io-9917"),
        "hook IO was exported"
    );
}

/// The model's answer: a held request for a prompt that says `HOLD`.
fn holding(body: &Value) -> Value {
    if body.to_string().contains("HOLD") {
        return json!({"hold": true});
    }
    support::prompted_call(body)
}

/// `observability-costs` → "Restart does not resurrect spans": the Turn a stop cuts off
/// ends as interrupted, and the next server over the same database starts new spans for
/// a Session it never saw created (read through the lookup), in new traces.
#[tokio::test(flavor = "multi_thread")]
async fn restart_does_not_resurrect_spans() {
    let database = tempfile::tempdir().unwrap();
    let file = database.path().join("events.db");
    let first = Receiver::start().await;
    let served = Served::open(Options {
        reply: holding,
        database: Some(file.clone()),
        ..Options::new(exporting(&first, json!({})))
    })
    .await;
    let id = served.session().await;
    served.run(&id, "hello").await;
    let cut_id = served.session().await;
    served.prompt(&cut_id, "HOLD the line").await;
    // The step has started once its assistant message exists; its request is held.
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let path = format!("/sessions/{cut_id}/messages");
            let (_, messages) = served.send("GET", &path, None).await;
            if messages.to_string().contains("\"kind\":\"assistant\"") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the Turn's step starts");
    served.model.release();
    let temp = served.stop().await;
    let of = |span: &_| attribute(span, "cyber.session.id").unwrap_or_default();
    let cut = first
        .named("cyber.turn")
        .into_iter()
        .find(|turn| of(turn) == cut_id)
        .expect("the cut-off Turn is exported");
    assert_eq!(
        attribute(&cut, "cyber.interrupted").as_deref(),
        Some("true")
    );
    assert_eq!(
        first.total("cyber.turn.count", &[("outcome", "interrupted")]),
        1.0
    );

    let second = Receiver::start().await;
    let served = Served::open(Options {
        database: Some(file),
        temp: Some(temp),
        ..Options::new(exporting(&second, json!({})))
    })
    .await;
    served.run(&id, "hello again").await;
    served.stop().await;
    let old: Vec<_> = first.spans().into_iter().map(|s| s.trace_id).collect();
    let session = second
        .named("cyber.session")
        .into_iter()
        .find(|s| of(s) == id)
        .expect("a new session span");
    assert_eq!(
        attribute(&session, "cyber.session.kind").as_deref(),
        Some("root")
    );
    let turns = second.named("cyber.turn");
    assert!(turns.iter().any(|turn| is_child(turn, &session)));
    for span in second.spans() {
        assert!(
            !old.contains(&span.trace_id),
            "{} joined an old trace",
            span.name
        );
        assert_ne!(of(&span), cut_id, "the cut-off Turn came back");
        assert_eq!(attribute(&span, "cyber.interrupted"), None, "{}", span.name);
    }
    // Created before the restart: not counted again.
    assert_eq!(second.total("cyber.session.count", &[]), 0.0);
}

/// `observability-costs` → "Restart does not resurrect spans", for a tool call: the stop
/// cuts off a `read` waiting for its permission. The first server exports its tool span
/// as interrupted, the restarted one, which settles nothing the first left open, exports
/// no tool span for it, and the call is counted once over both.
#[tokio::test(flavor = "multi_thread")]
async fn restart_counts_a_cut_off_tool_call_once() {
    let database = tempfile::tempdir().unwrap();
    let file = database.path().join("events.db");
    let config = |receiver| exporting(receiver, json!({"permissions": {"read": "ask"}}));
    let first = Receiver::start().await;
    let served = Served::open(Options {
        database: Some(file.clone()),
        interactive: true,
        ..Options::new(config(&first))
    })
    .await;
    std::fs::write(served.repo.join("notes.txt"), "notes").unwrap();
    let id = served.session().await;
    served
        .prompt(&id, "TOOL<<read>>{\"path\": \"notes.txt\"}")
        .await;
    tokio::time::timeout(Duration::from_secs(20), async {
        while served.runtime.pending_requests(Some(&id)).is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the read asks");
    let call_id = served
        .events(&id)
        .into_iter()
        .find(|event| event.kind == "session.tool.called.1")
        .and_then(|event| event.data["call_id"].as_str().map(str::to_string))
        .expect("the call");
    let temp = served.stop().await;
    let of_call = |span: &_| attribute(span, "cyber.call.id").as_deref() == Some(&call_id);
    let tool = first
        .named("cyber.tool")
        .into_iter()
        .find(|span| of_call(span))
        .expect("the cut-off call's span");
    assert_eq!(
        attribute(&tool, "cyber.interrupted").as_deref(),
        Some("true")
    );

    let second = Receiver::start().await;
    let served = Served::open(Options {
        database: Some(file),
        temp: Some(temp),
        interactive: true,
        ..Options::new(config(&second))
    })
    .await;
    served.run(&id, "hello again").await;
    let settled: Vec<_> = served
        .events(&id)
        .into_iter()
        .filter(|event| event.kind == "session.tool.settled.1")
        .map(|event| event.data)
        .collect();
    assert_eq!(settled.len(), 1, "settled once: {settled:?}");
    assert_eq!(settled[0]["status"], "interrupted");
    served.stop().await;
    assert!(second.named("cyber.tool").iter().all(|span| !of_call(span)));
    let calls = first.total("cyber.tool.calls", &[]) + second.total("cyber.tool.calls", &[]);
    assert_eq!(calls, 1.0, "{:?} {:?}", first.points(), second.points());
}

/// The model's answer for a held workflow agent: held until a restart continues it.
fn recovery_reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("was interrupted") {
        return json!({"content": "continued"});
    }
    if text.contains("HOLD-") {
        return json!({"hold": true});
    }
    support::prompted_call(body)
}

/// Workflow run `id` once it is no longer running or queued.
async fn ended_run(served: &Served, id: &str) -> Value {
    for _ in 0..600 {
        let (_, run) = served
            .send("GET", &format!("/workflows/runs/{id}"), None)
            .await;
        let status = &run["data"]["status"];
        if status != "running" && status != "queued" {
            return run["data"].clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("run {id} did not end");
}

const HELD: &str = "export const meta = { name: 'keep', description: 'Held' };
export default async () => (await agent('HOLD-keep')).text;
";

/// `observability-costs` → OpenTelemetry export after a restart: the work that the
/// restart recovery of `App::build` resumes (`workflows.resume_after_restart`) is
/// exported even when it settles before the server starts, because the exporter
/// subscribed before the recovery.
#[tokio::test(flavor = "multi_thread")]
async fn work_resumed_by_restart_recovery_is_exported() {
    let database = tempfile::tempdir().unwrap();
    let file = database.path().join("events.db");
    let config = |receiver| {
        exporting(
            receiver,
            json!({"workflows": {"resume_after_restart": true}}),
        )
    };
    let first = Receiver::start().await;
    let served = Served::open(Options {
        reply: recovery_reply,
        database: Some(file.clone()),
        ..Options::new(config(&first))
    })
    .await;
    let saved = served.temp.path().join("config").join("workflows");
    std::fs::create_dir_all(&saved).unwrap();
    std::fs::write(saved.join("keep.js"), HELD).unwrap();
    let owner = served.session().await;
    let (status, started) = served
        .send(
            "POST",
            "/workflows/runs",
            Some(json!({"session_id": owner, "name": "keep"})),
        )
        .await;
    assert!(status < 300, "{started}");
    let run_id = started["data"]["id"].as_str().unwrap().to_string();
    let at_model = || {
        served
            .model
            .requests()
            .iter()
            .any(|request| request.to_string().contains("HOLD-keep"))
    };
    tokio::time::timeout(Duration::from_secs(20), async {
        while !at_model() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the agent is at the model");
    let temp = served.stop().await;

    let second = Receiver::start().await;
    let served = Served::open(Options {
        reply: recovery_reply,
        database: Some(file),
        temp: Some(temp),
        serve_after: Duration::from_secs(3),
        ..Options::new(config(&second))
    })
    .await;
    let run = ended_run(&served, &run_id).await;
    assert_eq!(run["status"], "completed", "{run}");
    served.stop().await;
    assert_eq!(
        second.total("cyber.turn.count", &[("outcome", "completed")]),
        1.0,
        "the resumed agent's Turn: {:?}",
        second.points()
    );
    assert!(second.total("cyber.token.usage", &[("type", "input")]) > 0.0);
    let kind = |span: &_| attribute(span, "cyber.session.kind");
    assert!(
        second
            .named("cyber.turn")
            .iter()
            .any(|turn| kind(turn).as_deref() == Some("workflow_agent")),
        "the resumed Turn's span"
    );
}

/// The resource of every export: service, version, build and OS.
#[tokio::test(flavor = "multi_thread")]
async fn resource_has_service_and_os() {
    let receiver = Receiver::start().await;
    let served = Served::start(exporting(&receiver, json!({}))).await;
    let id = served.session().await;
    served.run(&id, "hello").await;
    let (_, status) = served.send("GET", "/telemetry", None).await;
    assert_eq!(status["data"]["enabled"], true, "{status}");
    assert_eq!(status["data"]["resource"]["account_signed_in"], false);
    served.stop().await;
    let resources = receiver.resources();
    assert!(!resources.is_empty());
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    for resource in resources {
        let get = |key: &str| {
            resource
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("service.name").as_deref(), Some("cyber"));
        assert_eq!(
            get("service.version").as_deref(),
            Some(cyber_app::version())
        );
        assert_eq!(get("os.type").as_deref(), Some(os));
        assert!(get("cyber.build").is_some());
        assert_eq!(get("cyber.account.sub"), None);
    }
}

#[path = "../../cyber-account/tests/support/oidc.rs"]
#[cfg(unix)]
mod oidc;

/// The account signed in at start adds its `sub` and organization id to the resource,
/// while the status route only says that one is signed in. Unix only: the file
/// credential store needs Unix owner-only storage.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn resource_has_the_signed_in_account() {
    let fixture = oidc::Fixture::start().await;
    let mut client = fixture.client().await;
    let tokens = fixture.bundle(&mut client).await;
    let account = tokens.account().clone();
    let receiver = Receiver::start().await;
    let config = exporting(
        &receiver,
        json!({"account": {"issuer": fixture.state.issuer, "client_id": "cyber-cli",
            "audience": "hub", "allow_loopback_http": true}}),
    );
    let served = Served::open(Options {
        tokens: Some(tokens),
        ..Options::new(config)
    })
    .await;
    let id = served.session().await;
    served.run(&id, "hello").await;
    let (_, status) = served.send("GET", "/telemetry", None).await;
    assert_eq!(
        status["data"]["resource"]["account_signed_in"], true,
        "{status}"
    );
    assert!(!status.to_string().contains(&account.sub), "{status}");
    served.stop().await;
    for resource in receiver.resources() {
        let get = |key: &str| {
            resource
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("cyber.account.sub"), Some(account.sub.clone()));
        assert_eq!(
            get("cyber.account.org_id"),
            account.org.as_ref().map(|o| o.id.clone())
        );
    }
}

/// The exported log records of `component`.
fn component_records(
    records: &[otlp_receiver::LogRecord],
    component: &str,
) -> Vec<otlp_receiver::LogRecord> {
    let entry = ("component".to_string(), component.to_string());
    records
        .iter()
        .filter(|record| log_attributes(record).contains(&entry))
        .cloned()
        .collect()
}

/// `observability-costs` → OpenTelemetry export, logs: the server's structured log
/// records reach the collector with their severity, `component` and identifier fields,
/// and without their message or the fields that can hold content: a message can quote a
/// path or error text.
#[tokio::test(flavor = "multi_thread")]
async fn structured_logs_are_exported_without_content_fields() {
    let receiver = Receiver::start().await;
    let served = Served::start(exporting(&receiver, json!({}))).await;
    let id = served.session().await;
    served.run(&id, "PRIVATE-PROMPT-6650").await;
    cyber_core::log::warn(
        "probe",
        "Could not truncate /private/msg-path-3307.log: ghp_abcdefghijklmnop",
        json!({"session_id": id, "path": "/private/secret-path-8812", "pid": 7}),
    );
    // Below the `info` filter: neither written nor exported.
    cyber_core::log::debug("probe", "filtered probe", json!({}));
    served.stop().await;

    let records = receiver.log_records();
    let pid = ("pid".to_string(), std::process::id().to_string());
    let server = component_records(&records, "server");
    let listening = server
        .iter()
        .find(|record| log_attributes(record).contains(&pid))
        .expect("the server's own lines are exported");
    assert_eq!(listening.severity_text, "info");
    let probes = component_records(&records, "probe");
    assert_eq!(probes.len(), 1, "the debug line is filtered: {probes:?}");
    let probe = &probes[0];
    assert_eq!(log_body(probe), "", "no message without log_prompts");
    assert_eq!(probe.severity_text, "warn");
    let attributes = log_attributes(probe);
    assert!(attributes.contains(&("component".into(), "probe".into())));
    assert!(attributes.contains(&("session_id".into(), id.clone())));
    assert!(attributes.contains(&("pid".into(), "7".into())));
    for record in &records {
        assert!(record.body.is_none(), "a body: {}", log_body(record));
    }
    const ALLOWED: [&str; 8] = [
        "component",
        "session_id",
        "request_id",
        "run_id",
        "call_id",
        "kind",
        "status",
        "pid",
    ];
    for record in &records {
        for (key, _) in log_attributes(record) {
            assert!(
                ALLOWED.contains(&key.as_str()),
                "{key} on {}",
                log_body(record)
            );
        }
    }
    let raw = receiver.raw();
    for content in [
        "secret-path-8812",
        "msg-path-3307",
        "Could not truncate",
        "PRIVATE-PROMPT-6650",
        "ghp_abcdefghijklmnop",
    ] {
        assert!(!raw.contains(content), "{content} was exported");
    }
}

/// The export status of the running server.
async fn telemetry_status(served: &Served) -> Value {
    let (status, body) = served.send("GET", "/telemetry", None).await;
    assert_eq!(status, 200, "{body}");
    body["data"].clone()
}

/// The signals whose exports failed, once one has.
async fn failed_signals(served: &Served) -> Vec<Value> {
    for _ in 0..300 {
        let status = telemetry_status(served).await;
        let failed: Vec<Value> = ["traces", "metrics", "logs"]
            .iter()
            .map(|signal| status["signals"][signal].clone())
            .filter(|signal| signal["failed"].as_u64().unwrap_or(0) > 0)
            .collect();
        if !failed.is_empty() {
            return failed;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no export failed");
}

/// The status route reports a failed export with its last error.
async fn assert_failure_reported(served: &Served) {
    for signal in failed_signals(served).await {
        let error = &signal["last_error"];
        let message = error["message"].as_str().unwrap_or_default();
        assert!(!message.is_empty(), "{signal}");
        assert!(error["at_ms"].as_i64().unwrap() > 0, "{signal}");
    }
}

/// Three Turns against `endpoint` complete within their normal time, a failure is
/// counted with its last error and reported in one log line, and the server still
/// stops within the shutdown bound.
async fn turns_survive(endpoint: &str) {
    let failing_before = logged("telemetry export failing").len();
    let served = Served::start(exporting_to(endpoint, json!({}))).await;
    let id = served.session().await;
    let started = std::time::Instant::now();
    for turn in 0..3 {
        served.run(&id, &format!("turn {turn}")).await;
    }
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    let (status, messages) = served
        .send("GET", &format!("/sessions/{id}/messages"), None)
        .await;
    assert_eq!(status, 200);
    let replies = messages.to_string().matches("\"ok\"").count();
    assert!(replies >= 3, "{messages}");

    assert_failure_reported(&served).await;
    let warned = logged("telemetry export failing");
    assert_eq!(warned.len(), failing_before + 1, "{warned:?}");
    assert!(warned.last().unwrap().contains("\"level\":\"warn\""));
    let stopping = std::time::Instant::now();
    served.stop().await;
    assert!(
        stopping.elapsed() < Duration::from_secs(5),
        "{:?}",
        stopping.elapsed()
    );
}

/// `observability-costs` → "Unreachable collector": a collector that refuses
/// connections and one that accepts and never answers never slow or fail a Turn.
#[tokio::test(flavor = "multi_thread")]
async fn unreachable_collector_never_affects_turns() {
    turns_survive(&otlp_receiver::refused().await).await;
    let silent = Silent::start().await;
    turns_survive(&silent.url).await;
}

/// A collector that redirects is never followed: the export and its secret header never
/// reach the redirect target, and the export counts as failed.
#[tokio::test(flavor = "multi_thread")]
async fn collector_redirects_are_not_followed() {
    let target = Receiver::start().await;
    let redirecting = otlp_receiver::Redirecting::start(&target.url).await;
    let served = Served::start(exporting_to(
        &redirecting.url,
        json!({"telemetry": {"otel": {"headers": {"x-api-key": "redirect-key-6610"}}}}),
    ))
    .await;
    let id = served.session().await;
    served.run(&id, "hello").await;
    assert_failure_reported(&served).await;
    served.stop().await;
    assert!(target.raw().is_empty(), "the redirect was followed");
    assert!(target.header("x-api-key").is_empty());
}

/// `observability-costs` → "Flush at shutdown": with the default 60 s metric interval,
/// a Turn's spans and metrics reach the collector when the server stops 1 s after it,
/// the stop takes less than 3 s, and the final counters are logged.
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_flushes_within_three_seconds() {
    let receiver = Receiver::start().await;
    let served = Served::start(exporting(&receiver, json!({}))).await;
    let id = served.session().await;
    served.run(&id, "hello").await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        receiver.points().is_empty(),
        "no metric export before the interval"
    );
    let stopping = std::time::Instant::now();
    served.stop().await;
    let elapsed = stopping.elapsed();
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");

    assert!(
        receiver
            .named("cyber.turn")
            .iter()
            .any(|turn| { attribute(turn, "cyber.session.id").as_deref() == Some(id.as_str()) })
    );
    assert_eq!(
        receiver.total("cyber.turn.count", &[("outcome", "completed")]),
        1.0
    );
    let stopped = logged("telemetry export stopped");
    assert_eq!(stopped.len(), 1, "{stopped:?}");
    let line: Value = serde_json::from_str(&stopped[0]).unwrap();
    assert!(line["traces"]["exported"].as_u64().unwrap() > 0, "{line}");
    assert_eq!(line["traces"]["failed"], 0, "{line}");
}

/// Regression: a `UserPromptSubmit` hook runs at admission, before the Drain's first
/// Turn. Its span is under the Session's span, which the Turn then joins, not the root
/// of a trace of its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_hook_is_under_the_session_span() {
    let receiver = Receiver::start().await;
    let served = Served::start(exporting(
        &receiver,
        json!({"hooks": {"UserPromptSubmit": [{"hooks": [
            {"type": "command", "command": "echo submitted", "id": "submit"},
        ]}]}}),
    ))
    .await;
    let id = served.session().await;
    served.run(&id, "hello").await;
    served.stop().await;

    let of = |span: &_| attribute(span, "cyber.session.id").unwrap_or_default();
    let hook = receiver
        .named("cyber.hook")
        .into_iter()
        .find(|h| attribute(h, "cyber.hook.event").as_deref() == Some("UserPromptSubmit"))
        .expect("the prompt hook's span");
    let sessions: Vec<_> = receiver
        .named("cyber.session")
        .into_iter()
        .filter(|s| of(s) == id)
        .collect();
    assert_eq!(sessions.len(), 1, "one activity period");
    assert!(
        is_child(&hook, &sessions[0]),
        "the hook is under its Session"
    );
    let turn = receiver
        .named("cyber.turn")
        .into_iter()
        .find(|t| of(t) == id)
        .expect("the Turn span");
    assert!(
        is_child(&turn, &sessions[0]),
        "the Turn joins the same span"
    );
    assert!(sessions[0].start_time_unix_nano <= hook.start_time_unix_nano);
}

/// Regression: interrupting a Turn while its permission request is pending withdraws
/// the request with a runtime reply marked `cancelled`, which counts as nobody's
/// decision: no `deny/user` decision and no `permission.decision` event.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_while_asking_is_not_a_user_denial() {
    let receiver = Receiver::start().await;
    let served = Served::open(Options {
        interactive: true,
        ..Options::new(exporting(
            &receiver,
            json!({"permissions": {"read": "ask"}}),
        ))
    })
    .await;
    std::fs::write(served.repo.join("notes.txt"), "notes").unwrap();
    let id = served.session().await;
    served
        .prompt(&id, "TOOL<<read>>{\"path\": \"notes.txt\"}")
        .await;
    tokio::time::timeout(Duration::from_secs(20), async {
        while served.runtime.pending_requests(Some(&id)).is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the read asks");
    let (status, body) = served
        .send("POST", &format!("/sessions/{id}/interrupt"), None)
        .await;
    assert!(status < 300, "{body}");
    tokio::time::timeout(Duration::from_secs(20), served.runtime.wait_idle(&id))
        .await
        .expect("the Drain stops");
    let replied: Vec<_> = served
        .events(&id)
        .into_iter()
        .filter(|event| event.kind == "permission.replied.1")
        .map(|event| event.data)
        .collect();
    assert_eq!(replied.len(), 1, "{replied:?}");
    assert_eq!(replied[0]["cancelled"], true, "{replied:?}");
    served.stop().await;

    assert_eq!(
        receiver.total("cyber.permission.decisions", &[]),
        0.0,
        "{:?}",
        receiver.points()
    );
    for span in receiver.named("cyber.tool") {
        assert!(span.events.is_empty(), "{:?}", span.events);
    }
}

/// Regression: the token and cost metrics count every billed model call the usage
/// ledger counts, the generated title's included, so they agree with `cyber usage`.
#[tokio::test(flavor = "multi_thread")]
async fn exported_usage_matches_the_usage_ledger() {
    let receiver = Receiver::start().await;
    let served = Served::open(Options {
        models: Some(json!({"main": {"cost": {"input": 10000.0, "output": 10000.0}}})),
        ..Options::new(exporting(&receiver, json!({})))
    })
    .await;
    let id = served.session().await;
    served.run(&id, "hello").await;
    // The title is generated after the Turn, with a model call of its own.
    tokio::time::timeout(Duration::from_secs(20), async {
        while !served.events(&id).iter().any(|event| {
            event.kind == "session.title.generated.1" && event.data["usage"].is_object()
        }) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the title is generated");
    let (status, usage) = served
        .send("GET", &format!("/usage?scope=session&id={id}"), None)
        .await;
    assert_eq!(status, 200, "{usage}");
    served.stop().await;

    let total = &usage["data"]["total"];
    for class in ["input", "output"] {
        let ledger = total["tokens"][class].as_f64().unwrap();
        assert!(ledger > 0.0, "{usage}");
        assert_eq!(
            receiver.total("cyber.token.usage", &[("type", class)]),
            ledger,
            "{class}: {:?}",
            receiver.points()
        );
    }
    let cost = receiver.total("cyber.cost.usage", &[]);
    assert!(
        (cost - total["cost"].as_f64().unwrap()).abs() < 1e-9,
        "{cost} {usage}"
    );
}

/// `observability-costs` → OpenTelemetry export: only a served App exports. An
/// in-process App (`cyber exec --embedded`, `--ephemeral`, `cyber tui --embedded`) runs
/// its Sessions without exporting, even with export on.
#[tokio::test(flavor = "multi_thread")]
async fn an_in_process_app_never_exports() {
    let receiver = Receiver::start().await;
    let app = ModeApp::start(Setup {
        config: exporting(&receiver, json!({})),
        ..Setup::default()
    })
    .await;
    let (status, created) = app.create("default").await;
    assert_eq!(status, 201, "{created}");
    app.run(created["data"]["id"].as_str().unwrap(), "hello")
        .await;
    app.app.runtime.shutdown().await;
    assert!(receiver.spans().is_empty());
    assert!(receiver.points().is_empty());
    assert!(receiver.log_records().is_empty());
}
