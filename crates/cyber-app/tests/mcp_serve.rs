//! MCP serve through the production application: the real `App` (AppHost, AppServices,
//! global configuration) on a TCP listener with the local password, a Turn run by a
//! local OpenAI-compatible model through the ordinary SDK routes, then read over
//! `/api/v1/mcp/serve`.

use std::sync::Arc;
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_server::runtime::messaging::{Direction, MessageRow};
use serde_json::{Value, json};

const PASSWORD: &str = "test-password-123456";
const ANSWER_LEN: usize = 5000;

/// The streamed `delta` a scripted model answers a request body with.
type Reply = fn(&Value) -> Value;

/// A blocking OpenAI-compatible endpoint that records every request body.
struct Model {
    url: String,
    requests: Arc<std::sync::Mutex<Vec<Value>>>,
    done: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// Every request gets one long text answer.
fn long_answer(_body: &Value) -> Value {
    json!({"content": format!("done {}", "x".repeat(ANSWER_LEN - 5))})
}

impl Model {
    fn start() -> Self {
        Self::scripted(long_answer)
    }

    fn scripted(reply: Reply) -> Self {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop = Arc::clone(&done);
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let thread = std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut buffer = [0; 8192];
                let mut body_start = 0;
                let complete = loop {
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        body_start = end + 4;
                        let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length = head
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .map_or(0, |value| value.trim().parse::<usize>().unwrap());
                        if bytes.len() >= end + 4 + length {
                            break true;
                        }
                    }
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break false,
                        Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                    }
                };
                if !complete {
                    continue;
                }
                let body: Value = serde_json::from_slice(&bytes[body_start..]).unwrap_or_default();
                let delta = reply(&body);
                seen.lock().unwrap().push(body);
                let finish = if delta.get("tool_calls").is_some() {
                    "tool_calls"
                } else {
                    "stop"
                };
                let chunk = json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}],
                    "usage":{"prompt_tokens":100,"completion_tokens":10}});
                let reply = format!("data: {chunk}\n\ndata: [DONE]\n\n");
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Self {
            url,
            requests,
            done,
            thread: Some(thread),
        }
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Client {
    http: reqwest::Client,
    api: String,
    directory: String,
}

impl Client {
    async fn sdk(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.api))
            .header("x-cyber-directory", &self.directory)
            .basic_auth("cyber", Some(PASSWORD));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        assert!(status.is_success(), "{path}: {status} {text}");
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    async fn mcp(
        &self,
        session: Option<&str>,
        frame: Value,
    ) -> (reqwest::StatusCode, reqwest::header::HeaderMap, Value) {
        let mut request = self
            .http
            .post(format!("{}/mcp/serve", self.api))
            .basic_auth("cyber", Some(PASSWORD))
            .header("accept", "application/json, text/event-stream")
            .json(&frame);
        if let Some(session) = session {
            request = request
                .header("mcp-session-id", session)
                .header("mcp-protocol-version", "2025-06-18");
        }
        let response = request.send().await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        let text = response.text().await.unwrap();
        let body = sse_frames(&text)
            .pop()
            .unwrap_or_else(|| serde_json::from_str(&text).unwrap_or(Value::Null));
        (status, headers, body)
    }

    async fn tool(&self, session: &str, name: &str, arguments: Value) -> Value {
        let frame = json!({"jsonrpc":"2.0","id":name,"method":"tools/call",
            "params":{"name":name,"arguments":arguments}});
        let (status, _, body) = self.mcp(Some(session), frame).await;
        assert_eq!(status, 200, "{body}");
        let result = body["result"].clone();
        assert_eq!(result["isError"], false, "{name}: {result}");
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, result["structuredContent"], "{name}");
        result
    }
}

/// The JSON-RPC frames of an SSE answer (none for a JSON answer); named events, such as
/// the prompt receipt, are not MCP frames.
fn sse_frames(text: &str) -> Vec<Value> {
    text.split("\n\n")
        .filter(|event| !event.lines().any(|line| line.starts_with("event:")))
        .filter_map(|event| {
            let data: String = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim_start)
                .collect();
            serde_json::from_str(&data).ok()
        })
        .collect()
}

/// The production application on a TCP listener with the local password, for a global
/// configuration naming the scripted model plus `global`, and a repository at `repo`
/// with the project configuration `project`.
struct Fixture {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    repo: std::path::PathBuf,
    application: App,
    client: Client,
    stop: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Fixture {
    async fn start(model: &Model, global: Value, project: Value) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let paths = paths(&root);
        paths.ensure().unwrap();
        write_global(&paths, model, global);
        std::fs::write(repo.join("cyber.jsonc"), project.to_string()).unwrap();
        Self::open(temp, root, repo).await
    }

    /// Replace the global configuration; Locations opened afterwards read it.
    fn write_global(&self, model: &Model, global: Value) {
        write_global(&paths(&self.root), model, global);
    }

