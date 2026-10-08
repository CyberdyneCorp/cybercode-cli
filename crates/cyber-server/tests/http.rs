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
    default: Option<String>,
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
        self.default.clone()
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
        service: None,
        runtime: h.runtime.clone(),
        remote_tools: Arc::default(),
        store: Arc::clone(&h.store),
        services: Arc::new(FakeServices {
            default: Some("test/main".into()),
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
async fn session_reports_effective_and_pending_modes_until_the_next_turn() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c0", "clock", "{}")]), text("done")],
        )],
        ..Setup::default()
    });
    h.tools.set("clock", Behavior::Gated);
    let api = Api::new(&h);
    let id = h.session().await;
    api.post(&format!("/sessions/{id}/prompt"), prompt("inspect"))
        .await;
    tokio::time::timeout(Duration::from_secs(5), h.tools.started.notified())
        .await
        .unwrap();
    let (status, switched) = api
        .post(&format!("/sessions/{id}/mode"), json!({"mode":"plan"}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(switched["data"]["mode"], "plan");
    assert_eq!(switched["data"]["effective_mode"], "default");
    assert_eq!(switched["data"]["pending_mode"], "plan");
    let (_, snapshot) = api.get(&format!("/sessions/{id}")).await;
    assert_eq!(snapshot["data"]["pending_mode"], "plan");
    h.tools.release.notify_one();
    h.settle(&id).await;
    let (_, idle) = api.get(&format!("/sessions/{id}")).await;
    assert_eq!(idle["data"]["effective_mode"], "plan");
    assert!(idle["data"]["pending_mode"].is_null());
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
    let usage_denied = client
        .get(format!("{base}/usage?scope=session&id=ses_missing"))
        .send()
        .await
        .unwrap();
    assert_eq!(usage_denied.status(), 401);
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
async fn location_stream_delivers_setup_output_and_filters_other_locations() {
    use base64::Engine as _;
    use cyber_core::worktrees::{Managed, SetupEvent, SetupSink, SetupStream};
    use cyber_server::runtime::CreateSession;
    use futures::StreamExt;

    let h = Harness::new(Setup::default());
    let id = h.session().await;
    let managed = Managed {
        id: "wt_stream".into(),
        name: "stream".into(),
        path: h.repo.canonicalize().unwrap(),
        branch: "cyber/stream".into(),
        base: "base".into(),
        common_dir: h.repo.join(".git"),
        ready: true,
        included: vec![],
    };
    let other = h
        .runtime
        .create_session(CreateSession {
            directory: h.dir.path().display().to_string(),
            model: "test/main".into(),
            ..CreateSession::default()
        })
        .await
        .unwrap();
    let mut other_managed = managed.clone();
    other_managed.id = "wt_other".into();
    other_managed.path = h.dir.path().canonicalize().unwrap();
    let base = tcp(&h, "pw").await;
    let response = reqwest::Client::new()
        .get(format!("{base}/event"))
        .basic_auth("cyber", Some("pw"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.bytes_stream();
    let sink = h
        .runtime
        .worktree_setup_sink(&id, "call_setup", &managed)
        .await
        .unwrap();
    let other_sink = h
        .runtime
        .worktree_setup_sink(&other.id, "call_other", &other_managed)
        .await
        .unwrap();
    other_sink
        .emit(SetupEvent::Started {
            index: 0,
            command: "other",
        })
        .unwrap();
    sink.emit(SetupEvent::Started {
        index: 0,
        command: "private source",
    })
    .unwrap();
    sink.emit(SetupEvent::Output {
        stream: SetupStream::Stdout,
        bytes: b"live\xff",
    })
    .unwrap();
    let received = async {
        let mut text = String::new();
        while !text.contains("bGl2Zf8=") {
            let chunk = stream.next().await.unwrap().unwrap();
            text.push_str(&String::from_utf8_lossy(&chunk));
        }
        text
    };
    let text = tokio::time::timeout(Duration::from_secs(5), received)
        .await
        .unwrap();
    assert!(text.contains("event: session.worktree.setup"));
    assert!(text.contains(&id) && text.contains("wt_stream") && text.contains("call_setup"));
    assert!(!text.contains(&other.id) && !text.contains("private source"));
    assert!(text.contains(&base64::engine::general_purpose::STANDARD.encode(b"live\xff")));
}

#[cfg(unix)]
#[tokio::test]
async fn location_stream_normalizes_session_directory_aliases() {
    use cyber_server::runtime::CreateSession;
    use futures::StreamExt;
    let h = Harness::new(Setup::default());
    let alias = h.dir.path().join("repo-alias");
    std::os::unix::fs::symlink(&h.repo, &alias).unwrap();
    let session = h
        .runtime
        .create_session(CreateSession {
            directory: alias.display().to_string(),
            model: "test/main".into(),
            ..CreateSession::default()
        })
        .await
        .unwrap();
    let base = tcp(&h, "pw").await;
    let response = reqwest::Client::new()
        .get(format!("{base}/event"))
        .basic_auth("cyber", Some("pw"))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    h.runtime.rename(&session.id, "alias event").await.unwrap();
    let received = async {
        let mut text = String::new();
        while !text.contains("alias event") {
            text.push_str(&String::from_utf8_lossy(
                &stream.next().await.unwrap().unwrap(),
            ));
        }
        text
    };
    let text = tokio::time::timeout(Duration::from_secs(2), received)
        .await
        .unwrap();
    assert!(text.contains(&session.id) && text.contains("session.renamed.1"));
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
        service: None,
        runtime: runtime.clone(),
        remote_tools: remote,
        store,
        services: Arc::new(FakeServices {
            default: Some("test/main".into()),
            tools: builtin,
        }),
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

#[tokio::test]
async fn worktree_request_validation_precedes_host_dispatch() {
    let h = Harness::new(Setup::default());
    let api = Api::new(&h);
    for body in [
        json!({"name": "../escape"}),
        json!({"call_id": "invalid call"}),
        json!({"session": {"id": "ses_existing"}}),
    ] {
        let (status, body, _) = api.call(Method::POST, "/worktrees", Some(body), &[]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["_tag"], "InvalidRequestError");
    }
    let (status, body, _) = api
        .call(Method::POST, "/worktrees", Some(json!({})), &[])
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["_tag"], "ServiceUnavailableError");
    let (status, body) = api.get("/worktrees").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["_tag"], "ServiceUnavailableError");
}

#[tokio::test]
async fn parent_endpoint_replies_to_child_requests_and_refuses_unrelated_sessions() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "shell", "{}")]), text("done")],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Ask("npm test".into()));
    let api = Api::new(&h);
    let parent = h.session().await;
    let unrelated = h.session().await;
    let child = h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    api.post(&format!("/sessions/{child}/prompt"), prompt("test"))
        .await;
    let pending = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(request) = h.runtime.pending_requests(Some(&child)).first().cloned() {
                break request;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (status, list) = api
        .get(&format!("/permissions/requests?session_id={parent}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["data"][0]["session_id"], child);
    let (status, _) = api
        .post(
            &format!("/sessions/{unrelated}/permissions/{}/reply", pending.id),
            json!({"reply": "once"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(h.runtime.pending_requests(Some(&child)).len(), 1);
    let (status, _) = api
        .post(
            &format!("/sessions/{parent}/permissions/{}/reply", pending.id),
            json!({"reply": "once"}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    h.settle(&child).await;
    assert_eq!(
        h.state(&child).await.calls["c1"].output.as_deref(),
        Some("ran npm test")
    );
    assert!(
        !h.store
            .read_events(&parent, -1, 500)
            .unwrap()
            .events
            .iter()
            .any(|e| e.kind.starts_with("permission."))
    );
}

async fn read_sse_until(
    stream: &mut (impl futures::Stream<Item = Result<axum::body::Bytes, reqwest::Error>> + Unpin),
    needle: &str,
) -> String {
    use futures::StreamExt;
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut buffer = String::new();
        while !buffer.contains(needle) {
            let chunk = stream
                .next()
                .await
                .expect("stream ended")
                .expect("stream failed");
            buffer.push_str(&String::from_utf8_lossy(&chunk));
        }
        buffer
    })
    .await
    .expect("expected event was not delivered")
}

#[tokio::test]
async fn child_requests_reach_parent_location_and_session_streams_without_changing_replay_cursor() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "shell", "{}")]), text("done")],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Ask("npm test".into()));
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: h.dir.path().display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            title: Some("Review changes (@general)".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let base = tcp(&h, "pw").await;
    let client = reqwest::Client::new();
    let cursor = h.state(&parent).await.last_seq;
    let res = client
        .get(format!("{base}/event"))
        .basic_auth("cyber", Some("pw"))
        .send()
        .await
        .unwrap();
    let mut location = res.bytes_stream();
    read_sse_until(&mut location, "server.connected").await;
    let res = client
        .get(format!("{base}/sessions/{parent}/events?after={cursor}"))
        .basic_auth("cyber", Some("pw"))
        .send()
        .await
        .unwrap();
    let mut session = res.bytes_stream();
    h.runtime
        .admit(
            &child,
            cyber_server::runtime::Admission::text("test", cyber_server::runtime::Delivery::Steer),
        )
        .await
        .unwrap();
    let location_data = read_sse_until(&mut location, "permission.asked.1").await;
    assert!(location_data.contains("Review changes (@general)") && location_data.contains(&child));
    let session_data = read_sse_until(&mut session, "permission.asked.1").await;
    assert!(
        !session_data.lines().any(|line| line.starts_with("id:")),
        "child relay must not replace the parent's durable cursor: {session_data}"
    );
    assert_eq!(h.state(&parent).await.last_seq, cursor);
    let request = h.runtime.pending_requests(Some(&child)).remove(0);
    // Reconnection recovers child requests without inventing parent durable events.
    let res = client
        .get(format!("{base}/sessions/{parent}/events"))
        .header("Last-Event-ID", format!("{parent}:{cursor}"))
        .basic_auth("cyber", Some("pw"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mut reconnected = res.bytes_stream();
    read_sse_until(&mut reconnected, &request.id).await;
    h.runtime
        .reply_permission(&request.id, cyber_server::runtime::PermissionReply::Once)
        .await
        .unwrap();
    let replied = read_sse_until(&mut session, "permission.replied.1").await;
    assert!(replied.contains(&request.id));
    h.settle(&child).await;
    assert_eq!(h.state(&parent).await.last_seq, cursor);
}

#[tokio::test]
async fn omitted_and_explicit_models_keep_distinct_selection_sources() {
    let h = Harness::new(Setup {
        scripts: vec![
            ("test/main", vec![text("explicit")]),
            ("other/main", vec![text("inherited")]),
        ],
        ..Setup::default()
    });
    h.tools.inference.lock().unwrap().model = Some("other/main".into());
    let api = Api::new(&h);
    let (status, inherited) = api.post("/sessions", json!({"title":"inherited"})).await;
    assert_eq!(status, StatusCode::CREATED, "{inherited}");
    assert_eq!(inherited["data"]["model"], "other/main");
    let (status, explicit) = api
        .post("/sessions", json!({"model":"test/main","title":"explicit"}))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(explicit["data"]["model"], "test/main");
    for session in [&inherited, &explicit] {
        let id = session["data"]["id"].as_str().unwrap();
        let (status, _) = api
            .post(&format!("/sessions/{id}/prompt"), prompt("go"))
            .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        h.settle(id).await;
    }
    assert_eq!(h.models.requests("other/main").len(), 1);
    assert_eq!(h.models.requests("test/main").len(), 1);
    assert!(
        inherited["data"].get("selection").is_none(),
        "private selection does not leak into API metadata"
    );
}

#[tokio::test]
async fn an_agent_model_can_create_a_session_without_a_global_default() {
    let h = Harness::new(Setup::default());
    h.tools.inference.lock().unwrap().model = Some("other/main".into());
    let mut state = state(&h, None);
    state.services = Arc::new(FakeServices {
        default: None,
        tools: h.tools.clone(),
    });
    let api = Api {
        client: EmbeddedClient::new(http::router(state)),
    };
    let (status, created) = api.post("/sessions", json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["data"]["model"], "other/main");
    h.tools.inference.lock().unwrap().model = None;
    let (status, error) = api.post("/sessions", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("No model is configured")
    );
}

#[tokio::test]
async fn typed_child_results_are_available_in_message_history() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery, StructuredSchema};
    for value in [json!({"ok":true}), Value::Null] {
        let encoded = value.to_string();
        let h = Harness::new(Setup {
            scripts: vec![(
                "test/main",
                vec![tools(&[("result", "return_result", &encoded)])],
            )],
            ..Setup::default()
        });
        let parent = h.session().await;
        let id = h
            .runtime
            .create_session(CreateSession {
                directory: h.repo.display().to_string(),
                model: "test/main".into(),
                parent_id: Some(parent),
                title: Some("Structured child".into()),
                output_schema: Some(StructuredSchema::new(json!({})).unwrap()),
                ..Default::default()
            })
            .await
            .unwrap()
            .id;
        h.runtime
            .admit(&id, Admission::text("inspect", Delivery::Queue))
            .await
            .unwrap();
        h.settle(&id).await;
        let api = Api::new(&h);
        let (status, page) = api.get(&format!("/sessions/{id}/messages?order=asc")).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(
            page["data"][1]["tools"][0].get("structured_output"),
            Some(&value),
            "{page}"
        );
    }
}

