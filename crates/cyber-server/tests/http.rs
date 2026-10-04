//! The HTTP API over the runtime, through the embedded transport and a real TCP listener.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use cyber_server::http::{
    self, AgentInfo, AppState, CommandInfo, EmbeddedClient, HttpOptions, ModelInfo, Services,
};
use cyber_server::runtime::{ToolDef, TurnContext};
use futures::future::BoxFuture;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use support::{Behavior, Harness, Setup, text, tools};

struct FakeServices {
    tools: Arc<support::Tools>,
}

impl Services for FakeServices {
    fn models(&self, _location: &Path) -> BoxFuture<'_, Result<Vec<ModelInfo>, String>> {
        Box::pin(async {
            Ok(vec![ModelInfo {
                id: "test/main".into(),
                provider: "test".into(),
                name: "Main".into(),
                available: true,
                context_limit: 200_000,
                reasoning: false,
            }])
        })
    }
    fn default_model(&self, _location: &Path) -> Option<String> {
        Some("test/main".into())
    }
    fn agents(&self, _location: &Path) -> Vec<AgentInfo> {
        vec![AgentInfo {
            name: "build".into(),
            description: "Default agent".into(),
            mode: "primary".into(),
        }]
    }
    fn tools(&self, turn: &TurnContext) -> Vec<ToolDef> {
        cyber_server::runtime::ToolHost::definitions(&*self.tools, turn)
    }
    fn commands(&self, _location: &Path) -> Vec<CommandInfo> {
        Vec::new()
    }
    fn find_files(&self, _location: &Path, query: &str, _limit: usize) -> Vec<String> {
        vec![format!("src/{query}.rs")]
    }
    fn expand_command(&self, _location: &Path, name: &str, arguments: &str) -> Option<String> {
        (name == "greet").then(|| format!("Say hello to {arguments}"))
    }
}

fn state(h: &Harness, password: Option<&str>) -> AppState {
    AppState {
        runtime: h.runtime.clone(),
        remote_tools: Arc::default(),
        store: Arc::clone(&h.store),
        services: Arc::new(FakeServices {
            tools: Arc::clone(&h.tools),
        }),
        options: Arc::new(HttpOptions {
            version: "0.1.0-test".into(),
            password: password.map(str::to_string),
            cors_origins: Vec::new(),
            default_directory: h.repo.clone(),
            features: vec!["session".into()],
        }),
    }
}

struct Api {
    client: EmbeddedClient,
}

impl Api {
    fn new(h: &Harness) -> Self {
        Self {
            client: EmbeddedClient::new(http::router(state(h, None))),
        }
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Value, axum::http::HeaderMap) {
        let mut req = Request::builder()
            .method(method)
            .uri(format!("http://cyber.internal/api/v1{path}"));
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string()))
                .unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = self.client.request(req).await;
        let (status, headers) = (res.status(), res.headers().clone());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value, headers)
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let (s, v, _) = self.call(Method::GET, path, None, &[]).await;
        (s, v)
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let (s, v, _) = self.call(Method::POST, path, Some(body), &[]).await;
        (s, v)
    }
}

fn prompt(text: &str) -> Value {
    json!({ "parts": [{ "type": "text", "text": text }] })
}

#[tokio::test]
async fn create_prompt_and_read_messages() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("hello there")])],
        ..Setup::default()
    });
    let api = Api::new(&h);
    let (status, created) = api.post("/sessions", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        created["location"]["directory"],
        json!(
            std::fs::canonicalize(&h.repo)
                .unwrap()
                .display()
                .to_string()
        )
    );
    let id = created["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["data"]["status"], "idle");

    let (status, receipt) = api
        .post(&format!("/sessions/{id}/prompt"), prompt("hi"))
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    assert_eq!(receipt["data"]["delivery"], "steer");
    h.settle(&id).await;

    let (_, page) = api.get(&format!("/sessions/{id}/messages?order=asc")).await;
    let kinds: Vec<&str> = page["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["user", "assistant"]);
    assert_eq!(page["data"][1]["text"], "hello there");

    let (_, list) = api.get("/sessions").await;
    assert_eq!(list["data"]["data"][0]["id"], json!(id));
}

#[tokio::test]
async fn errors_are_tagged() {
    let h = Harness::new(Setup::default());
    let api = Api::new(&h);
    let (status, body) = api.get("/sessions/ses_missing").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["_tag"], "SessionNotFoundError");
    let (status, body) = api
        .post("/sessions/ses_missing/mode", json!({"mode": "yolo"}))
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidRequestError"))
    );
    let (status, body) = api.get("/sessions?cursor=garbage").await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidCursorError"))
    );
}