    /// The production application over the database file of `root`.
    async fn open(
        temp: tempfile::TempDir,
        root: std::path::PathBuf,
        repo: std::path::PathBuf,
    ) -> Self {
        let application = App::build(AppOptions {
            paths: paths(&root),
            home: root.join("home"),
            database: DatabaseLocation::File(root.join("data").join("cyber.db")),
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
        let api = format!("http://{}/api/v1", listener.local_addr().unwrap());
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(cyber_server::http::serve_tcp(
            application.router(),
            listener,
            async {
                let _ = stopped.await;
            },
        ));
        let client = Client {
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            api,
            directory: repo.display().to_string(),
        };
        Self {
            _temp: temp,
            root,
            repo,
            application,
            client,
            stop,
            server,
        }
    }

    async fn close(self) {
        let _ = self.stop.send(());
        let _ = self.server.await;
        self.application.runtime.shutdown().await;
    }

    /// Stop the application and start a new one over the same files, as a server
    /// restart does.
    async fn restart(self) -> Self {
        let _ = self.stop.send(());
        let _ = self.server.await;
        self.application.runtime.shutdown().await;
        drop(self.application);
        Self::open(self._temp, self.root, self.repo).await
    }
}

/// The global configuration naming the scripted model, plus `global`.
fn write_global(paths: &Paths, model: &Model, global: Value) {
    let mut config = json!({
        "providers":{"test":{"api":{"type":"openai-compatible","url":format!("{}/v1",model.url),
            "settings":{"auth":"none"}},"models":{"main":{}}}},
    });
    for (key, value) in global.as_object().into_iter().flatten() {
        config[key] = value.clone();
    }
    std::fs::write(paths.config.join("cyber.json"), config.to_string()).unwrap();
}

fn paths(root: &std::path::Path) -> Paths {
    Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    }
}

#[tokio::test]
async fn production_server_serves_session_reading_tools_over_mcp() {
    let model = Model::start();
    // A project cannot configure MCP serve; its value is ignored, not an error.
    let fixture = Fixture::start(
        &model,
        json!({"mcp_serve":{"limits":{"max_depth":3}}}),
        json!({"mcp_serve":{"enabled":false}}),
    )
    .await;
    let (root, repo, client, application) = (
        &fixture.root,
        &fixture.repo,
        &fixture.client,
        &fixture.application,
    );

    // One ordinary Turn through the SDK routes.
    let created = client
        .sdk(
            reqwest::Method::POST,
            "/sessions",
            Some(json!({"model":"test/main","title":"MCP read"})),
        )
        .await;
    let session = created["data"]["id"].as_str().unwrap().to_owned();
    client
        .sdk(
            reqwest::Method::POST,
            &format!("/sessions/{session}/prompt"),
            Some(json!({"parts":[{"type":"text","text":"hello there"}]})),
        )
        .await;
    tokio::time::timeout(
        Duration::from_secs(20),
        application.runtime.wait_idle(&session),
    )
    .await
    .expect("the Turn settled");

    // The served tools are not agent tools.
    let offered = client.sdk(reqwest::Method::GET, "/tools", None).await;
    let offered = offered.to_string();
    assert!(offered.contains("\"read\""), "{offered}");
    assert!(!offered.contains("cyber_session"), "{offered}");
    assert!(!offered.contains("cyber_prompt"), "{offered}");

    let initialize = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"Claude Code","version":"2"}}});
    let (status, headers, init) = client.mcp(None, initialize).await;
    assert_eq!(status, 200, "{init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "cyber-code");
    assert_eq!(
        init["result"]["serverInfo"]["version"],
        cyber_app::version()
    );
    let mcp = headers["mcp-session-id"].to_str().unwrap().to_owned();
    let (status, _, _) = client
        .mcp(
            Some(&mcp),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        )
        .await;
    assert_eq!(status, 202);

    let listed = client.tool(&mcp, "cyber_session_list", json!({})).await;
    let sessions = listed["structuredContent"]["sessions"].as_array().unwrap();
    let summary = sessions
        .iter()
        .find(|s| s["session_id"] == json!(session))
        .unwrap();
    assert_eq!(summary["title"], "MCP read");
    assert_eq!(summary["directory"], json!(repo.display().to_string()));
    assert_eq!(summary["state"], "idle");
    assert_eq!(summary["model"], "test/main");
    assert_eq!(summary["mcp_depth"], 0);
    assert!(summary["agent"].is_string() && summary["mode"].is_string());
    assert_eq!(listed["structuredContent"]["truncated"], false);
    let filtered = client
        .tool(&mcp, "cyber_session_list", json!({"state":"running"}))
        .await;
    assert_eq!(filtered["structuredContent"]["sessions"], json!([]));
    let elsewhere = client
        .tool(
            &mcp,
            "cyber_session_list",
            json!({"directory": root.join("data").display().to_string()}),
        )
        .await;
    assert_eq!(elsewhere["structuredContent"]["sessions"], json!([]));

    let read = client
        .tool(&mcp, "cyber_session_read", json!({"session_id": session}))
        .await;
    assert_eq!(read["_meta"]["dev.cyber-code/untrusted"], true, "{read}");
    let read = &read["structuredContent"];
    assert_eq!(read["session"]["session_id"], json!(session));
    assert_eq!(read["session"]["state"], "idle");
    assert_eq!(read["pending"], json!([]));
    assert_eq!(read["inbox_pending"], 0);
    assert!(
        read["usage"]["input_tokens"].as_u64().unwrap() >= 100,
        "{read}"
    );
    assert!(
        read["usage"]["output_tokens"].as_u64().unwrap() >= 10,
        "{read}"
    );
    assert!(read["usage"].get("cost_usd").is_some());
    let last = &read["last_turn"];
    assert_eq!(last["stop_reason"], "completed", "{read}");
    assert!(last["finished_at"].is_i64());
    let preview = last["final_text_preview"].as_str().unwrap();
    assert!(preview.starts_with("done xxx"), "{preview}");
    assert!(preview.len() <= 2048, "{}", preview.len());
    assert!(preview.ends_with("bytes]"), "{preview}");
    assert!(preview.contains("[… truncated "), "{preview}");

    let tail = client
        .tool(
            &mcp,
            "cyber_session_transcript",
            json!({"session_id": session, "from": "end", "limit": 2}),
        )
        .await;
    assert_eq!(tail["_meta"]["dev.cyber-code/untrusted"], true);
    let items = tail["structuredContent"]["items"].as_array().unwrap();
    let roles: Vec<&str> = items.iter().map(|i| i["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant"]);
    assert!(items[0]["seq"].as_i64().unwrap() < items[1]["seq"].as_i64().unwrap());
    assert_eq!(items[0]["text"], "hello there");
    assert_eq!(items[0]["origin"], "user");
    assert!(items[0]["created_at"].is_i64());
    assert_eq!(items[1]["text"].as_str().unwrap().len(), ANSWER_LEN);
    assert_eq!(tail["structuredContent"]["truncated"], false);

    let first = client
        .tool(
            &mcp,
            "cyber_session_transcript",
            json!({"session_id": session, "max_bytes": 1024}),
        )
        .await["structuredContent"]
        .clone();
    assert_eq!(first["items"].as_array().unwrap().len(), 1, "{first}");
    assert_eq!(first["items"][0]["role"], "user");
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = client
        .tool(
            &mcp,
            "cyber_session_transcript",
            json!({"session_id": session, "max_bytes": 1024, "cursor": cursor}),
        )
        .await["structuredContent"]
        .clone();
    let items = second["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{second}");
    assert_eq!(items[0]["role"], "assistant");
    let text = items[0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("done x") && text.contains("[… truncated "),
        "{text}"
    );
    assert!(serde_json::to_string(&second["items"]).unwrap().len() <= 1024);
    assert_eq!(second["truncated"], true);
    assert!(second.get("next_cursor").is_none(), "{second}");

    let interrupted = client
        .tool(&mcp, "cyber_interrupt", json!({"session_id": session}))
        .await;
    assert_eq!(
        interrupted["structuredContent"],
        json!({"session_id": session, "was_running": false})
    );
    // The Session is unchanged by an idle interrupt.
    let after = client
        .tool(&mcp, "cyber_session_read", json!({"session_id": session}))
        .await;
    assert_eq!(after["structuredContent"]["session"]["state"], "idle");
    assert_eq!(
        after["structuredContent"]["last_turn"]["stop_reason"],
        "completed"
    );

    fixture.close().await;
}

