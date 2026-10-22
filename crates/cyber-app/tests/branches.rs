//! Session branches (`session-runtime` → Session branches) through the production App:
//! its HTTP router, its runtime and its snapshots, with a scripted model. A conversation
//! rewind keeps the cut messages as an inactive branch (also after a restart), checkout
//! by message or label rebuilds the model's context from that path, a busy Session
//! refuses, and `code` restores files through the three-phase revert, keeping user edits.
//! A compaction summary is reused only on the path it summarized.

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_server::runtime::{BranchCheckout, RevertTarget, RuntimeError};
use serde_json::{Value, json};

const SUMMARY_REQUEST: &str = "Conversation to summarize";
/// A summary request whose transcript holds this waits at the model's gate.
const HOLD_SUMMARY: &str = "HOLD-SUMMARY";
/// A background command that prints [`JOB_OUTPUT`] once `release.flag` exists.
const JOB_COMMAND: &str =
    "while [ ! -f release.flag ]; do sleep 0.05; done; echo background-job-output";
#[cfg(unix)]
const JOB_OUTPUT: &str = "background-job-output";

/// `WRITE-A` and `WRITE-B` write a file, `START-JOB` starts [`JOB_COMMAND`] as a
/// background Job, `GATE` waits at the model, and anything else is answered with text. A summary request is answered `SUMMARY-B` when its transcript
/// holds branch B's `two-b`, and `SUMMARY-A` otherwise, after the gate when it holds
/// [`HOLD_SUMMARY`].
fn reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({ "content": "Branches" });
    }
    if text.contains(SUMMARY_REQUEST) && text.contains(HOLD_SUMMARY) {
        return json!({ "gate": "summary", "then": { "content": "SUMMARY-A" } });
    }
    if text.contains(SUMMARY_REQUEST) {
        let summary = if text.contains("two-b") {
            "SUMMARY-B"
        } else {
            "SUMMARY-A"
        };
        return json!({ "content": summary });
    }
    let last = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .cloned()
        .unwrap_or_default();
    if last["role"] == "tool" {
        return json!({ "content": "Done." });
    }
    let last = last.to_string();
    if last.contains("START-JOB") {
        let arguments = json!({ "command": JOB_COMMAND, "background": true }).to_string();
        return json!({ "tool_calls": [{ "index": 0, "id": "call_job", "type": "function",
            "function": { "name": "bash", "arguments": arguments } }] });
    }
    let (file, content) = match () {
        _ if last.contains("WRITE-A") => ("a.txt", "alpha\n"),
        _ if last.contains("WRITE-B") => ("b.txt", "beta\n"),
        _ if last.contains("GATE") => {
            return json!({ "gate": "busy", "then": { "content": "Done." } });
        }
        _ => return json!({ "content": "Done." }),
    };
    let arguments = json!({ "path": file, "content": content }).to_string();
    json!({ "tool_calls": [{ "index": 0, "id": format!("call_{file}"), "type": "function",
        "function": { "name": "write", "arguments": arguments } }] })
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
    app: App,
    base: String,
    http: reqwest::Client,
    model: scripted_model::ScriptedModel,
}

fn options(root: &Path) -> AppOptions {
    let paths = Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    AppOptions {
        database: DatabaseLocation::File(paths.data.join("cyber.db")),
        paths,
        home: root.join("home"),
        default_directory: root.to_path_buf(),
        sandbox_policy: None,
        snapshots: true,
        interactive: true,
        password: Some("branches-password".into()),
        mcp_credentials: None,
        remote_skill_fetcher: None,
        workflow_cores: None,
        isolation: None,
    }
}

async fn serve(app: &App) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
    tokio::spawn(cyber_server::http::serve_tcp(
        app.router(),
        listener,
        std::future::pending::<()>(),
    ));
    base
}

fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

impl Fixture {
    async fn start() -> Self {
        Self::start_with(json!({})).await
    }