#[tokio::test]
async fn background_job_routes_list_scope_and_stop_a_child() {
    use cyber_server::runtime::{CreateSession, JobStatus};
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("cancel notice handled")])],
        ..Setup::default()
    });
    let parent = h.session().await;
    let other = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            title: Some("Background child".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let job = h
        .runtime
        .start_child_job(
            &parent,
            &child,
            "scan".into(),
            "Inspect project sources".into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    let api = Api::new(&h);
    let (status, list) = api.get(&format!("/jobs?session_id={parent}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["data"][0]["id"], job.id);
    assert!(
        api.get(&format!("/jobs?session_id={other}")).await.1["data"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let (status, stopped) = api.post(&format!("/jobs/{}/stop", job.id), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stopped["data"]["status"], "cancelled");
    assert_eq!(h.runtime.job(&job.id).unwrap().status, JobStatus::Cancelled);
    assert_eq!(api.get("/jobs/missing").await.0, StatusCode::NOT_FOUND);
}

async fn stream_binding_fixture(
    h: &Harness,
) -> (
    String,
    String,
    cyber_core::worktrees::Managed,
    cyber_core::worktrees::Managed,
) {
    let parent = h.session().await;
    let old_path = h.dir.path().join("old-checkout");
    let new_path = h.dir.path().join("new-checkout");
    std::fs::create_dir(&old_path).unwrap();
    std::fs::create_dir(&new_path).unwrap();
    let old = cyber_core::worktrees::Managed {
        id: "wt_old".into(),
        name: "stream-child".into(),
        path: old_path.canonicalize().unwrap(),
        common_dir: h.repo.join(".git"),
        branch: "cyber/stream-child".into(),
        base: "original".into(),
        ready: true,
        included: vec![],
    };
    let mut new = old.clone();
    new.id = "wt_new".into();
    new.path = new_path.canonicalize().unwrap();
    let child = h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: old.path.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            worktree_id: Some(old.id.clone()),
            child_worktree: Some(old.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    (parent, child, old, new)
}

fn sse_envelope(buffer: &str, kind: &str) -> Value {
    buffer
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find(|event| event["type"] == kind)
        .unwrap_or_else(|| panic!("Missing {kind}: {buffer}"))
}

#[tokio::test]
async fn session_stream_replays_location_timeline_across_rebinding_and_cursors() {
    let h = Harness::new(Setup::default());
    let (parent, child, old, new) = stream_binding_fixture(&h).await;
    h.runtime.rename(&child, "before relocation").await.unwrap();
    let before = h.state(&child).await.last_seq;
    let owner = h.runtime.claim_child_execution(&parent, &child).unwrap();
    h.runtime
        .rebind_child_worktree(&owner, &old, &new)
        .await
        .unwrap();
    let rebound = h.state(&child).await.last_seq;
    h.runtime
        .complete_child_worktree_setup(&child, &new)
        .await
        .unwrap();
    h.runtime.rename(&child, "after relocation").await.unwrap();
    let base = tcp(&h, "pw").await;
    let client = reqwest::Client::new();
    for after in [-1, before, rebound] {
        let response = client
            .get(format!("{base}/sessions/{child}/events?after={after}"))
            .basic_auth("cyber", Some("pw"))
            .send()
            .await
            .unwrap();
        let mut stream = response.bytes_stream();
        let buffer = read_sse_until(&mut stream, "after relocation").await;
        let rename = sse_envelope(&buffer, "session.renamed.1");
        let expected = if after == -1 { &old.path } else { &new.path };
        assert_eq!(rename["location"], expected.display().to_string());
        if after < rebound {
            let binding = sse_envelope(&buffer, "session.worktree.rebound.1");
            assert_eq!(binding["location"], new.path.display().to_string());
            assert_eq!(binding["durable"]["seq"], rebound);
        }
    }
    let api = Api::new(&h);
    let (_, history) = api.get(&format!("/sessions/{child}/history")).await;
    let events = history["data"].as_array().unwrap();
    assert_eq!(events[0]["location"], old.path.display().to_string());
    assert_eq!(
        events.last().unwrap()["location"],
        new.path.display().to_string()
    );
}

#[tokio::test]
async fn location_streams_receive_rebinding_then_route_followups_only_to_new_location() {
    let h = Harness::new(Setup::default());
    let (parent, child, old, new) = stream_binding_fixture(&h).await;
    let base = tcp(&h, "pw").await;
    let client = reqwest::Client::new();
    let old_response = client
        .get(format!("{base}/event"))
        .header("x-cyber-directory", old.path.display().to_string())
        .basic_auth("cyber", Some("pw"))
        .send()
        .await
        .unwrap();
    let new_response = client
        .get(format!("{base}/event"))
        .header("x-cyber-directory", new.path.display().to_string())
        .basic_auth("cyber", Some("pw"))
        .send()
        .await
        .unwrap();
    let session_response = client
        .get(format!("{base}/sessions/{child}/events"))
        .basic_auth("cyber", Some("pw"))
        .send()
        .await
        .unwrap();
    let mut old_stream = old_response.bytes_stream();
    let mut new_stream = new_response.bytes_stream();
    let mut session_stream = session_response.bytes_stream();
    read_sse_until(&mut old_stream, "server.connected").await;
    read_sse_until(&mut new_stream, "server.connected").await;
    h.runtime
        .rename(&child, "prime old directory")
        .await
        .unwrap();
    read_sse_until(&mut old_stream, "prime old directory").await;
    read_sse_until(&mut session_stream, "prime old directory").await;
    let owner = h.runtime.claim_child_execution(&parent, &child).unwrap();
    h.runtime
        .rebind_child_worktree(&owner, &old, &new)
        .await
        .unwrap();
    let old_notice = read_sse_until(&mut old_stream, "session.worktree.rebound.1").await;
    let new_notice = read_sse_until(&mut new_stream, "session.worktree.rebound.1").await;
    assert_eq!(
        sse_envelope(&old_notice, "session.worktree.rebound.1"),
        sse_envelope(&new_notice, "session.worktree.rebound.1")
    );
    assert_eq!(
        sse_envelope(&old_notice, "session.worktree.rebound.1")["location"],
        new.path.display().to_string()
    );
    let session_notice = read_sse_until(&mut session_stream, "session.worktree.rebound.1").await;
    assert_eq!(
        sse_envelope(&session_notice, "session.worktree.rebound.1")["location"],
        new.path.display().to_string()
    );
    use cyber_core::worktrees::{SetupEvent, SetupSink, SetupStream};
    let sink = h
        .runtime
        .worktree_setup_sink(&child, "call_stream_setup", &new)
        .await
        .unwrap();
    sink.emit(SetupEvent::Started {
        index: 0,
        command: "trusted setup",
    })
    .unwrap();
    sink.emit(SetupEvent::Output {
        stream: SetupStream::Stdout,
        bytes: b"setup progress",
    })
    .unwrap();
    let new_output = read_sse_until(&mut new_stream, "\"phase\":\"output\"").await;
    let session_output = read_sse_until(&mut session_stream, "\"phase\":\"output\"").await;
    assert_eq!(
        sse_envelope(&new_output, "session.worktree.setup")["location"],
        new.path.display().to_string()
    );
    assert_eq!(
        sse_envelope(&session_output, "session.worktree.setup")["location"],
        new.path.display().to_string()
    );
    assert!(
        !session_output.lines().any(|line| line.starts_with("id:")),
        "Setup output must preserve the durable cursor: {session_output}"
    );
    sink.emit(SetupEvent::Finished {
        index: 0,
        code: Some(0),
    })
    .unwrap();
    h.runtime
        .complete_child_worktree_setup(&child, &new)
        .await
        .unwrap();
    h.runtime
        .rename(&child, "new location followup")
        .await
        .unwrap();
    let buffer = read_sse_until(&mut new_stream, "new location followup").await;
    assert_eq!(
        sse_envelope(&buffer, "session.renamed.1")["location"],
        new.path.display().to_string()
    );
    let buffer = read_sse_until(&mut session_stream, "new location followup").await;
    assert_eq!(
        sse_envelope(&buffer, "session.worktree.setup_ready.1")["location"],
        new.path.display().to_string()
    );
    // An old-scope sentinel bounds observation without relying on a negative timeout.
    let sentinel = h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: old.path.display().to_string(),
            model: "test/main".into(),
            title: Some("old scope sentinel".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    h.runtime
        .rename(&sentinel.id, "old scope barrier")
        .await
        .unwrap();
    let buffer = read_sse_until(&mut old_stream, "old scope barrier").await;
    assert!(
        !buffer.contains("new location followup")
            && !buffer.contains("session.worktree.setup_ready.1"),
        "{buffer}"
    );
}

#[tokio::test]
async fn deleting_an_uncached_session_keeps_unrelated_instance_listeners_open() {
    let h = Harness::new(Setup::default());
    let deleted = h.session().await;
    let other = h.session().await;
    let base = tcp(&h, "pw").await;
    let response = reqwest::Client::new()
        .get(format!("{base}/event?scope=all"))
        .basic_auth("cyber", Some("pw"))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    read_sse_until(&mut stream, "server.connected").await;
    h.runtime.delete(&deleted).await.unwrap();
    let notice = read_sse_until(&mut stream, "session.deleted").await;
    assert_eq!(
        sse_envelope(&notice, "session.deleted")["data"]["session_id"],
        deleted
    );
    assert!(sse_envelope(&notice, "session.deleted")["location"].is_null());
    h.runtime
        .rename(&other, "unrelated stream remains open")
        .await
        .unwrap();
    read_sse_until(&mut stream, "unrelated stream remains open").await;
}

#[cfg(unix)]
#[tokio::test]
async fn cached_location_preserves_canonical_routing_after_a_directory_alias_disappears() {
    let h = Harness::new(Setup::default());
    let alias = h.dir.path().join("directory-alias");
    std::os::unix::fs::symlink(&h.repo, &alias).unwrap();
    let id = h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: alias.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let base = tcp(&h, "pw").await;
    let response = reqwest::Client::new()
        .get(format!("{base}/event"))
        .basic_auth("cyber", Some("pw"))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    read_sse_until(&mut stream, "server.connected").await;
    h.runtime.rename(&id, "alias cache primed").await.unwrap();
    read_sse_until(&mut stream, "alias cache primed").await;
    std::fs::remove_file(&alias).unwrap();
    h.runtime
        .rename(&id, "alias disappearance followup")
        .await
        .unwrap();
    let sentinel = h.session().await;
    h.runtime
        .rename(&sentinel, "alias scope barrier")
        .await
        .unwrap();
    let buffer = read_sse_until(&mut stream, "alias scope barrier").await;
    assert!(buffer.contains("alias disappearance followup"), "{buffer}");
    assert_eq!(
        sse_envelope(&buffer, "session.renamed.1")["location"],
        h.repo.canonicalize().unwrap().display().to_string()
    );
}

#[tokio::test]
async fn session_detail_and_listing_expose_separate_descendant_billing() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery};
    let mut h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![text("parent result"), text("child result")],
        )],
        ..Default::default()
    });
    h.repo = std::fs::canonicalize(&h.repo).unwrap();
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    for id in [&parent, &child.id] {
        h.runtime
            .admit(id, Admission::text("bill this task", Delivery::Steer))
            .await
            .unwrap();
        h.runtime.wait_idle(id).await;
    }
    let own = h.state(&parent).await.totals.cost;
    let cost = h.state(&child.id).await.totals.cost;
    assert!(own > 0.0 && cost > 0.0);
    let api = Api::new(&h);
    let (status, response, _) = api
        .call(Method::GET, &format!("/sessions/{parent}"), None, &[])
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["data"]["totals"]["cost"], own);
    assert_eq!(response["data"]["children_cost"], cost);
    assert_eq!(response["data"]["children_tokens"], 110);
    assert_eq!(response["data"]["children_unpriced_steps"], 0);
    assert_eq!(response["data"]["children_usage_complete"], true);
    assert_eq!(response["data"]["children_token_classes"]["input"], 100);
    assert_eq!(response["data"]["children_token_classes"]["output"], 10);
    assert_eq!(response["data"]["children_token_classes_complete"], true);
    let (status, response, _) = api
        .call(Method::GET, "/sessions?children=true", None, &[])
        .await;
    assert_eq!(status, StatusCode::OK);
    let listed = response["data"]["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == parent)
        .unwrap();
    assert_eq!(listed["cost"], own);
    assert_eq!(listed["children_cost"], cost);
    assert_eq!(listed["children_tokens"], 110);
    assert_eq!(listed["children_token_classes"]["input"], 100);
    assert_eq!(listed["children_token_classes_complete"], true);
}

#[tokio::test]
async fn session_api_persists_budget_and_refuses_invalid_or_reserved_objects() {
    use cyber_server::runtime::{Admission, Delivery};
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("should not dispatch")])],
        ..Default::default()
    });
    let api = Api::new(&h);
    for budget in [
        json!({"max_cost_usd":-1}),
        json!({"max_cost":1}),
        json!({"enforcement":"reserved","max_tokens":10}),
    ] {
        let (status, _, _) = api
            .call(
                Method::POST,
                "/sessions",
                Some(json!({"budget":budget})),
                &[],
            )
            .await;
        assert!(status.is_client_error(), "{budget}: {status}");
    }
    assert!(
        h.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .is_empty()
    );
    let (status, created, _) = api
        .call(
            Method::POST,
            "/sessions",
            Some(json!({"title":"Budget API","budget":{"max_tokens":0}})),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["data"]["budget"]["max_tokens"], 0);
    assert_eq!(created["data"]["budget"]["enforcement"], "soft");
    let id = created["data"]["id"].as_str().unwrap();
    h.runtime
        .admit(
            id,
            Admission::text("history ".repeat(10000), Delivery::Steer),
        )
        .await
        .unwrap();
    h.runtime.wait_idle(id).await;
    let (status, detail, _) = api
        .call(Method::GET, &format!("/sessions/{id}"), None, &[])
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["data"]["budget"], created["data"]["budget"]);
    assert!(h.models.requests("test/main").is_empty());
    let (status, refused, _) = api
        .call(
            Method::POST,
            &format!("/sessions/{id}/compact"),
            Some(json!({})),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["_tag"], "BudgetExceededError");
}