/// A model that answers a tool-enabled user turn with a `question` call, everything else
/// (the denied call's result, titles) with a short text.
fn asks_a_question(body: &Value) -> Value {
    let tools = body["tools"].as_array().is_some_and(|t| !t.is_empty());
    let last = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    if tools && last["role"] == "user" && last["content"].to_string().contains("ask me") {
        let arguments = json!({"questions":[{"question":"Which?","header":"Pick","options":[
            {"label":"A","description":"a"},{"label":"B","description":"b"}]}]});
        json!({"tool_calls":[{"index":0,"id":"call_q","type":"function",
            "function":{"name":"question","arguments":arguments.to_string()}}]})
    } else {
        json!({"content":"done"})
    }
}

impl Client {
    async fn initialize(&self, label: Option<&str>) -> String {
        let initialize = json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"Some IDE","version":"1"}}});
        let mut request = self
            .http
            .post(format!("{}/mcp/serve", self.api))
            .basic_auth("cyber", Some(PASSWORD))
            .header("accept", "application/json, text/event-stream")
            .json(&initialize);
        if let Some(label) = label {
            request = request.header("x-cyber-mcp-client", label);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 200);
        response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned()
    }

    async fn tool_error(&self, session: &str, name: &str, arguments: Value) -> Value {
        let frame = json!({"jsonrpc":"2.0","id":name,"method":"tools/call",
            "params":{"name":name,"arguments":arguments}});
        let (status, _, body) = self.mcp(Some(session), frame).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["result"]["isError"], true, "{body}");
        serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }
}