    /// Start with `extra` top-level keys in the global configuration.
    async fn start_with(extra: Value) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let options = options(&root);
        options.paths.ensure().unwrap();
        let model = scripted_model::ScriptedModel::start(reply);
        // A manual compaction keeps the newest user turn verbatim.
        let mut config = model.config();
        config["compaction"] = json!({ "keep": { "turns": 1 } });
        for (key, value) in extra.as_object().unwrap() {
            config[key] = value.clone();
        }
        std::fs::write(options.paths.config.join("cyber.json"), config.to_string()).unwrap();
        // Snapshots track git worktrees.
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        let app = App::build(options).await.unwrap();
        let base = serve(&app).await;
        Self {
            _temp: temp,
            root,
            work,
            app,
            base,
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            model,
        }
    }

    /// Stop the App and build a new one on the same database, as a server restart does.
    async fn restart(self) -> Self {
        self.app.runtime.shutdown().await;
        drop(self.app);
        let app = App::build(options(&self.root)).await.unwrap();
        let base = serve(&app).await;
        Self { app, base, ..self }
    }

    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .basic_auth("cyber", Some("branches-password"))
            .header("x-cyber-directory", self.work.display().to_string());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn session(&self) -> String {
        let body = json!({ "model": "test/main", "mode": "accept-edits" });
        let (status, created) = self.send("POST", "/sessions", Some(body)).await;
        assert_eq!(status, 201, "{created}");
        created["data"]["id"].as_str().unwrap().to_owned()
    }

    async fn admit(&self, id: &str, message: &str, text: &str) -> (u16, Value) {
        let body = json!({ "id": message, "parts": [{ "type": "text", "text": text }] });
        self.send("POST", &format!("/sessions/{id}/prompt"), Some(body))
            .await
    }

    /// Send a prompt and wait until its Turn has answered.
    async fn turn(&self, id: &str, message: &str, text: &str) {
        let (status, body) = self.admit(id, message, text).await;
        assert_eq!(status, 202, "{body}");
        self.wait_answered(id, message).await;
    }

    async fn wait_answered(&self, id: &str, message: &str) {
        for _ in 0..1000 {
            let entries = self.messages(id).await;
            let answered = entries.iter().skip_while(|e| e["id"] != message).any(|e| {
                e["kind"] == "assistant" && e["finished"] == true && e["calls"] == json!([])
            });
            if answered && !self.app.runtime.is_running(id) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{message} was not answered");
    }

    /// `GET .../messages` in order: what a client without branch support sees.
    async fn messages(&self, id: &str) -> Vec<Value> {
        let (status, page) = self
            .send(
                "GET",
                &format!("/sessions/{id}/messages?order=asc&limit=200"),
                None,
            )
            .await;
        assert_eq!(status, 200, "{page}");
        page["data"].as_array().unwrap().clone()
    }

    async fn user_texts(&self, id: &str) -> Vec<String> {
        self.messages(id)
            .await
            .iter()
            .filter(|e| e["kind"] == "user")
            .map(|e| e["parts"][0]["text"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The durable event types and payloads of a Session.
    async fn history(&self, id: &str) -> Vec<(String, Value)> {
        let (status, page) = self
            .send("GET", &format!("/sessions/{id}/history?limit=500"), None)
            .await;
        assert_eq!(status, 200, "{page}");
        page["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["type"].as_str().unwrap().to_owned(), e["data"].clone()))
            .collect()
    }

    /// The messages of the newest Turn request (not a title or summary request).
    fn last_context(&self) -> String {
        let requests = self.model.requests();
        let turn = requests
            .iter()
            .rev()
            .find(|r| {
                let text = r.to_string();
                !text.contains("Write a short title") && !text.contains(SUMMARY_REQUEST)
            })
            .unwrap();
        turn["messages"].to_string()
    }

    /// The newest summary request.
    fn last_summary_request(&self) -> String {
        let requests = self.model.requests();
        requests
            .iter()
            .rev()
            .map(Value::to_string)
            .find(|r| r.contains(SUMMARY_REQUEST))
            .unwrap()
    }

    /// `POST .../compact` on an idle Session, which summarizes before it answers.
    async fn compact(&self, id: &str) {
        let (status, body) = self
            .send("POST", &format!("/sessions/{id}/compact"), None)
            .await;
        assert_eq!(status, 202, "{body}");
    }

    async fn summary(&self, id: &str) -> Option<String> {
        let state = self.app.runtime.state(id).await.unwrap();
        state.compacted.map(|c| c.summary)
    }

    async fn rewind(&self, id: &str, message: &str, target: &str) {
        let body = json!({ "message_id": message, "target": target });
        let (status, body) = self
            .send("POST", &format!("/sessions/{id}/rewind"), Some(body))
            .await;
        assert_eq!(status, 200, "{body}");
    }

    async fn checkout(&self, id: &str, request: BranchCheckout) -> Result<String, RuntimeError> {
        let done = self.app.runtime.checkout(id, &request).await?;
        Ok(done.tip_message_id)
    }
}

/// User messages one, two, three; a conversation rewind to two; then two-b.
async fn two_branches(f: &Fixture) -> String {
    let id = f.session().await;
    for (message, text) in [
        ("msg_one", "one"),
        ("msg_two", "two"),
        ("msg_three", "three"),
    ] {
        f.turn(&id, message, text).await;
    }
    f.rewind(&id, "msg_two", "conversation").await;
    f.turn(&id, "msg_twob", "two-b").await;
    id
}

fn by_message(message: &str) -> BranchCheckout {
    BranchCheckout {
        message_id: Some(message.into()),
        ..BranchCheckout::default()
    }
}

/// Task 1.1 and the scenario "Rewind keeps the old attempt": the active path is
/// 1 → 2b, clients see that linear conversation, 2 and 3 stay as an inactive branch, the
/// model never sees them, and a restart replays the same two-branch tree.
#[tokio::test]
async fn a_rewind_keeps_the_cut_messages_as_an_inactive_branch() {
    let f = Fixture::start().await;
    let id = two_branches(&f).await;
    assert_eq!(f.user_texts(&id).await, ["one", "two-b"]);
    let context = f.last_context();
    assert!(
        context.contains("two-b") && !context.contains("three"),
        "{context}"
    );
    let state = f.app.runtime.state(&id).await.unwrap();
    let tree = state.branches();
    assert_eq!(tree.tips().len(), 2);
    assert!(tree.parent("msg_two").is_some());
    assert_eq!(tree.parent("msg_twob"), tree.parent("msg_two"), "siblings");
    assert!(tree.stashed("msg_three").is_some());
    // A rewound message keeps its ID: re-admitting it is a conflict, not a new branch.
    let (status, body) = f.admit(&id, "msg_three", "three").await;
    assert_eq!(status, 409, "{body}");

    let tips: Vec<String> = tree.tips().iter().map(|t| t.to_string()).collect();
    let f = f.restart().await;
    assert_eq!(f.user_texts(&id).await, ["one", "two-b"]);
    let replayed = f.app.runtime.state(&id).await.unwrap();
    assert_eq!(replayed.branches().tips(), tips);
    assert_eq!(replayed.entries, state.entries);
}

/// Task 1.2 and the scenario "Return to an earlier attempt": checkout by label goes to
/// the newest tip under the labeled message, the next Turn's context holds 1, 2 and 3
/// but not 2b, and `session.branch.checked_out.1` is recorded. Labels follow the grammar
/// and are unique; `exact` stops at the message itself.
#[tokio::test]
async fn checkout_by_label_rebuilds_the_context_from_that_branch() {
    let f = Fixture::start().await;
    let id = two_branches(&f).await;
    let runtime = &f.app.runtime;
    runtime
        .label_branch(&id, "fix-a", Some("msg_two"))
        .await
        .unwrap();
    runtime
        .label_branch(&id, "fix-a", Some("msg_two"))
        .await
        .unwrap();
    let bad = runtime
        .label_branch(&id, "has space", Some("msg_two"))
        .await;
    assert!(matches!(bad, Err(RuntimeError::Invalid(_))), "{bad:?}");
    let taken = runtime.label_branch(&id, "fix-a", Some("msg_one")).await;
    assert!(matches!(taken, Err(RuntimeError::Conflict(_))), "{taken:?}");
    let unknown = runtime.label_branch(&id, "other", Some("msg_nope")).await;
    assert!(
        matches!(unknown, Err(RuntimeError::MessageNotFound(_))),
        "{unknown:?}"
    );
    let cleared = runtime.label_branch(&id, "never-set", None).await;
    assert!(
        matches!(cleared, Err(RuntimeError::MessageNotFound(_))),
        "{cleared:?}"
    );
    let missing = f.checkout(&id, by_message("msg_nope")).await;
    assert!(
        matches!(missing, Err(RuntimeError::MessageNotFound(_))),
        "{missing:?}"
    );

    let state = runtime.state(&id).await.unwrap();
    let old_tip = state.branches().newest_tip("msg_three").unwrap().to_owned();
    let label = BranchCheckout {
        label: Some("fix-a".into()),
        ..BranchCheckout::default()
    };
    assert_eq!(f.checkout(&id, label).await.unwrap(), old_tip);
    assert_eq!(f.user_texts(&id).await, ["one", "two", "three"]);
    let recorded = f.history(&id).await;
    let checked_out: Vec<&Value> = recorded
        .iter()
        .filter(|(kind, _)| kind == "session.branch.checked_out.1")
        .map(|(_, data)| data)
        .collect();
    assert_eq!(
        checked_out,
        [&json!({ "tip_message_id": old_tip, "code_restored": false })]
    );
    assert!(
        recorded
            .iter()
            .any(|(kind, data)| kind == "session.branch.labeled.1"
                && *data == json!({ "name": "fix-a", "message_id": "msg_two" }))
    );

    f.turn(&id, "msg_four", "four").await;
    let context = f.last_context();
    for text in ["one", "two", "three", "four"] {
        assert!(
            context.contains(&format!("\"{text}\"")),
            "{text}: {context}"
        );
    }
    assert!(!context.contains("two-b"), "{context}");

    let exact = BranchCheckout {
        exact: true,
        ..by_message("msg_one")
    };
    assert_eq!(f.checkout(&id, exact).await.unwrap(), "msg_one");
    assert_eq!(f.user_texts(&id).await, ["one"]);
    let state = runtime.state(&id).await.unwrap();
    assert_eq!(state.active_tip(), Some("msg_one"));
    assert_eq!(state.branches().labels()["fix-a"], "msg_two");
}

/// The scenario "Checkout while running": a running Drain refuses with
/// `SessionBusyError` and the active path is unchanged.
#[tokio::test]
async fn checkout_while_running_is_refused() {
    let f = Fixture::start().await;
    let id = f.session().await;
    f.turn(&id, "msg_one", "one").await;
    let (status, body) = f.admit(&id, "msg_gate", "GATE").await;
    assert_eq!(status, 202, "{body}");
    while f.model.held().is_empty() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let before = f.app.runtime.state(&id).await.unwrap().entries;
    for code in [false, true] {
        let request = BranchCheckout {
            code,
            exact: true,
            ..by_message("msg_one")
        };
        let busy = f.checkout(&id, request).await;
        assert!(
            matches!(&busy, Err(e) if e.to_string().starts_with("SessionBusyError")),
            "{busy:?}"
        );
    }
    assert_eq!(f.app.runtime.state(&id).await.unwrap().entries, before);
    f.model.open_gate();
    f.wait_answered(&id, "msg_gate").await;
    assert!(
        !f.history(&id)
            .await
            .iter()
            .any(|(kind, _)| kind.starts_with("session.branch."))
    );

    // A staged conversation rewind would move the tip on the next prompt: refused.
    let body = json!({ "message_id": "msg_gate", "target": "conversation" });
    let (status, staged) = f
        .send("POST", &format!("/sessions/{id}/revert/stage"), Some(body))
        .await;
    assert_eq!(status, 200, "{staged}");
    let request = BranchCheckout {
        exact: true,
        ..by_message("msg_one")
    };
    let refused = f.checkout(&id, request).await;
    assert!(
        matches!(refused, Err(RuntimeError::Invalid(_))),
        "{refused:?}"
    );
    assert_eq!(f.user_texts(&id).await, ["one", "GATE"]);
}

/// Task 1.3: `code` restores the branch's last snapshot through a staged `code` revert,
/// so unrelated user edits survive and the diff is previewed; a user edit the restore
/// would overwrite fails closed with nothing checked out.
#[tokio::test]
async fn checkout_with_code_restores_the_branch_snapshot_and_keeps_user_edits() {
    let f = Fixture::start().await;
    let id = f.session().await;
    f.turn(&id, "msg_a", "WRITE-A").await;
    assert_eq!(
        std::fs::read_to_string(f.work.join("a.txt")).unwrap(),
        "alpha\n"
    );
    f.rewind(&id, "msg_a", "both").await;
    assert!(!f.work.join("a.txt").exists());
    f.turn(&id, "msg_b", "WRITE-B").await;
    std::fs::write(f.work.join("user.txt"), "mine\n").unwrap();

    let request = json!({ "message_id": "msg_a", "code": true });
    let (status, done) = f
        .send("POST", &format!("/sessions/{id}/checkout"), Some(request))
        .await;
    assert_eq!(status, 200, "{done}");
    let done = &done["data"];
    assert_eq!(done["code_restored"], true);
    let mut restored: Vec<&str> = done["restored_files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    restored.sort_unstable();
    assert_eq!(restored, ["a.txt", "b.txt"]);
    let revert = &done["revert"];
    assert_eq!(revert["target"], "code");
    let diff = revert["diff"].as_str().unwrap();
    assert!(diff.contains("+alpha"), "{diff}");
    assert_eq!(
        std::fs::read_to_string(f.work.join("a.txt")).unwrap(),
        "alpha\n"
    );
    assert!(!f.work.join("b.txt").exists());
    assert_eq!(
        std::fs::read_to_string(f.work.join("user.txt")).unwrap(),
        "mine\n"
    );
    assert_eq!(f.user_texts(&id).await, ["WRITE-A"]);
    let state = f.app.runtime.state(&id).await.unwrap();
    assert_eq!(
        state.revert.as_ref().map(|r| r.target),
        Some(RevertTarget::Code)
    );
    let recorded = f.history(&id).await;
    let (_, checked_out) = recorded
        .iter()
        .find(|(kind, _)| kind == "session.branch.checked_out.1")
        .unwrap();
    assert_eq!(checked_out["code_restored"], true);
    assert_eq!(checked_out["snapshot_id"], revert["applied"]);

    // The user changes a file the restore of msg_b's branch would delete.
    std::fs::write(f.work.join("a.txt"), "edited by the user\n").unwrap();
    let back = BranchCheckout {
        code: true,
        ..by_message("msg_b")
    };
    let conflict = f.checkout(&id, back).await;
    assert!(
        matches!(conflict, Err(RuntimeError::RewindConflict(_))),
        "{conflict:?}"
    );
    assert_eq!(
        std::fs::read_to_string(f.work.join("a.txt")).unwrap(),
        "edited by the user\n"
    );
    assert_eq!(f.user_texts(&id).await, ["WRITE-A"]);
    let checkouts = f
        .history(&id)
        .await
        .iter()
        .filter(|(kind, _)| kind == "session.branch.checked_out.1")
        .count();
    assert_eq!(checkouts, 1);
}

/// Task 2.1 and the scenario "Summary of a sibling is not reused": branch A (1, 2, 3) is
/// compacted, and B (1, 2b, 3b) shares only message 1 with it. B's Turns and B's own
/// compaction never see A's summary; checking A out again reuses A's summary (not B's),
/// checking B out reuses B's, and a restart replays the same choice.
#[tokio::test]
async fn a_sibling_summary_never_reaches_the_model() {
    let f = Fixture::start().await;
    let id = f.session().await;
    for (message, text) in [
        ("msg_one", "one"),
        ("msg_two", "two"),
        ("msg_three", "three"),
    ] {
        f.turn(&id, message, text).await;
    }
    f.compact(&id).await;
    assert_eq!(f.summary(&id).await.as_deref(), Some("SUMMARY-A"));

    f.rewind(&id, "msg_two", "conversation").await;
    assert_eq!(f.summary(&id).await, None, "A's summary covers msg_two");
    f.turn(&id, "msg_twob", "two-b").await;
    f.turn(&id, "msg_threeb", "three-b").await;
    let context = f.last_context();
    assert!(!context.contains("SUMMARY-A"), "{context}");
    assert!(
        context.contains("\"one\"") && context.contains("\"three-b\""),
        "{context}"
    );
    f.compact(&id).await;
    let request = f.last_summary_request();
    assert!(
        request.contains("two-b")
            && !request.contains("SUMMARY-A")
            && !request.contains("[user] three"),
        "{request}"
    );
    assert_eq!(f.summary(&id).await.as_deref(), Some("SUMMARY-B"));

    f.checkout(&id, by_message("msg_three")).await.unwrap();
    f.turn(&id, "msg_four", "four").await;
    let context = f.last_context();
    assert!(context.contains("SUMMARY-A"), "{context}");
    assert!(
        context.contains("\"three\"") && context.contains("\"four\""),
        "{context}"
    );
    for absent in ["SUMMARY-B", "two-b", "\"one\""] {
        assert!(!context.contains(absent), "{absent}: {context}");
    }

    f.checkout(&id, by_message("msg_threeb")).await.unwrap();
    f.turn(&id, "msg_fourb", "four-b").await;
    let context = f.last_context();
    assert!(
        context.contains("SUMMARY-B") && context.contains("\"four-b\""),
        "{context}"
    );
    assert!(
        !context.contains("SUMMARY-A") && !context.contains("\"four\""),
        "{context}"
    );

    let f = f.restart().await;
    assert_eq!(f.summary(&id).await.as_deref(), Some("SUMMARY-B"));
    f.checkout(&id, by_message("msg_four")).await.unwrap();
    assert_eq!(f.summary(&id).await.as_deref(), Some("SUMMARY-A"));
    f.turn(&id, "msg_five", "five").await;
    let context = f.last_context();
    assert!(
        context.contains("SUMMARY-A") && !context.contains("SUMMARY-B"),
        "{context}"
    );
}

/// Task 3.1 and the scenarios "Linear view for existing clients" and "Checkout by label"
/// over HTTP: the tree, labels, checkout (409 busy, 404 unknown) and `branches=all`.
#[tokio::test]
async fn branch_routes_serve_the_tree_labels_checkout_and_every_branch() {
    let f = Fixture::start().await;
    let id = two_branches(&f).await;
    let s = format!("/sessions/{id}");

    let (status, tree) = f.send("GET", &format!("{s}/tree"), None).await;
    assert_eq!(status, 200, "{tree}");
    let nodes = tree["data"].as_array().unwrap();
    let ids: Vec<&str> = nodes.iter().map(|n| n["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["msg_one", "msg_two", "msg_three", "msg_twob"]);
    let node = |id: &str| nodes.iter().find(|n| n["id"] == id).unwrap().clone();
    assert_eq!(node("msg_one")["parent_id"], Value::Null);
    assert_eq!(node("msg_one")["descendants"], 3);
    assert_eq!(node("msg_two")["parent_id"], "msg_one");
    assert_eq!(node("msg_twob")["parent_id"], "msg_one");
    assert_eq!(node("msg_three")["parent_id"], "msg_two");
    assert_eq!(node("msg_three")["preview"], "three");
    for (id, active, tip) in [
        ("msg_one", true, false),
        ("msg_two", false, false),
        ("msg_three", false, true),
        ("msg_twob", true, true),
    ] {
        assert_eq!(node(id)["active"], active, "{id}");
        assert_eq!(node(id)["tip"], tip, "{id}");
    }

    // Existing clients see the active path; `branches=all` adds the inactive branch.
    assert_eq!(f.user_texts(&id).await, ["one", "two-b"]);
    let (status, all) = f
        .send(
            "GET",
            &format!("{s}/messages?order=asc&limit=200&branches=all"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{all}");
    let all = all["data"].as_array().unwrap();
    let three = all.iter().find(|m| m["id"] == "msg_three").unwrap();
    let after_two = all
        .iter()
        .rfind(|m| m["kind"] == "assistant" && m["parent_message_id"] == "msg_two")
        .unwrap();
    assert_eq!(three["parent_message_id"], after_two["id"]);
    assert!(all.iter().all(|m| m.get("parent_message_id").is_some()));
    assert_eq!(all[0]["parent_message_id"], Value::Null);
    let (status, bad) = f
        .send("GET", &format!("{s}/messages?branches=some"), None)
        .await;
    assert_eq!(status, 400, "{bad}");

    // Labels: set, shown on the tree, conflict, unknown, cleared.
    let label = |message: &str| Some(json!({ "message_id": message }));
    let (status, body) = f
        .send("PUT", &format!("{s}/labels/fix-a"), label("msg_two"))
        .await;
    assert_eq!(status, 204, "{body}");
    let (_, tree) = f.send("GET", &format!("{s}/tree"), None).await;
    let two = tree["data"].as_array().unwrap()[1].clone();
    assert_eq!(two["labels"], json!(["fix-a"]));
    let (status, body) = f
        .send("PUT", &format!("{s}/labels/fix-a"), label("msg_one"))
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (409, Some("ConflictError"))
    );
    let (status, body) = f
        .send("PUT", &format!("{s}/labels/other"), label("msg_nope"))
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (404, Some("MessageNotFoundError"))
    );
    let (status, body) = f
        .send("PUT", &format!("{s}/labels/has%20space"), label("msg_two"))
        .await;
    assert_eq!(status, 400, "{body}");

    // Checkout by label goes to the newest tip under it.
    let (status, done) = f
        .send(
            "POST",
            &format!("{s}/checkout"),
            Some(json!({ "label": "fix-a" })),
        )
        .await;
    assert_eq!(status, 200, "{done}");
    let tip = done["data"]["tip_message_id"].as_str().unwrap().to_owned();
    assert_eq!(done["data"]["code_restored"], false);
    assert_eq!(f.user_texts(&id).await, ["one", "two", "three"]);
    let state = f.app.runtime.state(&id).await.unwrap();
    assert_eq!(state.active_tip(), Some(tip.as_str()));
    let (status, body) = f
        .send(
            "POST",
            &format!("{s}/checkout"),
            Some(json!({ "message_id": "msg_nope" })),
        )
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (404, Some("MessageNotFoundError"))
    );
    let (status, body) = f
        .send(
            "POST",
            &format!("{s}/checkout"),
            Some(json!({ "label": "nope" })),
        )
        .await;
    assert_eq!(status, 404, "{body}");

    // A running Drain refuses with 409 SessionBusyError and changes nothing.
    let (status, body) = f.admit(&id, "msg_gate", "GATE").await;
    assert_eq!(status, 202, "{body}");
    while f.model.held().is_empty() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let exact = json!({ "message_id": "msg_twob", "exact": true });
    let (status, body) = f.send("POST", &format!("{s}/checkout"), Some(exact)).await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (409, Some("SessionBusyError"))
    );
    f.model.open_gate();
    f.wait_answered(&id, "msg_gate").await;
    assert_eq!(f.user_texts(&id).await, ["one", "two", "three", "GATE"]);

    let (status, body) = f.send("DELETE", &format!("{s}/labels/fix-a"), None).await;
    assert_eq!(status, 204, "{body}");
    let (status, body) = f.send("DELETE", &format!("{s}/labels/fix-a"), None).await;
    assert_eq!(status, 404, "{body}");
    let state = f.app.runtime.state(&id).await.unwrap();
    assert!(state.branches().labels().is_empty());
}

/// A, active, is 1 → 2 (which holds [`HOLD_SUMMARY`]) → 3; B is 1 → 2b.
async fn held_summary_branches(f: &Fixture) -> String {
    let id = f.session().await;
    for (message, text) in [
        ("msg_one", "one"),
        ("msg_two", "two HOLD-SUMMARY"),
        ("msg_three", "three"),
    ] {
        f.turn(&id, message, text).await;
    }
    f.rewind(&id, "msg_two", "conversation").await;
    f.turn(&id, "msg_twob", "two-b").await;
    f.checkout(&id, by_message("msg_three")).await.unwrap();
    id
}

/// No Turn request on branch B holds A's summary.
fn assert_no_summary_a(f: &Fixture) {
    for request in f.model.requests() {
        let text = request.to_string();
        if text.contains("two-b") {
            assert!(!text.contains("SUMMARY-A"), "{text}");
        }
    }
}

fn is_busy(result: &Result<String, RuntimeError>) -> bool {
    matches!(result, Err(e) if e.to_string().starts_with("SessionBusyError"))
}

/// Regression: a manual compaction runs without a Drain, so only its in-process fence
/// guards it. While it writes branch A's summary a checkout to B (runtime and HTTP) and
/// the commit of a staged conversation rewind are refused with `SessionBusyError`;
/// the summary lands on A, and B's context never holds it, also after a restart.
#[tokio::test]
async fn checkout_during_a_compaction_is_refused() {
    let f = Fixture::start().await;
    let id = held_summary_branches(&f).await;
    let s = format!("/sessions/{id}");
    let race = async {
        while f.model.held().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let busy = f.checkout(&id, by_message("msg_twob")).await;
        let to_b = json!({ "message_id": "msg_twob" });
        let (status, body) = f.send("POST", &format!("{s}/checkout"), Some(to_b)).await;
        assert_eq!(
            (status, body["_tag"].as_str()),
            (409, Some("SessionBusyError"))
        );
        let rewind = json!({ "message_id": "msg_three", "target": "conversation" });
        let (status, body) = f
            .send("POST", &format!("{s}/revert/stage"), Some(rewind))
            .await;
        assert_eq!(status, 200, "{body}");
        let (status, body) = f.send("POST", &format!("{s}/revert/commit"), None).await;
        assert_eq!(
            (status, body["_tag"].as_str()),
            (409, Some("SessionBusyError"))
        );
        let (status, body) = f.send("POST", &format!("{s}/revert/clear"), None).await;
        assert_eq!(status, 204, "{body}");
        f.model.open_gate();
        busy
    };
    let compact = format!("{s}/compact");
    let ((status, body), busy) = tokio::join!(f.send("POST", &compact, None), race);
    assert_eq!(status, 202, "{body}");
    assert!(is_busy(&busy), "{busy:?}");
    assert_eq!(
        f.user_texts(&id).await,
        ["one", "two HOLD-SUMMARY", "three"]
    );
    assert_eq!(f.summary(&id).await.as_deref(), Some("SUMMARY-A"));

    f.checkout(&id, by_message("msg_twob")).await.unwrap();
    assert_eq!(f.summary(&id).await, None);
    f.turn(&id, "msg_threeb", "three-b").await;
    assert_no_summary_a(&f);
    let f = f.restart().await;
    assert_eq!(f.summary(&id).await, None);
    f.turn(&id, "msg_fourb", "four-b").await;
    assert_no_summary_a(&f);
    f.checkout(&id, by_message("msg_three")).await.unwrap();
    assert_eq!(f.summary(&id).await.as_deref(), Some("SUMMARY-A"));
}

/// A PreCompact HTTP hook that answers `{}` once the test sends on the returned channel,
/// and reports each request it receives.
fn gated_hook() -> (
    String,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    let (arrived_tx, arrived) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            stream
                .set_read_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            let mut buffer = [0u8; 8192];
            while matches!(stream.read(&mut buffer), Ok(n) if n > 0) {}
            arrived_tx.send(()).ok();
            released.recv().ok();
            let reply = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
            stream.write_all(reply.as_bytes()).ok();
        }
    });
    (url, arrived, release)
}

/// Regression: a checkout that lands before the compaction's start is recorded (while
/// the PreCompact hook runs) moved the path the summary would be of; the compaction
/// fails closed with `SessionBusyError` and nothing is summarized.
#[tokio::test]
async fn a_checkout_before_the_compaction_starts_fails_the_compaction() {
    let (url, arrived, release) = gated_hook();
    let hook = json!({ "type": "http", "url": url, "id": "gate" });
    let f = Fixture::start_with(json!({ "hooks": { "PreCompact": [{ "hooks": [hook] }] } })).await;
    let id = held_summary_branches(&f).await;
    let race = async {
        while arrived.try_recv().is_err() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let moved = f.checkout(&id, by_message("msg_twob")).await;
        release.send(()).unwrap();
        moved
    };
    let compact = format!("/sessions/{id}/compact");
    // Without the fence the summary request waits at the gate, so bound the wait.
    let both = async { tokio::join!(f.send("POST", &compact, None), race) };
    let ((status, body), moved) = tokio::time::timeout(Duration::from_secs(30), both)
        .await
        .expect("the compaction summarized a path that is no longer active");
    let tip = moved.unwrap();
    let state = f.app.runtime.state(&id).await.unwrap();
    assert_eq!(state.active_tip(), Some(tip.as_str()));
    assert_eq!(
        (status, body["_tag"].as_str()),
        (409, Some("SessionBusyError")),
        "{body}"
    );
    let requests = f.model.requests();
    assert!(
        !requests
            .iter()
            .any(|r| r.to_string().contains(SUMMARY_REQUEST))
    );
    assert!(
        f.history(&id)
            .await
            .iter()
            .any(|(kind, _)| kind == "session.compaction.failed.1")
    );
    assert_eq!(f.summary(&id).await, None);
    f.turn(&id, "msg_threeb", "three-b").await;
    assert_no_summary_a(&f);
}

/// Poll a checkout while it is refused as busy, for at most 30 seconds.
async fn checkout_once_free(f: &Fixture, id: &str, request: BranchCheckout) -> String {
    for _ in 0..3000 {
        let result = f.checkout(id, request.clone()).await;
        if !is_busy(&result) {
            return result.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the checkout stayed refused as busy");
}

/// A staged conversation rewind to `message` is committed by the next prompt, which is
/// admitted and answered.
async fn prompt_after_staged_rewind(f: &Fixture, id: &str, message: &str) {
    let body = json!({ "message_id": message, "target": "conversation" });
    let (status, staged) = f
        .send("POST", &format!("/sessions/{id}/revert/stage"), Some(body))
        .await;
    assert_eq!(status, 200, "{staged}");
    f.turn(id, "msg_after", "after").await;
}

/// Regression: a manual compaction whose HTTP client goes away is dropped mid-summary
/// with no outcome recorded. Its fence goes with it, so a checkout and a prompt that
/// commits a staged conversation rewind succeed without a restart.
#[tokio::test]
async fn a_dropped_compaction_request_leaves_the_session_usable() {
    let f = Fixture::start().await;
    let id = held_summary_branches(&f).await;
    let compact = format!("/sessions/{id}/compact");
    tokio::select! {
        (status, body) = f.send("POST", &compact, None) => panic!("answered {status}: {body}"),
        () = async {
            while f.model.held().is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        } => {}
    }
    // The client hung up; the server drops the compaction with the request.
    checkout_once_free(&f, &id, by_message("msg_twob")).await;
    assert_eq!(f.user_texts(&id).await, ["one", "two-b"]);
    prompt_after_staged_rewind(&f, &id, "msg_twob").await;
    assert_eq!(f.user_texts(&id).await, ["one", "after"]);
    assert_eq!(f.summary(&id).await, None);
    f.model.open_gate();
}

/// Regression: an interrupt drops a Drain's compaction while the summary request is
/// held, with no outcome recorded. A checkout and a prompt that commits a staged
/// conversation rewind still succeed without a restart.
#[tokio::test]
async fn an_interrupted_drain_compaction_leaves_the_session_usable() {
    let f = Fixture::start().await;
    let id = held_summary_branches(&f).await;
    let s = format!("/sessions/{id}");
    let (status, body) = f.admit(&id, "msg_gate", "GATE").await;
    assert_eq!(status, 202, "{body}");
    while f.model.held().is_empty() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Requested while the Drain runs, the compaction waits for its next boundary.
    let (status, body) = f.send("POST", &format!("{s}/compact"), None).await;
    assert_eq!(status, 202, "{body}");
    f.model.open_gate();
    while f.model.held() != ["summary"] {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let (status, body) = f.send("POST", &format!("{s}/interrupt"), None).await;
    assert_eq!(status, 204, "{body}");
    assert!(!f.app.runtime.is_running(&id));
    f.checkout(&id, by_message("msg_twob")).await.unwrap();
    assert_eq!(f.user_texts(&id).await, ["one", "two-b"]);
    prompt_after_staged_rewind(&f, &id, "msg_twob").await;
    assert_eq!(f.user_texts(&id).await, ["one", "after"]);
    assert_eq!(f.summary(&id).await, None);
    f.model.open_gate();
}

/// Regression: a background Job started on one branch hands its output back into the
/// active path when it ends. While it runs, a checkout (runtime and HTTP) is refused
/// with `SessionBusyError`, so the output never reaches the model on another branch;
/// once its notice is answered on its own branch, the checkout succeeds.
#[cfg(unix)]
#[tokio::test]
async fn checkout_while_a_background_job_runs_is_refused() {
    let f = Fixture::start_with(json!({ "permissions": { "bash": "allow" } })).await;
    let id = f.session().await;
    f.turn(&id, "msg_one", "one").await;
    f.turn(&id, "msg_beta", "beta-branch").await;
    f.rewind(&id, "msg_beta", "conversation").await;
    f.turn(&id, "msg_job", "START-JOB").await;
    let running = |f: &Fixture| {
        let jobs = f.app.runtime.jobs(Some(&id)).unwrap();
        jobs.iter()
            .any(|j| j.status == cyber_server::runtime::JobStatus::Running)
    };
    assert!(running(&f), "{:?}", f.app.runtime.jobs(Some(&id)));
    let busy = f.checkout(&id, by_message("msg_beta")).await;
    assert!(is_busy(&busy), "{busy:?}");
    let to_beta = json!({ "message_id": "msg_beta" });
    let (status, body) = f
        .send("POST", &format!("/sessions/{id}/checkout"), Some(to_beta))
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (409, Some("SessionBusyError")),
        "{body}"
    );
    assert_eq!(f.user_texts(&id).await, ["one", "START-JOB"]);

    std::fs::write(f.work.join("release.flag"), "").unwrap();
    for _ in 0..3000 {
        let answered = f
            .model
            .requests()
            .iter()
            .any(|r| r.to_string().contains(JOB_OUTPUT));
        if answered && !running(&f) && !f.app.runtime.is_running(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    checkout_once_free(&f, &id, by_message("msg_beta")).await;
    f.turn(&id, "msg_beta_two", "beta-two").await;
    for request in f.model.requests() {
        let text = request.to_string();
        if text.contains("beta-branch") {
            assert!(!text.contains(JOB_OUTPUT), "{text}");
        }
    }
    assert!(
        f.model
            .requests()
            .iter()
            .any(|r| r.to_string().contains(JOB_OUTPUT)),
        "the Job's notice was answered on its own branch"
    );
}
