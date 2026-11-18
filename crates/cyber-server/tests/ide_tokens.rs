//! Local IDE tokens (`server-api` → Local IDE token issuance, Local IDE token scope, Local
//! IDE token revocation and expiry) over a real TCP listener, the peer listener and the
//! router with a forged non-loopback peer address.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Extension;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use cyber_server::http::{
    self, AgentInfo, AppState, CommandInfo, HttpOptions, ModelInfo, PeerAddr, Services, Transport,
};
use cyber_server::runtime::{ToolDef, TurnContext};
use futures::StreamExt;
use futures::future::BoxFuture;
use serde_json::{Value, json};
use support::{Behavior, Harness, Setup, text, tools};
use tower::ServiceExt;

const PASSWORD: &str = "ide-token-owner-password";

struct FakeServices {
    tools: Arc<support::Tools>,
}

impl Services for FakeServices {
    fn models(&self, _location: &Path) -> BoxFuture<'_, Result<Vec<ModelInfo>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn default_model(&self, _location: &Path) -> Option<String> {
        Some("test/main".into())
    }
    fn agents(&self, _location: &Path) -> Vec<AgentInfo> {
        Vec::new()
    }
    fn tools(&self, turn: &TurnContext) -> Vec<ToolDef> {
        cyber_server::runtime::ToolHost::definitions(&*self.tools, turn)
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

fn state(h: &Harness) -> AppState {
    AppState {
        service: None,
        telemetry: None,
        ide: Default::default(),
        runtime: h.runtime.clone(),
        remote_tools: Arc::default(),
        store: Arc::clone(&h.store),
        services: Arc::new(FakeServices {
            tools: Arc::clone(&h.tools),
        }),
        options: Arc::new(HttpOptions {
            version: "0.1.0-test".into(),
            password: Some(PASSWORD.into()),
            cors_origins: Vec::new(),
            default_directory: h.repo.clone(),
            features: vec!["session".into(), "ide".into()],
        }),
    }
}

/// The main API on a loopback TCP listener; `http://127.0.0.1:<port>/api/v1`.
async fn serve(h: &Harness) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(http::serve_tcp(
        http::router(state(h)),
        listener,
        std::future::pending(),
    ));
    format!("http://{addr}/api/v1")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

async fn owner(method: reqwest::Method, url: String, body: Option<Value>) -> (u16, Value) {
    let mut request = client()
        .request(method, url)
        .basic_auth("cyber", Some(PASSWORD));
    if let Some(body) = body {
        request = request.json(&body);
    }
    send(request).await
}

async fn ide(
    token: &str,
    method: reqwest::Method,
    url: String,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = client().request(method, url).bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    send(request).await
}

/// Issue a `vscode` token as the owner: `(id, token)`.
async fn issue(base: &str) -> (String, String) {
    let (status, body) = owner(
        reqwest::Method::POST,
        format!("{base}/ide/tokens"),
        Some(json!({ "client": "vscode", "name": "test" })),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    let data = &body["data"];
    (
        data["id"].as_str().unwrap().into(),
        data["token"].as_str().unwrap().into(),
    )
}

fn out_of_scope(status: u16, body: &Value) -> bool {
    status == 403 && body["_tag"] == "ForbiddenError" && body["reason"] == "ide_token_scope"
}

/// "Issue a token" (server side), "Reissue rotates", "Listing hides secrets", invalid
/// expiries and clients, and idempotent DELETE.
#[tokio::test]
async fn tokens_are_issued_rotated_listed_and_revoked() {
    let h = Harness::new(Setup::default());
    let base = serve(&h).await;
    let (first_id, first) = issue(&base).await;
    assert!(first_id.starts_with("itk_"));
    assert!(first.starts_with("cyi_") && first.len() == 47, "{first}");
    let (status, _) = ide(
        &first,
        reqwest::Method::GET,
        format!("{base}/sessions"),
        None,
    )
    .await;
    assert_eq!(status, 200);

    let (second_id, second) = issue(&base).await;
    let (status, body) = ide(
        &first,
        reqwest::Method::GET,
        format!("{base}/sessions"),
        None,
    )
    .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (401, Some("UnauthorizedError"))
    );
    let (status, _) = ide(
        &second,
        reqwest::Method::GET,
        format!("{base}/sessions"),
        None,
    )
    .await;
    assert_eq!(status, 200);

    let (status, listed) = owner(reqwest::Method::GET, format!("{base}/ide/tokens"), None).await;
    assert_eq!(status, 200);
    let text = listed.to_string();
    assert!(!text.contains(&first) && !text.contains(&second), "{text}");
    assert!(!text.contains("sha256"), "{text}");
    let records = listed["data"].as_array().unwrap();
    assert_eq!(records.len(), 2);
    for field in [
        "id",
        "client",
        "name",
        "created_at",
        "expires_at",
        "last_used_at",
        "revoked_at",
    ] {
        assert!(records[0].get(field).is_some(), "{field}: {listed}");
    }
    assert_eq!(records[0]["id"], first_id.as_str());
    assert_eq!(records[0]["status"], "revoked");
    assert_eq!(records[1]["status"], "active");
    assert!(records[1]["last_used_at"].is_string());

    for body in [
        json!({ "client": "vscode", "expires_in_days": 0 }),
        json!({ "client": "vscode", "expires_in_days": 91 }),
        json!({ "client": "jetbrains" }),
        json!({ "client": "vscode", "token": "cyi_chosen" }),
    ] {
        let (status, refused) = owner(
            reqwest::Method::POST,
            format!("{base}/ide/tokens"),
            Some(body),
        )
        .await;
        assert!(status == 400 || status == 422, "{status} {refused}");
    }

    let revoke = format!("{base}/ide/tokens/{second_id}");
    let (status, revoked) = owner(reqwest::Method::DELETE, revoke.clone(), None).await;
    assert_eq!(status, 200, "{revoked}");
    assert_eq!(revoked["data"]["status"], "revoked");
    let (status, again) = owner(reqwest::Method::DELETE, revoke, None).await;
    assert_eq!(status, 200);
    assert_eq!(again["data"]["revoked_at"], revoked["data"]["revoked_at"]);
    let (status, _) = owner(
        reqwest::Method::DELETE,
        format!("{base}/ide/tokens/itk_missing"),
        None,
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = ide(
        &second,
        reqwest::Method::GET,
        format!("{base}/sessions"),
        None,
    )
    .await;
    assert_eq!(status, 401);
}

/// Table-driven: every operation of `sdk/openapi.json` served by the main API answers
/// `403 ide_token_scope` to an IDE token exactly when it is not in the allowlist.
#[tokio::test]
async fn exactly_the_allowlisted_operations_accept_a_token() {
    let h = Harness::new(Setup::default());
    let base = serve(&h).await;
    let (_, token) = issue(&base).await;
    let session = h.session().await;
    let document: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../sdk/openapi.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let allowed: Vec<&str> = http::ide::SCOPE.iter().map(|(_, _, id)| *id).collect();
    let mut checked = 0;
    for (path, operations) in document["paths"].as_object().unwrap() {
        for (method, operation) in operations.as_object().unwrap() {
            let id = operation["operationId"].as_str().unwrap();
            // Other listeners (peer, collaboration hub) and the public health route.
            if operation.get("x-cyber-listener").is_some() || id == "v1.health.get" {
                continue;
            }
            let concrete = concrete(path, &session);
            let method = reqwest::Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
            let response = client()
                .request(method.clone(), format!("http://{}{concrete}", host(&base)))
                .bearer_auth(&token)
                .json(&json!({}))
                .send()
                .await
                .unwrap();
            let status = response.status().as_u16();
            let streaming = response
                .headers()
                .get("content-type")
                .is_some_and(|v| v.as_bytes().starts_with(b"text/event-stream"));
            let body = match streaming {
                true => Value::Null,
                false => response.json().await.unwrap_or(Value::Null),
            };
            if allowed.contains(&id) {
                assert!(
                    !out_of_scope(status, &body) && status != 401,
                    "{id} ({method} {path}) refused a token: {status} {body}"
                );
            } else {
                assert!(
                    out_of_scope(status, &body),
                    "{id} ({method} {path}) accepted a token: {status} {body}"
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 200, "only {checked} operations");
    // The Session survived every refused DELETE and PATCH.
    assert!(h.runtime.state(&session).await.is_ok());
}

fn host(base: &str) -> &str {
    base.trim_start_matches("http://")
        .trim_end_matches("/api/v1")
}

fn concrete(path: &str, session: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment == "{sessionID}" {
                session.to_string()
            } else if segment.starts_with('{') {
                "x_1".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// "IDE token is not an owner credential", "Out-of-scope route", "A token cannot mint
/// tokens", "Bypass mode refused", and a Session create that would remove approval.
#[tokio::test]
async fn out_of_scope_requests_are_refused_before_their_handler() {
    let h = Harness::new(Setup::default());
    let base = serve(&h).await;
    let (_, token) = issue(&base).await;
    let session = h.session().await;
    let get = reqwest::Method::GET;
    let post = reqwest::Method::POST;

    let (status, body) = ide(&token, get.clone(), format!("{base}/config/language"), None).await;
    assert!(out_of_scope(status, &body), "{status} {body}");
    // Credential routes: provider sign-in and account removal.
    for (method, path) in [
        (post.clone(), "/providers/chatgpt/login"),
        (reqwest::Method::DELETE, "/account"),
    ] {
        let (status, body) = ide(&token, method, format!("{base}{path}"), Some(json!({}))).await;
        assert!(out_of_scope(status, &body), "{path}: {status} {body}");
    }
    let (status, body) = ide(
        &token,
        post.clone(),
        format!("{base}/ide/tokens"),
        Some(json!({ "client": "vscode" })),
    )
    .await;
    assert!(out_of_scope(status, &body), "{status} {body}");
    let (_, listed) = owner(get.clone(), format!("{base}/ide/tokens"), None).await;
    assert_eq!(
        listed["data"].as_array().unwrap().len(),
        1,
        "nothing issued"
    );

    for mode in ["bypass", "dont-ask"] {
        let (status, body) = ide(
            &token,
            post.clone(),
            format!("{base}/sessions/{session}/mode"),
            Some(json!({ "mode": mode })),
        )
        .await;
        assert!(out_of_scope(status, &body), "{mode}: {status} {body}");
        let (status, body) = ide(
            &token,
            post.clone(),
            format!("{base}/sessions"),
            Some(json!({ "mode": mode })),
        )
        .await;
        assert!(out_of_scope(status, &body), "{mode}: {status} {body}");
    }
    let state = h.runtime.state(&session).await.unwrap();
    assert_ne!(state.info.mode, "bypass");
    let (status, body) = ide(
        &token,
        post.clone(),
        format!("{base}/sessions"),
        Some(json!({ "rules": { "*": "allow" } })),
    )
    .await;
    assert!(out_of_scope(status, &body), "{status} {body}");

    let (status, body) = ide(
        &token,
        post.clone(),
        format!("{base}/sessions/{session}/mode"),
        Some(json!({ "mode": "plan" })),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = ide(&token, post, format!("{base}/sessions"), Some(json!({}))).await;
    assert_eq!(status, 201, "{body}");
}

/// "Token from the LAN refused", the query parameter, a malformed token, the peer
/// listener and the Unix transport.
#[tokio::test]
async fn tokens_are_accepted_only_from_the_socket_or_loopback() {
    let h = Harness::new(Setup::default());
    let base = serve(&h).await;
    let (_, token) = issue(&base).await;
    let session = h.session().await;

    // A request from a LAN address on the main listener (a TLS `0.0.0.0` bind).
    let router = http::router(state(&h));
    let from = |ip: &str, transport: Transport| {
        let mut request = Request::builder()
            .uri("http://cyber.example/api/v1/sessions")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(PeerAddr(format!("{ip}:50000").parse().unwrap()));
        request.extensions_mut().insert(transport);
        request
    };
    let tcp = router.clone().layer(Extension(Transport::Tcp));
    let lan = tcp
        .clone()
        .oneshot(from("192.168.1.20", Transport::Tcp))
        .await;
    assert_eq!(lan.unwrap().status(), StatusCode::UNAUTHORIZED);
    let mapped = tcp.oneshot(from("[::ffff:10.0.0.5]", Transport::Tcp)).await;
    assert_eq!(mapped.unwrap().status(), StatusCode::UNAUTHORIZED);
    let unix = router.layer(Extension(Transport::Unix));
    let socket = unix.oneshot(from("127.0.0.1", Transport::Unix)).await;
    assert_eq!(socket.unwrap().status(), StatusCode::OK);

    // `auth_token` never carries an IDE token, on the stream routes either.
    for path in [
        "/event",
        &format!("/sessions/{session}/events"),
        "/sessions",
    ] {
        let response = client()
            .get(format!("{base}{path}?auth_token={token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401, "{path}");
    }
    for malformed in ["cyi_", "cyi_short", &format!("{token}x")] {
        let (status, _) = ide(
            malformed,
            reqwest::Method::GET,
            format!("{base}/sessions"),
            None,
        )
        .await;
        assert_eq!(status, 401, "{malformed}");
    }
}

#[tokio::test]
async fn the_peer_listener_refuses_ide_tokens() {
    use cyber_server::http::peer::tls::{PeerIdentity, SubjectNames};
    let h = Harness::new(Setup::default());
    let base = serve(&h).await;
    let (_, token) = issue(&base).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let names = SubjectNames {
        machine: "desk".into(),
        address,
    };
    let identity = PeerIdentity::load_or_create(&h.dir.path().join("peer-tls"), &names).unwrap();
    let info = http::peer::PeerInfo {
        machine: "desk".into(),
        fingerprint: identity.fingerprint().to_string(),
        share_paths: false,
        attach_wait: http::peer::ATTACH_WAIT,
    };
    tokio::spawn(http::serve_peer(
        http::peer::router(state(&h), info),
        listener,
        Some(identity.server_config().unwrap()),
        std::future::pending(),
    ));
    let tls = reqwest::Client::builder()
        .no_proxy()
        .tls_danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    for path in ["/api/v1/peer/hello", "/api/v1/peer/attach/ses_x/messages"] {
        let response = tls
            .get(format!("https://{address}{path}"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 401, "{path}");
    }
}

/// "Revocation closes streams": the event stream ends within a second, the presence lease
/// the token created ends, and the next request gets `401`.
#[tokio::test]
async fn revocation_closes_streams_and_ends_leases() {
    let h = Harness::new(Setup::default());
    let base = serve(&h).await;
    let (id, token) = issue(&base).await;
    let session = h.session().await;
    let directory = std::fs::canonicalize(&h.repo).unwrap();
    let directory = directory.display().to_string();
    let (status, lease) = ide(
        &token,
        reqwest::Method::POST,
        format!("{base}/sessions/{session}/interactive-clients"),
        None,
    )
    .await;
    assert_eq!(status, 201, "{lease}");
    let client_id = lease["data"]["client_id"].as_str().unwrap().to_string();
    assert!(
        h.runtime
            .renew_interactive_client(&session, &directory, &client_id)
            .is_ok()
    );
    let stream = client()
        .get(format!("{base}/sessions/{session}/events"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200);
    let mut body = stream.bytes_stream();

    let started = Instant::now();
    let (status, _) = owner(
        reqwest::Method::DELETE,
        format!("{base}/ide/tokens/{id}"),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let ended = tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(chunk) = body.next().await {
            if chunk.is_err() {
                break;
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the stream outlived the revocation");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(
        h.runtime
            .renew_interactive_client(&session, &directory, &client_id)
            .is_err(),
        "the lease ended"
    );
    let (status, _) = ide(
        &token,
        reqwest::Method::GET,
        format!("{base}/sessions"),
        None,
    )
    .await;
    assert_eq!(status, 401);
}

/// "Expiry": an expired token gets `401` and is listed as expired.
#[tokio::test]
async fn expired_tokens_are_refused_and_listed_as_expired() {
    let h = Harness::new(Setup::default());
    let base = serve(&h).await;
    let (id, token) = issue(&base).await;
    let past = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        - 3_600_000;
    let row = id.clone();
    h.store
        .transaction(move |tx| {
            tx.execute(
                "UPDATE ide_token SET expires_ms = ?2 WHERE id = ?1",
                rusqlite::params![row, past],
            )?;
            Ok(())
        })
        .unwrap();
    let (status, body) = ide(
        &token,
        reqwest::Method::GET,
        format!("{base}/sessions"),
        None,
    )
    .await;
    assert_eq!(
        (status, body["_tag"].as_str()),
        (401, Some("UnauthorizedError"))
    );
    let (_, listed) = owner(reqwest::Method::GET, format!("{base}/ide/tokens"), None).await;
    assert_eq!(listed["data"][0]["id"], id.as_str());
    assert_eq!(listed["data"][0]["status"], "expired");
}

/// "Approval with an IDE token": the reply is applied as the owner's would be, and the
/// log names the token's ID, never its value.
#[tokio::test]
async fn approvals_with_a_token_are_applied_and_logged_by_id() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "shell", "{}")]), text("done")],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Ask("rm -rf build".into()));
    cyber_core::log::init(
        &h.dir.path().join("log"),
        cyber_core::log::Level::Info,
        false,
    );
    let records = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&records);
    cyber_core::log::set_sink(Some(Arc::new(move |record| {
        seen.lock().unwrap().push(format!(
            "{} {}",
            record.msg,
            Value::Object(record.fields.clone())
        ));
    })));
    let base = serve(&h).await;
    let (id, token) = issue(&base).await;
    let session = h.session().await;
    let (status, body) = ide(
        &token,
        reqwest::Method::POST,
        format!("{base}/sessions/{session}/prompt"),
        Some(json!({ "parts": [{ "type": "text", "text": "clean" }] })),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    let request = loop {
        let (_, list) = ide(
            &token,
            reqwest::Method::GET,
            format!("{base}/permissions/requests"),
            None,
        )
        .await;
        if let Some(r) = list["data"].as_array().and_then(|a| a.first()).cloned() {
            break r;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let rid = request["id"].as_str().unwrap();
    let (status, body) = ide(
        &token,
        reqwest::Method::POST,
        format!("{base}/sessions/{session}/permissions/{rid}/reply"),
        Some(json!({ "reply": "once" })),
    )
    .await;
    assert_eq!(status, 204, "{body}");
    h.settle(&session).await;
    let (_, page) = ide(
        &token,
        reqwest::Method::GET,
        format!("{base}/sessions/{session}/messages?order=asc"),
        None,
    )
    .await;
    assert_eq!(page["data"][1]["tools"][0]["output"], "ran rm -rf build");
    let logged = records.lock().unwrap().join("\n");
    assert!(
        logged.contains(&id) && logged.contains("v1.permission.reply"),
        "{logged}"
    );
    assert!(!logged.contains(&token), "{logged}");
    let file = std::fs::read_dir(h.dir.path().join("log"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap_or_default())
        .collect::<String>();
    assert!(!file.contains(&token));
}

fn set_agent_mode(h: &Harness, mode: Option<&str>) {
    h.tools.inference.lock().unwrap().permission_mode = mode.map(str::to_string);
}

async fn session_mode(h: &Harness, id: &str) -> String {
    h.runtime.state(id).await.unwrap().info.mode
}

/// Security review, case 1: an agent (or the Location's default agent) whose
/// `permission_mode` is `bypass` or `dont-ask` cannot give a token-created Session that
/// Mode; the owner still can. A safe agent default is resolved at creation, never left
/// for the first Safe Boundary.
#[tokio::test]
async fn an_agent_default_cannot_make_a_token_session_unattended() {
    let h = Harness::new(Setup::default());
    let base = serve(&h).await;
    let (_, token) = issue(&base).await;
    let post = reqwest::Method::POST;
    for mode in ["bypass", "dont-ask"] {
        set_agent_mode(&h, Some(mode));
        for body in [json!({}), json!({ "agent": "build" })] {
            let (status, refused) = ide(
                &token,
                post.clone(),
                format!("{base}/sessions"),
                Some(body.clone()),
            )
            .await;
            assert!(
                out_of_scope(status, &refused),
                "{mode} {body}: {status} {refused}"
            );
        }
        let (status, owned) =
            owner(post.clone(), format!("{base}/sessions"), Some(json!({}))).await;
        assert_eq!(status, 201, "{owned}");
        let id = owned["data"]["id"].as_str().unwrap();
        assert_eq!(
            session_mode(&h, id).await,
            mode,
            "the owner path is unchanged"
        );
    }
    set_agent_mode(&h, Some("plan"));
    let (status, created) = ide(
        &token,
        post.clone(),
        format!("{base}/sessions"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let id = created["data"]["id"].as_str().unwrap();
    assert_eq!(session_mode(&h, id).await, "plan");
    assert_eq!(created["data"]["pending_mode"], Value::Null);
}

/// Security review, cases 2 and 3: a token cannot create a child Session (which would
/// inherit from a Session it did not create, such as a `bypass` parent) or an unattended
/// one.
#[tokio::test]
async fn a_token_cannot_create_child_or_unattended_sessions() {
    let h = Harness::new(Setup::default());
    let base = serve(&h).await;
    let (_, token) = issue(&base).await;
    let parent = h
        .runtime
        .create_session(cyber_server::runtime::CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    for body in [
        json!({ "parent_id": parent }),
        json!({ "unattended": true }),
        json!({ "rules": { "*": "allow" } }),
    ] {
        let (status, refused) = ide(
            &token,
            reqwest::Method::POST,
            format!("{base}/sessions"),
            Some(body.clone()),
        )
        .await;
        assert!(out_of_scope(status, &refused), "{body}: {status} {refused}");
    }
    let children = h
        .runtime
        .list(&cyber_server::runtime::ListFilter {
            parent: Some(parent.clone()),
            ..Default::default()
        })
        .unwrap()
        .sessions;
    assert!(children.is_empty(), "no child was created");
}

/// Security review, case 4: the in-scope routes cannot reach an unattended Session. A
/// token cannot prompt a Session that already runs in `bypass` or `dont-ask` (the owner
/// switched it), a prompt is never interpreted as a slash command, and an `always` reply
/// saves only the request's own patterns, as a person approving in the editor would.
#[tokio::test]
async fn in_scope_routes_cannot_drive_or_reach_an_unattended_session() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![tools(&[("c1", "shell", "{}")]), text("done")],
        )],
        ..Setup::default()
    });
    h.tools.set("shell", Behavior::Ask("rm -rf build".into()));
    let base = serve(&h).await;
    let (_, token) = issue(&base).await;
    let post = reqwest::Method::POST;
    let prompt = |text: &str| json!({ "parts": [{ "type": "text", "text": text }] });

    let unattended = h.session().await;
    h.runtime
        .switch_mode(&unattended, "dont-ask")
        .await
        .unwrap();
    let (status, refused) = ide(
        &token,
        post.clone(),
        format!("{base}/sessions/{unattended}/prompt"),
        Some(prompt("go")),
    )
    .await;
    assert!(out_of_scope(status, &refused), "{status} {refused}");
    assert!(
        h.runtime
            .state(&unattended)
            .await
            .unwrap()
            .entries
            .is_empty()
    );

    let session = h.session().await;
    let (status, body) = ide(
        &token,
        post.clone(),
        format!("{base}/sessions/{session}/prompt"),
        Some(prompt("/mode bypass")),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    let request = loop {
        let (_, list) = ide(
            &token,
            reqwest::Method::GET,
            format!("{base}/permissions/requests"),
            None,
        )
        .await;
        if let Some(r) = list["data"].as_array().and_then(|a| a.first()).cloned() {
            break r;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(
        session_mode(&h, &session).await,
        "default",
        "no slash command ran"
    );
    let rid = request["id"].as_str().unwrap();
    let (status, body) = ide(
        &token,
        post,
        format!("{base}/sessions/{session}/permissions/{rid}/reply"),
        Some(json!({ "reply": "always" })),
    )
    .await;
    assert_eq!(status, 204, "{body}");
    h.settle(&session).await;
    let info = h.runtime.state(&session).await.unwrap().info;
    assert_eq!(info.mode, "default");
    assert!(
        info.rules.is_null() || info.rules == json!({}),
        "{}",
        info.rules
    );
}
