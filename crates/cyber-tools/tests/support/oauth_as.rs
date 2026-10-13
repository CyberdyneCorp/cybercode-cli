//! In-process fake OAuth 2.1 authorization server (and metadata host) on 127.0.0.1.
//! Shared by the tools and CLI suites through `#[path]`, so it depends only on axum, base64,
//! serde_json, sha2 and tokio.
//!
//! `/authorize` validates PKCE S256 and the RFC 8707 `resource`, then redirects to the
//! loopback callback with a single-use code; `/token` verifies the verifier, redirect URI
//! and `resource` before issuing tokens; `/register` performs RFC 7591 registration. Any
//! other GET serves a JSON document set by the test (protected-resource or AS metadata).
#![allow(dead_code)]
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::{Form, Json};
use base64::Engine;
use serde_json::{Value, json};
use sha2::Digest;

pub const ACCESS_PREFIX: &str = "access-TOKEN-SECRET-";
pub const REFRESH_PREFIX: &str = "refresh-TOKEN-SECRET-";
pub const REGISTERED_CLIENT: &str = "dyn-client";

/// One request the fake received.
#[derive(Clone, Debug)]
pub struct AsSeen {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub form: HashMap<String, String>,
    pub json: Value,
    pub authorization: Option<String>,
}

struct Grant {
    challenge: String,
    redirect_uri: String,
    resource: Option<String>,
    client_id: String,
    scope: Option<String>,
}

#[derive(Default)]
struct Inner {
    documents: BTreeMap<String, Value>,
    seen: Vec<AsSeen>,
    grants: HashMap<String, Grant>,
    refresh: HashMap<String, (Option<String>, String)>,
    issued: u32,
    /// The `resource` every authorize and token request must carry.
    resource: Option<String>,
    /// `expires_in` of issued tokens (3600 when unset).
    expires_in: Option<u64>,
    /// Token requests are recorded, then wait while this is set.
    hold: bool,
    released: Arc<tokio::sync::Notify>,
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Inner>>);

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap()
    }
}

pub struct FakeAs {
    /// `http://127.0.0.1:PORT`, also the issuer.
    pub origin: String,
    shared: Shared,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl FakeAs {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let shared = Shared::default();
        let router = axum::Router::new()
            .route("/authorize", axum::routing::get(authorize))
            .route("/token", axum::routing::post(token))
            .route("/register", axum::routing::post(register))
            .fallback(document)
            .with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            origin,
            shared,
            task: Some(task),
        }
    }

    /// RFC 8414 metadata for this issuer with registration and S256.
    pub fn metadata(&self) -> Value {
        let origin = &self.origin;
        json!({
            "issuer": origin,
            "authorization_endpoint": format!("{origin}/authorize"),
            "token_endpoint": format!("{origin}/token"),
            "registration_endpoint": format!("{origin}/register"),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "scopes_supported": ["mcp.read", "mcp.write"]
        })
    }

    /// RFC 9728 metadata naming this issuer for `resource`.
    pub fn protected_resource(&self, resource: &str) -> Value {
        json!({
            "resource": resource,
            "authorization_servers": [self.origin],
            "scopes_supported": ["mcp.read"],
            "bearer_methods_supported": ["header"]
        })
    }

    /// Serve `value` as JSON on GET `path`.
    pub fn serve(&self, path: &str, value: Value) {
        self.shared.lock().documents.insert(path.into(), value);
    }

    /// Serve the default metadata at the RFC 8414 well-known location.
    pub fn serve_metadata(&self) {
        self.serve("/.well-known/oauth-authorization-server", self.metadata());
    }

    /// Serve RFC 8414 metadata that offers no `registration_endpoint`.
    pub fn serve_metadata_without_registration(&self) {
        let mut metadata = self.metadata();
        metadata
            .as_object_mut()
            .unwrap()
            .remove("registration_endpoint");
        self.serve("/.well-known/oauth-authorization-server", metadata);
    }

    /// Require this RFC 8707 `resource` on authorize and token requests.
    pub fn expect_resource(&self, resource: &str) {
        self.shared.lock().resource = Some(resource.into());
    }

    /// Accept `token` as an outstanding refresh token, as if issued by an earlier grant.
    /// Seeding does not advance the issue counter, so the next issued tokens end in `1`.
    pub fn issue_refresh(&self, token: &str) {
        self.shared
            .lock()
            .refresh
            .insert(token.into(), (None, String::new()));
    }

    /// Issue tokens with this `expires_in`.
    pub fn set_expires_in(&self, seconds: u64) {
        self.shared.lock().expires_in = Some(seconds);
    }

    /// Forget an outstanding refresh token, as a revocation would.
    pub fn revoke_refresh(&self, token: &str) {
        self.shared.lock().refresh.remove(token);
    }

    /// Record token requests but answer none until [`Self::release_tokens`].
    pub fn hold_tokens(&self) {
        self.shared.lock().hold = true;
    }

    /// Answer held and later token requests.
    pub fn release_tokens(&self) {
        let released = {
            let mut inner = self.shared.lock();
            inner.hold = false;
            inner.released.clone()
        };
        released.notify_waiters();
    }

    /// Every token endpoint request.
    pub fn token_posts(&self) -> Vec<AsSeen> {
        self.requests_to("/token")
    }

    pub fn seen(&self) -> Vec<AsSeen> {
        self.shared.lock().seen.clone()
    }

    pub fn paths(&self) -> Vec<String> {
        self.seen().into_iter().map(|seen| seen.path).collect()
    }

    pub fn requests_to(&self, path: &str) -> Vec<AsSeen> {
        self.seen()
            .into_iter()
            .filter(|seen| seen.path == path)
            .collect()
    }
}

