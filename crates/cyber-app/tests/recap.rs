//! `GET|POST /api/v1/sessions/:id/recap` (`session-runtime` → Session recap) through the
//! production App's HTTP router with a scripted model: no recap before a finished Turn, a
//! `small`-role call with no tools and at most 120 output tokens, the cached recap making
//! no model call, the usage attributed as `recap`, and a failed call keeping the old one.
//! A model-written Session title and recap lose their control characters, since they reach
//! terminal window titles, the TUI and stdout of every client.

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::{Value, json};

static FAIL_RECAP: AtomicBool = AtomicBool::new(false);

fn is_recap(body: &Value) -> bool {
    body.to_string().contains("summarize a coding session")
}

fn reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") && text.contains("EVIL-TITLE") {
        return json!({ "content": "Evil\u{7}\u{1b}]52;c;SGFja2Vk\u{7}\u{1b}[2J\u{9b}2Jtitle" });
    }
    if text.contains("Write a short title") {
        return json!({ "content": "Recap" });
    }
    if is_recap(body) && text.contains("EVIL-RECAP") {
        return json!({ "content": "I \u{1b}[31mfixed\u{1b}[0m it\u{7}.\u{1b}]52;c;SGFja2Vk\u{7} Next\u{9b}2J I\u{202e}'ll\u{0} ship\u{7f}." });
    }
    if is_recap(body) {
        if FAIL_RECAP.load(Ordering::SeqCst) {
            return json!({ "fail": 500 });
        }
        return json!({ "content": "<think>hm</think>I answered the greeting.\nNext I'll wait for the task." });
    }
    json!({ "content": "Done." })
}

struct Fixture {
    app: App,
    base: String,
    http: reqwest::Client,
    dir: std::path::PathBuf,
    model: scripted_model::ScriptedModel,
    _temp: tempfile::TempDir,
}