#[tokio::test]
async fn session_usage_api_reads_durable_own_and_retained_descendant_totals() {
    use cyber_server::runtime::CreateSession;
    use cyber_store::{Expected, NewEvent};
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(root.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    h.state(&root).await;
    for (id, cost) in [(&root, Some(0.25)), (&child.id, None)] {
        h.store.append(id, Expected::Any, vec![NewEvent::new("usage.recorded.1", json!({
            "provider":"test", "model":"test/main", "purpose":"web_summary", "call_id":null,"duration_ms":1,
            "usage":{"input":10,"output":2,"reasoning":3,"cache_read":4,"cache_write":5}, "cost":cost
        }))]).unwrap();
    }
    h.runtime.delete(&child.id).await.unwrap();
    let (status, body) = Api::new(&h)
        .get(&format!("/usage?scope=session&id={root}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let report = &body["data"];
    assert_eq!(report["scope"], "session");
    assert_eq!(report["id"], root);
    assert_eq!(report["own"]["tokens"]["input"], 10);
    assert_eq!(report["descendants"]["tokens"]["cache_write"], 5);
    assert_eq!(report["total"]["tokens"]["input"], 20);
    assert_eq!(report["total"]["tokens"]["reasoning"], 6);
    assert_eq!(report["total"]["total_tokens"], 48);
    assert_eq!(report["total"]["cost"], 0.25);
    assert_eq!(report["total"]["unpriced_steps"], 1);
    assert_eq!(report["total"]["token_classes_complete"], true);
    assert_eq!(report["total"]["usage_complete"], true);
}

#[tokio::test]
async fn session_usage_api_rejects_missing_scope_and_preserves_unknown_history() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let api = Api::new(&h);
    for query in ["", "?scope=session", "?scope=unknown&id=x"] {
        let (status, body) = api.get(&format!("/usage{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["_tag"], "InvalidRequestError");
    }
    let (status, unavailable) = api.get("/usage?scope=run&id=x").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(unavailable["_tag"], "ServiceUnavailableError");
    assert_eq!(unavailable["service"], "usage");
    assert_eq!(
        api.get("/usage?scope=session&id=ses_missing").await.0,
        StatusCode::NOT_FOUND
    );
    let key = root.clone();
    h.store.transaction(move |db| {
        db.execute("UPDATE session SET children_usage_complete=0,children_token_classes_complete=0 WHERE id=?1", [key])?;
        Ok(())
    }).unwrap();
    let (status, body) = api.get(&format!("/usage?scope=session&id={root}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"]["usage_complete"], false);
    assert_eq!(body["data"]["total"]["token_classes_complete"], false);
}

#[tokio::test]
async fn session_usage_api_refuses_corrupt_or_overflowing_projections() {
    let h = Harness::new(Setup::default());
    let api = Api::new(&h);
    for corruption in [
        "input_tokens=-1",
        "children_token_classes='invalid'",
        "children_tokens=1",
        "input_tokens=9223372036854775807,output_tokens=9223372036854775807,reasoning_tokens=10",
        "children_cost=-0.5",
    ] {
        let root = h.session().await;
        let key = root.clone();
        let sql = format!("UPDATE session SET {corruption} WHERE id=?1");
        h.store
            .transaction(move |db| {
                db.execute(&sql, [key])?;
                Ok(())
            })
            .unwrap();
        let (status, body) = api.get(&format!("/usage?scope=session&id={root}")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{corruption}");
        assert_eq!(body["_tag"], "UnknownError");
        assert!(body.get("data").is_none());
    }
}

#[tokio::test]
async fn child_threads_are_parent_scoped_paginated_and_use_own_attempt_status() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery};
    let h = Harness::new(Setup {
        scripts: vec![
            (
                "test/main",
                vec![text("parent answer"), text("child answer")],
            ),
            ("test/summary", vec![text("Child finished its task")]),
        ],
        compaction: cyber_server::runtime::CompactionConfig {
            keep_tokens: 4,
            ..Default::default()
        },
        ..Default::default()
    });
    let parent = h.session().await;
    h.runtime
        .admit(&parent, Admission::text("parent prompt", Delivery::Steer))
        .await
        .unwrap();
    h.runtime.wait_idle(&parent).await;
    let outside = h.dir.path().join("child-worktree");
    std::fs::create_dir(&outside).unwrap();
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: outside.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            subagent_name: Some("review".into()),
            fork_from: Some(parent.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let other = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            subagent_name: Some("waiting".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let foreign_parent = h.session().await;
    h.runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(foreign_parent),
            subagent_name: Some("foreign".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let api = Api::new(&h);
    let path = format!("/sessions/{parent}/children?limit=1");
    let (status, page, _) = api.call(Method::GET, &path, None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["data"]["parent_id"], parent);
    assert_eq!(page["data"]["data"].as_array().unwrap().len(), 1);
    let cursor = page["data"]["cursor"]["next"].as_str().unwrap();
    let (_, second, _) = api
        .call(Method::GET, &format!("{path}&cursor={cursor}"), None, &[])
        .await;
    let mut rows = page["data"]["data"].as_array().unwrap().clone();
    rows.extend(second["data"]["data"].as_array().unwrap().clone());
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row["status"] == "waiting"));
    assert!(rows.iter().any(|row| row["session"]["id"] == child));
    assert!(rows.iter().any(|row| row["session"]["id"] == other));
    h.runtime
        .admit(&child, Admission::text("child prompt", Delivery::Steer))
        .await
        .unwrap();
    h.runtime.wait_idle(&child).await;
    let (_, page, _) = api
        .call(
            Method::GET,
            &format!("/sessions/{parent}/children"),
            None,
            &[],
        )
        .await;
    let completed = page["data"]["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["session"]["id"] == child)
        .unwrap();
    assert_eq!(completed["status"], "completed");
    assert_eq!(
        completed["session"]["directory"],
        outside.display().to_string()
    );
    h.runtime.compact(&child, None).await.unwrap();
    assert!(h.state(&child).await.compacted.is_some());
    let mut restarted = state(&h, None);
    restarted.runtime = h.restart();
    let replay = Api {
        client: EmbeddedClient::new(http::router(restarted)),
    };
    let (_, page) = replay.get(&format!("/sessions/{parent}/children")).await;
    let row = page["data"]["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["session"]["id"] == child)
        .unwrap();
    assert_eq!(row["status"], "completed");
    let (status, _, _) = api
        .call(Method::GET, "/sessions/ses_missing/children", None, &[])
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn child_threads_distinguish_running_waiting_and_failed() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery};
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c", "clock", "{}")]),
                support::error(
                    cyber_llm::ErrorKind::InvalidRequest,
                    "child provider failed",
                ),
            ],
        )],
        ..Default::default()
    });
    h.tools.set("clock", Behavior::Gated);
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let started = h.tools.started.notified();
    h.runtime
        .admit(&child, Admission::text("run", Delivery::Steer))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), started)
        .await
        .unwrap();
    let api = Api::new(&h);
    let path = format!("/sessions/{parent}/children");
    let (_, page) = api.get(&path).await;
    assert_eq!(page["data"]["data"][0]["status"], "running");
    h.runtime.interrupt(&child).await.unwrap();
    let (_, page) = api.get(&path).await;
    assert_eq!(page["data"]["data"][0]["status"], "failed");
    h.runtime
        .admit(&child, Admission::text("try again", Delivery::Steer))
        .await
        .unwrap();
    h.runtime.wait_idle(&child).await;
    let (_, page) = api.get(&path).await;
    assert_eq!(page["data"]["data"][0]["status"], "failed");
    assert!(
        page["data"]["data"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("child provider failed")
    );
}

