//! MCP serve (`mcp-server`): the Streamable HTTP endpoint at `/api/v1/mcp/serve` over a
//! real TCP listener (and the Unix socket), with its trust checks, protocol and tools.

mod support;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyber_core::config::McpServeSettings;
use cyber_server::http::{
    self, AgentInfo, ApiError, AppState, CommandInfo, HttpOptions, LocationDefaults,
    McpServeSessions, ModelInfo, Services,
};
use cyber_server::runtime::{ListFilter, ToolDef, TurnContext};
use futures::future::BoxFuture;
use reqwest::StatusCode;
use serde_json::{Value, json};
use support::{Behavior, Harness, Setup, text, tools};

const PASSWORD: &str = "s3cret";
const TOOLS: [&str; 10] = [
    "cyber_interrupt",
    "cyber_message_history",
    "cyber_message_list",
    "cyber_message_send",
    "cyber_prompt",
    "cyber_prompt_wait",
    "cyber_session_create",
    "cyber_session_list",
    "cyber_session_read",
    "cyber_session_transcript",
];

struct Catalog {
    serve: Mutex<McpServeSettings>,
    defaults: Mutex<LocationDefaults>,
    /// Holds the Session-creation route (its default model lookup) until released.
    creation_gate: Mutex<Option<Arc<Gate>>>,
}

/// A blocking hold inside a synchronous route step: `reached` when entered, then it waits
/// for `release`.
struct Gate {
    reached: tokio::sync::Notify,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

impl Services for Catalog {
    fn mcp_serve(&self) -> Result<McpServeSettings, ApiError> {
        Ok(self.serve.lock().unwrap().clone())
    }
    fn location_defaults(&self, _location: &Path) -> Result<LocationDefaults, ApiError> {
        Ok(self.defaults.lock().unwrap().clone())
    }
    fn models(&self, _location: &Path) -> BoxFuture<'_, Result<Vec<ModelInfo>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn default_model(&self, _location: &Path) -> Option<String> {
        let gate = self.creation_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.reached.notify_one();
            let _ = gate.release.lock().unwrap().recv();
        }
        Some("test/main".into())
    }
    fn agents(&self, _location: &Path) -> Vec<AgentInfo> {
        Vec::new()
    }
    fn tools(&self, _turn: &TurnContext) -> Vec<ToolDef> {
        Vec::new()
    }
    fn commands(&self, _location: &Path) -> Vec<CommandInfo> {
        Vec::new()
    }
    fn find_files(&self, _location: &Path, _query: &str, _limit: usize) -> Vec<String> {
        Vec::new()
    }
    fn expand_command(&self, _location: &Path, _name: &str, _arguments: &str) -> Option<String> {
        None
    }
}

struct Server {
    h: Harness,
    catalog: Arc<Catalog>,
    sessions: Arc<McpServeSessions>,
    /// Seconds added to the MCP session table's clock.
    skew: Arc<AtomicU64>,
    port: u16,
    client: reqwest::Client,
}

impl Server {
    async fn start(bind: &str) -> Self {
        Self::start_with(bind, Setup::default()).await
    }

    async fn start_with(bind: &str, setup: Setup) -> Self {
        let h = Harness::new(setup);
        let catalog = Arc::new(Catalog {
            serve: Mutex::new(McpServeSettings::from_config(&json!({})).unwrap()),
            defaults: Mutex::default(),
            creation_gate: Mutex::default(),
        });
        let state = AppState {
            service: None,
            telemetry: None,
            runtime: h.runtime.clone(),
            remote_tools: Arc::default(),
            store: Arc::clone(&h.store),
            services: Arc::clone(&catalog) as Arc<dyn Services>,
            options: Arc::new(HttpOptions {
                version: "9.9.9-test".into(),
                password: Some(PASSWORD.into()),
                cors_origins: Vec::new(),
                default_directory: h.repo.clone(),
                features: vec!["session".into()],
            }),
        };
        let skew = Arc::new(AtomicU64::new(0));
        let offset = Arc::clone(&skew);
        let base = std::time::Instant::now();
        let sessions = Arc::new(McpServeSessions::with_clock(Arc::new(move || {
            base + base.elapsed() + Duration::from_secs(offset.load(Ordering::SeqCst))
        })));
        let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(http::serve_tcp(
            http::router_with(state, Arc::clone(&sessions)),
            listener,
            std::future::pending(),
        ));
        Self {
            h,
            catalog,
            sessions,
            skew,
            port,
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/api/v1/mcp/serve", self.port)
    }

    fn post(&self, session: Option<&str>) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(self.url())
            .basic_auth("cyber", Some(PASSWORD))
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json");
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }
        request
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> (StatusCode, reqwest::header::HeaderMap, String) {
        let response = request.send().await.unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        (status, headers, response.text().await.unwrap())
    }

    async fn rpc(&self, session: Option<&str>, frame: Value) -> (StatusCode, Value) {
        let (status, headers, text) = self.send(self.post(session).body(frame.to_string())).await;
        let streamed = headers
            .get("content-type")
            .is_some_and(|t| t.to_str().unwrap().starts_with("text/event-stream"));
        if streamed {
            let stream = Sse::parse(&text);
            let reply = stream.frames.last().cloned().unwrap_or(Value::Null);
            return (status, reply);
        }
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn initialize(&self, version: &str) -> (String, Value) {
        self.initialize_as(version, None).await
    }

    /// `initialize` as `Test Client`, with an optional `X-Cyber-MCP-Client` label.
    async fn initialize_as(&self, version: &str, label: Option<&str>) -> (String, Value) {
        let frame = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion": version, "capabilities": {},
            "clientInfo": {"name": "Test Client", "version": "1"}}});
        let mut request = self.post(None).body(frame.to_string());
        if let Some(label) = label {
            request = request.header("x-cyber-mcp-client", label);
        }
        let (status, headers, text) = self.send(request).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(
            headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        let id = headers["mcp-session-id"].to_str().unwrap().to_owned();
        assert!(!id.is_empty() && id.bytes().all(|b| (0x21..=0x7e).contains(&b)));
        (id, serde_json::from_str(&text).unwrap())
    }

    async fn call(&self, session: &str, name: &str, arguments: Value) -> Value {
        let frame = json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":name,"arguments":arguments}});
        let (status, body) = self.rpc(Some(session), frame).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }
}

fn tool_error(result: &Value) -> Value {
    assert_eq!(result["isError"], true, "{result}");
    assert!(result.get("structuredContent").is_none(), "{result}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

fn tool_output(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    result["structuredContent"].clone()
}

/// Every Session of the server, archived and child Sessions included.
fn session_count(server: &Server) -> usize {
    let filter = ListFilter {
        include_archived: true,
        limit: Some(200),
        ..ListFilter::default()
    };
    server.h.runtime.list(&filter).unwrap().sessions.len()
}

#[tokio::test]
async fn missing_or_foreign_credentials_and_hosts_are_refused() {
    let server = Server::start("127.0.0.1:0").await;
    let frame = json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string();
    let anonymous = server
        .client
        .post(server.url())
        .body(frame.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
    assert_eq!(
        anonymous.headers()["www-authenticate"],
        "Basic realm=\"cyber\""
    );

    use base64::Engine;
    let token = base64::engine::general_purpose::STANDARD.encode(format!("cyber:{PASSWORD}"));
    let query = server
        .client
        .post(format!("{}?auth_token={token}", server.url()))
        .body(frame.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(query.status(), 401, "auth_token is never accepted");

    let (status, _, text) = server
        .send(
            server
                .post(None)
                .header("host", "attacker.example:4747")
                .body(frame.clone()),
        )
        .await;
    assert_eq!(status, 403);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["_tag"], "ForbiddenError");
    assert_eq!(body["reason"], "host_not_allowed");

    let bearer = server
        .client
        .post(server.url())
        .bearer_auth("account-token")
        .body(frame.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(bearer.status(), 403);
    let body: Value = bearer.json().await.unwrap();
    assert_eq!(body["_tag"], "ForbiddenError");
    assert_eq!(body["reason"], "loopback_only");

    for host in ["localhost", "localhost:4747", "127.0.0.2:80", "[::1]:4747"] {
        let (status, _, text) = server
            .send(server.post(None).header("host", host).body(frame.clone()))
            .await;
        // Allowed hosts reach the endpoint, which requires an MCP session for `ping`.
        assert_eq!(status, 400, "{host}: {text}");
    }
    assert_eq!(server.sessions.count(), 0);
}

/// A local address that is not loopback, found without sending a packet: connecting a
/// UDP socket only selects the route.
fn lan_address() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[tokio::test]
async fn non_loopback_peers_are_refused_even_with_credentials() {
    let server = Server::start("0.0.0.0:0").await;
    let Some(lan) = lan_address() else {
        eprintln!("skipped: this host has no non-loopback interface to connect through");
        return;
    };
    let frame = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"lan","version":"1"}}});
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let sent = client
        .post(format!("http://{lan}:{}/api/v1/mcp/serve", server.port))
        .basic_auth("cyber", Some(PASSWORD))
        .body(frame.to_string())
        .send()
        .await;
    let response = match sent {
        Ok(response) => response,
        Err(error) if error.is_connect() || error.is_timeout() => {
            eprintln!("skipped: {lan} is not reachable from this host ({error})");
            return;
        }
        Err(error) => panic!("{error}"),
    };
    assert_eq!(response.status(), 403);
    assert!(response.headers().get("mcp-session-id").is_none());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["_tag"], "ForbiddenError");
    assert_eq!(body["reason"], "loopback_only");
    assert_eq!(server.sessions.count(), 0, "no MCP session was created");
    // The same listener still serves loopback peers.
    server.initialize("2025-06-18").await;
    assert_eq!(server.sessions.count(), 1);
}