#[tokio::test]
async fn production_server_creates_bounded_non_interactive_sessions_over_mcp() {
    let model = Model::scripted(asks_a_question);
    let fixture = Fixture::start(
        &model,
        json!({"agents":{"autonomous":{"description":"Runs without asking","mode":"primary",
            "permission_mode":"auto"}}}),
        json!({"mode":"accept-edits"}),
    )
    .await;
    // The project Mode widens permissions, so it applies only to a trusted checkout.
    let loader =
        cyber_app::hook_config_loader(fixture.application.paths.clone(), fixture.root.join("home"));
    let resolved = loader(&fixture.repo).unwrap();
    cyber_core::trust::TrustStore::new(fixture.application.paths.trust_file())
        .approve(
            &resolved.trust.checkout_root,
            resolved.trust.digest.as_deref().unwrap(),
        )
        .unwrap();
    assert_eq!(loader(&fixture.repo).unwrap().value["mode"], "accept-edits");
    let client = &fixture.client;
    let mcp = client.initialize(Some("claude-code")).await;
    let directory = fixture.repo.display().to_string();

    // Project default clamped to the ceiling `default` (the global mode is unset).
    let created = client
        .tool(
            &mcp,
            "cyber_session_create",
            json!({"directory": directory, "model": "test/main", "title": "From MCP"}),
        )
        .await["structuredContent"]
        .clone();
    assert_eq!(created["effective_mode"], "default", "{created}");
    assert_eq!(created["session"]["mode"], "default");
    assert_eq!(created["session"]["mcp_client"], "claude-code");
    let session = created["session"]["session_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // An explicit agent whose permission_mode is above the ceiling is refused.
    let refused = client
        .tool_error(
            &mcp,
            "cyber_session_create",
            json!({"directory": directory, "model": "test/main", "agent": "autonomous"}),
        )
        .await;
    assert_eq!(refused["reason"], "mode_ceiling", "{refused}");
    assert_eq!(refused["ceiling"], "default");

    // An ordinary Session, visible through the route `cyber sessions` and the SDK use.
    let listed = client.sdk(reqwest::Method::GET, "/sessions", None).await;
    let rows = listed["data"]["data"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{listed}");
    assert_eq!(rows[0]["id"], json!(session));
    assert_eq!(rows[0]["mode"], "default");
    let only = client
        .tool(&mcp, "cyber_session_list", json!({"mcp_only": true}))
        .await;
    assert_eq!(
        only["structuredContent"]["sessions"][0]["mcp_client"],
        "claude-code"
    );

    // The exec non-interactive rules: `question` is never offered and never asked.
    client
        .sdk(
            reqwest::Method::POST,
            &format!("/sessions/{session}/prompt"),
            Some(json!({"parts":[{"type":"text","text":"ask me something"}]})),
        )
        .await;
    tokio::time::timeout(
        Duration::from_secs(20),
        fixture.application.runtime.wait_idle(&session),
    )
    .await
    .expect("the Turn settled");
    let requests = model.requests.lock().unwrap().clone();
    let offered: Vec<&str> = requests
        .iter()
        .filter_map(|body| body["tools"].as_array())
        .find(|tools| !tools.is_empty())
        .expect("a tool-enabled model request")
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect();
    assert!(offered.contains(&"read"), "{offered:?}");
    for asking in ["question", "plan_enter", "plan_exit"] {
        assert!(!offered.contains(&asking), "{asking} offered: {offered:?}");
    }
    let questions = client
        .sdk(reqwest::Method::GET, "/questions/requests", None)
        .await;
    assert_eq!(questions["data"], json!([]), "{questions}");
    let read = client
        .tool(&mcp, "cyber_session_read", json!({"session_id": session}))
        .await;
    assert_eq!(read["structuredContent"]["pending"], json!([]));
    assert_eq!(
        read["structuredContent"]["last_turn"]["stop_reason"], "completed",
        "{read}"
    );
    fixture.close().await;
}

/// Every request gets the same short answer.
fn short_answer(_body: &Value) -> Value {
    json!({"content": "all green"})
}

#[tokio::test]
async fn production_server_runs_prompts_over_mcp_and_rewaits_after_a_restart() {
    let model = Model::scripted(short_answer);
    let fixture = Fixture::start(&model, json!({}), json!({})).await;
    let mcp = fixture.client.initialize(Some("codex")).await;
    let directory = fixture.repo.display().to_string();

    // A waiting call answers over SSE: progress, then the completed result.
    let frame = json!({"jsonrpc":"2.0","id":"p1","method":"tools/call","params":{
        "name":"cyber_prompt","_meta":{"progressToken":"t1"},
        "arguments":{"directory": directory, "model": "test/main", "prompt": "fix the failing test",
            "idempotency_key": "app-1"}}});
    let response = fixture
        .client
        .http
        .post(format!("{}/mcp/serve", fixture.client.api))
        .basic_auth("cyber", Some(PASSWORD))
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &mcp)
        .json(&frame)
        .send()
        .await
        .unwrap();
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let text = response.text().await.unwrap();
    assert!(text.starts_with(": keep-alive"), "{text}");
    let frames = sse_frames(&text);
    let result = &frames.last().unwrap()["result"];
    assert_eq!(result["isError"], false, "{text}");
    let first = &result["structuredContent"];
    assert_eq!(first["status"], "completed", "{first}");
    assert_eq!(first["final_text"], "all green");
    assert_eq!(
        frames[0]["params"]["_meta"]["dev.cyber-code/progress"]["kind"],
        "admitted"
    );
    let session = first["session_id"].as_str().unwrap().to_owned();

    // A receipt without waiting; the run finishes before the restart.
    let receipt = fixture
        .client
        .tool(
            &mcp,
            "cyber_prompt",
            json!({"session_id": session, "prompt": "and the lint", "wait": false,
                "idempotency_key": "app-2"}),
        )
        .await["structuredContent"]
        .clone();
    assert_eq!(receipt["done"], false, "{receipt}");
    tokio::time::timeout(
        Duration::from_secs(20),
        fixture.application.runtime.wait_idle(&session),
    )
    .await
    .expect("the Turn settled");

    let fixture = fixture.restart().await;
    let mcp = fixture.client.initialize(Some("codex")).await;
    let ids = json!({"session_id": session, "message_id": receipt["message_id"]});
    let done =
        fixture.client.tool(&mcp, "cyber_prompt_wait", ids).await["structuredContent"].clone();
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["final_text"], "all green");
    assert_eq!(done["admitted_at"], receipt["admitted_at"]);
    // A retry under the first key replays from the stored receipt after the restart.
    let replayed = fixture
        .client
        .tool(
            &mcp,
            "cyber_prompt",
            json!({"directory": directory, "model": "test/main", "prompt": "fix the failing test",
                "idempotency_key": "app-1", "wait": false}),
        )
        .await["structuredContent"]
        .clone();
    assert_eq!(replayed["replayed"], true, "{replayed}");
    assert_eq!(replayed["message_id"], first["message_id"]);

    let read = fixture
        .client
        .tool(&mcp, "cyber_session_read", json!({"session_id": session}))
        .await["structuredContent"]
        .clone();
    let recent: Vec<&Value> = read["recent_prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["message_id"])
        .collect();
    assert_eq!(
        recent,
        [&receipt["message_id"], &first["message_id"]],
        "{read}"
    );
    assert_eq!(read["session"]["mcp_client"], "codex");
    // Usage attribution: the Turns MCP prompts started are `mcp` with the client label
    // (the title call is not part of a Turn).
    let stats = fixture
        .client
        .sdk(reqwest::Method::GET, "/usage/stats?period=all", None)
        .await;
    let origins: Vec<(&Value, &Value)> = stats["data"]["origins"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["turns"] != 0)
        .map(|o| (&o["origin"], &o["turns"]))
        .collect();
    assert_eq!(origins, [(&json!("mcp"), &json!(2))], "{stats}");
    let events = fixture
        .application
        .store
        .read_events(&session, -1, 500)
        .unwrap()
        .events;
    let steps: Vec<(&Value, &Value)> = events
        .iter()
        .filter(|e| e.kind == "session.step.ended.1")
        .map(|e| (&e.data["origin"], &e.data["mcp_client"]))
        .collect();
    assert_eq!(steps, [(&json!("mcp"), &json!("codex")); 2]);
    fixture.close().await;
}

#[tokio::test]
async fn production_server_runs_configured_prompt_hooks_for_mcp_prompts() {
    let model = Model::scripted(short_answer);
    let deny = r#"echo '{"decision":"deny","reason":"blocked by the prompt policy"}'"#;
    let fixture = Fixture::start(
        &model,
        json!({"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"command","command":deny}]}]}}),
        json!({}),
    )
    .await;
    let mcp = fixture.client.initialize(Some("codex")).await;
    let directory = fixture.repo.display().to_string();
    let created = fixture
        .client
        .tool(
            &mcp,
            "cyber_session_create",
            json!({"directory": directory, "model": "test/main"}),
        )
        .await;
    let session = created["structuredContent"]["session"]["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let denied = fixture
        .client
        .tool_error(
            &mcp,
            "cyber_prompt",
            json!({"session_id": session, "prompt": "do the thing"}),
        )
        .await;
    assert!(
        denied["message"]
            .as_str()
            .is_some_and(|m| m.contains("blocked by the prompt policy")),
        "{denied}"
    );
    let state = fixture.application.runtime.state(&session).await.unwrap();
    assert!(
        state.inbox.is_empty(),
        "nothing admitted: {:?}",
        state.inbox
    );
    assert!(
        model
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|body| !body["messages"].to_string().contains("do the thing")),
        "the denied prompt never reached the model"
    );
    fixture.close().await;
}

/// The text of a request's first user message (the prompt that started the Session).
fn first_user_text(body: &Value) -> String {
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    let Some(user) = messages.into_iter().find(|m| m["role"] == "user") else {
        return String::new();
    };
    message_text(&user)
}

/// A message's text: the string content, or its text parts joined.
fn message_text(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        parts => parts
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| part["text"].as_str())
            .collect(),
    }
}

/// The content of the request's last message when it is a tool result.
fn last_tool_result(body: &Value) -> Option<String> {
    let last = body["messages"].as_array()?.last()?.clone();
    (last["role"] == "tool").then(|| last["content"].to_string())
}

/// `marker dir=<path>|` in a prompt: the path.
fn marked_directory(text: &str) -> String {
    let start = text.find("dir=").map_or(0, |i| i + 4);
    text[start..]
        .split('|')
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn call_tool(name: &str, arguments: Value) -> Value {
    json!({"tool_calls":[{"index":0,"id":format!("call_{name}"),"type":"function",
        "function":{"name":format!("mcp__self__{name}"),"arguments":arguments.to_string()}}]})
}

/// A Session prompted `LEVEL-A dir=<B's directory>|` prompts a new Session there through
/// the configured `self` MCP server; that Session (`LEVEL-B`) tries to create another.
/// `SELF-CHECK` lists the Sessions and prompts the one it finds (itself).
fn recursive(body: &Value) -> Value {
    if body["messages"].to_string().contains("Write a short title") {
        return json!({"content": "Recursion"});
    }
    let user = first_user_text(body);
    match last_tool_result(body) {
        None if user.contains("LEVEL-A") => {
            let directory = marked_directory(&user);
            call_tool(
                "cyber_prompt",
                json!({"directory": directory, "model": "test/main",
                    "prompt": format!("LEVEL-B dir={directory}|"), "wait_seconds": 60}),
            )
        }
        None if user.contains("LEVEL-B") => call_tool(
            "cyber_session_create",
            json!({"directory": marked_directory(&user)}),
        ),
        None if user.contains("SELF-CHECK") => call_tool("cyber_session_list", json!({})),
        Some(result)
            if user.contains("SELF-CHECK")
                && result.contains("ses_")
                && !result.contains("self_target") =>
        {
            let start = result.find("ses_").unwrap();
            let id: String = result[start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            call_tool(
                "cyber_prompt",
                json!({"session_id": id, "prompt": "again", "wait": false}),
            )
        }
        _ => json!({"content": "finished"}),
    }
}

/// The production application with its own MCP serve endpoint configured as the remote
/// MCP server `self`, so Turns reach it through the real MCP pool.
async fn recursive_fixture(model: &Model) -> Fixture {
    let fixture = Fixture::start(model, json!({}), json!({})).await;
    let tools = ["cyber_prompt", "cyber_session_create", "cyber_session_list"];
    let permissions: serde_json::Map<String, Value> = tools
        .iter()
        .map(|tool| (format!("mcp__self__{tool}"), json!("allow")))
        .collect();
    fixture.write_global(
        model,
        json!({
            "permissions": permissions,
            "mcp":{"self":{"type":"remote","url":format!("{}/mcp/serve", fixture.client.api),
                "required": true,
                "headers":{"authorization":"Basic Y3liZXI6dGVzdC1wYXNzd29yZC0xMjM0NTY="}}},
        }),
    );
    fixture
}

/// The requests whose prompt contains `marker` and that carry a tool result.
fn tool_results(model: &Model, marker: &str) -> Vec<String> {
    model
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|body| first_user_text(body).contains(marker))
        .filter_map(last_tool_result)
        .collect()
}

#[tokio::test]
async fn production_mcp_pool_calls_are_bounded_by_the_recursion_guard() {
    let model = Model::scripted(recursive);
    let fixture = recursive_fixture(&model).await;
    let other = fixture.root.join("other");
    std::fs::create_dir_all(other.join(".git")).unwrap();
    let mcp = fixture.client.initialize(Some("claude-code")).await;
    let prompt = format!("LEVEL-A dir={}|", other.display());
    let result = fixture
        .client
        .tool(
            &mcp,
            "cyber_prompt",
            json!({"directory": fixture.repo.display().to_string(), "model": "test/main",
                "prompt": prompt, "wait_seconds": 120}),
        )
        .await["structuredContent"]
        .clone();
    assert_eq!(result["status"], "completed", "{result}");

    // A (depth 0) created and ran B through the pool (depth 1); B's own creation (depth
    // 2) was refused.
    let from_b = tool_results(&model, "LEVEL-B");
    assert_eq!(from_b.len(), 1, "{from_b:?}");
    assert!(from_b[0].contains("depth_exceeded"), "{}", from_b[0]);
    assert!(from_b[0].contains(r#"\"depth\":2"#), "{}", from_b[0]);
    let from_a = tool_results(&model, "LEVEL-A");
    assert_eq!(from_a.len(), 1, "{from_a:?}");
    assert!(from_a[0].contains("completed"), "{}", from_a[0]);
    let listed = fixture
        .client
        .tool(&mcp, "cyber_session_list", json!({}))
        .await["structuredContent"]
        .clone();
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 2, "{listed}");
    fixture.close().await;
}

#[tokio::test]
async fn production_mcp_pool_calls_cannot_prompt_their_own_session() {
    let model = Model::scripted(recursive);
    let fixture = recursive_fixture(&model).await;
    let mcp = fixture.client.initialize(Some("claude-code")).await;
    let result = fixture
        .client
        .tool(
            &mcp,
            "cyber_prompt",
            json!({"directory": fixture.repo.display().to_string(), "model": "test/main",
                "prompt": "SELF-CHECK", "wait_seconds": 120}),
        )
        .await["structuredContent"]
        .clone();
    assert_eq!(result["status"], "completed", "{result}");
    let session = result["session_id"].as_str().unwrap();
    let results = tool_results(&model, "SELF-CHECK");
    assert_eq!(results.len(), 2, "{results:?}");
    assert!(
        results[0].contains(session),
        "listed itself: {}",
        results[0]
    );
    assert!(results[1].contains("self_target"), "{}", results[1]);
    let state = fixture.application.runtime.state(session).await.unwrap();
    assert_eq!(
        state.inbox.len(),
        1,
        "only the external prompt was admitted"
    );
    fixture.close().await;
}

/// The text of the request's last user message (a prompt or a cross-session message).
fn last_user_text(body: &Value) -> String {
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    messages
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .map(message_text)
        .unwrap_or_default()
}

/// One tool call with its own ID, so repeated calls in a Session stay distinct.
fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"tool_calls":[{"index":0,"id":id,"type":"function",
        "function":{"name":name,"arguments":arguments.to_string()}}]})
}

