//! Peer grants and the peer guard over real listeners (`cross-session-messaging` → Peer
//! pairing and grants): the peer router served with TLS by `serve_peer`, and the main
//! router served by `serve_tcp`, over one runtime and store.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use cyber_server::http::peer::tls::{PeerIdentity, SubjectNames};
use cyber_server::http::{
    self, AgentInfo, AppState, CommandInfo, HttpOptions, ModelInfo, Services,
};
use cyber_server::runtime::messaging::{
    PeerAuthReason, PeerClient, PeerError, PeerScope, PeerTarget,
};
use cyber_server::runtime::peers::{NewGrant, grants};
use cyber_server::runtime::{ToolDef, TurnContext};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use support::{Harness, Setup};

const PASSWORD: &str = "secret";

struct NoServices;

impl Services for NoServices {
    fn models(&self, _location: &Path) -> BoxFuture<'_, Result<Vec<ModelInfo>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn default_model(&self, _location: &Path) -> Option<String> {
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

fn state(h: &Harness, password: Option<&str>) -> AppState {
    AppState {
        service: None,
        telemetry: None,
        runtime: h.runtime.clone(),
        remote_tools: Arc::default(),
        store: Arc::clone(&h.store),
        services: Arc::new(NoServices),
        options: Arc::new(HttpOptions {
            version: "0.1.0-test".into(),
            password: password.map(str::to_string),
            cors_origins: Vec::new(),
            default_directory: h.repo.clone(),
            features: Vec::new(),
        }),
    }
}

/// The main API on a loopback port; `http://127.0.0.1:<port>`.
async fn serve_main(h: &Harness, password: Option<&str>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(http::serve_tcp(
        http::router(state(h, password)),
        listener,
        std::future::pending(),
    ));
    url
}

/// The peer listener with TLS on a loopback port; its URL and fingerprint.
async fn serve_peer(h: &Harness) -> (String, String) {
    serve_peer_sharing(h, false).await
}

/// [`serve_peer`] with `peers.share_paths`.
async fn serve_peer_sharing(h: &Harness, share_paths: bool) -> (String, String) {
    serve_peer_waiting(h, share_paths, http::peer::ATTACH_WAIT).await
}

/// [`serve_peer_sharing`] whose attaches wait `attach_wait` for approval.
async fn serve_peer_waiting(
    h: &Harness,
    share_paths: bool,
    attach_wait: Duration,
) -> (String, String) {
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
        share_paths,
        attach_wait,
    };
    tokio::spawn(http::serve_peer(
        http::peer::router(state(h, Some(PASSWORD)), info),
        listener,
        Some(identity.server_config().unwrap()),
        std::future::pending(),
    ));
    (
        format!("https://{address}"),
        identity.fingerprint().to_string(),
    )
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .tls_danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

async fn get(url: &str, authorization: Option<&str>) -> (u16, Value) {
    let mut request = client().get(url);
    if let Some(value) = authorization {
        request = request.header("authorization", value);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn invite(h: &Harness, name: &str, scopes: Vec<PeerScope>, expires_ms: Option<i64>) -> String {
    let request = NewGrant {
        name: name.into(),
        scopes,
        expires_ms,
    };
    grants::create(&h.store, request, now_ms()).unwrap().token
}

/// Every text value of every `peer_grant` row.
fn stored_grant_text(h: &Harness) -> String {
    h.store
        .read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id || name || token_sha256 || scopes || created_ms || \
                 coalesce(expires_ms, '') FROM peer_grant",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?.join("\n"))
        })
        .unwrap()
}

/// Grants authenticate peer requests (`hello` answers with the grant's name and scopes),
/// the database keeps only hashes, route scopes are enforced (`403 PeerScopeError`), a
/// revoked or expired grant fails the next request with `401`, nothing but `Bearer cyp_…`
/// is accepted on the peer listener, `cyp_` tokens are refused on the main API, and each
/// grant is held to 120 requests a minute.
#[tokio::test]
async fn grants_authenticate_scope_and_revoke() {
    let h = Harness::new(Setup::default());
    let (peer, fingerprint) = serve_peer(&h).await;
    let main = serve_main(&h, Some(PASSWORD)).await;
    let hello = format!("{peer}/api/v1/peer/hello");
    let laptop = invite(&h, "laptop", vec![], None);
    let tablet = invite(
        &h,
        "tablet",
        vec![PeerScope::Message, PeerScope::Attach],
        None,
    );
    let bearer = |token: &str| format!("Bearer {token}");

    // `hello` names this machine and the caller's grant.
    let (status, body) = get(&hello, Some(&bearer(&laptop))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({"machine": "desk", "version": "0.1.0-test", "fingerprint": fingerprint,
            "scopes": ["message"], "grant": "laptop"})
    );
    let (status, body) = get(&hello, Some(&bearer(&tablet))).await;
    assert_eq!(
        (status, &body["scopes"]),
        (200, &json!(["message", "attach"]))
    );
    let used = grants::list(&h.store).unwrap();
    assert!(
        used.iter().all(|grant| grant.last_used_ms.is_some()),
        "{used:?}"
    );

    // The database holds the hash of each secret, never a token or its secret.
    let stored = stored_grant_text(&h);
    for token in [&laptop, &tablet] {
        let secret = &token[token.len() - 43..];
        assert!(!stored.contains(secret), "{stored}");
        assert!(!stored.contains(token.as_str()), "{stored}");
    }
    assert!(stored.contains("laptop") && stored.contains("[\"message\"]"));

    // A message grant cannot attach; an attach grant passes the scope check.
    let attach = format!("{peer}/api/v1/peer/attach/ses_x/events");
    let (status, body) = get(&attach, Some(&bearer(&laptop))).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        (&body["_tag"], &body["scope"]),
        (&json!("PeerScopeError"), &json!("attach"))
    );
    // Past the scope, nothing of a Session is sent without an approved attach.
    let (status, body) = get(&attach, Some(&bearer(&tablet))).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["_tag"], "PermissionDeniedError", "{body}");
    let (status, body) = get(
        &format!("{peer}/api/v1/peer/attach/ses_x/shell"),
        Some(&bearer(&tablet)),
    )
    .await;
    assert_eq!(status, 404, "{body}");

    // Only an active grant's Bearer token: no other credential, no detail.
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("cyber:{PASSWORD}"))
    );
    let unknown = format!("{}x", &laptop[..laptop.len() - 1]);
    for authorization in [None, Some(basic.clone()), Some(bearer(&unknown))] {
        for url in [hello.clone(), format!("{peer}/api/v1/peer/nothing")] {
            let (status, body) = get(&url, authorization.as_deref()).await;
            assert_eq!(status, 401, "{url} {authorization:?}: {body}");
            assert_eq!(body["message"], "missing or invalid credentials");
        }
    }
    let (status, _) = get(
        &format!("{peer}/api/v1/peer/nothing"),
        Some(&bearer(&laptop)),
    )
    .await;
    assert_eq!(status, 404);

    // The main API refuses peer tokens, with or without a password.
    let open = serve_main(&h, None).await;
    for base in [&main, &open] {
        let (status, body) = get(&format!("{base}/api/v1/sessions"), Some(&bearer(&laptop))).await;
        assert_eq!(status, 401, "{base}: {body}");
        let (status, _) = get(&format!("{base}/api/v1/event?auth_token={laptop}"), None).await;
        assert_eq!(status, 401, "{base}");
    }
    let (status, _) = get(&format!("{main}/api/v1/sessions"), Some(&basic)).await;
    assert_eq!(status, 200);

    // Revoked: the next request fails.
    assert!(grants::revoke(&h.store, "laptop", now_ms()).unwrap());
    let (status, body) = get(&hello, Some(&bearer(&laptop))).await;
    assert_eq!((status, &body["_tag"]), (401, &json!("UnauthorizedError")));
    let (status, _) = get(&hello, Some(&bearer(&tablet))).await;
    assert_eq!(status, 200);

    // Expired: refused from its expiry on.
    let brief = invite(&h, "brief", vec![], Some(now_ms() + 400));
    let (status, _) = get(&hello, Some(&bearer(&brief))).await;
    assert_eq!(status, 200);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (status, _) = get(&hello, Some(&bearer(&brief))).await;
    assert_eq!(status, 401);

    // 120 requests a minute per grant; other grants keep their own budget.
    let burst = invite(&h, "burst", vec![], None);
    for i in 0..120 {
        let (status, body) = get(&hello, Some(&bearer(&burst))).await;
        assert_eq!(status, 200, "{i}: {body}");
    }
    let (status, body) = get(&hello, Some(&bearer(&burst))).await;
    assert_eq!(status, 429, "{body}");
    assert_eq!(body["_tag"], "RateLimitedError");
    assert!(
        body["retryAfterMs"].as_u64().is_some_and(|ms| ms > 0),
        "{body}"
    );
    let (status, _) = get(&hello, Some(&bearer(&tablet))).await;
    assert_eq!(status, 200);
}

