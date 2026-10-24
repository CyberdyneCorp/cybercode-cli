//! Output styles through the production App (`system-context` → Provider base prompts
//! and output styles, Output style selection; `server-api` → Output style routes): the
//! real AppHost on a TCP listener and a local scripted model whose captured request
//! bodies show exactly what reached the provider.

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::path::{Path, PathBuf};
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use reqwest::{Client, Method};
use scripted_model::ScriptedModel;
use serde_json::{Value, json};

const PASSWORD: &str = "output-styles-test";
const HELD: &str =
    "project output styles are held until this checkout is trusted (cyber trust inspect)";

/// `READ-IT` holds its first step at the `first` gate and then reads a file; a tool
/// result is answered with `done`, anything else with `ok`.
fn reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({"content": "Styles"});
    }
    let last = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    if last["role"] == "tool" {
        return json!({"content": "done"});
    }
    if last.to_string().contains("READ-IT") {
        let arguments = json!({"path": "notes.txt"}).to_string();
        return json!({"gate": "first", "then": {"tool_calls": [{"index": 0, "id": "call_read",
            "type": "function", "function": {"name": "read", "arguments": arguments}}]}});
    }
    json!({"content": "ok"})
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    config: PathBuf,
    model: ScriptedModel,
    app: App,
    url: String,
    client: Client,
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
        snapshots: false,
        interactive: true,
        password: Some(PASSWORD.into()),
        mcp_credentials: None,
        remote_skill_fetcher: None,
        workflow_cores: None,
        isolation: None,
    }
}

async fn serve(app: &App) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api/v1", listener.local_addr().unwrap());
    tokio::spawn(cyber_server::http::serve_tcp(
        app.router(),
        listener,
        std::future::pending::<()>(),
    ));
    url
}