impl Drop for FakeAs {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn record(
    shared: &Shared,
    method: &str,
    path: &str,
    query: HashMap<String, String>,
    form: HashMap<String, String>,
    json: Value,
    headers: &HeaderMap,
) {
    shared.lock().seen.push(AsSeen {
        method: method.into(),
        path: path.into(),
        query,
        form,
        json,
        authorization: headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    });
}

fn bad(reason: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"invalid_request","error_description":reason})),
    )
        .into_response()
}

fn s256(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier))
}

async fn document(State(shared): State<Shared>, uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().to_string();
    record(
        &shared,
        "GET",
        &path,
        HashMap::new(),
        HashMap::new(),
        Value::Null,
        &headers,
    );
    match shared.lock().documents.get(&path) {
        Some(value) => Json(value.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn authorize(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    record(
        &shared,
        "GET",
        "/authorize",
        query.clone(),
        HashMap::new(),
        Value::Null,
        &headers,
    );
    let field = |name: &str| query.get(name).cloned().unwrap_or_default();
    if field("response_type") != "code" || field("code_challenge_method") != "S256" {
        return bad("response_type and S256 are required");
    }
    let challenge = field("code_challenge");
    if challenge.len() != 43 || field("state").is_empty() || field("client_id").is_empty() {
        return bad("challenge, state and client_id are required");
    }
    let redirect = field("redirect_uri");
    if !redirect.starts_with("http://127.0.0.1:") || !redirect.ends_with("/mcp/oauth/callback") {
        return bad("redirect_uri must be the loopback callback");
    }
    let mut inner = shared.lock();
    if inner.resource.is_some() && query.get("resource") != inner.resource.as_ref() {
        return bad("resource mismatch");
    }
    inner.issued += 1;
    let code = format!("code-{}", inner.issued);
    inner.grants.insert(
        code.clone(),
        Grant {
            challenge,
            redirect_uri: redirect.clone(),
            resource: query.get("resource").cloned(),
            client_id: field("client_id"),
            scope: query.get("scope").cloned(),
        },
    );
    let location = format!("{redirect}?code={code}&state={}", field("state"));
    Response::builder()
        .status(StatusCode::FOUND)
        .header("location", location)
        .body(axum::body::Body::empty())
        .unwrap()
}

async fn token(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    record(
        &shared,
        "POST",
        "/token",
        HashMap::new(),
        form.clone(),
        Value::Null,
        &headers,
    );
    held(&shared).await;
    let mut inner = shared.lock();
    let expected = inner.resource.clone();
    if expected.is_some() && form.get("resource") != expected.as_ref() {
        return bad("resource mismatch");
    }
    let scope = match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => match redeem(&mut inner, &form) {
            Some(scope) => scope,
            None => return grant_error(),
        },
        Some("refresh_token") => {
            let presented = form.get("refresh_token").cloned().unwrap_or_default();
            match inner.refresh.remove(&presented) {
                Some((scope, _)) => scope,
                None => return grant_error(),
            }
        }
        _ => return bad("unsupported grant"),
    };
    inner.issued += 1;
    let refresh = format!("{REFRESH_PREFIX}{}", inner.issued);
    let client = form.get("client_id").cloned().unwrap_or_default();
    inner
        .refresh
        .insert(refresh.clone(), (scope.clone(), client));
    let mut body = json!({
        "access_token": format!("{ACCESS_PREFIX}{}", inner.issued),
        "token_type": "Bearer",
        "expires_in": inner.expires_in.unwrap_or(3600),
        "refresh_token": refresh,
    });
    if let Some(scope) = scope {
        body["scope"] = json!(scope);
    }
    Json(body).into_response()
}

/// Wait while token requests are held.
async fn held(shared: &Shared) {
    loop {
        let released = shared.lock().released.clone();
        let notified = released.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !shared.lock().hold {
            return;
        }
        notified.await;
    }
}

fn grant_error() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":"invalid_grant"})),
    )
        .into_response()
}

/// Single-use code redemption bound to the PKCE challenge, redirect URI and client;
/// `None` when the grant is unknown or any binding differs.
fn redeem(inner: &mut Inner, form: &HashMap<String, String>) -> Option<Option<String>> {
    let code = form.get("code").cloned().unwrap_or_default();
    let grant = inner.grants.remove(&code)?;
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let valid = s256(&verifier) == grant.challenge
        && form.get("redirect_uri") == Some(&grant.redirect_uri)
        && form.get("resource") == grant.resource.as_ref()
        && form.get("client_id") == Some(&grant.client_id);
    valid.then_some(grant.scope)
}

async fn register(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    record(
        &shared,
        "POST",
        "/register",
        HashMap::new(),
        HashMap::new(),
        body.clone(),
        &headers,
    );
    if !body["redirect_uris"].is_array() {
        return bad("redirect_uris required");
    }
    (
        StatusCode::CREATED,
        Json(json!({
            "client_id": REGISTERED_CLIENT,
            "client_id_issued_at": 1,
            "redirect_uris": body["redirect_uris"],
            "token_endpoint_auth_method": "none"
        })),
    )
        .into_response()
}

/// A plain HTTP/1.1 GET that follows nothing: `(status, location, body)`. Used to play the
/// browser: fetch the authorize URL, then call the loopback callback directly.
pub async fn get(url: &str) -> (u16, Option<String>, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let rest = url.strip_prefix("http://").expect("http URL");
    let (authority, target) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    let mut stream = tokio::net::TcpStream::connect(authority).await.unwrap();
    let request =
        format!("GET {target} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    let _ = stream.read_to_end(&mut bytes).await;
    let text = String::from_utf8_lossy(&bytes).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let location = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("location")
            .then(|| value.trim().to_string())
    });
    (status, location, body.to_string())
}