/// The same refusal without depending on the host's interfaces or firewall: the router as
/// `serve_tcp` runs it, with the peer address `serve_tcp` attaches to each request.
#[tokio::test]
async fn a_tcp_request_from_a_lan_peer_is_refused_before_any_mcp_session() {
    use tower::ServiceExt;
    let server = Server::start("127.0.0.1:0").await;
    let state = AppState {
        service: None,
        telemetry: None,
        runtime: server.h.runtime.clone(),
        remote_tools: Arc::default(),
        store: Arc::clone(&server.h.store),
        services: Arc::clone(&server.catalog) as Arc<dyn Services>,
        options: Arc::new(HttpOptions {
            version: "9.9.9-test".into(),
            password: Some(PASSWORD.into()),
            cors_origins: Vec::new(),
            default_directory: server.h.repo.clone(),
            features: Vec::new(),
        }),
    };
    let router = http::router_with(state, Arc::clone(&server.sessions))
        .layer(axum::Extension(http::Transport::Tcp));
    use base64::Engine;
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("cyber:{PASSWORD}"));
    for (peer, status, reason) in [
        ("192.168.1.20:50000", 403, Some("loopback_only")),
        ("[fe80::1]:50000", 403, Some("loopback_only")),
        ("[::ffff:192.168.1.20]:50000", 403, Some("loopback_only")),
        ("[::ffff:127.0.0.1]:50000", 200, None),
        ("[::1]:50000", 200, None),
    ] {
        let frame = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"lan","version":"1"}}});
        let mut request = axum::http::Request::post("/api/v1/mcp/serve")
            .header("host", "127.0.0.1:4747")
            .header("authorization", format!("Basic {basic}"))
            .body(axum::body::Body::from(frame.to_string()))
            .unwrap();
        request
            .extensions_mut()
            .insert(http::PeerAddr(peer.parse().unwrap()));
        let before = server.sessions.count();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status().as_u16(), status, "{peer}");
        let created = server.sessions.count() - before;
        if let Some(reason) = reason {
            let body = axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["reason"], reason, "{peer}");
            assert_eq!(created, 0, "{peer}: no MCP session");
        } else {
            assert_eq!(created, 1, "{peer}");
        }
    }
}

#[tokio::test]
async fn mcp_sessions_follow_the_streamable_http_rules() {
    let server = Server::start("127.0.0.1:0").await;
    let get = server
        .client
        .get(server.url())
        .basic_auth("cyber", Some(PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 405);

    let ping = json!({"jsonrpc":"2.0","id":2,"method":"ping"});
    let (status, body) = server.rpc(None, ping.clone()).await;
    assert_eq!(status, 400);
    assert_eq!(body["_tag"], "InvalidRequestError");
    assert_eq!(body["reason"], "mcp_session_required");
    let (status, body) = server.rpc(Some("mcp_unknown"), ping.clone()).await;
    assert_eq!(status, 404);
    assert_eq!(body["_tag"], "NotFoundError");
    assert_eq!(body["reason"], "mcp_session");

    let (session, _) = server.initialize("2025-06-18").await;
    let note = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let (status, _, text) = server
        .send(server.post(Some(&session)).body(note.to_string()))
        .await;
    assert_eq!(status, 202);
    assert!(text.is_empty());
    let reply = json!({"jsonrpc":"2.0","id":"srv-1","result":{}});
    let (status, _, text) = server
        .send(server.post(Some(&session)).body(reply.to_string()))
        .await;
    assert_eq!(status, 202);
    assert!(text.is_empty());
    let (status, body) = server.rpc(Some(&session), ping.clone()).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"jsonrpc":"2.0","id":2,"result":{}}));

    let delete = |id: &str| {
        server
            .client
            .delete(server.url())
            .basic_auth("cyber", Some(PASSWORD))
            .header("mcp-session-id", id)
            .send()
    };
    assert!(delete(&session).await.unwrap().status().is_success());
    let (status, body) = server.rpc(Some(&session), ping.clone()).await;
    assert_eq!(status, 404);
    assert_eq!(body["reason"], "mcp_session");
    assert_eq!(delete(&session).await.unwrap().status(), 404);

    // Activity keeps an MCP session; 30 idle minutes end it.
    let (session, _) = server.initialize("2025-06-18").await;
    server.skew.store(29 * 60, Ordering::SeqCst);
    assert_eq!(server.rpc(Some(&session), ping.clone()).await.0, 200);
    server.skew.store(58 * 60, Ordering::SeqCst);
    assert_eq!(server.rpc(Some(&session), ping.clone()).await.0, 200);
    server.skew.store(89 * 60, Ordering::SeqCst);
    let (status, body) = server.rpc(Some(&session), ping).await;
    assert_eq!(status, 404);
    assert_eq!(body["reason"], "mcp_session");
}

#[tokio::test]
async fn oversized_bodies_and_disabled_serve_are_refused_before_dispatch() {
    let server = Server::start("127.0.0.1:0").await;
    let (session, _) = server.initialize("2025-06-18").await;
    let filler = "x".repeat(2 * 1024 * 1024);
    let frame = json!({"jsonrpc":"2.0","id":3,"method":"ping","params":{"_meta":{"pad":filler}}});
    let (status, body) = server.rpc(Some(&session), frame).await;
    assert_eq!(status, 400);
    assert_eq!(body["_tag"], "InvalidRequestError");
    assert_eq!(body["reason"], "too_large");

    server.catalog.serve.lock().unwrap().enabled = false;
    let (status, body) = server
        .rpc(
            Some(&session),
            json!({"jsonrpc":"2.0","id":4,"method":"ping"}),
        )
        .await;
    assert_eq!(status, 503);
    assert_eq!(body["_tag"], "ServiceUnavailableError");
    assert_eq!(body["service"], "mcp_serve");
    let frame = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
    let (status, _, _) = server.send(server.post(None).body(frame.to_string())).await;
    assert_eq!(status, 503);
}

#[tokio::test]
async fn one_protocol_revision_is_negotiated() {
    let server = Server::start("127.0.0.1:0").await;
    let (session, init) = server.initialize("2025-11-25").await;
    let result = &init["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert_eq!(
        result["capabilities"],
        json!({"tools":{"listChanged":false}})
    );
    assert_eq!(
        result["serverInfo"],
        json!({"name":"cyber-code","title":"Cyber Code","version":"9.9.9-test"})
    );
    let instructions = result["instructions"].as_str().unwrap();
    assert!(instructions.len() <= 2048, "{}", instructions.len());
    assert!(instructions.contains("untrusted data"), "{instructions}");
    assert!(
        instructions.contains("answered by a human"),
        "{instructions}"
    );
    assert!(instructions.contains("idempotency_key"), "{instructions}");
    for tool in TOOLS {
        assert!(instructions.contains(tool), "{tool}");
    }

    let ping = json!({"jsonrpc":"2.0","id":5,"method":"ping"});
    let (status, _, text) = server
        .send(
            server
                .post(Some(&session))
                .header("mcp-protocol-version", "2024-11-05")
                .body(ping.to_string()),
        )
        .await;
    assert_eq!(status, 400);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["error"]["code"], -32600);
    assert_eq!(body["error"]["data"]["supported"], json!(["2025-06-18"]));
    let (status, _, _) = server
        .send(
            server
                .post(Some(&session))
                .header("mcp-protocol-version", "2025-06-18")
                .body(ping.to_string()),
        )
        .await;
    assert_eq!(status, 200);

    let (status, body) = server
        .rpc(
            Some(&session),
            json!({"jsonrpc":"2.0","id":6,"method":"tasks/list"}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["error"]["code"], -32601);
    for method in ["resources/list", "prompts/list", "sampling/createMessage"] {
        let (_, body) = server
            .rpc(
                Some(&session),
                json!({"jsonrpc":"2.0","id":6,"method":method}),
            )
            .await;
        assert_eq!(body["error"]["code"], -32601, "{method}");
    }

    let (status, _, text) = server
        .send(server.post(Some(&session)).body("{not json"))
        .await;
    assert_eq!(status, 400);
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["error"]["code"], -32700);
    for frame in [
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
        json!([{"jsonrpc":"2.0","id":1,"method":"ping"}]),
        json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":1,"method":7}),
    ] {
        let (status, body) = server.rpc(Some(&session), frame.clone()).await;
        assert_eq!(status, 400, "{frame}");
        assert_eq!(body["error"]["code"], -32600, "{frame}");
    }
}

