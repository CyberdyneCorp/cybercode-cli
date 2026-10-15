//! MCP serve through the production application: the real `App` (AppHost, AppServices,
//! global configuration) on a TCP listener with the local password, a Turn run by a
//! local OpenAI-compatible model through the ordinary SDK routes, then read over
//! `/api/v1/mcp/serve`.

use std::sync::Arc;
use std::time::Duration;

use cyber_app::{App, AppOptions};
use cyber_core::paths::{DatabaseLocation, Paths};
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
    match &user["content"] {
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