#[tokio::test]
async fn child_threads_show_only_their_own_pending_user_requests() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery};
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("ask1", "shell", "{}")]), text("done")],
        )],
        ..Default::default()
    });
    h.tools.set("shell", Behavior::Ask("approval".into()));
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    h.runtime
        .admit(&child, Admission::text("ask", Delivery::Steer))
        .await
        .unwrap();
    let requests = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let requests = h.runtime.pending_requests(Some(&child));
            if !requests.is_empty() {
                break requests;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let api = Api::new(&h);
    let (_, page) = api.get(&format!("/sessions/{parent}/children")).await;
    assert_eq!(page["data"]["data"][0]["status"], "waiting");
    assert_eq!(
        page["data"]["data"][0]["reason"],
        "Waiting for a user reply"
    );
    let (status, _) = api
        .post(
            &format!("/sessions/{child}/permissions/{}/reply", requests[0].id),
            json!({"reply":"once"}),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    h.runtime.wait_idle(&child).await;
    let (_, page) = api.get(&format!("/sessions/{parent}/children")).await;
    assert_eq!(page["data"]["data"][0]["status"], "completed");
    let base = tcp(&h, "secret").await;
    let client = reqwest::Client::new();
    let path = format!("{base}/sessions/{parent}/children");
    let response = client.get(&path).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = client
        .get(&path)
        .basic_auth("cyber", Some("secret"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn child_thread_prompt_starts_a_fresh_structured_attempt_and_preserves_receipts() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery, StructuredSchema};
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("first", "return_result", "1")]),
                tools(&[("second", "return_result", "2")]),
            ],
        )],
        ..Default::default()
    });
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            subagent_name: Some("review".into()),
            output_schema: Some(StructuredSchema::new(json!({"type":"integer"})).unwrap()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    h.runtime
        .admit(&child, Admission::text("initial", Delivery::Steer))
        .await
        .unwrap();
    h.runtime.wait_idle(&child).await;
    assert_eq!(h.state(&child).await.structured_result(), Some(&json!(1)));
    let api = Api::new(&h);
    let path = format!("/sessions/{child}/prompt");
    let mut followup = prompt("follow up");
    followup["id"] = json!("msg_user_followup");
    let (status, receipt) = api.post(&path, followup.clone()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    h.runtime.wait_idle(&child).await;
    assert_eq!(h.state(&child).await.structured_result(), Some(&json!(2)));
    let seq = h.state(&child).await.last_seq;
    let (status, retry) = api.post(&path, followup).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    for field in ["session_id", "message_id", "delivery", "admitted_seq"] {
        assert_eq!(retry["data"][field], receipt["data"][field]);
    }
    assert_eq!(retry["data"]["status"], "promoted");
    assert_eq!(h.state(&child).await.last_seq, seq);
    let mut conflict = prompt("different prompt");
    conflict["id"] = json!("msg_user_followup");
    assert_eq!(api.post(&path, conflict).await.0, StatusCode::CONFLICT);
    assert_eq!(h.state(&child).await.structured_result(), Some(&json!(2)));
    assert_eq!(h.models.requests("test/main").len(), 2);
    assert_eq!(
        h.state(&child).await.info.subagent_name.as_deref(),
        Some("review")
    );
    assert_eq!(
        h.state(&child).await.output_schema().unwrap().schema(),
        &json!({"type":"integer"})
    );
}

#[tokio::test]
async fn child_thread_continuation_refuses_a_result_owner_and_preserves_unnamed_identity() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery, StructuredSchema};
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("first", "return_result", "1")]),
                tools(&[("second", "return_result", "2")]),
            ],
        )],
        ..Default::default()
    });
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            output_schema: Some(StructuredSchema::new(json!({"type":"integer"})).unwrap()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let owner = h.runtime.claim_child_execution(&parent, &child).unwrap();
    h.runtime
        .admit(&child, Admission::text("initial", Delivery::Steer))
        .await
        .unwrap();
    h.runtime.wait_idle(&child).await;
    let before = h.state(&child).await;
    let api = Api::new(&h);
    let path = format!("/sessions/{child}/prompt");
    let (status, refused) = api.post(&path, prompt("follow up")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["_tag"], "SessionBusyError");
    assert_eq!(h.state(&child).await, before);
    drop(owner);
    assert_eq!(
        api.post(&path, prompt("follow up")).await.0,
        StatusCode::ACCEPTED
    );
    h.runtime.wait_idle(&child).await;
    let after = h.state(&child).await;
    assert_eq!(after.structured_result(), Some(&json!(2)));
    assert_eq!(after.info.subagent_name, None);
    let lookup = child.clone();
    let stored_name: Option<String> = h
        .store
        .read(move |db| {
            db.query_row(
                "SELECT subagent_name FROM session WHERE id=?1",
                [lookup],
                |row| row.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(stored_name, None);
    h.runtime.shutdown().await;
    let replay = h.restart().state(&child).await.unwrap();
    assert_eq!(replay.structured_result(), Some(&json!(2)));
    assert_eq!(replay.info.subagent_name, None);
}

#[tokio::test]
async fn child_thread_held_inputs_preserve_terminal_result_and_unknown_outcomes_refuse() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery, StructuredSchema};
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("first", "return_result", "1")]),
                tools(&[("write1", "write", "{}")]),
            ],
        )],
        ..Default::default()
    });
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            output_schema: Some(StructuredSchema::new(json!({"type":"integer"})).unwrap()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    h.runtime
        .admit(&child, Admission::text("initial", Delivery::Steer))
        .await
        .unwrap();
    h.runtime.wait_idle(&child).await;
    let api = Api::new(&h);
    let path = format!("/sessions/{child}/prompt");
    let mut held = prompt("later");
    held["delivery"] = json!("hold");
    assert_eq!(api.post(&path, held).await.0, StatusCode::ACCEPTED);
    let mut sleeping = prompt("do not wake");
    sleeping["resume"] = json!(false);
    assert_eq!(api.post(&path, sleeping).await.0, StatusCode::ACCEPTED);
    assert_eq!(h.state(&child).await.structured_result(), Some(&json!(1)));
    assert_eq!(h.models.requests("test/main").len(), 1);
    let uncertain = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    h.tools.set("write", Behavior::Crash);
    h.runtime
        .admit(&uncertain, Admission::text("write", Delivery::Steer))
        .await
        .unwrap();
    h.runtime.wait_idle(&uncertain).await;
    let before = h.state(&uncertain).await;
    assert!(!before.unresolved().is_empty());
    let (status, refused) = api
        .post(&format!("/sessions/{uncertain}/prompt"), prompt("retry"))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(refused["message"].as_str().unwrap().contains("recovery"));
    assert_eq!(h.state(&uncertain).await, before);
}