#[tokio::test]
async fn tools_are_strict_and_map_route_errors_unchanged() {
    let server = Server::start("127.0.0.1:0").await;
    let (session, _) = server.initialize("2025-06-18").await;
    let (_, listed) = server
        .rpc(
            Some(&session),
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
        )
        .await;
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, TOOLS);
    for tool in tools {
        let name = &tool["name"];
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
        assert_eq!(tool["outputSchema"]["type"], "object", "{name}");
        for hint in [
            "readOnlyHint",
            "destructiveHint",
            "idempotentHint",
            "openWorldHint",
        ] {
            assert!(tool["annotations"][hint].is_boolean(), "{name} {hint}");
        }
        assert!(!tool["description"].as_str().unwrap().is_empty());
    }
    let hints = |name: &str| {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        let a = &tool["annotations"];
        [
            &a["readOnlyHint"],
            &a["destructiveHint"],
            &a["idempotentHint"],
            &a["openWorldHint"],
        ]
        .map(|v| v.as_bool().unwrap())
    };
    assert_eq!(hints("cyber_session_list"), [true, false, true, false]);
    assert_eq!(hints("cyber_prompt_wait"), [true, false, true, false]);
    assert_eq!(hints("cyber_session_create"), [false, false, false, false]);
    assert_eq!(hints("cyber_prompt"), [false, true, false, true]);
    assert_eq!(hints("cyber_interrupt"), [false, true, true, false]);
    assert_eq!(hints("cyber_message_list"), [true, false, true, false]);
    assert_eq!(hints("cyber_message_history"), [true, false, true, false]);
    assert_eq!(hints("cyber_message_send"), [false, false, false, true]);

    let error = tool_error(
        &server
            .call(&session, "cyber_session_list", json!({"limit":500}))
            .await["result"],
    );
    assert_eq!(error["_tag"], "InvalidRequestError");
    assert_eq!(error["reason"], "schema");
    assert_eq!(error["path"], "/limit");
    let error = tool_error(
        &server
            .call(
                &session,
                "cyber_session_read",
                json!({"session_id":"ses_1","extra":1}),
            )
            .await["result"],
    );
    assert_eq!(
        (error["reason"].as_str(), error["path"].as_str()),
        (Some("schema"), Some("/extra"))
    );
    let error = tool_error(&server.call(&session, "cyber_session_read", json!({})).await["result"]);
    assert_eq!(
        (error["reason"].as_str(), error["path"].as_str()),
        (Some("schema"), Some("/session_id"))
    );

    let unknown = server.call(&session, "cyber_nope", json!({})).await;
    assert_eq!(unknown["error"]["code"], -32602, "{unknown}");

    let missing = tool_error(
        &server
            .call(
                &session,
                "cyber_session_read",
                json!({"session_id":"ses_missing"}),
            )
            .await["result"],
    );
    assert_eq!(missing["_tag"], "SessionNotFoundError");
    let cursor = tool_error(
        &server
            .call(&session, "cyber_session_list", json!({"cursor":"garbage"}))
            .await["result"],
    );
    assert_eq!(cursor["_tag"], "InvalidCursorError");

    // Reading tools see every Session of the server; the list is a tool success.
    let id = server.h.session().await;
    let listed = &server.call(&session, "cyber_session_list", json!({})).await["result"];
    assert_eq!(listed["isError"], false);
    assert_eq!(
        serde_json::from_str::<Value>(listed["content"][0]["text"].as_str().unwrap()).unwrap(),
        listed["structuredContent"]
    );
    assert_eq!(
        listed["structuredContent"]["sessions"][0]["session_id"],
        json!(id)
    );
    assert_eq!(listed["structuredContent"]["sessions"][0]["state"], "idle");
}

#[cfg(unix)]
#[tokio::test]
async fn same_user_unix_socket_needs_no_password() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = Server::start("127.0.0.1:0").await;
    let socket = server.h.dir.path().join("s.sock");
    let state = AppState {
        service: None,
        telemetry: None,
        runtime: server.h.runtime.clone(),
        remote_tools: Arc::default(),
        store: Arc::clone(&server.h.store),
        services: Arc::clone(&server.catalog) as Arc<dyn Services>,
        options: Arc::new(HttpOptions {
            version: "9.9.9-test".into(),
            password: Some(PASSWORD.into()),
            cors_origins: Vec::new(),
            default_directory: server.h.repo.clone(),
            features: Vec::new(),
        }),
    };
    let path = socket.clone();
    tokio::spawn(async move {
        http::serve_unix(http::router(state), &path, std::future::pending()).await
    });
    let mut stream = loop {
        match tokio::net::UnixStream::connect(&socket).await {
            Ok(stream) => break stream,
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    };
    let body = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"bridge","version":"1"}}})
    .to_string();
    let request = format!(
        "POST /api/v1/mcp/serve HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response.to_ascii_lowercase().contains("mcp-session-id:"),
        "{response}"
    );
    assert!(
        response.contains("\"protocolVersion\":\"2025-06-18\""),
        "{response}"
    );
}

// Session creation over MCP (`mcp-server` → Session creation over MCP).

impl Server {
    async fn create(&self, session: &str, arguments: Value) -> Value {
        self.call(session, "cyber_session_create", arguments).await["result"].clone()
    }

    fn repo(&self) -> String {
        self.h.repo.display().to_string()
    }

    /// POST a frame and read its whole answer, JSON or SSE.
    async fn stream(&self, session: &str, frame: Value) -> (StatusCode, Sse) {
        let (status, headers, text) = self
            .send(self.post(Some(session)).body(frame.to_string()))
            .await;
        let kind = headers
            .get("content-type")
            .map(|t| t.to_str().unwrap().to_owned())
            .unwrap_or_default();
        if kind.starts_with("text/event-stream") {
            (status, Sse::parse(&text))
        } else {
            let frame = serde_json::from_str(&text).unwrap_or(Value::Null);
            (
                status,
                Sse {
                    comments: Vec::new(),
                    frames: vec![frame],
                    receipts: Vec::new(),
                },
            )
        }
    }

    /// POST a frame in the background; the answer is the whole body.
    fn spawn_post(&self, session: &str, frame: &Value) -> tokio::task::JoinHandle<Sse> {
        let request = self.post(Some(session)).body(frame.to_string());
        tokio::spawn(async move {
            let response = request.send().await.unwrap();
            Sse::parse(&response.text().await.unwrap())
        })
    }

    /// `cyber_prompt` and its result, through whichever answer the server chose.
    async fn prompt(&self, session: &str, arguments: Value) -> Value {
        let (status, sse) = self.stream(session, prompt_frame(9, arguments, None)).await;
        assert_eq!(status, StatusCode::OK);
        sse.result(9)
    }

    async fn wait(&self, session: &str, arguments: Value) -> Value {
        let frame = json!({"jsonrpc":"2.0","id":8,"method":"tools/call",
            "params":{"name":"cyber_prompt_wait","arguments":arguments}});
        let (status, sse) = self.stream(session, frame).await;
        assert_eq!(status, StatusCode::OK);
        sse.result(8)
    }

    fn set_serve(&self, config: Value) {
        *self.catalog.serve.lock().unwrap() = McpServeSettings::from_config(&config).unwrap();
    }

    /// A `tools/call` carrying the recursion marker headers the stdio bridge forwards.
    async fn call_marked(
        &self,
        session: &str,
        name: &str,
        arguments: Value,
        markers: &[(&str, &str)],
    ) -> Value {
        let frame = json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":name,"arguments":arguments}});
        let mut request = self.post(Some(session)).body(frame.to_string());
        for (name, value) in markers {
            request = request.header(*name, *value);
        }
        let (status, _, text) = self.send(request).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let body = Sse::parse(&text)
            .frames
            .pop()
            .unwrap_or_else(|| serde_json::from_str(&text).unwrap());
        body["result"].clone()
    }
}