/// The `mcp-server` ping-pong: A (`PING-A sid=<B>|`) prompts B through the `self` MCP
/// server without waiting; B (`PONG-B`) wakes A with the native `send_message`; A, woken
/// by the message, prompts B again. `AGAIN dir=<path>|` prompts a new Session there.
fn ping_pong(body: &Value) -> Value {
    if body["messages"].to_string().contains("Write a short title") {
        return json!({"content": "Ping"});
    }
    if last_tool_result(body).is_some() {
        return json!({"content": "finished"});
    }
    let (first, last) = (first_user_text(body), last_user_text(body));
    let b = || marked_directory(&first.replace("sid=", "dir="));
    if last.contains("PONG-B") {
        call(
            "call_send",
            "send_message",
            json!({"to": "alpha", "text": "PING-BACK"}),
        )
    } else if last.contains("cross_session_message") {
        call(
            "call_prompt_2",
            "mcp__self__cyber_prompt",
            json!({"session_id": b(), "prompt": "PONG-AGAIN", "wait": false}),
        )
    } else if last.contains("PING-A") {
        call(
            "call_prompt_1",
            "mcp__self__cyber_prompt",
            json!({"session_id": b(), "prompt": "PONG-B", "wait": false}),
        )
    } else if last.contains("AGAIN") {
        call(
            "call_prompt_3",
            "mcp__self__cyber_prompt",
            json!({"directory": marked_directory(&last), "model": "test/main",
                "prompt": "LEAF", "wait": false}),
        )
    } else {
        json!({"content": "finished"})
    }
}