#[tokio::test]
async fn idempotency_keys_replay_and_reject_mismatches() {
    let h = Harness::new(Setup::default());
    let api = Api::new(&h);
    let key = [("idempotency-key", "k-1")];
    let (s1, first, _) = api
        .call(
            Method::POST,
            "/sessions",
            Some(json!({"title": "one"})),
            &key,
        )
        .await;
    let (s2, again, headers) = api
        .call(
            Method::POST,
            "/sessions",
            Some(json!({"title": "one"})),
            &key,
        )
        .await;
    assert_eq!((s1, s2), (StatusCode::CREATED, StatusCode::CREATED));
    assert_eq!(first["data"]["id"], again["data"]["id"]);
    assert_eq!(headers.get("idempotent-replayed").unwrap(), "true");
    let (s3, body, _) = api
        .call(
            Method::POST,
            "/sessions",
            Some(json!({"title": "two"})),
            &key,
        )
        .await;
    assert_eq!(
        (s3, body["_tag"].as_str()),
        (StatusCode::CONFLICT, Some("ConflictError"))
    );
}

#[tokio::test]
async fn permission_requests_are_listed_and_replied() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "shell", "{}")]), text("done")],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Ask("rm -rf build".into()));
    let api = Api::new(&h);
    let id = h.session().await;
    api.post(&format!("/sessions/{id}/prompt"), prompt("clean"))
        .await;
    let request = loop {
        let (_, list) = api.get("/permissions/requests").await;
        if let Some(r) = list["data"].as_array().and_then(|a| a.first()).cloned() {
            break r;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(request["action"], "bash");
    let rid = request["id"].as_str().unwrap();
    let (status, _) = api
        .post(
            &format!("/sessions/{id}/permissions/{rid}/reply"),
            json!({"reply": "once"}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    h.settle(&id).await;
    let (_, page) = api.get(&format!("/sessions/{id}/messages?order=asc")).await;
    assert_eq!(page["data"][1]["tools"][0]["output"], "ran rm -rf build");
}

#[tokio::test]
async fn history_pages_durable_events() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("ok")])],
        ..Setup::default()
    });
    let api = Api::new(&h);
    let id = h.session().await;
    api.post(&format!("/sessions/{id}/prompt"), prompt("hi"))
        .await;
    h.settle(&id).await;
    let (_, page) = api.get(&format!("/sessions/{id}/history?limit=2")).await;
    assert_eq!(page["data"].as_array().unwrap().len(), 2);
    assert_eq!(page["hasMore"], true);
    assert_eq!(page["data"][0]["type"], "session.created.1");
    assert_eq!(page["data"][0]["durable"]["seq"], 0);
    let (status, _) = api.get(&format!("/sessions/{id}/history?limit=501")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn every_documented_operation_is_routed() {
    let h = Harness::new(Setup::default());
    let api = Api::new(&h);
    let id = h.session().await;
    let doc = http::openapi::document("test");
    let mut ids = std::collections::HashSet::new();
    for (method, path) in http::openapi::routes() {
        if path.ends_with("/events") || path == "/event" || path == "/ws" {
            continue;
        }
        let concrete = path
            .replace("{sessionID}", &id)
            .replace("{messageID}", "msg_x")
            .replace("{requestID}", "per_x");
        let m = Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
        let (status, body, _) = api.call(m, &concrete, Some(json!({})), &[]).await;
        // An unrouted path is an untagged 404; handler failures always carry `_tag`.
        assert!(
            status != StatusCode::NOT_FOUND || body["_tag"].is_string(),
            "{method} {path}: {status} {body}"
        );
        assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "{method} {path}");
        let op = &doc["paths"][format!("/api/v1{path}")][method];
        assert!(
            ids.insert(op["operationId"].as_str().unwrap().to_string()),
            "duplicate id for {path}"
        );
    }
    assert!(doc["components"]["schemas"]["Session"].is_object());
    assert!(doc["components"]["schemas"]["Data_Session"].is_object());
}