impl Fixture {
    async fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let options = options(&root);
        options.paths.ensure().unwrap();
        let model = ScriptedModel::start(reply);
        let config = options.paths.config.clone();
        std::fs::write(config.join("cyber.json"), model.config().to_string()).unwrap();
        let styles = config.join("output-styles");
        std::fs::create_dir_all(&styles).unwrap();
        std::fs::write(styles.join("terse.md"), "Be terse.").unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("notes.txt"), "notes\n").unwrap();
        let app = App::build(options).await.unwrap();
        let url = serve(&app).await;
        Self {
            _temp: temp,
            root,
            repo,
            config,
            model,
            app,
            url,
            client: Client::builder().no_proxy().build().unwrap(),
        }
    }

    /// Stop the App and build a new one on the same database, as a server restart does.
    async fn restart(self) -> Self {
        self.app.runtime.shutdown().await;
        drop(self.app);
        let app = App::build(options(&self.root)).await.unwrap();
        let url = serve(&app).await;
        Self { app, url, ..self }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        directory: &Path,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.url))
            .header("x-cyber-directory", directory.display().to_string())
            .basic_auth("cyber", Some(PASSWORD));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn ok(&self, method: Method, path: &str, directory: &Path, body: Option<Value>) -> Value {
        let (status, value) = self.send(method, path, directory, body).await;
        assert!((200..300).contains(&status), "{path}: {status} {value}");
        value
    }

    async fn session(&self, directory: &Path, extra: Value) -> String {
        let mut body = json!({"model": "test/main", "mode": "bypass"});
        for (key, value) in extra.as_object().unwrap() {
            body[key] = value.clone();
        }
        let created = self
            .ok(Method::POST, "/sessions", directory, Some(body))
            .await;
        created["data"]["id"].as_str().unwrap().to_owned()
    }

    async fn switch(&self, session: &str, style: Value) -> (u16, Value) {
        let path = format!("/sessions/{session}/output-style");
        self.send(
            Method::POST,
            &path,
            &self.repo,
            Some(json!({"style": style})),
        )
        .await
    }

    async fn prompt(&self, session: &str, text: &str) {
        let path = format!("/sessions/{session}/prompt");
        let body = json!({"parts": [{"type": "text", "text": text}]});
        self.ok(Method::POST, &path, &self.repo, Some(body)).await;
    }

    /// Wait until the model has answered `turns` Turn requests and the Session is idle.
    async fn settled(&self, session: &str, turns: usize) {
        for _ in 0..1000 {
            if self.turns().len() >= turns && !self.app.runtime.is_running(session) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{session} did not answer {turns} Turn requests");
    }

    /// Prompt and wait for the `steps` requests of its Turn.
    async fn turn(&self, session: &str, text: &str, steps: usize) {
        let before = self.turns().len();
        self.prompt(session, text).await;
        self.settled(session, before + steps).await;
    }

    /// Turn requests (not title requests), oldest first.
    fn turns(&self) -> Vec<Value> {
        self.model
            .requests()
            .into_iter()
            .filter(|body| !body.to_string().contains("Write a short title"))
            .collect()
    }

    /// The durable event types and payloads of a Session.
    async fn history(&self, session: &str) -> Vec<(String, Value)> {
        let path = format!("/sessions/{session}/history?limit=500");
        let page = self.ok(Method::GET, &path, &self.repo, None).await;
        page["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["type"].as_str().unwrap().to_owned(), e["data"].clone()))
            .collect()
    }

    /// The system prefixes of the Session's Context Epochs, oldest first.
    async fn prefixes(&self, session: &str) -> Vec<String> {
        self.history(session)
            .await
            .into_iter()
            .filter(|(kind, _)| kind == "session.context.epoch_started.1")
            .map(|(_, data)| data["system_prefix"].as_str().unwrap().to_owned())
            .collect()
    }
}

/// The joined system text of a Chat Completions request.
fn system(body: &Value) -> String {
    let first = &body["messages"][0];
    assert_eq!(first["role"], "system", "{body}");
    first["content"].as_str().unwrap().to_owned()
}

fn style_text(name: &str) -> String {
    let style = cyber_core::output_styles::builtins()
        .into_iter()
        .find(|style| style.name == name)
        .unwrap();
    format!("Output style: {name}\n\n{}", style.text)
}

#[tokio::test]
async fn output_style_routes_switch_through_a_new_epoch() {
    let f = Fixture::start().await;
    let (concise, explanatory) = (style_text("concise"), style_text("explanatory"));

    // Created with `concise`: the style ends the epoch's prefix, which starts system[0].
    let session = f.session(&f.repo, json!({"output_style": "concise"})).await;
    let shown = f
        .ok(Method::GET, &format!("/sessions/{session}"), &f.repo, None)
        .await;
    assert_eq!(shown["data"]["output_style"], "concise");
    f.prompt(&session, "READ-IT").await;
    for _ in 0..1000 {
        if f.model.held() == ["first"] {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(f.model.held(), ["first"]);

    // Switched while the Turn runs: the Turn's later step keeps the prefix.
    let (status, switched) = f.switch(&session, json!("explanatory")).await;
    assert_eq!(status, 200, "{switched}");
    assert_eq!(switched["data"]["output_style"], "explanatory");
    f.model.open_gate();
    f.settled(&session, 2).await;
    let turn = f.turns();
    assert_eq!(turn.len(), 2, "a tool step and its answer");
    let prefix = f.prefixes(&session).await;
    assert_eq!(prefix.len(), 1);
    assert!(prefix[0].ends_with(&concise), "{}", prefix[0]);
    assert!(system(&turn[0]).starts_with(&prefix[0]));
    assert_eq!(system(&turn[1]), system(&turn[0]));
    assert!(!system(&turn[0]).contains(&explanatory));

    // The next Turn starts a new epoch ending with `explanatory`, and nothing about the
    // style arrives as a mid-conversation message.
    f.turn(&session, "hello", 1).await;
    let prefix = f.prefixes(&session).await;
    assert_eq!(prefix.len(), 2);
    assert!(prefix[1].ends_with(&explanatory), "{}", prefix[1]);
    let last = f.turns().pop().unwrap();
    assert!(system(&last).starts_with(&prefix[1]));
    assert!(!system(&last).contains(&concise));
    let history = f.history(&session).await;
    let kinds: Vec<&str> = history.iter().map(|(kind, _)| kind.as_str()).collect();
    assert!(
        kinds.contains(&"session.output_style.switched.1"),
        "{kinds:?}"
    );
    assert!(
        !history
            .iter()
            .any(|(kind, data)| kind == "session.context.updated.1"
                && data.to_string().contains("output-style")),
        "{history:?}"
    );
    // Switching to the current style is a no-op.
    let switches = |history: &[(String, Value)]| {
        history
            .iter()
            .filter(|(kind, _)| kind == "session.output_style.switched.1")
            .count()
    };
    assert_eq!(f.switch(&session, json!("explanatory")).await.0, 200);
    assert_eq!(switches(&f.history(&session).await), 1);

    // Refused: a path-like name, a held project style and a child Session.
    let (status, error) = f.switch(&session, json!("../../etc/passwd")).await;
    assert_eq!(status, 400, "{error}");
    assert!(
        error.to_string().contains("invalid output style name"),
        "{error}"
    );
    let held = f.root.join("held");
    std::fs::create_dir_all(held.join(".git")).unwrap();
    let reviewer = held.join(".cyber/output-styles/reviewer.md");
    std::fs::create_dir_all(reviewer.parent().unwrap()).unwrap();
    std::fs::write(&reviewer, "Review every change.").unwrap();
    let in_held = f.session(&held, json!({})).await;
    let (status, error) = f.switch(&in_held, json!("reviewer")).await;
    assert_eq!(status, 400, "{error}");
    assert!(
        error
            .to_string()
            .contains("is held until this checkout is trusted"),
        "{error}"
    );
    let (status, error) = f
        .send(
            Method::POST,
            "/sessions",
            &held,
            Some(json!({"model": "test/main", "output_style": "reviewer"})),
        )
        .await;
    assert_eq!(status, 400, "{error}");
    let child = f.session(&f.repo, json!({"parent_id": session})).await;
    let (status, error) = f.switch(&child, json!("concise")).await;
    assert_eq!(status, 400, "{error}");
    assert!(error.to_string().contains("primary Sessions"), "{error}");
    assert_eq!(switches(&f.history(&session).await), 1);

    // The listing reports the held project style.
    let listing = f
        .ok(Method::GET, "/config/output-styles", &held, None)
        .await;
    let names: Vec<&str> = listing["data"]["styles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|style| style["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["default", "concise", "explanatory", "learning", "terse"]
    );
    assert_eq!(listing["data"]["issues"][0]["message"], HELD);

    // The global default never names a project style, and the document is unchanged.
    let document = f.config.join("cyber.json");
    let before = std::fs::read(&document).unwrap();
    let body = json!({"style": "reviewer"});
    let (status, error) = f
        .send(Method::PUT, "/config/output-style", &held, Some(body))
        .await;
    assert_eq!(status, 400, "{error}");
    assert!(
        error.to_string().contains("defined only by a project"),
        "{error}"
    );
    assert_eq!(std::fs::read(&document).unwrap(), before);
    let body = json!({"style": "terse"});
    let set = f
        .ok(Method::PUT, "/config/output-style", &held, Some(body))
        .await;
    assert_eq!(set["data"]["configured"]["name"], "terse");
    assert_eq!(set["data"]["effective"], "terse");
    let written: Value = serde_json::from_slice(&std::fs::read(&document).unwrap()).unwrap();
    assert_eq!(written["output_style"], "terse");
    assert_eq!(written["model"], "test/main");
}

/// `session-runtime` → Output style survives a restart.
#[tokio::test]
async fn output_style_survives_a_restart() {
    let f = Fixture::start().await;
    let session = f.session(&f.repo, json!({})).await;
    let (status, switched) = f.switch(&session, json!("concise")).await;
    assert_eq!(status, 200, "{switched}");
    let f = f.restart().await;
    let shown = f
        .ok(Method::GET, &format!("/sessions/{session}"), &f.repo, None)
        .await;
    assert_eq!(shown["data"]["output_style"], "concise");
    f.turn(&session, "hello", 1).await;
    let prefix = f.prefixes(&session).await;
    assert!(prefix[0].ends_with(&style_text("concise")), "{prefix:?}");
    assert!(system(&f.turns().pop().unwrap()).starts_with(&prefix[0]));

    // `null` returns to the configured style (none here, so `default`) at the next Turn.
    let (status, switched) = f.switch(&session, Value::Null).await;
    assert_eq!(status, 200, "{switched}");
    assert!(switched["data"].get("output_style").is_none(), "{switched}");
    f.turn(&session, "again", 1).await;
    let prefix = f.prefixes(&session).await;
    assert_eq!(prefix.len(), 2);
    assert!(!prefix[1].contains("Output style:"), "{}", prefix[1]);
}

fn git(directory: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(directory)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// Regression: `POST /worktrees` dropped `session.output_style`, so `cyber exec
/// --worktree --output-style` started a Session without the style, and never refused
/// an unknown one.
#[tokio::test]
async fn worktree_sessions_take_the_requested_output_style() {
    let f = Fixture::start().await;
    let repo = f.root.join("checkout");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["config", "user.name", "Styles"]);
    git(&repo, &["config", "user.email", "styles@example.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("tracked"), "base\n").unwrap();
    git(&repo, &["add", "tracked"]);
    git(&repo, &["commit", "--quiet", "-m", "base"]);
    let start = |name: &str, style: &str| {
        let session = json!({"model": "test/main", "mode": "bypass", "output_style": style});
        let body = json!({"name": name, "session": session});
        f.send(Method::POST, "/worktrees", &repo, Some(body))
    };

    let (status, error) = start("unknown", "missing").await;
    assert_eq!(status, 400, "{error}");
    assert!(error.to_string().contains("missing"), "{error}");
    let listed = f.ok(Method::GET, "/worktrees", &repo, None).await;
    assert!(!listed.to_string().contains("unknown"), "{listed}");

    let (status, created) = start("styled", "concise").await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["data"]["session"]["output_style"], "concise");
    let session = created["data"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.turn(&session, "hello", 1).await;
    let prefix = f.prefixes(&session).await;
    assert!(prefix[0].ends_with(&style_text("concise")), "{prefix:?}");
}