/// The tool results of the requests made for Session `marker`'s first prompt, in order.
async fn results_until(model: &Model, marker: &str, count: usize) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let results = tool_results(model, marker);
        if results.len() >= count || tokio::time::Instant::now() > deadline {
            return results;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Task 7 (`mcp-server` → MCP recursion guard, scenarios "Ping-pong between existing
/// Sessions" and "Messaged Session uses MCP again after its run") on the production
/// application: a `send_message` leg carries the sender's depth into the run it wakes.
#[tokio::test]
async fn a_send_message_leg_carries_the_depth_and_the_ping_pong_is_refused() {
    let model = Model::scripted(ping_pong);
    let fixture = recursive_fixture(&model).await;
    let (client, runtime) = (&fixture.client, &fixture.application.runtime);
    let create = |title: &'static str| {
        client.sdk(
            reqwest::Method::POST,
            "/sessions",
            Some(json!({"model": "test/main", "title": title})),
        )
    };
    let a = create("alpha").await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let b = create("beta").await["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let path = format!("/sessions/{a}/prompt");
    let prompt = |text: String| {
        client.sdk(
            reqwest::Method::POST,
            &path,
            Some(json!({"parts":[{"type":"text","text":text}]})),
        )
    };
    prompt(format!("PING-A sid={b}|")).await;

    // A (run at depth 0) prompted idle B (depth 1); B's `send_message` woke A, whose run
    // then had depth 1, so A's `cyber_prompt` on B was depth 2 and refused.
    let from_a = results_until(&model, "PING-A", 2).await;
    assert_eq!(from_a.len(), 2, "{from_a:?}");
    assert!(!from_a[0].contains("depth_exceeded"), "{}", from_a[0]);
    assert!(from_a[1].contains("depth_exceeded"), "{}", from_a[1]);
    assert!(from_a[1].contains(r#"\"depth\":2"#), "{}", from_a[1]);
    let from_b = results_until(&model, "PONG-B", 1).await;
    assert!(from_b[0].contains("delivered"), "{from_b:?}");
    let received = runtime.message_rows(&a).unwrap();
    assert_eq!(received.len(), 1, "{received:?}");
    assert_eq!(received[0].depth, 1, "the message stores B's run depth");
    let wait = |id: String| async move {
        tokio::time::timeout(Duration::from_secs(60), runtime.wait_idle(&id))
            .await
            .expect("the Session settled");
    };
    wait(a.clone()).await;
    wait(b.clone()).await;
    let b_state = runtime.state(&b).await.unwrap();
    assert!(
        !b_state.inbox.iter().any(|row| row
            .parts
            .iter()
            .any(|p| { serde_json::to_string(p).unwrap().contains("PONG-AGAIN") })),
        "the refused prompt admitted nothing"
    );

    // After that run, a user prompt starts A at depth 0: its request is depth 1 and works.
    let other = fixture.root.join("other");
    std::fs::create_dir_all(other.join(".git")).unwrap();
    prompt(format!("AGAIN dir={}|", other.display())).await;
    let from_a = results_until(&model, "PING-A", 3).await;
    assert_eq!(from_a.len(), 3, "{from_a:?}");
    assert!(!from_a[2].contains("depth_exceeded"), "{}", from_a[2]);
    assert!(from_a[2].contains("ses_"), "{}", from_a[2]);
    wait(a.clone()).await;
    fixture.close().await;
}

impl Client {
    /// A `tools/call` with `extra` params (such as the caller `_meta`); its `result`.
    async fn call_with(&self, session: &str, name: &str, arguments: Value, extra: Value) -> Value {
        let mut params = json!({"name": name, "arguments": arguments});
        for (key, value) in extra.as_object().into_iter().flatten() {
            params[key] = value.clone();
        }
        let frame = json!({"jsonrpc":"2.0","id":name,"method":"tools/call","params":params});
        let (status, _, body) = self.mcp(Some(session), frame).await;
        assert_eq!(status, 200, "{body}");
        body["result"].clone()
    }

    async fn session(&self, body: Value) -> String {
        let created = self
            .sdk(reqwest::Method::POST, "/sessions", Some(body))
            .await;
        created["data"]["id"].as_str().unwrap().to_owned()
    }
}

/// The JSON of a tool result's text content (the error body of an `isError` result).
fn content(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// The `_meta` caller marker Cyber's own MCP client adds for Session `id`.
fn as_caller(id: &str) -> Value {
    json!({"_meta": {"dev.cyber-code/caller": {"session_id": id, "depth": 0}}})
}

struct Messaging<'a> {
    fixture: &'a Fixture,
    mcp: String,
}

impl Messaging<'_> {
    async fn send(&self, arguments: Value) -> Value {
        self.fixture
            .client
            .tool(&self.mcp, "cyber_message_send", arguments)
            .await["structuredContent"]
            .clone()
    }

    async fn send_error(&self, arguments: Value) -> Value {
        self.fixture
            .client
            .tool_error(&self.mcp, "cyber_message_send", arguments)
            .await
    }

    async fn history(&self, arguments: Value) -> Value {
        self.fixture
            .client
            .tool(&self.mcp, "cyber_message_history", arguments)
            .await["structuredContent"]
            .clone()
    }

    async fn settle(&self, id: &str) {
        tokio::time::timeout(
            Duration::from_secs(30),
            self.fixture.application.runtime.wait_idle(id),
        )
        .await
        .expect("the Session settled");
    }

    /// `cyber_message_send` marked as called by Session `caller` at `depth`; its result.
    async fn send_as(&self, caller: &str, arguments: Value, depth: u32) -> Value {
        let meta =
            json!({"_meta": {"dev.cyber-code/caller": {"session_id": caller, "depth": depth}}});
        self.fixture
            .client
            .call_with(&self.mcp, "cyber_message_send", arguments, meta)
            .await
    }

    fn records(&self, id: &str) -> Vec<MessageRow> {
        self.fixture.application.runtime.message_rows(id).unwrap()
    }
}

/// "Send to a running Session" with an idle target, and the replay: one message, its
/// history entry, origin `client`, and the Turn it starts attributed to `mcp`.
async fn a_client_message_is_delivered_once(m: &Messaging<'_>, b: &str) {
    let arguments = json!({"to": "migration", "text": "schema v2 is merged",
        "idempotency_key": "send-1"});
    let sent = m.send(arguments.clone()).await;
    assert_eq!(sent["outcome"], "delivered", "{sent}");
    assert_eq!(sent["to"]["session_id"], json!(b));
    let replayed = m.send(arguments.clone()).await;
    assert_eq!(replayed["replayed"], true, "{replayed}");
    assert_eq!(replayed["message_id"], sent["message_id"]);
    let mut other = arguments;
    other["text"] = json!("schema v3 is merged");
    let mismatch = m.send_error(other).await;
    assert_eq!(mismatch["reason"], "idempotency_mismatch", "{mismatch}");
    m.settle(b).await;
    the_client_message_is_recorded_once(m, b, &sent["message_id"]).await;
    the_message_turn_is_mcp_usage(m, b);
}

async fn the_client_message_is_recorded_once(m: &Messaging<'_>, b: &str, message_id: &Value) {
    let records = m.records(b);
    assert_eq!(records.len(), 1, "one message: {records:?}");
    assert_eq!(records[0].origin_kind, "client");
    assert_eq!(records[0].peer_address, "mcp:zed");
    assert_eq!(records[0].sender_key, "mcp:zed");
    let history = m.history(json!({"session_id": b})).await;
    let messages = history["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{history}");
    let message = &messages[0];
    assert_eq!(&message["message_id"], message_id);
    assert_eq!(message["direction"], "in");
    assert_eq!(message["outcome"], "delivered");
    assert_eq!(message["text"], "schema v2 is merged");
    assert!(message["created_at"].is_i64(), "{history}");
    assert_eq!(history["truncated"], false);
}

/// The Turn the message started is MCP-driven usage with the client label.
fn the_message_turn_is_mcp_usage(m: &Messaging<'_>, b: &str) {
    let events = m
        .fixture
        .application
        .store
        .read_events(b, -1, 500)
        .unwrap()
        .events;
    let steps: Vec<(&Value, &Value)> = events
        .iter()
        .filter(|e| e.kind == "session.step.ended.1")
        .map(|e| (&e.data["origin"], &e.data["mcp_client"]))
        .collect();
    assert_eq!(steps, [(&json!("mcp"), &json!("zed"))], "{events:?}");
}

/// "Cyber Session sends as itself": A's address is refused by B's rule.
async fn a_caller_session_sends_as_itself(m: &Messaging<'_>, a: &str, b: &str) {
    let result = m
        .send_as(a, json!({"to": "migration", "text": "from alpha"}), 0)
        .await;
    assert_eq!(result["isError"], false, "{result}");
    let sent = &result["structuredContent"];
    assert_eq!(sent["outcome"], "refused", "{sent}");
    assert_eq!(sent["reason"], "inbound_rule");
    let out = m.records(a);
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0].direction, Direction::Out);
    assert_eq!(out[0].origin_kind, "session");
    let refused = m
        .records(b)
        .into_iter()
        .find(|r| r.origin_kind == "session");
    assert!(
        refused.is_some_and(|r| r.text.is_none()),
        "metadata only in B"
    );
}

/// The self-target and depth guards refuse before anything is sent; a message A sends is
/// stored with the request depth (A's run depth + 1), which the run that reads it takes.
async fn the_recursion_guard_applies_to_sends(m: &Messaging<'_>, a: &str, d: &str) {
    let sent_before = m.records(a).len();
    let itself = m.send_as(a, json!({"to": a, "text": "me"}), 0).await;
    assert_eq!(content(&itself)["reason"], "self_target", "{itself}");
    let deep = m
        .send_as(a, json!({"to": "migration", "text": "deep"}), 2)
        .await;
    assert_eq!(content(&deep)["reason"], "depth_exceeded", "{deep}");
    assert_eq!(
        m.records(a).len(),
        sent_before,
        "the refused calls sent nothing"
    );

    let probe = m.send_as(a, json!({"to": d, "text": "how deep?"}), 0).await;
    let outcome = &probe["structuredContent"]["outcome"];
    assert_eq!(outcome, "delivered", "{probe}");
    let received = m.records(d);
    assert_eq!((received.len(), received[0].depth), (1, 1), "{received:?}");
    assert_eq!(received[0].origin_kind, "session");
    m.settle(d).await;
}

/// "Exec Session refuses", "Oversized message" and an ambiguous address.
async fn refusals_and_errors_pass_through(m: &Messaging<'_>, exec: &str) {
    let sent = m.send(json!({"to": exec, "text": "are you there?"})).await;
    assert_eq!(
        (sent["outcome"].as_str(), sent["reason"].as_str()),
        (Some("refused"), Some("non_interactive")),
        "{sent}"
    );
    let large = m
        .send_error(json!({"to": "migration", "text": "x".repeat(20 * 1024)}))
        .await;
    assert_eq!(large["_tag"], "MessageTooLargeError", "{large}");
    let ambiguous = m.send_error(json!({"to": "twin", "text": "which?"})).await;
    assert_eq!(ambiguous["_tag"], "AmbiguousAddressError", "{ambiguous}");
    assert_eq!(ambiguous["candidates"].as_array().map(Vec::len), Some(2));
    let missing = m.send_error(json!({"to": "nobody", "text": "hi"})).await;
    assert_eq!(missing["_tag"], "AddressNotFoundError", "{missing}");
}

/// Regression: a client's `rate_limited` refusal records nothing, so a retry with its
/// idempotency key is no replay: it is checked again in full, and at a depth over the
/// limit it fails with `depth_exceeded` and sends nothing.
async fn a_retry_after_an_unrecorded_refusal_is_checked_again(m: &Messaging<'_>, e: &str) {
    for n in 0..10 {
        let sent = m.send(json!({"to": e, "text": format!("burst {n}")})).await;
        assert_eq!(sent["outcome"], "delivered", "{sent}");
    }
    let arguments = json!({"to": e, "text": "one more", "idempotency_key": "burst-11"});
    let limited = m.send(arguments.clone()).await;
    assert_eq!(
        (limited["outcome"].as_str(), limited["reason"].as_str()),
        (Some("refused"), Some("rate_limited")),
        "{limited}"
    );
    let deep = json!({"_meta": {"dev.cyber-code/caller": {"depth": 50}}});
    let retried = m
        .fixture
        .client
        .call_with(&m.mcp, "cyber_message_send", arguments, deep)
        .await;
    assert_eq!(retried["isError"], true, "{retried}");
    assert_eq!(content(&retried)["reason"], "depth_exceeded", "{retried}");
    assert_eq!(m.records(e).len(), 10, "the retry sent nothing");
    m.settle(e).await;
}

/// Regression: without a caller Session the sender is the MCP client `mcp:<label>`, so a
/// target's `refuse: ["mcp:*"]` refuses it (it used to be `client:mcp:<label>`, which the
/// pattern never matched, and the message was delivered), and its key is `mcp:<label>`.
async fn an_mcp_client_matches_mcp_patterns(m: &Messaging<'_>, guarded: &str) {
    let sent = m.send(json!({"to": guarded, "text": "let me in"})).await;
    assert_eq!(
        (sent["outcome"].as_str(), sent["reason"].as_str()),
        (Some("refused"), Some("inbound_rule")),
        "{sent}"
    );
    let records = m.records(guarded);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].direction, Direction::In);
    assert_eq!(records[0].sender_key, "mcp:zed");
    assert_eq!(records[0].peer_address, "mcp:zed");
    assert!(records[0].text.is_none(), "metadata only");
    let state = m.fixture.application.runtime.state(guarded).await.unwrap();
    assert!(state.inbox.is_empty(), "nothing admitted");
}