fn target(url: &str, fingerprint: &str, token: &str) -> PeerTarget {
    PeerTarget {
        name: "desk".into(),
        url: url.into(),
        fingerprint: fingerprint.into(),
        token: token.into(),
    }
}

async fn hello_error(target: PeerTarget) -> PeerError {
    PeerClient::new(target).unwrap().hello().await.unwrap_err()
}

/// Task 4.1, scenario "Certificate does not match the pin": the peer client accepts only
/// the pinned SPKI hash. A server presenting another key fails the handshake with
/// `PeerAuthError { reason: fingerprint_mismatch }` before the request (token included) is
/// sent; the pinned key answers `hello`; `401` and `403 PeerScopeError` map to
/// `unauthorized` and `scope`; a closed port is `PeerUnreachableError`.
#[tokio::test]
async fn the_peer_client_accepts_only_the_pinned_key() {
    let h = Harness::new(Setup::default());
    let (peer, fingerprint) = serve_peer(&h).await;
    let token = invite(&h, "laptop", vec![], None);

    // Another key: the pin of a different identity.
    let other_dir = tempfile::tempdir().unwrap();
    let names = SubjectNames {
        machine: "desk".into(),
        address: "127.0.0.1:1".parse().unwrap(),
    };
    let other = PeerIdentity::load_or_create(other_dir.path(), &names).unwrap();
    assert_ne!(other.fingerprint(), fingerprint);
    let error = hello_error(target(&peer, other.fingerprint(), &token)).await;
    assert_eq!(
        error,
        PeerError::Auth {
            peer: "desk".into(),
            reason: PeerAuthReason::FingerprintMismatch
        }
    );
    assert!(error.to_string().starts_with("PeerAuthError"), "{error}");
    // Nothing reached the peer guard: the grant was never used.
    assert_eq!(grants::list(&h.store).unwrap()[0].last_used_ms, None);

    // The pinned key: `hello` names the peer, this machine's grant and the scopes.
    let client = PeerClient::new(target(&peer, &fingerprint, &token)).unwrap();
    let (hello, latency) = client.hello().await.unwrap();
    assert_eq!(
        (hello.machine.as_str(), hello.grant.as_str(), hello.scopes),
        ("desk", "laptop", vec!["message".to_string()])
    );
    assert_eq!(hello.fingerprint, fingerprint);
    assert!(latency < Duration::from_secs(10));
    assert!(!format!("{client:?}").contains(&token));

    // A message grant on an attach route is `scope`; a wrong token is `unauthorized`.
    let error = client
        .request(reqwest::Method::GET, "/attach/ses_x/events", None)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        PeerError::Auth {
            peer: "desk".into(),
            reason: PeerAuthReason::Scope
        }
    );
    let wrong = format!("{}x", &token[..token.len() - 1]);
    let error = hello_error(target(&peer, &fingerprint, &wrong)).await;
    assert_eq!(
        error,
        PeerError::Auth {
            peer: "desk".into(),
            reason: PeerAuthReason::Unauthorized
        }
    );
    assert!(grants::revoke(&h.store, "laptop", now_ms()).unwrap());
    let error = hello_error(target(&peer, &fingerprint, &token)).await;
    assert!(
        matches!(
            error,
            PeerError::Auth {
                reason: PeerAuthReason::Unauthorized,
                ..
            }
        ),
        "{error}"
    );

    // Nobody listening: unreachable, with a cause and no token.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let error = hello_error(target(&format!("https://{closed}"), &fingerprint, &token)).await;
    assert!(
        matches!(&error, PeerError::Unreachable { peer, cause } if peer == "desk" && !cause.contains(&token)),
        "{error}"
    );
}