async fn tcp(h: &Harness, password: &str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = http::router(state(h, Some(password)));
    tokio::spawn(http::serve_tcp(router, listener, std::future::pending()));
    format!("http://{addr}/api/v1")
}

#[tokio::test]
async fn tcp_requires_the_password_except_for_health() {
    let h = Harness::new(Setup::default());
    let base = tcp(&h, "s3cret").await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{base}/health"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let denied = client.get(format!("{base}/sessions")).send().await.unwrap();
    assert_eq!(denied.status(), 401);
    let ok = client
        .get(format!("{base}/sessions"))
        .basic_auth("cyber", Some("s3cret"))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    use base64::Engine;
    let token = base64::engine::general_purpose::STANDARD.encode("cyber:s3cret");
    let ignored = client
        .get(format!("{base}/sessions?auth_token={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        ignored.status(),
        401,
        "auth_token only works on stream routes"
    );
    let stream = client
        .get(format!("{base}/event?auth_token={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    assert_eq!(stream.headers()["cache-control"], "no-cache, no-transform");
}

#[tokio::test]
async fn origins_are_checked_and_preflight_is_answered() {
    let h = Harness::new(Setup::default());
    let base = tcp(&h, "pw").await;
    let client = reqwest::Client::new();
    let evil = client
        .get(format!("{base}/health"))
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(evil.status(), 403);
    let pre = client
        .request(reqwest::Method::OPTIONS, format!("{base}/sessions"))
        .header("origin", "http://localhost:5173")
        .send()
        .await
        .unwrap();
    assert_eq!(pre.status(), 204);
    assert_eq!(pre.headers()["access-control-max-age"], "86400");
}

#[tokio::test]
async fn session_stream_replays_then_follows_without_gaps() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("first"), text("second")])],
        ..Setup::default()
    });
    let base = tcp(&h, "pw").await;
    let id = h.session().await;
    let client = reqwest::Client::new();
    let api = Api::new(&h);
    api.post(&format!("/sessions/{id}/prompt"), prompt("one"))
        .await;
    h.settle(&id).await;
    let res = client
        .get(format!("{base}/sessions/{id}/events?after=2"))
        .basic_auth("cyber", Some("pw"))
        .send()
        .await
        .unwrap();
    let mut stream = res.bytes_stream();
    api.post(&format!("/sessions/{id}/prompt"), prompt("two"))
        .await;
    let mut seqs: Vec<i64> = Vec::new();
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    use futures::StreamExt;
    while !buf.contains("second") && tokio::time::Instant::now() < deadline {
        let Ok(Some(Ok(chunk))) = tokio::time::timeout(Duration::from_secs(5), stream.next()).await
        else {
            break;
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
    }
    for line in buf.lines().filter_map(|l| l.strip_prefix("data: ")) {
        let v: Value = serde_json::from_str(line).unwrap();
        seqs.push(v["durable"]["seq"].as_i64().unwrap());
    }
    assert_eq!(seqs.first(), Some(&3));
    assert!(
        seqs.windows(2).all(|w| w[1] == w[0] + 1),
        "gapless: {seqs:?}"
    );
    assert!(buf.contains("second"));
}

#[tokio::test]
async fn large_responses_are_compressed() {
    let h = Harness::new(Setup::default());
    let base = tcp(&h, "pw").await;
    let raw = reqwest::Client::builder()
        .no_gzip()
        .no_zstd()
        .build()
        .unwrap();
    let res = raw
        .get(format!("{base}/openapi.json"))
        .basic_auth("cyber", Some("pw"))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["content-encoding"], "gzip");
}