#[tokio::test]
async fn session_creation_bounds_the_mode_by_the_ceiling() {
    let server = Server::start("127.0.0.1:0").await;
    let (session, _) = server.initialize("2025-06-18").await;
    let repo = server.repo();

    // Mode above the ceiling: the global mode is unset, so the ceiling is `default`.
    let refused = tool_error(
        &server
            .create(&session, json!({"directory": repo, "mode": "auto"}))
            .await,
    );
    assert_eq!(refused["_tag"], "ForbiddenError", "{refused}");
    assert_eq!(refused["reason"], "mode_ceiling");
    assert_eq!(refused["ceiling"], "default");
    assert_eq!(session_count(&server), 0, "no Session row");

    // Bypass always refused, even under the highest ceiling.
    server.set_serve(json!({"mcp_serve": {"max_mode": "auto"}}));
    let bypass = tool_error(
        &server
            .create(&session, json!({"directory": repo, "mode": "bypass"}))
            .await,
    );
    assert_eq!(
        (bypass["reason"].as_str(), bypass["ceiling"].as_str()),
        (Some("mode_ceiling"), Some("auto"))
    );
    server.set_serve(json!({}));

    // Project default clamped.
    server.catalog.defaults.lock().unwrap().mode = Some("accept-edits".into());
    let clamped = tool_output(&server.create(&session, json!({"directory": repo})).await);
    assert_eq!(clamped["effective_mode"], "default", "{clamped}");
    assert_eq!(clamped["session"]["mode"], "default");
    let id = clamped["session"]["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let state = server.h.state(&id).await;
    assert_eq!(state.info.mode, "default");
    // Exec non-interactive rules: the Session never waits on a human answer.
    for action in ["question", "plan_enter", "plan_exit"] {
        assert_eq!(state.info.rules[action], "deny", "{action}");
    }
    server.catalog.defaults.lock().unwrap().mode = None;

    // Default agent clamped; the same agent named explicitly is refused.
    server.h.tools.inference.lock().unwrap().permission_mode = Some("auto".into());
    let agent = tool_output(&server.create(&session, json!({"directory": repo})).await);
    assert_eq!(agent["effective_mode"], "default", "{agent}");
    let before = session_count(&server);
    let explicit = tool_error(
        &server
            .create(&session, json!({"directory": repo, "agent": "build"}))
            .await,
    );
    assert_eq!(explicit["reason"], "mode_ceiling", "{explicit}");
    assert_eq!(session_count(&server), before);
    server.h.tools.inference.lock().unwrap().permission_mode = None;

    // Safer Mode accepted.
    let safer = tool_output(
        &server
            .create(
                &session,
                json!({"directory": repo, "mode": "dont-ask", "title": "Safe"}),
            )
            .await,
    );
    assert_eq!(safer["effective_mode"], "dont-ask");
    assert_eq!(safer["session"]["mode"], "dont-ask");
    assert_eq!(safer["session"]["title"], "Safe");
    assert_eq!(safer["session"]["state"], "idle");
    assert_eq!(safer["session"]["mcp_client"], "test-client");
    assert!(safer.get("replayed").is_none(), "{safer}");
}

#[tokio::test]
async fn session_creation_requires_an_existing_absolute_directory() {
    let server = Server::start("127.0.0.1:0").await;
    let (session, _) = server.initialize("2025-06-18").await;
    let file = server.h.repo.join("file.txt");
    std::fs::write(&file, "x").unwrap();
    let missing = server.h.repo.join("missing");
    for directory in [
        "repo".to_owned(),
        missing.display().to_string(),
        file.display().to_string(),
    ] {
        let error = tool_error(
            &server
                .create(&session, json!({"directory": directory}))
                .await,
        );
        assert_eq!(error["_tag"], "InvalidRequestError", "{directory}: {error}");
        assert_eq!(error["reason"], "not_a_directory", "{directory}");
    }
    assert_eq!(session_count(&server), 0);
}

#[tokio::test]
async fn idempotent_creation_replays_once_and_refuses_other_arguments() {
    let server = Server::start("127.0.0.1:0").await;
    let (session, _) = server.initialize("2025-06-18").await;
    let repo = server.repo();
    let arguments = json!({"directory": repo, "title": "Once", "idempotency_key": "k1"});
    let first = tool_output(&server.create(&session, arguments.clone()).await);
    let second = tool_output(&server.create(&session, arguments).await);
    assert_eq!(
        first["session"]["session_id"],
        second["session"]["session_id"]
    );
    assert!(first.get("replayed").is_none(), "{first}");
    assert_eq!(second["replayed"], true, "{second}");
    assert_eq!(second["effective_mode"], first["effective_mode"]);
    assert_eq!(second["session"]["mcp_client"], "test-client");
    assert_eq!(session_count(&server), 1, "one Session");

    // Another directory with the same key, from another connection.
    let other = server.h.dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let (session2, _) = server.initialize("2025-06-18").await;
    let mismatch = tool_error(
        &server
            .create(
                &session2,
                json!({"directory": other.display().to_string(), "title": "Once", "idempotency_key": "k1"}),
            )
            .await,
    );
    assert_eq!(mismatch["_tag"], "ConflictError", "{mismatch}");
    assert_eq!(mismatch["reason"], "idempotency_mismatch");
    assert_eq!(session_count(&server), 1);

    // A 64-character key fits the 128-character server-api limit.
    let long = "k".repeat(64);
    let fits = tool_output(
        &server
            .create(
                &session,
                json!({"directory": repo, "idempotency_key": long}),
            )
            .await,
    );
    assert!(fits["session"]["session_id"].is_string());
    let unprintable = tool_error(
        &server
            .create(
                &session,
                json!({"directory": repo, "idempotency_key": "a b"}),
            )
            .await,
    );
    assert_eq!(
        (unprintable["reason"].as_str(), unprintable["path"].as_str()),
        (Some("schema"), Some("/idempotency_key"))
    );
    assert_eq!(session_count(&server), 2);
}

#[tokio::test]
async fn each_connection_creates_at_most_its_quota_and_replays_are_not_counted() {
    let server = Server::start("127.0.0.1:0").await;
    let (session, _) = server.initialize("2025-06-18").await;
    let repo = server.repo();
    for _ in 0..7 {
        tool_output(&server.create(&session, json!({"directory": repo})).await);
    }
    let eighth = json!({"directory": repo, "idempotency_key": "eighth"});
    let created = tool_output(&server.create(&session, eighth.clone()).await);
    assert_eq!(session_count(&server), 8);

    let refused = tool_error(&server.create(&session, json!({"directory": repo})).await);
    assert_eq!(refused["_tag"], "ForbiddenError", "{refused}");
    assert_eq!(refused["reason"], "session_quota");
    assert_eq!(refused["limit"], 8);
    assert_eq!(session_count(&server), 8, "no row for a refused call");

    // A retry of the 8th create at the quota replays.
    let retried = tool_output(&server.create(&session, eighth).await);
    assert_eq!(retried["replayed"], true, "{retried}");
    assert_eq!(
        retried["session"]["session_id"],
        created["session"]["session_id"]
    );

    // The count belongs to the MCP connection.
    let (fresh, _) = server.initialize("2025-06-18").await;
    tool_output(&server.create(&fresh, json!({"directory": repo})).await);
    assert_eq!(session_count(&server), 9);
}

#[tokio::test]
async fn mcp_only_lists_the_sessions_created_through_mcp() {
    let server = Server::start("127.0.0.1:0").await;
    let plain = server.h.session().await;
    let (session, _) = server
        .initialize_as("2025-06-18", Some("claude-code"))
        .await;
    let created = tool_output(
        &server
            .create(&session, json!({"directory": server.repo()}))
            .await,
    );
    let id = created["session"]["session_id"].clone();
    assert_eq!(created["session"]["mcp_client"], "claude-code");

    let all = tool_output(&server.call(&session, "cyber_session_list", json!({})).await["result"]);
    let ids: Vec<&Value> = all["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| &s["session_id"])
        .collect();
    assert!(ids.contains(&&json!(plain)) && ids.contains(&&id), "{all}");
    let listed_plain = all["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == json!(plain))
        .unwrap();
    assert!(listed_plain.get("mcp_client").is_none(), "{listed_plain}");

    let only = tool_output(
        &server
            .call(&session, "cyber_session_list", json!({"mcp_only": true}))
            .await["result"],
    );
    let sessions = only["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{only}");
    assert_eq!(sessions[0]["session_id"], id);
    assert_eq!(sessions[0]["mcp_client"], "claude-code");

    let read = tool_output(
        &server
            .call(&session, "cyber_session_read", json!({"session_id": id}))
            .await["result"],
    );
    assert_eq!(read["session"]["mcp_client"], "claude-code");
}

/// One answer read as SSE: its comment lines, its JSON-RPC frames in order, and the
/// `dev.cyber-code/receipt` events (not MCP frames).
struct Sse {
    comments: Vec<String>,
    frames: Vec<Value>,
    receipts: Vec<Value>,
}

impl Sse {
    fn parse(text: &str) -> Self {
        let mut comments = Vec::new();
        let mut frames = Vec::new();
        let mut receipts = Vec::new();
        for block in text.split("\n\n") {
            let mut data = String::new();
            let mut receipt = false;
            for line in block.lines() {
                if let Some(comment) = line.strip_prefix(':') {
                    comments.push(comment.trim().to_owned());
                } else if let Some(line) = line.strip_prefix("data:") {
                    data.push_str(line.trim_start());
                } else if let Some(name) = line.strip_prefix("event:") {
                    assert_eq!(name.trim(), "dev.cyber-code/receipt", "{text}");
                    receipt = true;
                }
            }
            if data.is_empty() {
                continue;
            }
            let value = serde_json::from_str(&data).unwrap();
            if receipt {
                receipts.push(value);
            } else {
                frames.push(value);
            }
        }
        Self {
            comments,
            frames,
            receipts,
        }
    }

    /// The tool result answering request `id`, which must be the last frame.
    fn result(&self, id: u64) -> Value {
        let last = self.frames.last().expect("an answer");
        assert_eq!(last["id"], id, "{:?}", self.frames);
        last["result"].clone()
    }

    fn progress(&self) -> Vec<&Value> {
        self.frames
            .iter()
            .filter(|f| f["method"] == "notifications/progress")
            .collect()
    }
}

fn prompt_frame(id: u64, arguments: Value, token: Option<&str>) -> Value {
    let mut params = json!({"name":"cyber_prompt","arguments":arguments});
    if let Some(token) = token {
        params["_meta"] = json!({"progressToken": token});
    }
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":params})
}

fn scripted(turns: Vec<Vec<cyber_llm::adapters::ScriptStep>>) -> Setup {
    Setup {
        scripts: vec![("test/main", turns)],
        ..Setup::default()
    }
}

async fn tool_started(server: &Server) {
    tokio::time::timeout(Duration::from_secs(10), server.h.tools.started.notified())
        .await
        .expect("the tool started");
}

fn user_entries(state: &cyber_server::runtime::SessionState, message: &str) -> usize {
    state
        .entries
        .iter()
        .filter(|e| matches!(e, cyber_server::runtime::Entry::User { id, .. } if id == message))
        .count()
}

#[tokio::test]
async fn prompt_arguments_are_checked_before_any_write() {
    let server = Server::start("127.0.0.1:0").await;
    let (mcp, _) = server.initialize("2025-06-18").await;
    let existing = server.h.session().await;
    let repo = server.repo();
    let large = "x".repeat(200 * 1024);
    let cases = [
        (json!({"prompt":"hi"}), "exclusive_fields"),
        (
            json!({"prompt":"hi","session_id":existing,"directory":repo}),
            "exclusive_fields",
        ),
        (
            json!({"prompt":"hi","session_id":existing,"mode":"plan"}),
            "session_settings_on_existing",
        ),
        (json!({"prompt":large,"directory":repo}), "too_large"),
        (json!({"prompt":large,"session_id":existing}), "too_large"),
    ];
    for (arguments, reason) in cases {
        let error = tool_error(&server.prompt(&mcp, arguments).await);
        assert_eq!(error["_tag"], "InvalidRequestError", "{error}");
        assert_eq!(error["reason"], reason, "{error}");
    }
    assert_eq!(session_count(&server), 1, "nothing created");
    assert!(
        server.h.state(&existing).await.inbox.is_empty(),
        "nothing admitted"
    );
}

#[tokio::test]
async fn a_prompt_runs_to_completion_with_progress_and_replays_by_key() {
    let server = Server::start_with("127.0.0.1:0", scripted(vec![text("all fixed")])).await;
    let (mcp, _) = server
        .initialize_as("2025-06-18", Some("claude-code"))
        .await;
    let arguments = json!({"directory": server.repo(), "prompt": "fix the failing test",
        "idempotency_key": "p1"});
    let (status, sse) = server
        .stream(&mcp, prompt_frame(5, arguments.clone(), Some("tok")))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        sse.comments.iter().any(|c| c == "keep-alive"),
        "{:?}",
        sse.comments
    );
    let result = tool_output(&sse.result(5));
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(result["done"], true);
    assert_eq!(result["stop_reason"], "completed");
    assert_eq!(result["final_text"], "all fixed");
    assert!(result["admitted_at"].is_i64() && result["finished_at"].is_i64());
    assert!(result.get("replayed").is_none());
    let message = result["message_id"].as_str().unwrap().to_owned();
    let session = result["session_id"].as_str().unwrap().to_owned();

    // Progress: first `admitted` with the message, strictly increasing, then the result.
    let notes = sse.progress();
    assert!(!notes.is_empty(), "{:?}", sse.frames);
    assert_eq!(
        notes.len() + 1,
        sse.frames.len(),
        "nothing after the result"
    );
    // The receipt event names the prompt for a client whose stream drops.
    assert_eq!(
        sse.receipts,
        vec![json!({"session_id": session, "message_id": message})],
    );
    let first = &notes[0]["params"];
    assert_eq!(first["progressToken"], "tok");
    assert_eq!(
        first["_meta"]["dev.cyber-code/progress"]["kind"],
        "admitted"
    );
    assert_eq!(first["_meta"]["dev.cyber-code/progress"]["v"], 1);
    assert_eq!(
        first["_meta"]["dev.cyber-code/progress"]["message_id"],
        json!(message)
    );
    let mut last = -1.0;
    for note in &notes {
        let params = &note["params"];
        let progress = params["progress"].as_f64().unwrap();
        assert!(progress > last, "{notes:?}");
        last = progress;
        assert!(params["message"].as_str().unwrap().chars().count() <= 200);
        assert_eq!(params["_meta"]["dev.cyber-code/untrusted"], true);
    }

    // An exact retry replays the same prompt and waits on the same run.
    let replay = tool_output(&server.prompt(&mcp, arguments.clone()).await);
    assert_eq!(replay["replayed"], true, "{replay}");
    assert_eq!(replay["message_id"], json!(message));
    assert_eq!(replay["session_id"], json!(session));
    assert_eq!(replay["status"], "completed");
    let mut no_wait = arguments.clone();
    no_wait["wait"] = json!(false);
    no_wait["wait_seconds"] = json!(3);
    let replay = tool_output(&server.prompt(&mcp, no_wait).await);
    assert_eq!(replay["replayed"], true, "{replay}");
    assert_eq!(replay["message_id"], json!(message));
    // The key is shared by every connection.
    let (other, _) = server.initialize("2025-06-18").await;
    let replay = tool_output(&server.prompt(&other, arguments.clone()).await);
    assert_eq!(replay["message_id"], json!(message));

    // Another prompt under the same key conflicts before any write.
    let mut changed = arguments;
    changed["prompt"] = json!("something else");
    let mismatch = tool_error(&server.prompt(&mcp, changed.clone()).await);
    assert_eq!(mismatch["_tag"], "ConflictError", "{mismatch}");
    assert_eq!(mismatch["reason"], "idempotency_mismatch");
    // The same prompt to the Session by id is another call too.
    changed["prompt"] = json!("fix the failing test");
    changed.as_object_mut().unwrap().remove("directory");
    changed["session_id"] = json!(session);
    let mismatch = tool_error(&server.prompt(&mcp, changed).await);
    assert_eq!(mismatch["reason"], "idempotency_mismatch", "{mismatch}");

    let state = server.h.state(&session).await;
    assert_eq!(user_entries(&state, &message), 1, "the prompt ran once");
    assert_eq!(state.inbox.len(), 1);
    assert_eq!(
        server.h.models.requests("test/main").len(),
        1,
        "one model call"
    );
    assert_eq!(session_count(&server), 1);

    let read = tool_output(
        &server
            .call(&mcp, "cyber_session_read", json!({"session_id": session}))
            .await["result"],
    );
    assert_eq!(read["session"]["mcp_client"], "claude-code");
    let recent = read["recent_prompts"].as_array().unwrap();
    assert_eq!(recent.len(), 1, "{read}");
    assert_eq!(recent[0]["message_id"], json!(message));
    assert_eq!(recent[0]["status"], "completed");
    assert_eq!(recent[0]["admitted_at"], result["admitted_at"]);
}

#[tokio::test]
async fn prompt_text_is_literal_and_prompt_hooks_run() {
    let server = Server::start_with("127.0.0.1:0", scripted(vec![text("noted")])).await;
    let (mcp, _) = server.initialize("2025-06-18").await;
    let id = server.h.session().await;
    let mode = server.h.state(&id).await.info.mode;
    let result = tool_output(
        &server
            .prompt(&mcp, json!({"session_id": id, "prompt": "/mode bypass"}))
            .await,
    );
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(server.h.state(&id).await.info.mode, mode, "Mode unchanged");
    let requests = server.h.models.requests("test/main");
    assert!(
        support::transcript(&requests[0]).contains("/mode bypass"),
        "the text reached the model as user input"
    );

    *server.h.tools.prompt_denial.lock().unwrap() = Some("no prompts today".into());
    let denied = tool_error(
        &server
            .prompt(&mcp, json!({"session_id": id, "prompt": "again"}))
            .await,
    );
    assert!(
        denied["message"]
            .as_str()
            .unwrap()
            .contains("no prompts today"),
        "{denied}"
    );
    assert_eq!(server.h.state(&id).await.inbox.len(), 1, "nothing admitted");
}

#[tokio::test]
async fn waits_return_receipts_poll_and_reattach() {
    let turns = vec![
        tools(&[("c1", "shell", "{}")]),
        text("finished"),
        text("second answer"),
    ];
    let server = Server::start_with("127.0.0.1:0", scripted(turns)).await;
    server.h.tools.set("shell", Behavior::Gated);
    let (mcp, _) = server.initialize("2025-06-18").await;
    let started = std::time::Instant::now();
    let timed_out = tool_output(
        &server
            .prompt(
                &mcp,
                json!({"directory": server.repo(), "prompt": "run it", "wait_seconds": 1}),
            )
            .await,
    );
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(timed_out["done"], false, "{timed_out}");
    assert!(matches!(
        timed_out["status"].as_str(),
        Some("queued" | "running")
    ));
    tool_started(&server).await;
    let ids = json!({"session_id": timed_out["session_id"], "message_id": timed_out["message_id"]});

    let mut poll = ids.clone();
    poll["wait_seconds"] = json!(0);
    let polled = tool_output(&server.wait(&mcp, poll).await);
    assert_eq!(polled["status"], "running", "{polled}");
    assert_eq!(polled["admitted_at"], timed_out["admitted_at"]);

    let unknown = tool_error(
        &server
            .wait(
                &mcp,
                json!({"session_id": timed_out["session_id"], "message_id": "msg_missing"}),
            )
            .await,
    );
    assert_eq!(unknown["_tag"], "MessageNotFoundError", "{unknown}");
    let unknown = tool_error(
        &server
            .wait(
                &mcp,
                json!({"session_id": "ses_missing", "message_id": timed_out["message_id"]}),
            )
            .await,
    );
    assert_eq!(unknown["_tag"], "SessionNotFoundError", "{unknown}");

    server.h.tools.release.notify_one();
    let done = tool_output(&server.wait(&mcp, ids).await);
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["final_text"], "finished");
    assert!(
        done["usage"]["input_tokens"].as_u64().unwrap() > 0,
        "{done}"
    );

    // A receipt without waiting, then the wait.
    let receipt = tool_output(
        &server
            .prompt(
                &mcp,
                json!({"session_id": timed_out["session_id"], "prompt": "again", "wait": false}),
            )
            .await,
    );
    assert_eq!(receipt["done"], false, "{receipt}");
    assert!(matches!(
        receipt["status"].as_str(),
        Some("queued" | "running")
    ));
    let done = tool_output(
        &server
            .wait(
                &mcp,
                json!({"session_id": receipt["session_id"], "message_id": receipt["message_id"]}),
            )
            .await,
    );
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["final_text"], "second answer");
}