/// The Location directory of the one Session `base` lists, which must be `session`.
async fn listed_directory(base: &str, bearer: &str, session: &str) -> String {
    let (status, body) = get(&format!("{base}/api/v1/peer/sessions"), Some(bearer)).await;
    assert_eq!(status, 200, "{body}");
    let sessions = body["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{body}");
    assert_eq!(sessions[0]["session_id"], session, "{body}");
    sessions[0]["location"]["directory"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// `GET /api/v1/peer/sessions` (scope `message`) lists root Sessions with their workspace
/// name only, or their full Location with `peers.share_paths`;
/// `limit` caps the listing, and an `attach`-only grant is refused.
#[tokio::test]
async fn peer_listings_share_paths_only_when_configured() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let (url, _) = serve_peer_sharing(&h, true).await;
    let (private, _) = serve_peer(&h).await;
    let token = invite(&h, "laptop", vec![], None);
    let bearer = format!("Bearer {token}");
    let workspace = h.repo.file_name().unwrap().to_string_lossy().into_owned();
    let shared = listed_directory(&url, &bearer, &root).await;
    let shared = Path::new(&shared);
    assert!(shared.is_absolute(), "{shared:?}");
    assert_eq!(shared.file_name().unwrap().to_string_lossy(), workspace);
    assert_eq!(listed_directory(&private, &bearer, &root).await, workspace);
    h.session().await;
    let (_, body) = get(
        &format!("{url}/api/v1/peer/sessions?limit=1"),
        Some(&bearer),
    )
    .await;
    assert_eq!(body["sessions"].as_array().unwrap().len(), 1, "{body}");
    let attach = format!(
        "Bearer {}",
        invite(&h, "tablet", vec![PeerScope::Attach], None)
    );
    let (status, body) = get(&format!("{url}/api/v1/peer/sessions"), Some(&attach)).await;
    assert_eq!(
        (status, &body["_tag"]),
        (403, &json!("PeerScopeError")),
        "{body}"
    );
    let (status, _) = get(
        &format!("{url}/api/v1/peer/sessions/{root}/status"),
        Some(&attach),
    )
    .await;
    assert_eq!(status, 403);
}

/// OpenAPI group `peer` (design section 4): every peer listener route is in the document
/// under tag `peer` with the grant's bearer scheme, is served by the peer listener over
/// TLS (a handler answers: never an untagged 404 or a 405), and is not an operation of
/// the main API, so `cyber api`, JSON-RPC and MCP cannot reach it there.
#[tokio::test]
async fn every_documented_peer_operation_is_routed_on_the_peer_listener() {
    let h = Harness::new(Setup::default());
    let id = h.session().await;
    // The attach request is withdrawn unanswered at once.
    let (base, _) = serve_peer_waiting(&h, false, Duration::from_millis(50)).await;
    let token = invite(
        &h,
        "laptop",
        vec![PeerScope::Message, PeerScope::Attach],
        None,
    );
    let doc = http::openapi::document("test");
    let routes = http::openapi::peer_routes();
    assert!(routes.len() >= 14, "{routes:?}");
    for (method, path) in routes {
        let op = &doc["paths"][format!("/api/v1{path}")][method];
        let operation_id = op["operationId"].as_str().unwrap();
        assert!(operation_id.starts_with("v1.peer."), "{operation_id}");
        assert_eq!(op["tags"], json!(["peer"]), "{operation_id}");
        assert_eq!(
            op["security"],
            json!([{"peerBearer": []}]),
            "{operation_id}"
        );
        assert_eq!(http::openapi::lookup(operation_id), None, "{operation_id}");
        assert!(
            !http::openapi::routes().contains(&(method, path)),
            "{method} {path} is on the main API"
        );
        let concrete = path
            .replace("{sessionID}", &id)
            .replace("{messageID}", "msgx_unknown")
            .replace("{requestID}", "per_x");
        let m = reqwest::Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
        let mut request = client()
            .request(m, format!("{base}/api/v1{concrete}"))
            .bearer_auth(&token);
        if method != "get" && method != "delete" {
            request = request.json(&json!({}));
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        assert_ne!(status, 405, "{method} {path}");
        assert!(
            status != 404
                || body["_tag"] != "NotFoundError"
                || body["message"] != "no such peer route",
            "{method} {path}: {status} {body}"
        );
    }
    assert_eq!(
        doc["components"]["securitySchemes"]["peerBearer"]["scheme"],
        "bearer"
    );
}

/// A root Session of `h` titled `title`.
async fn titled(h: &Harness, title: &str) -> String {
    let req = cyber_server::runtime::CreateSession {
        directory: h.repo.display().to_string(),
        model: "test/main".into(),
        title: Some(title.into()),
        ..Default::default()
    };
    h.runtime.create_session(req).await.unwrap().id
}

/// `POST /api/v1/peer/messages` with `token`: one message from `from_session` to `to`.
async fn post_message(base: &str, token: &str, id: &str, to: &str, from: &str) -> (u16, Value) {
    let body = json!({"id": id, "to": to, "text": "hi", "hops": 1, "from_session": from});
    let response = client()
        .post(format!("{base}/api/v1/peer/messages"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// Regression (scenario "Refused grant hides Sessions"): a peer's `to` was resolved among
/// every root Session, so an ambiguous name answered `409` with every candidate's ID and
/// absolute Location, Sessions that refuse the grant included, and a send to a refusing
/// Session confirmed it with its ID. Now a grant sees only the Sessions its listing shows:
/// a name shared with a hidden Session resolves to the shown one, an ambiguous name lists
/// only shown candidates by workspace name, and a hidden Session is not found.
#[tokio::test]
async fn peer_sends_never_reveal_sessions_hidden_from_the_grant() {
    use cyber_server::runtime::messaging::InboundOverride;
    let h = Harness::new(Setup::default());
    let shown = titled(&h, "api").await;
    let hidden = titled(&h, "api").await;
    let refuse = InboundOverride {
        inbound: cyber_core::config::InboundPolicy::Refuse,
        reason: None,
    };
    h.runtime.set_inbound(&hidden, Some(refuse)).await.unwrap();
    let (peer, _) = serve_peer(&h).await;
    let token = invite(&h, "laptop", vec![], None);
    let repo = h.repo.display().to_string();
    let workspace = h.repo.file_name().unwrap().to_string_lossy().into_owned();

    // The one shown Session of that name receives it.
    let (status, body) = post_message(&peer, &token, "msgx_one", "api", "ses_lap").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["target_session_id"], shown.as_str(), "{body}");
    assert!(!body.to_string().contains(&hidden), "{body}");

    // Ambiguous among shown Sessions: only those, by workspace name.
    let other = titled(&h, "api").await;
    let (status, body) = post_message(&peer, &token, "msgx_two", "api", "ses_lap").await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["_tag"], "AmbiguousAddressError", "{body}");
    let text = body.to_string();
    assert!(!text.contains(&hidden), "{text}");
    assert!(!text.contains(&repo), "{text}");
    let candidates = body["candidates"].as_array().unwrap();
    let mut ids: Vec<_> = candidates
        .iter()
        .map(|c| c["session_id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    let mut expected = vec![shown.clone(), other.clone()];
    expected.sort();
    assert_eq!(ids, expected, "{body}");
    for candidate in candidates {
        assert_eq!(
            candidate["location"]["directory"],
            workspace.as_str(),
            "{body}"
        );
    }

    // A hidden Session is not found, by ID too, and records nothing.
    let (status, body) = post_message(&peer, &token, "msgx_three", &hidden, "ses_lap").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["_tag"], "AddressNotFoundError", "{body}");
    assert!(body.get("target_session_id").is_none(), "{body}");
    assert!(h.runtime.message_rows(&hidden).unwrap().is_empty());
}

/// Regression (design section 7): a peer's limits were keyed on the `from_session` label
/// it chooses, so a new label per message escaped the 10-per-minute limit and grew a
/// refusing target's log with every request. Every Session of one grant now counts
/// together: the eleventh message from eleven labels is `rate_limited`, and a target
/// records at most 10 refusals of the grant a minute.
#[tokio::test]
async fn peer_limits_count_every_session_of_the_grant() {
    let h = Harness::new(Setup::default());
    let migration = titled(&h, "migration").await;
    let (peer, _) = serve_peer(&h).await;
    let token = invite(&h, "laptop", vec![], None);
    for i in 0..10 {
        let (status, body) = post_message(
            &peer,
            &token,
            &format!("msgx_r{i}"),
            "migration",
            &format!("ses_l{i}"),
        )
        .await;
        assert_eq!(
            (status, &body["outcome"]),
            (200, &json!("held")),
            "{i}: {body}"
        );
    }
    let (status, body) = post_message(&peer, &token, "msgx_r10", "migration", "ses_l10").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        (&body["outcome"], &body["reason"], &body["limit"]),
        (
            &json!("refused"),
            &json!("rate_limited"),
            &json!("per_target_minute")
        ),
        "{body}"
    );
    // Another grant keeps its own budget.
    let other = invite(&h, "tablet", vec![], None);
    let (_, body) = post_message(&peer, &other, "msgx_t0", "migration", "ses_l0").await;
    assert_eq!(body["outcome"], "held", "{body}");
    assert_eq!(h.runtime.message_rows(&migration).unwrap().len(), 11);

    // A target that refuses every Session label of the grant (but is shown to it).
    h.tools.messaging.lock().unwrap().inbound.refuse = vec!["ses_*@laptop".into()];
    let noisy = titled(&h, "noisy").await;
    for i in 0..15 {
        let (status, body) = post_message(
            &peer,
            &token,
            &format!("msgx_n{i}"),
            "noisy",
            &format!("ses_n{i}"),
        )
        .await;
        assert_eq!(
            (status, &body["outcome"]),
            (200, &json!("refused")),
            "{i}: {body}"
        );
        let reason = if i < 10 {
            "inbound_rule"
        } else {
            "rate_limited"
        };
        assert_eq!(body["reason"], reason, "{i}: {body}");
    }
    let recorded = h.runtime.message_rows(&noisy).unwrap();
    assert_eq!(recorded.len(), 10, "{recorded:?}");
}

/// A raw TCP connection to `address` and whether the server closes it within `within`
/// without having sent anything.
async fn closed_within(stream: &mut tokio::net::TcpStream, within: Duration) -> bool {
    use tokio::io::AsyncReadExt;
    let mut byte = [0u8; 1];
    matches!(
        tokio::time::timeout(within, stream.read(&mut byte)).await,
        Ok(Ok(0) | Err(_))
    )
}

/// Regression: the peer listener spawned a task per connection with no cap and no
/// timeout after the TLS handshake, so any host on the LAN could hold idle connections
/// until the shared server ran out of file descriptors. Through `serve_peer`, a
/// connection past the cap of 64 is closed at once while the main API still answers, a
/// freed slot is served again, and (through `serve_peer_with`) a connection that sends no
/// request, or stays idle after one, is closed after the header timeout.
#[tokio::test]
async fn peer_connections_are_capped_and_time_out() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let h = Harness::new(Setup::default());
    let (peer, _) = serve_peer(&h).await;
    let main = serve_main(&h, Some(PASSWORD)).await;
    let address = peer.trim_start_matches("https://").to_string();
    let limit = http::PeerLimits::default().connections;
    let mut held = Vec::new();
    for _ in 0..limit {
        held.push(tokio::net::TcpStream::connect(&address).await.unwrap());
    }
    // Every slot is taken (connections still in their TLS handshake hold one).
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut extra = tokio::net::TcpStream::connect(&address).await.unwrap();
    assert!(closed_within(&mut extra, Duration::from_secs(3)).await);
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("cyber:{PASSWORD}"))
    );
    let (status, _) = get(&format!("{main}/api/v1/health"), Some(&basic)).await;
    assert_eq!(status, 200);
    for stream in &mut held[..limit - 1] {
        assert!(!closed_within(stream, Duration::from_millis(1)).await);
    }
    drop(held);
    let token = invite(&h, "laptop", vec![], None);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let response = client()
            .get(format!("{peer}/api/v1/peer/hello"))
            .bearer_auth(&token)
            .send()
            .await;
        if response.is_ok_and(|r| r.status() == 200) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no slot was freed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The header timeout, on a plain loopback listener with a short one.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let plain = listener.local_addr().unwrap();
    let info = http::peer::PeerInfo {
        machine: "desk".into(),
        fingerprint: "sha256:plain".into(),
        share_paths: false,
        attach_wait: http::peer::ATTACH_WAIT,
    };
    let limits = http::PeerLimits {
        connections: 4,
        header_timeout: Duration::from_millis(300),
    };
    tokio::spawn(http::serve_peer_with(
        http::peer::router(state(&h, Some(PASSWORD)), info),
        listener,
        None,
        limits,
        std::future::pending(),
    ));
    let mut silent = tokio::net::TcpStream::connect(plain).await.unwrap();
    assert!(closed_within(&mut silent, Duration::from_secs(3)).await);
    let mut stalled = tokio::net::TcpStream::connect(plain).await.unwrap();
    stalled
        .write_all(b"GET /api/v1/peer/hello HTTP/1.1\r\n")
        .await
        .unwrap();
    assert!(closed_within(&mut stalled, Duration::from_secs(3)).await);
    let mut idle = tokio::net::TcpStream::connect(plain).await.unwrap();
    let request = format!(
        "GET /api/v1/peer/hello HTTP/1.1\r\nhost: desk\r\nauthorization: Bearer {token}\r\n\r\n"
    );
    idle.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(3), idle.read_to_end(&mut answer)).await;
    assert!(read.is_ok(), "an idle keep-alive connection stays open");
    let answer = String::from_utf8_lossy(&answer);
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
}

