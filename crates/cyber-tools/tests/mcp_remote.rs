//! Configured remote MCP servers over Streamable HTTP: framing, network policy, cleanup and
//! credential redaction against a real in-process HTTP server on 127.0.0.1.
#[path = "support/mcp_http.rs"]
mod mcp_http;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use cyber_core::config::{self, LoadRequest, Resolved};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_server::runtime::{McpConnectionPhase, McpConnectionStatus, mcp_connections};
use cyber_tools::mcp::{
    HttpFailure, IdleUpdate, LaunchFailure, LocalLauncher, LocalServer, McpError, McpLocationPin,
    RemoteTransport,
};
use mcp_http::{Behavior, McpHttp, SESSION};
use serde_json::{Value, json};

struct Fixture {
    _root: tempfile::TempDir,
    repo: PathBuf,
    home: PathBuf,
    temp: PathBuf,
    paths: Paths,
    env: HashMap<String, String>,
    /// Windows process ownership needs the trusted helper; header commands run with it.
    helper: Option<PathBuf>,
}

impl Fixture {
    fn new(config: Value) -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let repo = root.path().join("repo");
        let temp = root.path().join("scratch");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let env = HashMap::from([(
            "CYBER_HOME".into(),
            root.path().join("cyber").display().to_string(),
        )]);
        let paths = Paths::resolve(&env, &home);
        paths.ensure().unwrap();
        std::fs::write(paths.config.join("cyber.jsonc"), config.to_string()).unwrap();
        Self {
            _root: root,
            repo: repo.canonicalize().unwrap(),
            home,
            temp,
            paths,
            env,
            helper: cfg!(windows).then(cyber_sandbox::find_helper).flatten(),
        }
    }

    fn remote(url: &str, extra: Value) -> Self {
        let mut config = json!({"mcp":{"remote":{"type":"remote","url":url,"headers":{"x-api-key":"PRIVATE-HEADER"}}}});
        merge(&mut config, extra);
        Self::new(config)
    }

    /// Static headers plus a `headers_command` whose output overrides them.
    fn commanded(url: &str, command: &str, refresh: Option<u32>) -> Self {
        let mut remote = json!({"type":"remote","url":url,"headers_command":command,
            "headers":{"x-api-key":"PRIVATE-HEADER","authorization":"Bearer PRIVATE-STATIC"}});
        if let Some(refresh) = refresh {
            remote["headers_refresh_seconds"] = json!(refresh);
        }
        Self::new(json!({"mcp":{"remote":remote}}))
    }

    fn load(&self) -> Resolved {
        config::load(&LoadRequest {
            location: &self.repo,
            paths: &self.paths,
            env: &self.env,
            home: &self.home,
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap()
    }

    fn launcher<'a>(&'a self, loaded: &'a Resolved, trust: &'a TrustStore) -> LocalLauncher<'a> {
        LocalLauncher {
            resolved: loaded,
            trust,
            location: &self.repo,
            home: &self.home,
            temp_dir: &self.temp,
            helper: self.helper.as_deref(),
            credential_env_names: &[],
        }
    }

    async fn connect(&self) -> Result<LocalServer, cyber_tools::mcp::LocalLaunchError> {
        let loaded = self.load();
        let trust = TrustStore::new(self.paths.trust_file());
        self.launcher(&loaded, &trust)
            .connect("remote", || Ok(()))
            .await
    }
}

fn merge(target: &mut Value, extra: Value) {
    if let (Some(target), Value::Object(extra)) = (target.as_object_mut(), extra) {
        for (key, value) in extra {
            target.insert(key, value);
        }
    }
}

fn store() -> std::sync::Arc<cyber_store::Store> {
    std::sync::Arc::new(
        cyber_store::Store::open(cyber_store::StoreOptions::new(
            cyber_core::paths::DatabaseLocation::Memory,
            cyber_server::runtime::Runtime::registry(),
        ))
        .unwrap(),
    )
}