#[tokio::test]
async fn a_cancelled_wait_frees_its_slot_while_the_turn_keeps_running() {
    let turns = vec![tools(&[("c1", "shell", "{}")]), text("finished")];
    let server = Server::start_with("127.0.0.1:0", scripted(turns)).await;
    server.h.tools.set("shell", Behavior::Gated);
    server.set_serve(json!({"mcp_serve":{"limits":{"max_waits":1}}}));
    let (mcp, _) = server.initialize("2025-06-18").await;
    let arguments = json!({"directory": server.repo(), "prompt": "long", "idempotency_key": "long-1",
        "wait_seconds": 600});
    let waiting = server.spawn_post(&mcp, &prompt_frame(11, arguments.clone(), Some("tok")));
    tool_started(&server).await;

    // The wait slot is taken: other waiting calls are refused before any write.
    let refused = tool_error(
        &server
            .prompt(&mcp, json!({"directory": server.repo(), "prompt": "other"}))
            .await,
    );
    assert_eq!(refused["_tag"], "RateLimitedError", "{refused}");
    assert_eq!(refused["reason"], "concurrency");
    assert_eq!(refused["retryAfterMs"], 5000);
    assert_eq!(session_count(&server), 1, "nothing created");
    let mut receipt = arguments.clone();
    receipt["wait"] = json!(false);
    let receipt = tool_output(&server.prompt(&mcp, receipt).await);
    assert_eq!(receipt["replayed"], true);
    let ids = json!({"session_id": receipt["session_id"], "message_id": receipt["message_id"]});
    let mut five = ids.clone();
    five["wait_seconds"] = json!(5);
    let refused = tool_error(&server.wait(&mcp, five).await);
    assert_eq!(refused["reason"], "concurrency", "{refused}");

    // Cancellation ends the wait without a result and frees the slot.
    let (status, _) = server
        .rpc(
            Some(&mcp),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled",
                "params":{"requestId":11,"reason":"user"}}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let ended = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("the wait ended")
        .unwrap();
    assert!(
        ended.frames.iter().all(|f| f.get("id").is_none()),
        "{:?}",
        ended.frames
    );
    let mut poll = ids.clone();
    poll["wait_seconds"] = json!(0);
    let polled = tool_output(&server.wait(&mcp, poll).await);
    assert_eq!(polled["status"], "running", "the Turn keeps running");

    // Deleting the MCP session ends a wait too.
    let mut long = ids.clone();
    long["wait_seconds"] = json!(600);
    let frame = json!({"jsonrpc":"2.0","id":12,"method":"tools/call",
        "params":{"name":"cyber_prompt_wait","arguments":long}});
    let second = server.spawn_post(&mcp, &frame);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let deleted = server
        .client
        .delete(server.url())
        .basic_auth("cyber", Some(PASSWORD))
        .header("mcp-session-id", &mcp)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let ended = tokio::time::timeout(Duration::from_secs(10), second)
        .await
        .expect("the wait ended")
        .unwrap();
    assert!(ended.frames.iter().all(|f| f.get("id").is_none()));
    let session = receipt["session_id"].as_str().unwrap();
    assert!(server.h.runtime.is_running(session), "never interrupted");

    server.h.tools.release.notify_one();
    let (fresh, _) = server.initialize("2025-06-18").await;
    let done = tool_output(&server.wait(&fresh, ids).await);
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["final_text"], "finished");
    let shells = server
        .h
        .tools
        .executed_names()
        .iter()
        .filter(|n| *n == "shell")
        .count();
    assert_eq!(shells, 1);
}