#[tokio::test]
async fn child_thread_continuation_holds_ownership_through_running_steer_and_interrupt() {
    use cyber_server::runtime::CreateSession;
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![tools(&[("clock1", "clock", "{}")])])],
        ..Default::default()
    });
    h.tools.set("clock", Behavior::UntilCancelled);
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            subagent_name: Some("review".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let api = Api::new(&h);
    let path = format!("/sessions/{child}/prompt");
    assert_eq!(api.post(&path, prompt("run")).await.0, StatusCode::ACCEPTED);
    tokio::time::timeout(Duration::from_secs(3), h.tools.started.notified())
        .await
        .unwrap();
    assert!(h.runtime.claim_child_execution(&parent, &child).is_err());
    assert_eq!(
        api.post(&path, prompt("hint while running")).await.0,
        StatusCode::ACCEPTED
    );
    let events = h.store.read_events(&child, -1, 200).unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "session.subagent.resumed.1")
            .count(),
        1
    );
    let reset = events
        .iter()
        .position(|event| event.kind == "session.subagent.resumed.1")
        .unwrap();
    assert_eq!(events[reset + 1].kind, "session.prompt.admitted.1");
    assert!(h.runtime.claim_child_execution(&parent, &child).is_err());
    h.runtime.interrupt(&child).await.unwrap();
    h.runtime.wait_idle(&child).await;
    let _next_owner = h.runtime.claim_child_execution(&parent, &child).unwrap();
}