#[tokio::test]
async fn commands_expand_into_the_prompt() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("hello Ada")])],
        ..Setup::default()
    });
    let api = Api::new(&h);
    let id = h.session().await;
    let (status, _) = api
        .post(
            &format!("/sessions/{id}/command"),
            json!({"name": "greet", "arguments": "Ada"}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    h.settle(&id).await;
    let (_, page) = api.get(&format!("/sessions/{id}/messages?order=asc")).await;
    assert_eq!(page["data"][0]["parts"][0]["text"], "Say hello to Ada");
    let (status, body) = api
        .post(&format!("/sessions/{id}/command"), json!({"name": "nope"}))
        .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (StatusCode::NOT_FOUND, Some("CommandNotFoundError"))
    );
}

/// `sdk/openapi.json` is what SDKs are generated from; regenerate with
/// `UPDATE_OPENAPI=1 cargo test -p cyber-server --test http openapi_document_is_current`.
#[test]
fn openapi_document_is_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdk/openapi.json");
    let current = serde_json::to_string_pretty(&http::openapi::document("1")).unwrap() + "\n";
    if std::env::var_os("UPDATE_OPENAPI").is_some() {
        std::fs::write(path, &current).unwrap();
    }
    let committed = std::fs::read_to_string(path).unwrap_or_default();
    assert!(
        committed == current,
        "sdk/openapi.json is stale; rerun with UPDATE_OPENAPI=1"
    );
}

#[tokio::test]
async fn websocket_json_rpc_calls_operations_and_streams_events() {
    use futures::{SinkExt, StreamExt};
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("over rpc")])],
        ..Setup::default()
    });
    let base = tcp(&h, "pw").await;
    use base64::Engine;
    let token = base64::engine::general_purpose::STANDARD.encode("cyber:pw");
    let url = format!("{}/ws?auth_token={token}", base.replace("http://", "ws://"));
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let send = |v: Value| tokio_tungstenite::tungstenite::Message::Text(v.to_string().into());
    ws.send(send(json!({"jsonrpc": "2.0", "id": 1, "method": "v1.session.create", "params": {"body": {"title": "rpc"}}}))).await.unwrap();
    let mut session = String::new();
    let mut subscribed = false;
    let mut saw_text = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && !saw_text {
        let Ok(Some(Ok(frame))) = tokio::time::timeout(Duration::from_secs(5), ws.next()).await
        else {
            break;
        };
        let v: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
        match v["id"].as_i64() {
            Some(1) => {
                session = v["result"]["data"]["id"].as_str().unwrap().to_string();
                ws.send(send(json!({"jsonrpc": "2.0", "id": 2, "method": "v1.session.events", "params": {"path": {"sessionID": session}}}))).await.unwrap();
            }
            Some(2) => {
                subscribed = v["result"]["subscription"].is_string();
                ws.send(send(json!({"jsonrpc": "2.0", "id": 3, "method": "v1.session.prompt", "params": {"path": {"sessionID": session}, "body": {"parts": [{"type": "text", "text": "hi"}]}}}))).await.unwrap();
            }
            Some(3) => assert_eq!(v["result"]["data"]["delivery"], "steer"),
            _ => {
                saw_text |= v["params"]["event"]["type"] == "session.text.ended.1"
                    && v["params"]["event"]["data"]["text"] == "over rpc"
            }
        }
    }
    assert!(subscribed && saw_text);
    ws.send(send(json!({"jsonrpc": "2.0", "id": 4, "method": "v1.session.get", "params": {"path": {"sessionID": "ses_missing"}}}))).await.unwrap();
    loop {
        let frame = ws.next().await.unwrap().unwrap();
        let v: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
        if v["id"] == 4 {
            assert_eq!(v["error"]["data"]["_tag"], "SessionNotFoundError");
            break;
        }
    }
}

/// Built-in fakes plus client-registered tools, as the app composes them.
struct Composite {
    builtin: Arc<support::Tools>,
    remote: Arc<http::remote_tools::RemoteTools>,
}

