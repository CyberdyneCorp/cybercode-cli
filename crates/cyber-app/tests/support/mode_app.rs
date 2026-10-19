//! A production App on a temporary checkout, served over HTTP with a scripted model, for
//! Mode admission and sandbox escape tests. The model runs what the prompt asks for:
//! `RUN<<command>>` calls `bash` with that command, `TOOL<<name>>{json}` calls `name`
//! with that input, `GATE<<label>>` answers through the scripted model's gate `label`
//! (the Turn runs until [`ScriptedModel::open_gate`]), and a tool result is answered with
//! `done`.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::{Value, json};

use super::scripted_model::{Reply, ScriptedModel};

const PASSWORD: &str = "mode-app-password";

pub struct ModeApp {
    pub temp: tempfile::TempDir,
    pub root: PathBuf,
    pub repo: PathBuf,
    pub app: App,
    pub base: String,
    http: reqwest::Client,
    pub model: ScriptedModel,
}

/// Options of the server under test.
pub struct Setup {
    pub sandbox_policy: Option<&'static str>,
    pub isolation: cyber_tools::IsolationOptions,
    pub interactive: bool,
    /// Merged into the global configuration.
    pub config: Value,
    /// A file database, so a second App can reopen it.
    pub database: Option<PathBuf>,
    /// The scripted model's answers; [`prompted_call`] when unset.
    pub reply: Option<Reply>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            sandbox_policy: None,
            isolation: cyber_tools::IsolationOptions::default(),
            interactive: false,
            config: json!({}),
            database: None,
            reply: None,
        }
    }
}

/// The scripted model's answer: the tool call the newest user prompt asks for.
pub fn prompted_call(body: &Value) -> Value {
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    let last = messages.last().cloned().unwrap_or(Value::Null);
    if last["role"] == "tool" {
        return json!({"content": "done"});
    }
    let text = match &last["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    let between = |open: &str| {
        let start = text.find(open)? + open.len();
        let end = text[start..].find(">>")?;
        Some((
            text[start..start + end].to_string(),
            text[start + end + 2..].to_string(),
        ))
    };
    if let Some((label, _)) = between("GATE<<") {
        return json!({ "gate": label });
    }
    let (name, arguments) = if let Some((command, _)) = between("RUN<<") {
        (
            "bash".to_string(),
            json!({ "command": command }).to_string(),
        )
    } else if let Some((name, rest)) = between("TOOL<<") {
        let input = rest.lines().next().unwrap_or("{}").trim().to_string();
        (name, input)
    } else {
        return json!({"content": "ok"});
    };
    json!({"tool_calls": [{"index": 0, "id": format!("call_{:04}", messages.len()), "type": "function",
        "function": {"name": name, "arguments": arguments}}]})
}

impl ModeApp {
    pub async fn start(setup: Setup) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let model = ScriptedModel::start(setup.reply.unwrap_or(prompted_call));
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git/hooks")).unwrap();
        let app = Self::build(&root, &repo, &model, &setup).await;
        let base = Self::serve(&app).await;
        Self {
            temp,
            root,
            repo,
            app,
            base,
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            model,
        }
    }

    /// Stop this server and serve a new App on the same checkout, configuration root and
    /// model, built with `setup` (give the same `database` to keep the Sessions).
    pub async fn restart(self, setup: Setup) -> Self {
        self.app.runtime.shutdown().await;
        let app = Self::build(&self.root, &self.repo, &self.model, &setup).await;
        let base = Self::serve(&app).await;
        Self { app, base, ..self }
    }

    async fn serve(app: &App) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(cyber_server::http::serve_tcp(
            app.router(),
            listener,
            std::future::pending::<()>(),
        ));
        base
    }

    pub async fn build(
        root: &std::path::Path,
        repo: &std::path::Path,
        model: &ScriptedModel,
        setup: &Setup,
    ) -> App {
        let paths = Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        paths.ensure().unwrap();
        let mut config = model.config();
        if let (Some(config), Some(extra)) = (config.as_object_mut(), setup.config.as_object()) {
            config.extend(extra.clone());
        }
        std::fs::write(paths.config.join("cyber.json"), config.to_string()).unwrap();
        App::build(AppOptions {
            paths,
            home: root.join("home"),
            database: setup
                .database
                .clone()
                .map_or(DatabaseLocation::Memory, DatabaseLocation::File),
            default_directory: repo.to_path_buf(),
            sandbox_policy: setup.sandbox_policy.map(str::to_string),
            snapshots: false,
            interactive: setup.interactive,
            password: Some(PASSWORD.into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: Some(setup.isolation),
        })
        .await
        .unwrap()
    }

    /// The status and JSON body of an API request in the checkout's Location.
    pub async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.send_with(method, path, body, &[]).await
    }

    /// [`ModeApp::send`] with extra request headers.
    pub async fn send_with(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut request = self
            .http
            .request(method, format!("{}/api/v1{path}", self.base))
            .basic_auth("cyber", Some(PASSWORD))
            .header("x-cyber-directory", self.repo.display().to_string());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    /// Create a Session in `mode`; the status and body of the request.
    pub async fn create(&self, mode: &str) -> (u16, Value) {
        self.send("POST", "/sessions", Some(json!({ "mode": mode })))
            .await
    }

    /// Prompt and wait for the Drain to settle.
    pub async fn run(&self, id: &str, text: &str) {
        let (status, body) = self
            .send(
                "POST",
                &format!("/sessions/{id}/prompt"),
                Some(json!({"parts": [{"type": "text", "text": text}], "delivery": "queue"})),
            )
            .await;
        assert_eq!(status, 202, "{body}");
        tokio::time::timeout(Duration::from_secs(60), self.app.runtime.wait_idle(id))
            .await
            .expect("the Drain settles");
    }

    /// Outputs of every settled tool call of a Session, in call order.
    pub async fn outputs(&self, id: &str) -> Vec<String> {
        let state = self.app.runtime.state(id).await.unwrap();
        let mut calls: Vec<_> = state.calls.values().cloned().collect();
        calls.sort_by_key(|call| call.call_id.clone());
        calls
            .into_iter()
            .map(|call| call.output.unwrap_or_default())
            .collect()
    }

    /// Durable event kinds of a Session.
    pub fn events(&self, id: &str) -> Vec<cyber_store::StoredEvent> {
        self.app.store.read_events(id, -1, 500).unwrap().events
    }
}