#[tokio::test]
async fn interrupt_ends_a_wait_as_interrupted() {
    let server = Server::start_with(
        "127.0.0.1:0",
        scripted(vec![tools(&[("c1", "shell", "{}")])]),
    )
    .await;
    server.h.tools.set("shell", Behavior::UntilCancelled);
    let (mcp, _) = server.initialize("2025-06-18").await;
    let arguments =
        json!({"directory": server.repo(), "prompt": "forever", "idempotency_key": "int"});
    let waiting = server.spawn_post(&mcp, &prompt_frame(21, arguments.clone(), None));
    tool_started(&server).await;
    let mut receipt = arguments;
    receipt["wait"] = json!(false);
    let receipt = tool_output(&server.prompt(&mcp, receipt).await);
    let interrupted = tool_output(
        &server
            .call(
                &mcp,
                "cyber_interrupt",
                json!({"session_id": receipt["session_id"]}),
            )
            .await["result"],
    );
    assert_eq!(interrupted["was_running"], true);
    let ended = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("the wait ended")
        .unwrap();
    let result = tool_output(&ended.result(21));
    assert_eq!(result["status"], "interrupted", "{result}");
    assert_eq!(result["done"], true);
}

#[tokio::test]
async fn max_running_sessions_refuses_prompts_but_never_creation() {
    let turns = vec![tools(&[("c1", "shell", "{}")]), text("done")];
    let server = Server::start_with("127.0.0.1:0", scripted(turns)).await;
    server.h.tools.set("shell", Behavior::Gated);
    server.set_serve(json!({"mcp_serve":{"limits":{"max_running_sessions":1}}}));
    let (mcp, _) = server.initialize("2025-06-18").await;
    let busy = json!({"directory": server.repo(), "prompt": "busy", "wait": false,
        "idempotency_key": "busy"});
    let first = tool_output(&server.prompt(&mcp, busy.clone()).await);
    tool_started(&server).await;

    let idle = server.h.session().await;
    for arguments in [
        json!({"directory": server.repo(), "prompt": "more", "wait": false}),
        json!({"session_id": idle, "prompt": "more", "wait": false}),
    ] {
        let refused = tool_error(&server.prompt(&mcp, arguments).await);
        assert_eq!(refused["_tag"], "RateLimitedError", "{refused}");
        assert_eq!(refused["reason"], "concurrency");
        assert_eq!(refused["retryAfterMs"], 5000);
    }
    assert_eq!(session_count(&server), 2, "no Session for a refused call");
    assert!(server.h.state(&idle).await.inbox.is_empty());
    tool_output(
        &server
            .create(&mcp, json!({"directory": server.repo()}))
            .await,
    );
    let retried = tool_output(&server.prompt(&mcp, busy).await);
    assert_eq!(retried["replayed"], true, "{retried}");
    assert_eq!(retried["message_id"], first["message_id"]);
    server.h.tools.release.notify_one();
}

#[tokio::test]
async fn a_request_raised_during_the_wait_returns_waiting_for_approval() {
    let turns = vec![tools(&[("c1", "ask", "{}")]), text("approved")];
    let server = Server::start_with("127.0.0.1:0", scripted(turns)).await;
    server
        .h
        .tools
        .set("ask", Behavior::AskConfirm("rm -rf /repo".into()));
    let (mcp, _) = server.initialize("2025-06-18").await;
    let id = server.h.session().await;
    let started = std::time::Instant::now();
    let result = tool_output(
        &server
            .prompt(&mcp, json!({"session_id": id, "prompt": "clean up"}))
            .await,
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "returned early, not at wait_seconds"
    );
    assert_eq!(result["status"], "waiting_for_approval", "{result}");
    assert_eq!(result["done"], false);
    let pending = result["pending"].as_array().unwrap();
    assert_eq!(pending.len(), 1, "{result}");
    assert_eq!(pending[0]["kind"], "permission");

    // An already pending request does not end the next wait early.
    let ids = json!({"session_id": id, "message_id": result["message_id"]});
    let mut short = ids.clone();
    short["wait_seconds"] = json!(1);
    let started = std::time::Instant::now();
    let again = tool_output(&server.wait(&mcp, short).await);
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(again["status"], "waiting_for_approval");

    let request = server.h.runtime.pending_requests(Some(&id)).remove(0);
    server
        .h
        .runtime
        .reply_permission(&request.id, cyber_server::runtime::PermissionReply::Once)
        .await
        .unwrap();
    let done = tool_output(&server.wait(&mcp, ids).await);
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["final_text"], "approved");
}