impl Fixture {
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
        let mut config = model.config();
        config["providers"]["test"]["models"] = json!({
            "main": { "cost": { "input": 10.0, "output": 20.0 } },
            "small": { "cost": { "input": 1.0, "output": 2.0 } },
        });
        config["small_model"] = json!("test/small");
        std::fs::write(paths.config.join("cyber.json"), config.to_string()).unwrap();
        let app = App::build(AppOptions {
            paths,
            home: root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: root.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some("recap-password".into()),
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
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        Self {
            app,
            base,
            http,
            dir: root,
            model,
            _temp: temp,
        }
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .basic_auth("cyber", Some("recap-password"))
            .header("x-cyber-directory", self.dir.display().to_string());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn turn(&self, id: &str, text: &str) {
        let prompt = json!({ "parts": [{ "type": "text", "text": text }] });
        let (status, body) = self
            .send(
                reqwest::Method::POST,
                &format!("/sessions/{id}/prompt"),
                Some(prompt),
            )
            .await;
        assert_eq!(status, 202, "{body}");
        for _ in 0..1000 {
            let (_, session) = self
                .send(reqwest::Method::GET, &format!("/sessions/{id}"), None)
                .await;
            if !self.app.runtime.is_running(id) && session["data"]["title"] == "Recap" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the Turn did not finish");
    }

    fn recap_calls(&self) -> Vec<Value> {
        self.model.requests().into_iter().filter(is_recap).collect()
    }

    async fn cost(&self, id: &str) -> f64 {
        let (_, session) = self
            .send(reqwest::Method::GET, &format!("/sessions/{id}"), None)
            .await;
        session["data"]["totals"]["cost"].as_f64().unwrap()
    }
}

#[tokio::test]
async fn recap_is_generated_once_cached_attributed_and_kept_on_failure() {
    let fx = Fixture::start().await;
    let (_, created) = fx
        .send(
            reqwest::Method::POST,
            "/sessions",
            Some(json!({ "model": "test/main" })),
        )
        .await;
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    let path = format!("/sessions/{id}/recap");

    // Nothing to recap before the first finished Turn.
    let (status, body) = fx.send(reqwest::Method::GET, &path, None).await;
    assert_eq!(status, 200);
    assert!(body["data"].is_null(), "{body}");
    let (status, body) = fx.send(reqwest::Method::POST, &path, None).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["_tag"], "ConflictError");
    assert_eq!(body["reason"], "nothing_to_recap");
    assert!(fx.recap_calls().is_empty());

    fx.turn(&id, "Hello").await;
    let before = fx.cost(&id).await;
    let (status, first) = fx.send(reqwest::Method::POST, &path, None).await;
    assert_eq!(status, 200, "{first}");
    let recap = first["data"].clone();
    assert_eq!(
        recap["text"],
        "I answered the greeting. Next I'll wait for the task."
    );
    assert_eq!(recap["model"], "test/small");
    assert!(recap["through_seq"].as_i64().unwrap() > 0);
    let calls = fx.recap_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["model"], "small", "the small role answers");
    assert!(calls[0]["tools"].as_array().is_none_or(Vec::is_empty));
    let limit = calls[0]["max_tokens"]
        .as_u64()
        .or(calls[0]["max_completion_tokens"].as_u64());
    assert_eq!(limit, Some(120), "{}", calls[0]);
    assert!(calls[0].to_string().contains("Hello"));

    // The usage is recorded as `recap` and counted in the Session's totals.
    let sid = id.clone();
    let purposes: Vec<String> = fx
        .app
        .store
        .read(move |db| {
            let mut statement =
                db.prepare("SELECT purpose FROM usage_step WHERE session_id = ?1")?;
            let rows = statement
                .query_map([&sid], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(rows)
        })
        .unwrap();
    assert!(purposes.iter().any(|p| p == "recap"), "{purposes:?}");
    assert!(fx.cost(&id).await > before);

    // Cached: the stored recap covers the latest finished Turn, so no model call.
    let (status, again) = fx.send(reqwest::Method::POST, &path, None).await;
    assert_eq!(status, 200);
    assert_eq!(again["data"], recap);
    assert_eq!(fx.recap_calls().len(), 1);
    let (_, stored) = fx.send(reqwest::Method::GET, &path, None).await;
    assert_eq!(stored["data"], recap);

    // A newer Turn makes it stale; a failed call answers the model error and keeps it.
    fx.turn(&id, "Next").await;
    FAIL_RECAP.store(true, Ordering::SeqCst);
    let (status, failed) = fx.send(reqwest::Method::POST, &path, None).await;
    FAIL_RECAP.store(false, Ordering::SeqCst);
    assert_eq!(status, 503, "{failed}");
    assert_eq!(failed["service"], "model");
    let (_, kept) = fx.send(reqwest::Method::GET, &path, None).await;
    assert_eq!(kept["data"], recap);
    let (status, fresh) = fx.send(reqwest::Method::POST, &path, None).await;
    assert_eq!(status, 200);
    assert!(fresh["data"]["through_seq"].as_i64() > recap["through_seq"].as_i64());

    let (status, _) = fx
        .send(reqwest::Method::POST, "/sessions/ses_missing/recap", None)
        .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn a_model_written_title_has_no_control_characters() {
    let fx = Fixture::start().await;
    let (_, created) = fx
        .send(
            reqwest::Method::POST,
            "/sessions",
            Some(json!({ "model": "test/main" })),
        )
        .await;
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    let prompt = json!({ "parts": [{ "type": "text", "text": "EVIL-TITLE" }] });
    let path = format!("/sessions/{id}/prompt");
    let (status, body) = fx.send(reqwest::Method::POST, &path, Some(prompt)).await;
    assert_eq!(status, 202, "{body}");
    let mut title = String::new();
    for _ in 0..1000 {
        let (_, session) = fx
            .send(reqwest::Method::GET, &format!("/sessions/{id}"), None)
            .await;
        title = session["data"]["title"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if title.starts_with("Evil") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(title, "Evil]52;c;SGFja2Vk[2J2Jtitle");
}

/// Regression: the server kept escape sequences of a model-written recap, so a client
/// that printed it unfiltered let the model drive the terminal.
#[tokio::test]
async fn a_model_written_recap_has_no_control_characters() {
    let fx = Fixture::start().await;
    let (_, created) = fx
        .send(
            reqwest::Method::POST,
            "/sessions",
            Some(json!({ "model": "test/main" })),
        )
        .await;
    let id = created["data"]["id"].as_str().unwrap().to_owned();
    fx.turn(&id, "EVIL-RECAP").await;
    let path = format!("/sessions/{id}/recap");
    let (status, generated) = fx.send(reqwest::Method::POST, &path, None).await;
    assert_eq!(status, 200, "{generated}");
    let (_, stored) = fx.send(reqwest::Method::GET, &path, None).await;
    for recap in [&generated["data"], &stored["data"]] {
        let text = recap["text"].as_str().unwrap();
        assert_eq!(text, "I [31mfixed[0m it.]52;c;SGFja2Vk Next2J I'll ship.");
        assert!(
            !text
                .chars()
                .any(|c| c.is_control() || ('\u{202a}'..='\u{202e}').contains(&c)),
            "{text:?}"
        );
    }
}