#[tokio::test]
async fn child_thread_unacknowledged_settlement_is_durable_and_refuses_restart_dispatch() {
    use cyber_server::runtime::{
        Admission, ChildContinuation, ChildExecution, CreateSession, Delivery, Runtime,
        RuntimeOptions, SessionInfo, SessionState, ToolHost,
    };
    use futures::future::BoxFuture;
    use tokio_util::sync::CancellationToken;
    struct Stalled(Arc<tokio::sync::Notify>);
    impl ChildContinuation for Stalled {
        fn settle(
            self: Box<Self>,
            completed: bool,
            _cancel: CancellationToken,
        ) -> BoxFuture<'static, Result<Option<Value>, String>> {
            Box::pin(async move {
                assert!(completed);
                self.0.notify_one();
                std::future::pending().await
            })
        }
    }
    struct Host {
        tools: Arc<support::Tools>,
        started: Arc<tokio::sync::Notify>,
    }
    impl ToolHost for Host {
        fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
            self.tools.definitions(turn)
        }
        fn execute(
            &self,
            call: cyber_server::runtime::Invocation,
            cancel: CancellationToken,
        ) -> BoxFuture<'_, cyber_server::runtime::ToolOutcome> {
            self.tools.execute(call, cancel)
        }
        fn prepare_child_continuation<'a>(
            &'a self,
            _parent: &'a SessionInfo,
            _child: &'a SessionState,
            _owner: &'a ChildExecution,
            _cancel: CancellationToken,
        ) -> BoxFuture<'a, Result<Option<Box<dyn ChildContinuation>>, String>> {
            Box::pin(async {
                Ok(Some(
                    Box::new(Stalled(self.started.clone())) as Box<dyn ChildContinuation>
                ))
            })
        }
    }
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("done")])],
        ..Default::default()
    });
    let started = Arc::new(tokio::sync::Notify::new());
    let runtime = Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: h.models.clone(),
        tools: Arc::new(Host {
            tools: h.tools.clone(),
            started: started.clone(),
        }),
        global_config_dir: h.dir.path().join("global"),
        shell: "bash".into(),
        claude_compat: true,
        compaction: Default::default(),
        retry: Default::default(),
        max_steps: None,
        today: None,
        interactive: true,
        snapshots: Arc::new(cyber_server::runtime::NoSnapshots),
    });
    let parent = runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let child = runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    runtime
        .admit_user(&child, Admission::text("run", Delivery::Steer))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    assert!(runtime.claim_child_execution(&parent, &child).is_err());
    tokio::time::timeout(Duration::from_secs(5), runtime.interrupt(&child))
        .await
        .unwrap()
        .unwrap();
    let events = h.store.read_events(&child, -1, 200).unwrap().events;
    let settlement = events
        .iter()
        .find(|e| e.kind == "session.child.continuation_settled.1")
        .unwrap();
    assert_eq!(settlement.data["unknown"], true);
    runtime.shutdown().await;
    let restarted = h.restart();
    let before = restarted.state(&child).await.unwrap();
    assert!(restarted.wake(&child).await.is_err());
    assert!(restarted.resume(&child).await.is_err());
    assert!(restarted.shell(&child, "echo must-not-run").await.is_err());
    assert!(restarted.compact(&child, None).await.is_err());
    assert!(restarted.repair_context(&child).await.is_err());
    assert!(
        restarted
            .admit_user(&child, Admission::text("retry", Delivery::Steer))
            .await
            .unwrap_err()
            .to_string()
            .contains("unknown")
    );
    let owner = restarted.claim_child_execution(&parent, &child).unwrap();
    assert!(
        restarted
            .resume_child(
                &owner,
                Admission::text("retry", Delivery::Queue),
                "review".into(),
                None
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("unknown")
    );
    assert_eq!(restarted.state(&child).await.unwrap(), before);
    let mut app = state(&h, None);
    app.runtime = restarted;
    let api = Api {
        client: EmbeddedClient::new(http::router(app)),
    };
    let (_, page) = api.get(&format!("/sessions/{parent}/children")).await;
    assert_eq!(page["data"]["data"][0]["status"], "failed");
    assert!(
        page["data"]["data"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("acknowledge")
    );
    assert_eq!(h.models.requests("test/main").len(), 1);
}

#[tokio::test]
async fn child_thread_deferred_input_runs_a_fresh_structured_attempt_on_wake_or_release() {
    use cyber_server::runtime::{
        Admission, CreateSession, Delivery, InputStatus, StructuredSchema,
    };
    for held in [false, true] {
        let h = Harness::new(Setup {
            scripts: vec![(
                "test/main",
                vec![
                    tools(&[("first", "return_result", "1")]),
                    tools(&[("second", "return_result", "2")]),
                ],
            )],
            ..Default::default()
        });
        let parent = h.session().await;
        let child = h
            .runtime
            .create_session(CreateSession {
                directory: h.repo.display().to_string(),
                model: "test/main".into(),
                parent_id: Some(parent),
                subagent_name: Some("review".into()),
                output_schema: Some(StructuredSchema::new(json!({"type":"integer"})).unwrap()),
                ..Default::default()
            })
            .await
            .unwrap()
            .id;
        h.runtime
            .admit(&child, Admission::text("initial", Delivery::Steer))
            .await
            .unwrap();
        h.runtime.wait_idle(&child).await;
        let api = Api::new(&h);
        let mut deferred = prompt("deferred follow up");
        deferred["id"] = json!("msg_deferred");
        deferred["delivery"] = json!(if held { "hold" } else { "queue" });
        deferred["resume"] = json!(false);
        assert_eq!(
            api.post(&format!("/sessions/{child}/prompt"), deferred)
                .await
                .0,
            StatusCode::ACCEPTED
        );
        let before = h.state(&child).await;
        let seq = before.input("msg_deferred").unwrap().admitted_seq;
        assert_eq!(before.structured_result(), Some(&json!(1)));
        if held {
            assert_eq!(
                api.post(
                    &format!("/sessions/{child}/inbox/msg_deferred/release"),
                    json!({"delivery":"queue"})
                )
                .await
                .0,
                StatusCode::NO_CONTENT
            );
        } else {
            assert_eq!(
                api.post(&format!("/sessions/{child}/wake"), json!({}))
                    .await
                    .0,
                StatusCode::NO_CONTENT
            );
        }
        h.runtime.wait_idle(&child).await;
        let state = h.state(&child).await;
        assert_eq!(state.structured_result(), Some(&json!(2)), "held={held}");
        assert_eq!(
            state.input("msg_deferred").unwrap().status,
            InputStatus::Promoted
        );
        assert_eq!(state.input("msg_deferred").unwrap().admitted_seq, seq);
        assert_eq!(state.inbox.len(), 2);
        let last = state.last_seq;
        assert_eq!(
            api.post(&format!("/sessions/{child}/wake"), json!({}))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        h.runtime.wait_idle(&child).await;
        assert_eq!(h.state(&child).await.last_seq, last);
        assert_eq!(h.models.requests("test/main").len(), 2);
    }
}

#[tokio::test]
async fn child_thread_held_release_and_queued_wake_refuse_result_ownership_without_mutation() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery, StructuredSchema};
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![tools(&[("first", "return_result", "1")])])],
        ..Default::default()
    });
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            output_schema: Some(StructuredSchema::new(json!({"type":"integer"})).unwrap()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    h.runtime
        .admit(&child, Admission::text("initial", Delivery::Steer))
        .await
        .unwrap();
    h.runtime.wait_idle(&child).await;
    let held = h
        .runtime
        .admit_user(&child, Admission::text("held", Delivery::Hold))
        .await
        .unwrap();
    let mut queued = Admission::text("queue", Delivery::Queue);
    queued.resume = false;
    h.runtime.admit_user(&child, queued).await.unwrap();
    let _owner = h.runtime.claim_child_execution(&parent, &child).unwrap();
    let before = h.state(&child).await;
    let api = Api::new(&h);
    assert_eq!(
        api.post(
            &format!("/sessions/{child}/inbox/{}/release", held.message_id),
            json!({"delivery":"steer"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api.post(&format!("/sessions/{child}/wake"), json!({}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(h.state(&child).await, before);
    assert_eq!(h.models.requests("test/main").len(), 1);
}

#[tokio::test]
async fn child_thread_explicit_wakes_preserve_structured_queue_order_and_edited_input() {
    use cyber_server::runtime::{
        Admission, CreateSession, Delivery, InputStatus, StructuredSchema,
    };
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("first", "return_result", "1")]),
                tools(&[("second", "return_result", "2")]),
                tools(&[("third", "return_result", "3")]),
            ],
        )],
        ..Default::default()
    });
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent),
            output_schema: Some(StructuredSchema::new(json!({"type":"integer"})).unwrap()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    h.runtime
        .admit(&child, Admission::text("initial", Delivery::Steer))
        .await
        .unwrap();
    h.runtime.wait_idle(&child).await;
    let mut receipts = Vec::new();
    for text in ["obsolete queue text", "last queue text"] {
        let mut admission = Admission::text(text, Delivery::Queue);
        admission.resume = false;
        receipts.push(h.runtime.admit_user(&child, admission).await.unwrap());
    }
    h.runtime
        .edit_input(
            &child,
            &receipts[0].message_id,
            Some(vec![cyber_llm::Content::Text {
                text: "edited first queue text".into(),
            }]),
            None,
        )
        .await
        .unwrap();
    for (index, value) in [(0, 2), (1, 3)] {
        h.runtime.wake(&child).await.unwrap();
        h.runtime.wait_idle(&child).await;
        let state = h.state(&child).await;
        assert_eq!(state.structured_result(), Some(&json!(value)));
        assert_eq!(
            state.input(&receipts[index].message_id).unwrap().status,
            InputStatus::Promoted
        );
        assert_eq!(
            state
                .input(&receipts[index].message_id)
                .unwrap()
                .admitted_seq,
            receipts[index].admitted_seq
        );
        if index == 0 {
            assert_eq!(
                state.input(&receipts[1].message_id).unwrap().status,
                InputStatus::Pending
            );
        }
    }
    let request = serde_json::to_string(&h.models.requests("test/main")[1]).unwrap();
    assert!(request.contains("edited first queue text"));
    assert!(!request.contains("obsolete queue text"));
    assert_eq!(h.state(&child).await.inbox.len(), 3);
}

