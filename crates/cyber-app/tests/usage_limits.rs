//! `GET /api/v1/usage/limits` (`observability-costs` → Provider-reported limits) through
//! the production App's HTTP router with a scripted local OpenAI-compatible model: a
//! gateway that sends no usage window headers yields an empty list, and the windows a
//! response reports appear with their latest reading.

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::{Value, json};

/// Plain answers, except that a prompt naming a plan used percent reports plan windows.
fn reply(body: &Value) -> Value {
    let text = body.to_string();
    for used in ["0.25", "0.5"] {
        if text.contains(&format!("PLAN {used}")) {
            return json!({ "content": "Planned.", "headers": {
                "anthropic-ratelimit-unified-5h-utilization": used,
                "anthropic-ratelimit-unified-5h-reset": "1792130400",
                "anthropic-ratelimit-unified-7d-utilization": "0.6",
            } });
        }
    }
    json!({ "content": "Done." })
}

struct Server {
    _temp: tempfile::TempDir,
    _model: scripted_model::ScriptedModel,
    app: App,
    base: String,
    http: reqwest::Client,
}

impl Server {
    async fn start() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let paths = Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        paths.ensure().unwrap();
        let model = scripted_model::ScriptedModel::start(reply);
        std::fs::write(paths.config.join("cyber.json"), model.config().to_string()).unwrap();
        let app = App::build(AppOptions {
            paths,
            home: root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: root.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some("limits-password".into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
        tokio::spawn(cyber_server::http::serve_tcp(
            app.router(),
            listener,
            std::future::pending::<()>(),
        ));
        Self {
            _temp: temp,
            _model: model,
            app,
            base,
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .basic_auth("cyber", Some("limits-password"));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        assert!(
            response.status().is_success(),
            "{path}: {}",
            response.status()
        );
        response.json().await.unwrap()
    }

    async fn limits(&self) -> Vec<Value> {
        let reply = self.send(reqwest::Method::GET, "/usage/limits", None).await;
        reply["data"]["limits"].as_array().unwrap().clone()
    }

    /// A Session that answered `text`, with its title generated.
    async fn answered(&self, text: &str) {
        let created = self
            .send(
                reqwest::Method::POST,
                "/sessions",
                Some(json!({ "model": "test/main", "mode": "default" })),
            )
            .await;
        let id = created["data"]["id"].as_str().unwrap().to_owned();
        self.send(
            reqwest::Method::POST,
            &format!("/sessions/{id}/prompt"),
            Some(json!({ "parts": [{ "type": "text", "text": text }] })),
        )
        .await;
        for _ in 0..1000 {
            let session = self
                .send(reqwest::Method::GET, &format!("/sessions/{id}"), None)
                .await;
            if !self.app.runtime.is_running(&id) && session["data"]["title"].is_string() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{id} did not answer");
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[tokio::test]
async fn a_gateway_without_window_headers_reports_nothing() {
    let server = Server::start().await;
    assert!(server.limits().await.is_empty());
    server.answered("Hello").await;
    // Local usage happened, but no limit is derived from it.
    assert_eq!(server.limits().await, Vec::<Value>::new());
}

#[tokio::test]
async fn reported_windows_appear_with_their_latest_reading() {
    let server = Server::start().await;
    let before = now_ms();
    server.answered("PLAN 0.25").await;
    let limits = server.limits().await;
    assert_eq!(limits.len(), 2, "{limits:?}");
    let five = &limits[0];
    assert_eq!(five["provider"], "test");
    assert_eq!(five["window"], "5h");
    assert_eq!(five["label"], "5-hour limit");
    assert_eq!(five["period"], "daily");
    assert_eq!(five["used_percent"], 25.0);
    assert_eq!(five["resets_ms"], 1_792_130_400_000_i64);
    let observed = five["observed_ms"].as_i64().unwrap();
    assert!(observed >= before && observed <= now_ms(), "{observed}");
    let week = &limits[1];
    assert_eq!(week["window"], "7d");
    assert_eq!(week["label"], "Weekly limit");
    assert_eq!(week["period"], "weekly");
    assert_eq!(week["used_percent"], 60.0);

    // A later response replaces the reading of each window it reports.
    server.answered("PLAN 0.5").await;
    let limits = server.limits().await;
    assert_eq!(limits.len(), 2);
    assert_eq!(limits[0]["used_percent"], 50.0);
    assert!(limits[0]["observed_ms"].as_i64().unwrap() >= observed);
}