impl cyber_server::runtime::ToolHost for Composite {
    fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        let mut defs = cyber_server::runtime::ToolHost::definitions(&*self.builtin, turn);
        defs.extend(self.remote.definitions());
        defs
    }

    fn execute(
        &self,
        call: cyber_server::runtime::Invocation,
        cancel: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'_, cyber_server::runtime::ToolOutcome> {
        if self.remote.has(&call.name) {
            return Box::pin(self.remote.execute(call, cancel));
        }
        cyber_server::runtime::ToolHost::execute(&*self.builtin, call, cancel)
    }
}

#[tokio::test]
async fn clients_register_tools_that_the_model_can_call() {
    use futures::{SinkExt, StreamExt};
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let store = Arc::new(support::open_store(&dir.path().join("cyber.db")));
    let models = support::models(
        vec![(
            "test/main",
            vec![
                tools(&[("c1", "lookup_ticket", r#"{"id":"T-1"}"#)]),
                text("done"),
            ],
        )],
        200_000,
        &[],
    );
    let builtin = support::Tools::new();
    let remote = Arc::new(http::remote_tools::RemoteTools::default());
    let runtime = cyber_server::runtime::Runtime::new(cyber_server::runtime::RuntimeOptions {
        store: Arc::clone(&store),
        resolver: Arc::clone(&models) as Arc<dyn cyber_server::runtime::ModelResolver>,
        tools: Arc::new(Composite {
            builtin: Arc::clone(&builtin),
            remote: Arc::clone(&remote),
        }),
        global_config_dir: dir.path().join("global"),
        shell: "sh".into(),
        claude_compat: false,
        compaction: cyber_server::runtime::CompactionConfig::default(),
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: None,
        interactive: true,
        snapshots: Arc::new(cyber_server::runtime::NoSnapshots),
    });
    let state = AppState {
        runtime: runtime.clone(),
        remote_tools: remote,
        store,
        services: Arc::new(FakeServices { tools: builtin }),
        options: Arc::new(HttpOptions {
            version: "t".into(),
            password: None,
            cors_origins: Vec::new(),
            default_directory: repo.clone(),
            features: Vec::new(),
        }),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(http::serve_tcp(
        http::router(state),
        listener,
        std::future::pending(),
    ));
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/ws"))
        .await
        .unwrap();
    let send = |v: Value| tokio_tungstenite::tungstenite::Message::Text(v.to_string().into());
    ws.send(send(json!({"jsonrpc": "2.0", "id": 1, "method": "v1.tool.register", "params": {"name": "lookup_ticket", "description": "Find a ticket", "input": {"type": "object"}}}))).await.unwrap();
    let reply: Value =
        serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(reply["result"]["registered"], "lookup_ticket");
    let bad = json!({"jsonrpc": "2.0", "id": 2, "method": "v1.tool.register", "params": {"name": "shell"}});
    ws.send(send(bad)).await.unwrap();
    let reply: Value =
        serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("built-in")
    );

    let id = runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    runtime
        .admit(
            &id,
            cyber_server::runtime::Admission::text(
                "look up T-1",
                cyber_server::runtime::Delivery::Steer,
            ),
        )
        .await
        .unwrap();
    // The server asks this client to run the tool; answer it.
    let call: Value = loop {
        let v: Value =
            serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        if v["method"] == "tool.execute" {
            break v;
        }
    };
    assert_eq!(call["params"]["input"]["id"], "T-1");
    ws.send(send(
        json!({"jsonrpc": "2.0", "id": call["id"], "result": "T-1: login page broken"}),
    ))
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), runtime.wait_idle(&id))
        .await
        .unwrap();
    let state = runtime.state(&id).await.unwrap();
    assert_eq!(
        state.calls["c1"].output.as_deref(),
        Some("T-1: login page broken")
    );

    drop(ws);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let turn = TurnContext {
        session_id: id,
        directory: repo.display().to_string(),
        agent: "build".into(),
        mode: "default".into(),
        prefers_apply_patch: false,
        rules: Value::Null,
    };
    let offered = cyber_server::runtime::ToolHost::definitions(
        &Composite {
            builtin: support::Tools::new(),
            remote: Arc::default(),
        },
        &turn,
    );
    assert!(!offered.iter().any(|d| d.spec.name == "lookup_ticket"));
}