/// Regression: unauthenticated peer requests skipped every limit, and each one looked
/// its token up in the store. After 30 invalid tokens from one address in a minute, that
/// address is refused with `429` before any lookup, valid token or not; other listeners
/// (and so other addresses) are unaffected.
#[tokio::test]
async fn invalid_peer_tokens_are_limited_per_address() {
    let h = Harness::new(Setup::default());
    let (peer, _) = serve_peer(&h).await;
    let token = invite(&h, "laptop", vec![], None);
    let hello = format!("{peer}/api/v1/peer/hello");
    let wrong = format!("Bearer {}x", &token[..token.len() - 1]);
    for i in 0..30 {
        let (status, _) = get(&hello, Some(&wrong)).await;
        assert_eq!(status, 401, "{i}");
    }
    let (status, body) = get(&hello, Some(&wrong)).await;
    assert_eq!(
        (status, &body["_tag"]),
        (429, &json!("RateLimitedError")),
        "{body}"
    );
    let (status, _) = get(&hello, Some(&format!("Bearer {token}"))).await;
    assert_eq!(status, 429);
    // No token at all costs no lookup and is still a plain `401`.
    let (status, _) = get(&hello, None).await;
    assert_eq!(status, 401);
    let (fresh, _) = serve_peer(&h).await;
    let (status, _) = get(
        &format!("{fresh}/api/v1/peer/hello"),
        Some(&format!("Bearer {token}")),
    )
    .await;
    assert_eq!(status, 200);
}
