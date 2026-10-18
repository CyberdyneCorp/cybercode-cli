//! The production App over HTTP with a scripted model (`session-runtime` → Side chat,
//! `background-tasks` → PTY sessions, Model interaction with PTYs, Notify tool, Send
//! file to user): a side question answered with read-only tools leaves the Session's
//! history unchanged; a model-started PTY is attached with a single-use ticket; notify
//! and send_file reach clients as durable events and a download.

#[path = "support/scripted_model.rs"]
mod scripted_model;

use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use serde_json::{Value, json};

const PASSWORD: &str = "deliveries-password";

fn last(body: &Value) -> Value {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default()
}

fn tool_call(calls: &[(&str, &str, Value)]) -> Value {
    let calls: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(index, (id, name, arguments))| {
            json!({"index": index, "id": id, "type": "function",
                   "function": {"name": name, "arguments": arguments.to_string()}})
        })
        .collect();
    json!({"tool_calls": calls})
}

fn reply(body: &Value) -> Value {
    let text = body.to_string();
    if text.contains("Write a short title") {
        return json!({"content": "Deliveries"});
    }
    let last = last(body);
    let from_tool = last["role"] == "tool";
    let side = text.contains("side question");
    if side && !from_tool {
        // A side chat may only read; the write is refused as an unknown tool.
        return tool_call(&[
            ("side_read", "read", json!({"path": "main.rs"})),
            (
                "side_write",
                "write",
                json!({"path": "evil.txt", "content": "x"}),
            ),
        ]);
    }
    if side {
        return json!({"content": "main.rs prints SIDE-ANSWER."});
    }
    let asked = last["content"].to_string();
    if !from_tool && asked.contains("open a terminal") {
        return tool_call(&[("call_pty", "pty_start", json!({"command": "cat"}))]);
    }
    if !from_tool && asked.contains("tell me when done") {
        return tool_call(&[
            (
                "call_notify",
                "notify",
                json!({"title": "Build finished", "body": "All 42 tests passed"}),
            ),
            (
                "call_send",
                "send_file",
                json!({"paths": ["report.txt"], "caption": "The report"}),
            ),
        ]);
    }
    json!({"content": "Done."})
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
        config["permissions"] = json!({"bash": "allow"});
        std::fs::write(paths.config.join("cyber.json"), config.to_string()).unwrap();
        let app = App::build(AppOptions {
            paths,
            home: root.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: root.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some(PASSWORD.into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: None,
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
            app,
            base,
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            dir: root,
            model,
            _temp: temp,
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .basic_auth("cyber", Some(PASSWORD))
            .header("x-cyber-directory", self.dir.display().to_string())
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self.request(method, path);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn session(&self) -> String {
        let (status, body) = self
            .send(
                reqwest::Method::POST,
                "/sessions",
                Some(json!({"model": "test/main"})),
            )
            .await;
        assert_eq!(status, 201, "{body}");
        body["data"]["id"].as_str().unwrap().to_string()
    }

    async fn turn(&self, id: &str, text: &str) {
        let prompt = json!({"parts": [{"type": "text", "text": text}]});
        let path = format!("/sessions/{id}/prompt");
        let (status, body) = self.send(reqwest::Method::POST, &path, Some(prompt)).await;
        assert_eq!(status, 202, "{body}");
        tokio::time::timeout(Duration::from_secs(30), self.app.runtime.wait_idle(id))
            .await
            .expect("the Turn finished");
    }

    async fn messages(&self, id: &str) -> Value {
        let path = format!("/sessions/{id}/messages?limit=200");
        self.send(reqwest::Method::GET, &path, None).await.1["data"].clone()
    }

    async fn events(&self, id: &str, kind: &str) -> Vec<Value> {
        let path = format!("/sessions/{id}/history?limit=500");
        let (_, page) = self.send(reqwest::Method::GET, &path, None).await;
        page["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["type"].as_str().unwrap_or_default().starts_with(kind))
            .map(|event| event["data"].clone())
            .collect()
    }
}

#[tokio::test]
async fn side_chat_answers_with_read_only_tools_and_leaves_history_unchanged() {
    let f = Fixture::start().await;
    std::fs::write(
        f.dir.join("main.rs"),
        "fn main() { println!(\"SIDE-ANSWER\"); }\n",
    )
    .unwrap();
    let id = f.session().await;
    f.turn(&id, "hello").await;
    let before = f.messages(&id).await;
    let calls_before = f.app.runtime.state(&id).await.unwrap().calls.len();

    let (status, body) = f
        .send(
            reqwest::Method::POST,
            &format!("/sessions/{id}/side"),
            Some(json!({"question": "what does main.rs print?"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["answer"], "main.rs prints SIDE-ANSWER.");
    assert_eq!(body["data"]["tools"], json!(["read"]));
    assert!(!f.dir.join("evil.txt").exists());

    // The model saw the conversation and the file, with read-only tools only.
    let side: Vec<Value> = f
        .model
        .requests()
        .into_iter()
        .filter(|request| request.to_string().contains("side question"))
        .collect();
    assert_eq!(side.len(), 2);
    let offered: Vec<&str> = side[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    assert!(
        !offered.is_empty()
            && offered
                .iter()
                .all(|name| ["read", "list", "glob", "grep"].contains(name)),
        "{offered:?}"
    );
    assert!(side[0].to_string().contains("hello"));
    let answered = side[1].to_string();
    assert!(
        answered.contains("SIDE-ANSWER") && answered.contains("Unknown tool in a side chat: write"),
        "{answered}"
    );

    // Nothing entered the Session's history.
    assert_eq!(f.messages(&id).await, before);
    let state = f.app.runtime.state(&id).await.unwrap();
    assert_eq!(state.calls.len(), calls_before);
    assert!(state.inbox.iter().all(|row| {
        !row.parts
            .iter()
            .any(|p| format!("{p:?}").contains("main.rs print"))
    }));
    let usage = f.events(&id, "usage.recorded").await;
    assert!(
        usage.iter().any(|u| u["purpose"] == "side_chat"),
        "{usage:?}"
    );

    let (status, _) = f
        .send(
            reqwest::Method::POST,
            &format!("/sessions/{id}/side"),
            Some(json!({"question": "  "})),
        )
        .await;
    assert_eq!(status, 400);
}

#[cfg(unix)]
#[tokio::test]
async fn model_pty_attaches_over_http_with_a_single_use_ticket() {
    use futures::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;
    let f = Fixture::start().await;
    let id = f.session().await;
    f.turn(&id, "open a terminal").await;
    let (_, jobs) = f
        .send(
            reqwest::Method::GET,
            &format!("/jobs?session_id={id}"),
            None,
        )
        .await;
    let job = &jobs["data"][0];
    assert_eq!(
        (job["kind"].as_str(), job["status"].as_str()),
        (Some("pty"), Some("running")),
        "{jobs}"
    );
    let job_id = job["id"].as_str().unwrap().to_string();

    let (status, ticket) = f
        .send(
            reqwest::Method::POST,
            &format!("/jobs/{job_id}/pty/ticket"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{ticket}");
    let ticket = ticket["data"]["ticket"].as_str().unwrap().to_string();
    let host = f.base.trim_start_matches("http://");
    let url = format!("ws://{host}/jobs/{job_id}/pty/connect?ticket={ticket}");
    let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let attached = socket.next().await.unwrap().unwrap();
    assert!(
        attached.to_text().unwrap().contains("\"attached\""),
        "{attached:?}"
    );
    socket
        .send(Message::Binary(b"ping\n".to_vec().into()))
        .await
        .unwrap();
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        while !String::from_utf8_lossy(&seen).contains("ping\r\nping\r\n") {
            if let Some(Ok(Message::Binary(bytes))) = socket.next().await {
                seen.extend_from_slice(&bytes);
            }
        }
    })
    .await
    .expect("cat echoed the input");
    let reuse = tokio_tungstenite::connect_async(&url).await;
    assert!(reuse.is_err(), "a ticket opens one socket");

    let (status, stopped) = f
        .send(reqwest::Method::POST, &format!("/jobs/{job_id}/stop"), None)
        .await;
    assert_eq!(status, 200, "{stopped}");
    assert_eq!(stopped["data"]["status"], "cancelled");
    let log = std::fs::read_to_string(stopped["data"]["output_path"].as_str().unwrap()).unwrap();
    assert!(log.contains("ping"), "{log}");
}

#[tokio::test]
async fn notify_and_send_file_reach_clients_over_http() {
    let f = Fixture::start().await;
    std::fs::write(f.dir.join("report.txt"), "42 passed\n").unwrap();
    let id = f.session().await;
    f.turn(&id, "build it and tell me when done").await;

    let notified = f.events(&id, "session.notification.sent").await;
    assert_eq!(notified.len(), 1, "{notified:?}");
    assert_eq!(notified[0]["title"], "Build finished");
    assert_eq!(notified[0]["body"], "All 42 tests passed");
    let sent = f.events(&id, "session.files.sent").await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["caption"], "The report");
    assert_eq!(sent[0]["call_id"], "call_send");
    let file = &sent[0]["files"][0];
    assert_eq!(file["name"], "report.txt");

    // The model sees both results; the file downloads as an attachment.
    let messages = f.messages(&id).await.to_string();
    assert!(
        messages.contains("The user was notified") && messages.contains("delivered"),
        "{messages}"
    );
    std::fs::write(f.dir.join("report.txt"), "changed\n").unwrap();
    let path = format!("/sessions/{id}/files/{}", file["id"].as_str().unwrap());
    let response = f.request(reqwest::Method::GET, &path).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/plain");
    assert!(
        response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("report.txt")
    );
    assert_eq!(response.text().await.unwrap(), "42 passed\n");
    let (status, _) = f
        .send(
            reqwest::Method::GET,
            &format!("/sessions/{id}/files/file_unknown"),
            None,
        )
        .await;
    assert_eq!(status, 404);
}