#[tokio::test]
async fn a_run_waits_for_the_jobs_it_started_and_their_handback() {
    let turns = vec![text("started the job"), text("handled the result")];
    let server = Server::start_with("127.0.0.1:0", scripted(turns)).await;
    let (mcp, _) = server.initialize("2025-06-18").await;
    let parent = server.h.session().await;
    let receipt = tool_output(
        &server
            .prompt(
                &mcp,
                json!({"session_id": parent, "prompt": "go", "wait": false}),
            )
            .await,
    );
    let message = receipt["message_id"].as_str().unwrap().to_owned();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while server
        .h
        .state(&parent)
        .await
        .input(&message)
        .unwrap()
        .status
        != cyber_server::runtime::InputStatus::Promoted
    {
        assert!(std::time::Instant::now() < deadline, "promoted");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let child = server
        .h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: server.repo(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            title: Some("Background child".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let (finish, finished) = tokio::sync::oneshot::channel::<()>();
    server
        .h
        .runtime
        .start_child_job(
            &parent,
            &child,
            "worker".into(),
            "background work".into(),
            None,
            Box::pin(async move {
                let _ = finished.await;
                Ok(json!({"answer": 42}))
            }),
        )
        .await
        .unwrap();
    server.h.settle(&parent).await;

    // The Session is idle, but the Job the run started has not settled.
    let ids = json!({"session_id": parent, "message_id": message});
    let mut poll = ids.clone();
    poll["wait_seconds"] = json!(0);
    let polled = tool_output(&server.wait(&mcp, poll).await);
    assert_eq!(polled["status"], "running", "{polled}");
    assert_eq!(polled["done"], false);

    finish.send(()).unwrap();
    let done = tool_output(&server.wait(&mcp, ids).await);
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(
        done["final_text"], "handled the result",
        "the handback Turn"
    );
}

#[tokio::test]
async fn a_call_cancelled_before_admission_still_admits() {
    let server = Server::start_with("127.0.0.1:0", scripted(vec![text("admitted anyway")])).await;
    let hold = Arc::new(tokio::sync::Notify::new());
    *server.h.tools.prompt_hold.lock().unwrap() = Some(Arc::clone(&hold));
    let (mcp, _) = server.initialize("2025-06-18").await;
    let id = server.h.session().await;
    let arguments = json!({"session_id": id, "prompt": "held", "idempotency_key": "held-1"});
    let waiting = server.spawn_post(&mcp, &prompt_frame(31, arguments.clone(), None));
    // In the hook, so the call is tracked as in flight and not yet admitted.
    tokio::time::timeout(
        Duration::from_secs(10),
        server.h.tools.prompt_reached.acquire(),
    )
    .await
    .expect("the prompt reached the hook")
    .unwrap()
    .forget();
    assert!(
        server.h.state(&id).await.inbox.is_empty(),
        "held in the hook"
    );
    let (status, _) = server
        .rpc(
            Some(&mcp),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":31}}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let ended = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("the wait ended")
        .unwrap();
    assert!(ended.frames.iter().all(|f| f.get("id").is_none()));

    hold.notify_one();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while server.h.state(&id).await.inbox.is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "admitted after the cancel"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    *server.h.tools.prompt_hold.lock().unwrap() = None;
    let replay = tool_output(&server.prompt(&mcp, arguments).await);
    assert_eq!(replay["replayed"], true, "{replay}");
    assert_eq!(replay["status"], "completed");
    assert_eq!(replay["final_text"], "admitted anyway");
    assert_eq!(server.h.state(&id).await.inbox.len(), 1);
}

/// Usage attribution: the MCP serve record exists before the prompt is admitted, so the
/// Turn's first step is `mcp` (regression: it was written after the admission returned,
/// and a step that began first was attributed to `user`).
#[tokio::test]
async fn a_prompt_is_recorded_as_mcp_before_its_turn_starts() {
    let server = Server::start_with("127.0.0.1:0", scripted(vec![text("attributed")])).await;
    let hold = Arc::new(tokio::sync::Notify::new());
    *server.h.tools.prompt_hold.lock().unwrap() = Some(Arc::clone(&hold));
    let (mcp, _) = server
        .initialize_as("2025-06-18", Some("claude-code"))
        .await;
    let id = server.h.session().await;
    let waiting = server.spawn_post(
        &mcp,
        &prompt_frame(41, json!({"session_id": id, "prompt": "held"}), None),
    );
    tokio::time::timeout(
        Duration::from_secs(10),
        server.h.tools.prompt_reached.acquire(),
    )
    .await
    .expect("the prompt reached the hook")
    .unwrap()
    .forget();
    let held = cyber_store::mcp_serve::recent_inputs(&server.h.store, &id, 10).unwrap();
    assert_eq!(held.len(), 1, "recorded while the admission is in the hook");
    assert_eq!(held[0].mcp_client, "claude-code");

    hold.notify_one();
    let sse = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("the prompt finished")
        .unwrap();
    let result = tool_output(&sse.result(41));
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(result["message_id"], json!(held[0].message_id));
    let origins: Vec<(String, Option<String>)> = server
        .h
        .store
        .read(|db| {
            let mut statement = db.prepare("SELECT origin, mcp_client FROM usage_step")?;
            let rows = statement
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(origins, [("mcp".into(), Some("claude-code".into()))]);
}

/// A prompt the admission refuses leaves no MCP serve record behind.
#[tokio::test]
async fn a_refused_prompt_leaves_no_mcp_record() {
    let server = Server::start("127.0.0.1:0").await;
    *server.h.tools.prompt_denial.lock().unwrap() = Some("no prompts today".into());
    let (mcp, _) = server.initialize("2025-06-18").await;
    let id = server.h.session().await;
    for arguments in [
        json!({"session_id": id, "prompt": "denied"}),
        json!({"session_id": id, "prompt": "denied", "idempotency_key": "d1"}),
    ] {
        let denied = tool_error(&server.prompt(&mcp, arguments).await);
        assert!(
            denied["message"]
                .as_str()
                .unwrap()
                .contains("no prompts today"),
            "{denied}"
        );
    }
    assert!(
        cyber_store::mcp_serve::recent_inputs(&server.h.store, &id, 10)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_closed_stream_ends_the_wait_and_frees_its_slot() {
    let turns = vec![tools(&[("c1", "shell", "{}")]), text("finished")];
    let server = Server::start_with("127.0.0.1:0", scripted(turns)).await;
    server.h.tools.set("shell", Behavior::Gated);
    server.set_serve(json!({"mcp_serve":{"limits":{"max_waits":1}}}));
    let (mcp, _) = server.initialize("2025-06-18").await;
    let arguments =
        json!({"directory": server.repo(), "prompt": "long", "idempotency_key": "gone"});
    let waiting = server.spawn_post(&mcp, &prompt_frame(41, arguments.clone(), None));
    tool_started(&server).await;
    waiting.abort();
    let mut receipt = arguments;
    receipt["wait"] = json!(false);
    let receipt = tool_output(&server.prompt(&mcp, receipt).await);
    let ids = json!({"session_id": receipt["session_id"], "message_id": receipt["message_id"],
        "wait_seconds": 1});
    // The server notices the closed stream at its next keep-alive at the latest.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let result = server.wait(&mcp, ids.clone()).await;
        if result["isError"] == false {
            assert_eq!(result["structuredContent"]["status"], "running");
            break;
        }
        assert_eq!(tool_error(&result)["reason"], "concurrency");
        assert!(std::time::Instant::now() < deadline, "the slot was freed");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    server.h.tools.release.notify_one();
}

#[tokio::test]
async fn a_prompt_result_covers_only_its_own_run() {
    use cyber_server::runtime::{Admission, Delivery};
    let turns = vec![
        tools(&[("c1", "shell", "{}")]),
        text("mcp answer"),
        text("queued human answer"),
        tools(&[("c2", "shell", "{}")]),
        text("later human answer"),
    ];
    let server = Server::start_with("127.0.0.1:0", scripted(turns)).await;
    server.h.tools.set("shell", Behavior::Gated);
    let (mcp, _) = server.initialize("2025-06-18").await;
    let id = server.h.session().await;
    let receipt = tool_output(
        &server
            .prompt(
                &mcp,
                json!({"session_id": id, "prompt": "from mcp", "wait": false}),
            )
            .await,
    );
    tool_started(&server).await;
    // A human queues a prompt while the MCP prompt's run is active.
    server
        .h
        .runtime
        .admit(&id, Admission::text("queued by a human", Delivery::Queue))
        .await
        .unwrap();
    server.h.tools.release.notify_one();
    server.h.settle(&id).await;
    let ids = json!({"session_id": id, "message_id": receipt["message_id"],
        "wait_seconds": 0});
    let first = tool_output(&server.wait(&mcp, ids.clone()).await);
    assert_eq!(first["status"], "completed", "{first}");
    assert_eq!(first["final_text"], "mcp answer", "{first}");
    assert_eq!(first["turns"], 1, "{first}");

    // A later human run neither reopens nor changes the MCP prompt's result.
    server
        .h
        .runtime
        .admit(
            &id,
            Admission::text("a later human prompt", Delivery::Steer),
        )
        .await
        .unwrap();
    tool_started(&server).await;
    let during = tool_output(&server.wait(&mcp, ids.clone()).await);
    assert_eq!(during["status"], "completed", "{during}");
    assert_eq!(during["final_text"], "mcp answer");
    assert!(during.get("pending").is_none(), "{during}");
    let read = tool_output(
        &server
            .call(&mcp, "cyber_session_read", json!({"session_id": id}))
            .await["result"],
    );
    assert_eq!(read["session"]["state"], "running", "{read}");
    assert_eq!(read["recent_prompts"][0]["status"], "completed", "{read}");
    server.h.tools.release.notify_one();
    server.h.settle(&id).await;
    let after = tool_output(&server.wait(&mcp, ids).await);
    assert_eq!(after["final_text"], "mcp answer", "{after}");
    assert_eq!(after["turns"], 1);
    assert_eq!(after["usage"], first["usage"], "no later usage is summed");
}

#[tokio::test]
async fn concurrent_prompts_cannot_exceed_max_running_sessions() {
    let server = Server::start_with("127.0.0.1:0", scripted(vec![text("one"), text("two")])).await;
    server.set_serve(json!({"mcp_serve":{"limits":{"max_running_sessions":2}}}));
    let hold = Arc::new(tokio::sync::Notify::new());
    *server.h.tools.prompt_hold.lock().unwrap() = Some(Arc::clone(&hold));
    let (mcp, _) = server.initialize("2025-06-18").await;
    let mut sessions = Vec::new();
    for _ in 0..5 {
        sessions.push(server.h.session().await);
    }
    let calls: Vec<_> = sessions
        .iter()
        .enumerate()
        .map(|(n, id)| {
            let frame = prompt_frame(
                60 + n as u64,
                json!({"session_id": id, "prompt": "go", "wait": false}),
                None,
            );
            let request = server.post(Some(&mcp)).body(frame.to_string());
            tokio::spawn(async move {
                let text = request.send().await.unwrap().text().await.unwrap();
                serde_json::from_str::<Value>(&text).unwrap()["result"].clone()
            })
        })
        .collect();
    // Two calls reserve the two runs and wait in the prompt hook; the other three are
    // refused although nothing is running yet.
    tokio::time::timeout(
        Duration::from_secs(10),
        server.h.tools.prompt_reached.acquire_many(2),
    )
    .await
    .expect("two prompts admitted to the hook")
    .unwrap()
    .forget();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while calls.iter().filter(|call| call.is_finished()).count() < 3 {
        assert!(std::time::Instant::now() < deadline, "three calls refused");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(server.h.tools.prompt_reached.available_permits(), 0);
    hold.notify_waiters();
    let mut refused = 0;
    for call in calls {
        let result = call.await.unwrap();
        if result["isError"] == true {
            assert_eq!(tool_error(&result)["reason"], "concurrency", "{result}");
            refused += 1;
        }
    }
    assert_eq!(refused, 3);
    let mut admitted = 0;
    for id in &sessions {
        server.h.settle(id).await;
        admitted += usize::from(!server.h.state(id).await.inbox.is_empty());
    }
    assert_eq!(admitted, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cancelled_creation_is_still_recorded_and_counted() {
    let server = Server::start("127.0.0.1:0").await;
    server.set_serve(json!({"mcp_serve":{"limits":{"sessions_per_connection":1}}}));
    let (release, released) = std::sync::mpsc::channel();
    let gate = Arc::new(Gate {
        reached: tokio::sync::Notify::new(),
        release: Mutex::new(released),
    });
    *server.catalog.creation_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let (mcp, _) = server.initialize("2025-06-18").await;
    let frame = json!({"jsonrpc":"2.0","id":51,"method":"tools/call","params":{
        "name":"cyber_session_create","arguments":{"directory": server.repo()}}});
    let waiting = server.spawn_post(&mcp, &frame);
    tokio::time::timeout(Duration::from_secs(10), gate.reached.notified())
        .await
        .expect("the creation route is running");
    let (status, _) = server
        .rpc(
            Some(&mcp),
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":51}}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let ended = tokio::time::timeout(Duration::from_secs(10), waiting)
        .await
        .expect("cancellation ends the answer")
        .unwrap();
    assert!(ended.frames.is_empty(), "no result: {:?}", ended.frames);
    *server.catalog.creation_gate.lock().unwrap() = None;
    release.send(()).unwrap();

    // The creation completes with its MCP serve record and its quota slot.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let listed = loop {
        let list = tool_output(
            &server
                .call(&mcp, "cyber_session_list", json!({"mcp_only": true}))
                .await["result"],
        );
        if list["sessions"].as_array().is_some_and(|s| !s.is_empty()) {
            break list;
        }
        assert!(std::time::Instant::now() < deadline, "recorded: {list}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(listed["sessions"][0]["mcp_client"].is_string(), "{listed}");
    let quota = tool_error(
        &server
            .create(&mcp, json!({"directory": server.repo()}))
            .await,
    );
    assert_eq!(quota["reason"], "session_quota", "{quota}");
}

#[tokio::test]
async fn the_recursion_guard_refuses_deep_and_self_targeted_calls() {
    let turns = vec![tools(&[("c1", "shell", "{}")]), text("done")];
    let server = Server::start_with("127.0.0.1:0", scripted(turns)).await;
    server.h.tools.set("shell", Behavior::Gated);
    let (mcp, _) = server.initialize("2025-06-18").await;
    let repo = server.repo();
    let create = json!({"directory": repo});

    // The bridge's depth marker at max_depth (2) refuses before any write.
    let before = session_count(&server);
    let deep = tool_error(
        &server
            .call_marked(
                &mcp,
                "cyber_session_create",
                create.clone(),
                &[("x-cyber-mcp-depth", "2")],
            )
            .await,
    );
    assert_eq!(deep["_tag"], "ForbiddenError", "{deep}");
    assert_eq!(deep["reason"], "depth_exceeded");
    assert_eq!(
        (deep["depth"].clone(), deep["max_depth"].clone()),
        (json!(2), json!(2))
    );
    assert_eq!(session_count(&server), before, "nothing created");

    // A Session never drives itself or its ancestors.
    let parent = server.h.session().await;
    let child = server
        .h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: repo.clone(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let caller = [("x-cyber-mcp-caller", child.as_str())];
    for (tool, target) in [
        ("cyber_prompt", &child),
        ("cyber_prompt", &parent),
        ("cyber_interrupt", &parent),
    ] {
        let mut arguments = json!({"session_id": target});
        if tool == "cyber_prompt" {
            arguments["prompt"] = json!("loop");
            arguments["wait"] = json!(false);
        }
        let refused = tool_error(&server.call_marked(&mcp, tool, arguments, &caller).await);
        assert_eq!(
            refused["reason"], "self_target",
            "{tool} {target}: {refused}"
        );
    }
    for id in [&parent, &child] {
        assert!(
            server.h.state(id).await.inbox.is_empty(),
            "nothing admitted"
        );
    }

    // An MCP prompt at depth 1 gives its run depth 1, recorded with the input; a call
    // from inside that run (the caller marker alone) is at depth 2.
    let receipt = tool_output(
        &server
            .call_marked(
                &mcp,
                "cyber_prompt",
                json!({"directory": repo, "prompt": "work", "wait": false}),
                &[("x-cyber-mcp-depth", "1")],
            )
            .await,
    );
    tool_started(&server).await;
    let worker = receipt["session_id"].as_str().unwrap().to_owned();
    let message = receipt["message_id"].as_str().unwrap();
    let record = cyber_store::mcp_serve::input(&server.h.store, message)
        .unwrap()
        .unwrap();
    assert_eq!(record.depth, 1);
    let read = tool_output(
        &server
            .call(&mcp, "cyber_session_read", json!({"session_id": worker}))
            .await["result"],
    );
    assert_eq!(read["session"]["mcp_depth"], 1, "{read}");
    let from_run = [("x-cyber-mcp-caller", worker.as_str())];
    let nested = tool_error(
        &server
            .call_marked(&mcp, "cyber_session_create", create.clone(), &from_run)
            .await,
    );
    assert_eq!(nested["reason"], "depth_exceeded", "{nested}");
    assert_eq!(nested["depth"], 2);
    // Reading is never depth-limited.
    tool_output(
        &server
            .call_marked(
                &mcp,
                "cyber_session_list",
                json!({}),
                &[("x-cyber-mcp-depth", "9")],
            )
            .await,
    );

    // Once that run ends, the Session's depth is 0 again.
    server.h.tools.release.notify_one();
    server.h.settle(&worker).await;
    tool_output(
        &server
            .call_marked(&mcp, "cyber_session_create", create, &from_run)
            .await,
    );
}