/// History pages bounded by `max_bytes`: one record per page, every record once, in order.
async fn history_pages_fit_max_bytes(m: &Messaging<'_>, c: &str) {
    let mut sent = Vec::new();
    for n in 0..3 {
        let text = format!("{n} {}", "y".repeat(600));
        sent.push(m.send(json!({"to": c, "text": text})).await["message_id"].clone());
    }
    let mut seen = Vec::new();
    let mut cursor: Option<Value> = None;
    loop {
        let mut arguments = json!({"session_id": c, "max_bytes": 1024});
        if let Some(cursor) = &cursor {
            arguments["cursor"] = cursor.clone();
        }
        let page = m.history(arguments).await;
        assert!(page.to_string().len() < 1024 + 200, "{page}");
        let messages = page["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1, "{page}");
        seen.push(messages[0]["message_id"].clone());
        match page.get("next_cursor") {
            Some(next) => cursor = Some(next.clone()),
            None => break,
        }
    }
    assert_eq!(seen, sent);
    let long = m.send(json!({"to": c, "text": "z".repeat(4000)})).await;
    assert_eq!(long["outcome"], "delivered", "{long}");
    let cut = m
        .history(json!({"session_id": c, "cursor": cursor_after(m, c, 3).await, "max_bytes": 1024}))
        .await;
    assert_eq!(cut["truncated"], true, "{cut}");
    assert!(
        cut["messages"][0]["text"]
            .as_str()
            .unwrap()
            .ends_with("bytes]")
    );
}

/// The history cursor after the first `n` records of `session`.
async fn cursor_after(m: &Messaging<'_>, session: &str, n: u64) -> Value {
    let page = m.history(json!({"session_id": session, "limit": n})).await;
    page["next_cursor"].clone()
}

/// `mcp-server` → Messaging over MCP on the production application: every scenario of
/// the requirement, plus the replay, the guards and history paging.
#[tokio::test]
async fn messaging_tools_map_onto_the_messaging_api() {
    let model = Model::scripted(short_answer);
    let project = json!({"messaging": {"inbound": {"refuse": ["alpha"]}}});
    let fixture = Fixture::start(&model, json!({}), project).await;
    let client = &fixture.client;
    let titled = |title: &str| json!({"model": "test/main", "title": title});
    let a = client.session(titled("alpha")).await;
    let b = client.session(titled("migration")).await;
    let c = client.session(titled("archive-notes")).await;
    // In another Location, outside the project rule that refuses alpha.
    let other = fixture.root.join("other");
    std::fs::create_dir_all(other.join(".git")).unwrap();
    let elsewhere = Client {
        http: client.http.clone(),
        api: client.api.clone(),
        directory: other.display().to_string(),
    };
    let d = elsewhere.session(titled("depth-probe")).await;
    let exec = client
        .session(json!({"model": "test/main", "title": "exec run",
            "messaging": {"inbound": "refuse", "reason": "non_interactive"}}))
        .await;
    let e = client.session(titled("rate-probe")).await;
    // A Location whose project refuses every MCP client.
    let guarded_dir = fixture.root.join("guarded");
    std::fs::create_dir_all(guarded_dir.join(".git")).unwrap();
    let refuse_mcp = json!({"messaging": {"inbound": {"refuse": ["mcp:*"]}}});
    std::fs::write(guarded_dir.join("cyber.jsonc"), refuse_mcp.to_string()).unwrap();
    let guarded = Client {
        http: client.http.clone(),
        api: client.api.clone(),
        directory: guarded_dir.display().to_string(),
    }
    .session(titled("guarded"))
    .await;
    client.session(titled("twin")).await;
    client.session(titled("twin")).await;
    let m = Messaging {
        mcp: client.initialize(Some("zed")).await,
        fixture: &fixture,
    };

    let listed = client
        .call_with(&m.mcp, "cyber_message_list", json!({}), as_caller(&a))
        .await;
    let sessions = listed["structuredContent"]["sessions"].as_array().unwrap();
    assert!(
        sessions.iter().all(|s| s["session_id"] != json!(a)),
        "{listed}"
    );
    let exec_row = sessions
        .iter()
        .find(|s| s["session_id"] == json!(exec))
        .unwrap();
    assert_eq!(exec_row["canReceive"], false, "{exec_row}");
    let b_row = sessions
        .iter()
        .find(|s| s["address"] == "migration")
        .unwrap();
    assert_eq!(b_row["canReceive"], false, "B refuses alpha: {b_row}");

    a_client_message_is_delivered_once(&m, &b).await;
    an_mcp_client_matches_mcp_patterns(&m, &guarded).await;
    a_caller_session_sends_as_itself(&m, &a, &b).await;
    the_recursion_guard_applies_to_sends(&m, &a, &d).await;
    refusals_and_errors_pass_through(&m, &exec).await;
    history_pages_fit_max_bytes(&m, &c).await;
    m.settle(&c).await;
    a_retry_after_an_unrecorded_refusal_is_checked_again(&m, &e).await;

    // The served tools are never offered to Cyber's own agents.
    let offered = model
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|body| body["tools"].to_string().contains("cyber_message_send"));
    assert!(!offered, "cyber_message_send is not an agent tool");
    fixture.close().await;
}