#[tokio::test]
async fn json_responses_initialize_discover_call_and_echo_session_headers() {
    let http = McpHttp::start(Behavior::default()).await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let mut server = fixture.connect().await.unwrap();
    assert_eq!(server.tools().len(), 1);
    assert_eq!(server.tools()[0].exposed_name, "mcp__remote__echo");
    assert_eq!(server.metadata()["serverInfo"]["name"], "remote-fixture");
    let result = server
        .call_exposed_tool(
            "mcp__remote__echo",
            json!({"text":"hello remote"}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "hello remote");

    let initialize = &http.posts("initialize")[0];
    assert_eq!(initialize.header("mcp-session-id"), None);
    assert_eq!(initialize.header("mcp-protocol-version"), None);
    assert_eq!(initialize.header("content-type"), Some("application/json"));
    assert_eq!(
        initialize.header("accept"),
        Some("application/json, text/event-stream")
    );
    assert_eq!(initialize.header("x-api-key"), Some("PRIVATE-HEADER"));
    let initialized = &http.posts("notifications/initialized")[0];
    assert_eq!(initialized.status, 202);
    for later in [
        initialized,
        &http.posts("tools/list")[0],
        &http.posts("tools/call")[0],
    ] {
        assert_eq!(later.header("mcp-session-id"), Some(SESSION));
        assert_eq!(later.header("mcp-protocol-version"), Some("2025-11-25"));
        assert_eq!(later.header("x-api-key"), Some("PRIVATE-HEADER"));
    }
    assert!(server.shutdown().await.0);
}

#[tokio::test]
async fn event_stream_progress_resets_the_call_timer_and_delivers_the_result() {
    let http = McpHttp::start(Behavior {
        sse: true,
        delay: Duration::from_millis(700),
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let mut server = fixture.connect().await.unwrap();
    let started = std::time::Instant::now();
    let result = server
        .call_tool("echo", json!({"text":"streamed"}), Duration::from_secs(1))
        .await
        .unwrap();
    assert!(started.elapsed() > Duration::from_secs(1));
    assert_eq!(result["content"][0]["text"], "streamed");
    assert!(!server.unresolved());
    assert!(server.shutdown().await.0);
}

#[tokio::test]
async fn oversized_body_fails_closed() {
    let http = McpHttp::start(Behavior {
        oversized: true,
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let error = fixture
        .connect()
        .await
        .err()
        .expect("oversized body accepted");
    assert!(error.acknowledged);
    assert!(error.diagnostic.contains("1 MiB"), "{}", error.diagnostic);
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let target = McpHttp::start(Behavior::default()).await;
    let http = McpHttp::start(Behavior {
        redirect: Some(target.url.clone()),
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let error = fixture.connect().await.err().expect("redirect followed");
    assert!(error.diagnostic.contains("302"), "{}", error.diagnostic);
    assert_eq!(http.requests().len(), 1);
    assert!(target.requests().is_empty());
}

#[tokio::test]
async fn sandboxed_network_off_and_unlisted_proxy_host_refuse_before_contact() {
    let http = McpHttp::start(Behavior::default()).await;
    for sandbox in [
        json!({"policy":"workspace-write","network":"off","apply_to":["mcp"]}),
        json!({"policy":"workspace-write","network":"proxy","allowed_domains":["example.test"],"apply_to":["mcp"]}),
    ] {
        let fixture = Fixture::remote(&http.url, json!({"sandbox":sandbox}));
        let error = fixture.connect().await.err().expect("policy bypassed");
        assert!(error.acknowledged);
        assert!(error.diagnostic.contains("network"), "{}", error.diagnostic);
    }
    assert!(http.requests().is_empty());
    // The same policy without the MCP scope leaves the remote network on.
    let fixture = Fixture::remote(
        &http.url,
        json!({"sandbox":{"policy":"workspace-write","network":"off"}}),
    );
    let mut server = fixture.connect().await.unwrap();
    assert!(server.shutdown().await.0);
}

#[tokio::test]
async fn shutdown_deletes_the_session_and_tolerates_405_or_an_unreachable_server() {
    let http = McpHttp::start(Behavior::default()).await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let mut server = fixture.connect().await.unwrap();
    assert!(server.shutdown().await.0);
    let deletes = http.deletes();
    assert_eq!(deletes.len(), 1);
    assert_eq!(deletes[0].header("mcp-session-id"), Some(SESSION));
    assert_eq!(
        deletes[0].header("mcp-protocol-version"),
        Some("2025-11-25")
    );
    assert!(server.shutdown().await.0);
    assert_eq!(http.deletes().len(), 1);

    let refused = McpHttp::start(Behavior {
        delete_status: Some(405),
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(&refused.url, json!({}));
    let mut server = fixture.connect().await.unwrap();
    assert!(server.shutdown().await.0);
    assert_eq!(refused.deletes()[0].status, 405);

    let mut gone = McpHttp::start(Behavior::default()).await;
    let fixture = Fixture::remote(&gone.url, json!({}));
    let mut server = fixture.connect().await.unwrap();
    gone.stop().await;
    assert!(server.shutdown().await.0);
    server.settle_after_shutdown(|| Ok(())).unwrap();
}

#[tokio::test]
async fn owned_remote_failures_redact_url_queries_and_header_values() {
    let url = mcp_http::unreachable("/mcp?token=PRIVATE-QUERY");
    let fixture = Fixture::new(
        json!({"mcp":{"remote":{"type":"remote","url":url,"headers":{"authorization":"Bearer PRIVATE-HEADER"}}}}),
    );
    let loaded = fixture.load();
    let trust = TrustStore::new(fixture.paths.trust_file());
    let store = store();
    let error = fixture
        .launcher(&loaded, &trust)
        .connect_owned("remote", store.clone(), |_| async {
            Ok(McpLocationPin::unmanaged())
        })
        .await
        .err()
        .expect("unreachable server connected");
    let records = mcp_connections(&store, &fixture.repo).unwrap();
    assert_eq!(records[0].phase, McpConnectionPhase::Settled);
    let recorded = records[0].error.clone().unwrap_or_default();
    for text in [
        error.to_string(),
        format!("{error:?}"),
        error.diagnostic.clone(),
        recorded.clone(),
    ] {
        assert!(!text.contains("PRIVATE"), "{text}");
    }
    assert!(recorded.contains("remote"), "{recorded}");
}

#[tokio::test]
async fn owned_remote_server_reuses_durable_phases_and_settles_on_shutdown() {
    let http = McpHttp::start(Behavior::default()).await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let loaded = fixture.load();
    let trust = TrustStore::new(fixture.paths.trust_file());
    let store = store();
    let mut server = fixture
        .launcher(&loaded, &trust)
        .connect_owned("remote", store.clone(), |_| async {
            Ok(McpLocationPin::unmanaged())
        })
        .await
        .unwrap();
    assert_eq!(server.record().phase, McpConnectionPhase::Running);
    let (record, _) = server.shutdown().await.unwrap();
    assert_eq!(record.phase, McpConnectionPhase::Settled);
    assert_eq!(record.acknowledged, Some(true));
    assert_eq!(http.deletes().len(), 1);
}

#[tokio::test]
async fn legacy_sse_fallback_after_405_discovers_and_calls() {
    let http = McpHttp::start(Behavior {
        status: Some(405),
        legacy: true,
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let mut server = fixture.connect().await.unwrap();
    assert_eq!(server.remote_transport(), Some(RemoteTransport::Sse));
    assert_eq!(server.tools()[0].exposed_name, "mcp__remote__echo");
    let result = server
        .call_exposed_tool(
            "mcp__remote__echo",
            json!({"text":"over legacy sse"}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "over legacy sse");
    let requests = http.requests();
    // Exactly one Streamable HTTP attempt, then the legacy stream and its message endpoint.
    let streamable: Vec<_> = requests
        .iter()
        .filter(|seen| seen.method == "POST" && seen.path == "/mcp")
        .collect();
    assert_eq!(streamable.len(), 1);
    assert_eq!(streamable[0].status, 405);
    let gets = http.gets();
    assert_eq!(gets.len(), 1);
    assert_eq!(gets[0].header("accept"), Some("text/event-stream"));
    assert_eq!(gets[0].header("x-api-key"), Some("PRIVATE-HEADER"));
    let messages: Vec<_> = requests
        .iter()
        .filter(|seen| seen.path == "/messages")
        .collect();
    assert_eq!(
        messages.iter().map(|seen| seen.rpc()).collect::<Vec<_>>(),
        vec![
            Some("initialize"),
            Some("notifications/initialized"),
            Some("tools/list"),
            Some("tools/call")
        ]
    );
    for message in messages {
        assert_eq!(message.status, 202);
        assert_eq!(message.header("x-api-key"), Some("PRIVATE-HEADER"));
        assert_eq!(message.header("content-type"), Some("application/json"));
    }
    assert!(server.shutdown().await.0);
    assert!(http.deletes().is_empty());
    server.settle_after_shutdown(|| Ok(())).unwrap();
}

#[tokio::test]
async fn unauthorized_initialize_needs_auth_without_fallback() {
    for code in [401, 403] {
        let http = McpHttp::start(Behavior {
            status: Some(code),
            legacy: true,
            ..Behavior::default()
        })
        .await;
        let fixture = Fixture::remote(&http.url, json!({}));
        let error = fixture
            .connect()
            .await
            .err()
            .expect("unauthorized connected");
        assert_eq!(
            error.kind(),
            LaunchFailure::NeedsAuth,
            "{}",
            error.diagnostic
        );
        assert!(error.acknowledged);
        assert!(
            !error.diagnostic.contains("PRIVATE"),
            "{}",
            error.diagnostic
        );
        assert_eq!(http.requests().len(), 1, "HTTP {code}");
        assert!(http.gets().is_empty(), "HTTP {code} fell back to SSE");
    }
    // Other failures keep the ordinary classification.
    let http = McpHttp::start(Behavior {
        status: Some(500),
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let error = fixture.connect().await.err().expect("500 connected");
    assert_eq!(error.kind(), LaunchFailure::Failed);
    assert!(http.gets().is_empty(), "HTTP 500 must not fall back");
}

#[tokio::test]
async fn cross_origin_sse_endpoint_is_refused_before_contact() {
    let other = McpHttp::start(Behavior::default()).await;
    let http = McpHttp::start(Behavior {
        status: Some(405),
        legacy: true,
        endpoint: Some(other.url.replace("/mcp", "/messages?sessionId=stolen")),
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let error = fixture
        .connect()
        .await
        .err()
        .expect("cross-origin endpoint used");
    assert!(error.acknowledged);
    assert!(error.diagnostic.contains("origin"), "{}", error.diagnostic);
    assert_eq!(http.gets().len(), 1);
    assert!(
        http.requests().iter().all(|seen| seen.path == "/mcp"),
        "a message was posted after a refused endpoint"
    );
    assert!(other.requests().is_empty());
}

#[tokio::test]
async fn fallback_keeps_the_original_absolute_deadline() {
    let http = McpHttp::start(Behavior {
        status: Some(405),
        legacy: true,
        slow: Duration::from_millis(1500),
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(
        &http.url,
        json!({"mcp":{"remote":{"type":"remote","url":http.url,"timeout":2}}}),
    );
    let started = std::time::Instant::now();
    let error = fixture
        .connect()
        .await
        .err()
        .expect("slow fallback connected");
    let elapsed = started.elapsed();
    assert!(error.acknowledged);
    assert_eq!(error.kind(), LaunchFailure::Failed);
    assert_eq!(http.gets().len(), 1, "the fallback never started");
    assert!(
        elapsed >= Duration::from_millis(1900) && elapsed < Duration::from_millis(2900),
        "{elapsed:?}"
    );
}

async fn eventually(what: &str, ready: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what} never happened"));
}

/// Drive the idle pump like the pool monitor until `done` holds; return each update.
async fn pump_until(
    server: &mut LocalServer,
    done: impl Fn(&IdleUpdate) -> bool,
) -> Vec<IdleUpdate> {
    let mut updates = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let update = server.poll_idle(Duration::from_millis(100)).await.unwrap();
            let finished = done(&update);
            updates.push(update);
            if finished {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the idle pump never reached the expected state");
    updates
}

async fn streaming_server(behavior: Behavior) -> (McpHttp, Fixture, LocalServer) {
    let http = McpHttp::start(Behavior {
        stream: true,
        ..behavior
    })
    .await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let server = fixture.connect().await.unwrap();
    eventually("the GET event stream", || http.stream_open()).await;
    (http, fixture, server)
}

#[tokio::test]
async fn list_changed_on_the_get_stream_refreshes_and_retires_removed_tools() {
    let (http, _fixture, mut server) = streaming_server(Behavior::default()).await;
    let get = &http.gets()[0];
    assert_eq!(get.header("accept"), Some("text/event-stream"));
    assert_eq!(get.header("mcp-session-id"), Some(SESSION));
    assert_eq!(get.header("mcp-protocol-version"), Some("2025-11-25"));
    assert_eq!(get.header("x-api-key"), Some("PRIVATE-HEADER"));
    http.set_tools(&["fresh"]);
    assert!(http.push(json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})));
    let updates = pump_until(&mut server, |update| update.tools_changed).await;
    assert!(updates.iter().all(|update| !update.closed));
    server.refresh_tools(Duration::from_secs(5)).await.unwrap();
    let names: Vec<_> = server
        .tools()
        .iter()
        .map(|tool| tool.exposed_name.as_str())
        .collect();
    assert_eq!(names, vec!["mcp__remote__fresh"]);
    let stale = server
        .call_exposed_tool("mcp__remote__echo", json!({}), Duration::from_secs(5))
        .await
        .unwrap_err();
    assert!(matches!(stale, McpError::StaleTool(_)), "{stale:?}");
    let result = server
        .call_exposed_tool(
            "mcp__remote__fresh",
            json!({"text":"after refresh"}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "after refresh");
    // An open GET stream is the liveness signal; the client sends no pings.
    assert!(http.posts("ping").is_empty());
    assert!(server.shutdown().await.0);
}

#[tokio::test]
async fn server_ping_on_the_get_stream_is_answered_by_an_accepted_post() {
    let (http, _fixture, mut server) = streaming_server(Behavior::default()).await;
    assert!(http.push(json!({"jsonrpc":"2.0","id":"server-ping","method":"ping"})));
    pump_until(&mut server, |_| !http.replies("server-ping").is_empty()).await;
    let reply = &http.replies("server-ping")[0];
    assert_eq!(
        reply.body,
        json!({"jsonrpc":"2.0","id":"server-ping","result":{}})
    );
    assert_eq!(reply.status, 202);
    assert_eq!(reply.header("mcp-session-id"), Some(SESSION));
    assert_eq!(reply.header("mcp-protocol-version"), Some("2025-11-25"));
    assert!(server.shutdown().await.0);
}

#[tokio::test]
async fn expired_session_404_is_transport_loss_not_a_tool_error() {
    let http = McpHttp::start(Behavior::default()).await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let mut server = fixture.connect().await.unwrap();
    http.expire_session();
    let error = server
        .call_exposed_tool("mcp__remote__echo", json!({}), Duration::from_secs(5))
        .await
        .unwrap_err();
    assert!(
        matches!(error, McpError::Http(HttpFailure::SessionExpired)),
        "{error:?}"
    );
    assert_eq!(http.posts("tools/call")[0].status, 404);
    assert!(
        server
            .poll_idle(Duration::from_millis(100))
            .await
            .map_or(true, |update| update.closed)
    );
    assert!(server.shutdown().await.0);
}

#[tokio::test]
async fn unanswered_client_ping_without_a_get_stream_is_loss_within_the_cadence() {
    let http = McpHttp::start(Behavior {
        ping_hang: true,
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let mut server = fixture.connect().await.unwrap();
    assert_eq!(http.gets()[0].status, 405);
    let started = std::time::Instant::now();
    // The pool monitor polls once per second.
    let lost = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            match server.poll_idle(Duration::from_millis(100)).await {
                Ok(update) if !update.closed => {}
                _ => break,
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await;
    assert!(lost.is_ok(), "an unresponsive server stayed healthy");
    // One ten-second cadence plus a five-second ping timeout bounds detection.
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(10) && elapsed < Duration::from_secs(18),
        "{elapsed:?}"
    );
    let pings = http.posts("ping");
    assert_eq!(pings.len(), 1);
    assert_eq!(pings[0].header("mcp-session-id"), Some(SESSION));
    assert!(server.shutdown().await.0);
}

#[tokio::test]
async fn get_stream_floods_and_server_requests_are_refused_in_bounded_batches() {
    let (http, _fixture, mut server) = streaming_server(Behavior::default()).await;
    for index in 0..500 {
        assert!(http.push(
            json!({"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":index}})
        ));
    }
    for (id, method, params) in [
        (
            "elicit",
            "elicitation/create",
            json!({"mode":"form","message":"name?","requestedSchema":{"type":"object","properties":{"name":{"type":"string"}}}}),
        ),
        (
            "sample",
            "sampling/createMessage",
            json!({"messages":[],"maxTokens":1}),
        ),
        ("roots", "roots/list", json!({})),
    ] {
        assert!(http.push(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})));
    }
    let updates = pump_until(&mut server, |_| {
        ["elicit", "sample", "roots"]
            .iter()
            .all(|id| !http.replies(id).is_empty())
    })
    .await;
    assert!(updates.iter().all(|update| !update.closed));
    // Each idle tick handles at most 32 messages, so 503 need at least 16 ticks.
    assert!(updates.len() >= 16, "{}", updates.len());
    let elicit = &http.replies("elicit")[0].body;
    assert!(
        elicit["result"]["action"] == "decline" || elicit["error"].is_object(),
        "{elicit}"
    );
    assert!(http.replies("sample")[0].body["error"].is_object());
    // Remote servers never receive local roots.
    assert_eq!(http.replies("roots")[0].body["error"]["code"], -32601);
    for id in ["elicit", "sample", "roots"] {
        assert_eq!(http.replies(id)[0].status, 202);
    }
    assert!(
        !server
            .poll_idle(Duration::from_millis(100))
            .await
            .unwrap()
            .closed
    );
    assert!(server.shutdown().await.0);
}

#[tokio::test]
async fn polled_call_stream_resumes_with_last_event_id_on_the_same_session() {
    let http = McpHttp::start(Behavior {
        poll: true,
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let mut server = fixture.connect().await.unwrap();
    let started = std::time::Instant::now();
    let result = server
        .call_exposed_tool(
            "mcp__remote__echo",
            json!({"text":"resumed"}),
            Duration::from_secs(20),
        )
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "resumed");
    assert!(started.elapsed() < Duration::from_secs(5));
    let resumed: Vec<_> = http
        .gets()
        .into_iter()
        .filter(|get| get.header("last-event-id").is_some())
        .collect();
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0].header("last-event-id"), Some("call-1"));
    assert_eq!(resumed[0].header("accept"), Some("text/event-stream"));
    assert_eq!(resumed[0].header("mcp-session-id"), Some(SESSION));
    assert_eq!(resumed[0].header("x-api-key"), Some("PRIVATE-HEADER"));
    assert_eq!(http.posts("initialize").len(), 1);
    assert!(
        !server
            .poll_idle(Duration::from_millis(100))
            .await
            .unwrap()
            .closed
    );
    assert!(server.shutdown().await.0);
}

#[tokio::test]
async fn call_stream_closed_before_its_response_fails_promptly() {
    let http = McpHttp::start(Behavior {
        truncate: true,
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let mut server = fixture.connect().await.unwrap();
    let started = std::time::Instant::now();
    let error = server
        .call_exposed_tool("mcp__remote__echo", json!({}), Duration::from_secs(30))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, McpError::Http(HttpFailure::Request(message)) if message.contains("before response")),
        "{error:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    // Without an event id there is nothing to resume.
    assert!(
        http.gets()
            .iter()
            .all(|get| get.header("last-event-id").is_none())
    );
    assert!(server.shutdown().await.0);
}

#[tokio::test]
async fn cleanly_closed_get_stream_reopens_with_last_event_id_without_loss() {
    let (http, _fixture, mut server) = streaming_server(Behavior::default()).await;
    assert!(http.push(json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})));
    pump_until(&mut server, |update| update.tools_changed).await;
    server.refresh_tools(Duration::from_secs(5)).await.unwrap();
    http.drop_stream();
    eventually("the reopened GET event stream", || {
        http.gets().len() == 2 && http.stream_open()
    })
    .await;
    let reopened = &http.gets()[1];
    assert_eq!(reopened.status, 200);
    assert_eq!(reopened.header("last-event-id"), Some("ev-1"));
    assert_eq!(reopened.header("mcp-session-id"), Some(SESSION));
    assert!(
        !server
            .poll_idle(Duration::from_millis(100))
            .await
            .unwrap()
            .closed
    );
    let result = server
        .call_exposed_tool(
            "mcp__remote__echo",
            json!({"text":"same session"}),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "same session");
    assert_eq!(http.posts("initialize").len(), 1);
    assert_eq!(
        http.posts("tools/call")[0].header("mcp-session-id"),
        Some(SESSION)
    );
    assert!(http.posts("ping").is_empty());
    assert!(server.shutdown().await.0);
}

#[tokio::test]
async fn get_stream_client_errors_mean_no_stream_rather_than_loss() {
    for (code, sessionless) in [(404, true), (400, false)] {
        let http = McpHttp::start(Behavior {
            get_status: Some(code),
            sessionless,
            ..Behavior::default()
        })
        .await;
        let fixture = Fixture::remote(&http.url, json!({}));
        let mut server = fixture.connect().await.unwrap();
        eventually("the GET answer", || !http.gets().is_empty()).await;
        for _ in 0..3 {
            let update = server.poll_idle(Duration::from_millis(100)).await.unwrap();
            assert!(!update.closed, "HTTP {code} on GET was loss");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let result = server
            .call_exposed_tool(
                "mcp__remote__echo",
                json!({"text":"post only"}),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], "post only");
        let gets = http.gets();
        assert_eq!(gets.len(), 1, "HTTP {code} on GET was retried");
        assert_eq!(gets[0].status, code);
        assert_eq!(gets[0].header("mcp-session-id").is_none(), sessionless);
        assert!(server.shutdown().await.0);
    }
}

#[tokio::test]
async fn get_stream_refusal_after_initialize_settles_needs_auth() {
    let http = McpHttp::start(Behavior {
        get_status: Some(403),
        ..Behavior::default()
    })
    .await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let loaded = fixture.load();
    let trust = TrustStore::new(fixture.paths.trust_file());
    let store = store();
    let connected = fixture
        .launcher(&loaded, &trust)
        .connect_owned("remote", store.clone(), |_| async {
            Ok(McpLocationPin::unmanaged())
        })
        .await;
    // The refusal may race initial discovery; either way it settles `needs_auth`.
    match connected {
        Err(error) => {
            assert_eq!(
                error.kind(),
                LaunchFailure::NeedsAuth,
                "{}",
                error.diagnostic
            );
            assert!(error.acknowledged);
        }
        Ok(mut server) => {
            tokio::time::timeout(Duration::from_secs(5), async {
                while server
                    .poll_idle(Duration::from_millis(100))
                    .await
                    .is_ok_and(|update| !update.closed)
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("a refused GET stream stayed healthy");
            let (record, _) = server.shutdown().await.unwrap();
            assert_eq!(record.status, McpConnectionStatus::NeedsAuth);
        }
    }
    let records = mcp_connections(&store, &fixture.repo).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].phase, McpConnectionPhase::Settled);
    assert_eq!(records[0].status, McpConnectionStatus::NeedsAuth);
    assert_eq!(records[0].acknowledged, Some(true));
    assert!(!records[0].error.clone().unwrap().contains("PRIVATE"));
    assert_eq!(http.gets().len(), 1);
}

#[tokio::test]
async fn legacy_stream_or_endpoint_refusal_needs_auth_without_further_requests() {
    let cases = [
        (Some(401), None, 2),
        (Some(403), None, 2),
        (None, Some(401), 3),
        (None, Some(403), 3),
    ];
    for (get_status, message_status, requests) in cases {
        let http = McpHttp::start(Behavior {
            status: Some(405),
            legacy: true,
            get_status,
            message_status,
            ..Behavior::default()
        })
        .await;
        let fixture = Fixture::remote(&http.url, json!({}));
        let loaded = fixture.load();
        let trust = TrustStore::new(fixture.paths.trust_file());
        let store = store();
        let error = fixture
            .launcher(&loaded, &trust)
            .connect_owned("remote", store.clone(), |_| async {
                Ok(McpLocationPin::unmanaged())
            })
            .await
            .err()
            .expect("refused legacy server connected");
        let case = format!("GET {get_status:?}, endpoint {message_status:?}");
        assert_eq!(
            error.kind(),
            LaunchFailure::NeedsAuth,
            "{case}: {}",
            error.diagnostic
        );
        assert!(error.acknowledged, "{case}");
        assert!(error.retry_safe(), "{case}");
        assert!(!error.diagnostic.contains("PRIVATE"), "{case}");
        let seen = http.requests();
        assert_eq!(seen.len(), requests, "{case}: {seen:?}");
        assert_eq!(seen[0].status, 405, "{case}");
        let refused = seen.last().unwrap();
        assert_eq!(
            Some(refused.status),
            get_status.or(message_status),
            "{case}"
        );
        if message_status.is_some() {
            assert_eq!(refused.path, "/messages", "{case}");
            assert_eq!(refused.rpc(), Some("initialize"), "{case}");
        }
        let records = mcp_connections(&store, &fixture.repo).unwrap();
        assert_eq!(records[0].phase, McpConnectionPhase::Settled, "{case}");
        assert_eq!(records[0].status, McpConnectionStatus::NeedsAuth, "{case}");
        assert_eq!(records[0].acknowledged, Some(true), "{case}");
    }
}

const AMBIENT: &str = "CYBER_TEST_AMBIENT_PROXY_CHILD";

/// Runs `ambient_proxy_child` in a fresh copy of this test binary whose environment routes
/// every scheme through a recording listener; the remote connection must never reach it.
#[test]
fn ambient_proxy_environment_is_ignored() {
    let ambient = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    ambient.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", ambient.local_addr().unwrap());
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["ambient_proxy_child", "--exact", "--ignored", "--nocapture"])
        .env(AMBIENT, "1")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy");
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env(name, &proxy);
    }
    let output = command.output().unwrap();
    let detail = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{detail}");
    assert!(detail.contains("1 passed"), "{detail}");
    assert!(
        ambient.accept().is_err(),
        "the ambient proxy received a connection"
    );
}

#[tokio::test]
#[ignore = "run by ambient_proxy_environment_is_ignored"]
async fn ambient_proxy_child() {
    assert_eq!(std::env::var(AMBIENT).as_deref(), Ok("1"));
    let http = McpHttp::start(Behavior::default()).await;
    let fixture = Fixture::remote(&http.url, json!({}));
    let mut server = fixture.connect().await.unwrap();
    assert_eq!(server.tools()[0].exposed_name, "mcp__remote__echo");
    assert!(server.shutdown().await.0);
    assert!(!http.posts("tools/list").is_empty());
}

/// A file a `headers_command` prints, so tests can rotate the credential it supplies.
struct Tokens {
    _dir: tempfile::TempDir,
    file: PathBuf,
}

impl Tokens {
    fn new(content: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().canonicalize().unwrap().join("headers.json");
        std::fs::write(&file, content).unwrap();
        Self { _dir: dir, file }
    }

    fn write(&self, content: &str) {
        std::fs::write(&self.file, content).unwrap();
    }

    /// PowerShell on Windows, `/bin/sh` elsewhere; `code` is the exit status afterwards.
    fn command(&self, code: Option<i32>) -> String {
        let path = self.file.display().to_string();
        #[cfg(windows)]
        let print = format!(
            "Get-Content -Raw -LiteralPath '{}'",
            path.replace('\'', "''")
        );
        #[cfg(not(windows))]
        let print = format!("cat '{}'", path.replace('\'', "'\\''"));
        match code {
            Some(code) => format!("{print}; exit {code}"),
            None => print,
        }
    }
}

#[tokio::test]
async fn headers_command_output_merges_over_static_headers_on_every_request() {
    let http = McpHttp::start(Behavior::default()).await;
    let tokens = Tokens::new(r#"{"Authorization":"Bearer T1","X-Extra":"extra"}"#);
    let fixture = Fixture::commanded(&http.url, &tokens.command(None), None);
    let mut server = fixture.connect().await.unwrap();
    server
        .call_tool("echo", json!({"text":"hi"}), Duration::from_secs(5))
        .await
        .unwrap();
    let posts: Vec<_> = http
        .requests()
        .into_iter()
        .filter(|seen| seen.method == "POST")
        .collect();
    assert!(posts.len() >= 4, "{posts:?}");
    for seen in posts {
        assert_eq!(seen.header("authorization"), Some("Bearer T1"));
        assert_eq!(seen.header("x-extra"), Some("extra"));
        assert_eq!(seen.header("x-api-key"), Some("PRIVATE-HEADER"));
    }
    assert!(server.shutdown().await.0);
    assert_eq!(http.deletes()[0].header("authorization"), Some("Bearer T1"));
}

#[tokio::test]
async fn invalid_headers_command_output_fails_closed_before_contact_without_values() {
    let http = McpHttp::start(Behavior::default()).await;
    let oversized = format!(r#"{{"x-token":"PRIVATE-{}"}}"#, "x".repeat(70 * 1024));
    let cases = [
        ("PRIVATE-NOT-JSON", None),
        (r#"{"x-token":"PRIVATE-VALUE","x-count":1}"#, None),
        (r#"["PRIVATE-ARRAY"]"#, None),
        (oversized.as_str(), None),
        (r#"{"Mcp-Session-Id":"PRIVATE-SESSION"}"#, None),
        (r#"{"Content-Type":"PRIVATE-TYPE"}"#, None),
        (r#"{"bad header":"PRIVATE-NAME"}"#, None),
        (r#"{"x-token":"PRIVATE-LINE\r\nx-injected: y"}"#, None),
        (r#"{"Authorization":"Bearer PRIVATE-EXIT"}"#, Some(3)),
    ];
    for (output, code) in cases {
        let tokens = Tokens::new(output);
        let fixture = Fixture::commanded(&http.url, &tokens.command(code), None);
        let error = fixture
            .connect()
            .await
            .err()
            .unwrap_or_else(|| panic!("accepted {}", &output[..output.len().min(40)]));
        assert!(error.acknowledged);
        assert_eq!(error.kind(), LaunchFailure::Failed);
        for text in [error.to_string(), format!("{error:?}")] {
            assert!(!text.contains("PRIVATE"), "{text}");
            assert!(text.contains("headers_command"), "{text}");
        }
        if code.is_some() {
            assert!(
                error.diagnostic.contains("status 3"),
                "{}",
                error.diagnostic
            );
        }
    }
    assert!(http.requests().is_empty(), "{:?}", http.requests());
}

#[tokio::test]
async fn owned_headers_command_failure_settles_a_redacted_record() {
    let http = McpHttp::start(Behavior::default()).await;
    let tokens = Tokens::new(r#"{"Authorization":"Bearer PRIVATE-EXIT"}"#);
    let fixture = Fixture::commanded(&http.url, &tokens.command(Some(7)), None);
    let loaded = fixture.load();
    let trust = TrustStore::new(fixture.paths.trust_file());
    let store = store();
    let error = fixture
        .launcher(&loaded, &trust)
        .connect_owned("remote", store.clone(), |_| async {
            Ok(McpLocationPin::unmanaged())
        })
        .await
        .err()
        .expect("failing headers_command connected");
    assert!(error.retry_safe());
    let records = mcp_connections(&store, &fixture.repo).unwrap();
    assert_eq!(records[0].phase, McpConnectionPhase::Settled);
    assert_eq!(records[0].acknowledged, Some(true));
    let recorded = serde_json::to_string(&records[0]).unwrap();
    assert!(!recorded.contains("PRIVATE"), "{recorded}");
    assert!(http.requests().is_empty());
}

#[tokio::test]
async fn headers_refresh_swaps_credentials_on_the_live_session_and_failure_is_loss() {
    let http = McpHttp::start(Behavior::default()).await;
    let tokens = Tokens::new(r#"{"Authorization":"Bearer T1"}"#);
    let fixture = Fixture::commanded(&http.url, &tokens.command(None), Some(1));
    let mut server = fixture.connect().await.unwrap();
    tokens.write(r#"{"Authorization":"Bearer T2"}"#);
    // Before the interval elapses the old credential stays in use.
    assert!(
        !server
            .poll_idle(Duration::from_millis(50))
            .await
            .unwrap()
            .closed
    );
    server
        .call_tool("echo", json!({"text":"early"}), Duration::from_secs(5))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        !server
            .poll_idle(Duration::from_millis(50))
            .await
            .unwrap()
            .closed
    );
    server
        .call_tool("echo", json!({"text":"late"}), Duration::from_secs(5))
        .await
        .unwrap();
    let calls = http.posts("tools/call");
    assert_eq!(calls[0].header("authorization"), Some("Bearer T1"));
    assert_eq!(calls[1].header("authorization"), Some("Bearer T2"));
    assert_eq!(calls[1].header("mcp-session-id"), Some(SESSION));
    assert_eq!(
        http.posts("initialize").len(),
        1,
        "refresh must not reconnect"
    );

    // A failed refresh never keeps the stale credential: it is reported as loss.
    tokens.write("PRIVATE-BROKEN");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let error = server
        .poll_idle(Duration::from_millis(50))
        .await
        .expect_err("a failed refresh stayed healthy");
    assert!(!format!("{error:?}").contains("PRIVATE"), "{error:?}");
    assert!(error.to_string().contains("headers_command"), "{error}");
    assert!(server.shutdown().await.0);
}

/// A `headers_command` that creates `marker` before printing `tokens`.
fn marking_command(marker: &std::path::Path, tokens: &Tokens) -> String {
    let path = marker.display().to_string();
    #[cfg(windows)]
    let touch = format!(
        "New-Item -ItemType File -Force -Path '{}' | Out-Null",
        path.replace('\'', "''")
    );
    #[cfg(not(windows))]
    let touch = format!("touch '{}'", path.replace('\'', "'\\''"));
    format!("{touch}; {}", tokens.command(None))
}

#[tokio::test]
async fn sandboxed_remote_server_refuses_headers_command_before_it_runs() {
    let http = McpHttp::start(Behavior::default()).await;
    let tokens = Tokens::new(r#"{"Authorization":"Bearer PRIVATE-CMD"}"#);
    let marker = tokens.file.with_file_name("command-ran");
    let fixture = Fixture::new(json!({
        "sandbox":{"policy":"workspace-write","network":"on","apply_to":["mcp"]},
        "mcp":{"remote":{"type":"remote","url":http.url,"headers_command":marking_command(&marker, &tokens)}}
    }));
    let error = fixture.connect().await.err().expect("sandbox bypassed");
    assert!(error.acknowledged);
    assert_eq!(error.kind(), LaunchFailure::Failed);
    assert!(
        error.diagnostic.contains("headers_command") && error.diagnostic.contains("sandbox"),
        "{}",
        error.diagnostic
    );
    assert!(!marker.exists(), "headers_command ran outside the sandbox");
    assert!(http.requests().is_empty());

    // A sandboxed MCP-tool hook demands the sandbox even without `apply_to`.
    let fixture = Fixture::commanded(&http.url, &marking_command(&marker, &tokens), None);
    let loaded = fixture.load();
    let trust = TrustStore::new(fixture.paths.trust_file());
    let cancel = tokio_util::sync::CancellationToken::new();
    let error = fixture
        .launcher(&loaded, &trust)
        .connect_for_hook(
            "remote",
            || Ok(()),
            tokio::time::Instant::now() + Duration::from_secs(5),
            true,
            &cancel,
        )
        .await
        .err()
        .expect("hook sandbox bypassed");
    assert!(
        error.diagnostic.contains("headers_command"),
        "{}",
        error.diagnostic
    );
    assert!(!marker.exists(), "headers_command ran outside the sandbox");
    assert!(http.requests().is_empty());
}

#[tokio::test]
async fn due_headers_refresh_runs_before_a_call_without_a_monitor_tick() {
    let http = McpHttp::start(Behavior::default()).await;
    http.require_authorization(Some("Bearer T1"));
    let tokens = Tokens::new(r#"{"Authorization":"Bearer T1"}"#);
    let fixture = Fixture::commanded(&http.url, &tokens.command(None), Some(1));
    let mut server = fixture.connect().await.unwrap();
    // The credential expires exactly at the refresh boundary.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    http.require_authorization(Some("Bearer T2"));
    tokens.write(r#"{"Authorization":"Bearer T2"}"#);
    server
        .call_tool("echo", json!({"text":"busy"}), Duration::from_secs(5))
        .await
        .expect("the call used headers past their refresh window");
    let calls = http.posts("tools/call");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].header("authorization"), Some("Bearer T2"));
    assert_eq!(http.posts("initialize").len(), 1);

    // A failed refresh stays due: no later request reuses the stale credential.
    tokens.write("PRIVATE-BROKEN");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    for _ in 0..2 {
        let error = server
            .call_tool("echo", json!({"text":"stale"}), Duration::from_secs(5))
            .await
            .expect_err("a failed refresh sent the stale credential");
        assert!(!format!("{error:?}").contains("PRIVATE"), "{error:?}");
    }
    assert_eq!(http.posts("tools/call").len(), 1);
    assert!(server.poll_idle(Duration::from_millis(50)).await.is_err());
    assert!(server.shutdown().await.0);
}