#[tokio::test]
async fn explicit_subtree_stop_api_preserves_input_and_unrelated_jobs() {
    use cyber_server::runtime::{Admission, CreateSession, Delivery, JobStatus};
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let unrelated = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            parent_id: Some(root.clone()),
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let job = h
        .runtime
        .start_child_job(
            &root,
            &child,
            "owned".into(),
            "owned".into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    let unrelated_child = h
        .runtime
        .create_session(CreateSession {
            parent_id: Some(unrelated.clone()),
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let other = h
        .runtime
        .start_child_job(
            &unrelated,
            &unrelated_child,
            "other".into(),
            "other".into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    let mut input = Admission::text("preserved", Delivery::Queue);
    input.resume = false;
    h.runtime.admit(&root, input).await.unwrap();
    let inbox = h.runtime.state(&root).await.unwrap().inbox;
    let api = Api::new(&h);
    let path = format!("/sessions/{root}/stop-subtree");
    let (status, report, _) = api.call(Method::POST, &path, None, &[]).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["data"]["session_id"], root);
    assert_eq!(report["data"]["status"], "acknowledged");
    assert_eq!(report["data"]["persisted"], true);
    assert_eq!(report["data"]["problems"], json!([]));
    assert_eq!(h.runtime.job(&job.id).unwrap().status, JobStatus::Cancelled);
    assert_eq!(h.runtime.job(&other.id).unwrap().status, JobStatus::Running);
    assert_eq!(
        h.runtime.state(&root).await.unwrap().inbox.len(),
        inbox.len()
    );
    assert_eq!(
        h.runtime.state(&root).await.unwrap().inbox[0].message_id,
        inbox[0].message_id
    );
    assert!(h.runtime.capture_child_admission(&root).is_err());
    assert!(h.runtime.capture_child_admission(&unrelated).is_ok());
    let (status, repeated, _) = api.call(Method::POST, &path, None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(repeated["data"]["scope_id"], report["data"]["scope_id"]);
    assert_eq!(
        api.post(&format!("/sessions/{root}/prompt"), prompt("late"))
            .await
            .0,
        StatusCode::CONFLICT
    );
    h.runtime.cancel_job(&other.id).await.unwrap();
}

#[tokio::test]
async fn explicit_subtree_stop_api_retains_unknown_and_reassesses_same_scope() {
    use cyber_server::runtime::CreateSession;
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            parent_id: Some(root.clone()),
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let owner = h.runtime.claim_child_execution(&root, &child).unwrap();
    let api = Api::new(&h);
    let path = format!("/sessions/{child}/stop-subtree");
    let headers = [("idempotency-key", "subtree-stop-observation")];
    let (status, unknown, _) = api.call(Method::POST, &path, None, &headers).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unknown["data"]["status"], "unknown");
    assert_eq!(unknown["data"]["persisted"], true);
    assert!(!unknown["data"]["problems"].as_array().unwrap().is_empty());
    drop(owner);
    let (_, replay, _) = api.call(Method::POST, &path, None, &headers).await;
    assert_eq!(replay, unknown);
    let (status, settled, _) = api.call(Method::POST, &path, None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settled["data"]["status"], "acknowledged");
    assert_eq!(settled["data"]["scope_id"], unknown["data"]["scope_id"]);
    assert!(h.runtime.capture_child_admission(&child).is_err());
}

#[tokio::test]
async fn explicit_subtree_stop_api_requires_authentication_and_existing_session() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let base = tcp(&h, "secret").await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{base}/sessions/{root}/stop-subtree"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(h.runtime.capture_child_admission(&root).is_ok());
    let response = client
        .post(format!("{base}/sessions/ses_missing/stop-subtree"))
        .basic_auth("cyber", Some("secret"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let error: Value = response.json().await.unwrap();
    assert_eq!(error["_tag"], "SessionNotFoundError");
}
