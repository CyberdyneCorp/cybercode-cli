//! Local IDE tokens on the main API (`server-api` → Local IDE token issuance, Local IDE
//! token scope, Local IDE token revocation and expiry).
//!
//! The `ide` routes issue, list and revoke tokens for the owner. [`admit`] authenticates a
//! request that presents `Authorization: Bearer cyi_…`: only on the Unix socket or from a
//! loopback peer of the main TCP listener, only for the operations in [`SCOPE`], and never
//! to remove human approval. It runs in the guard, before routing reaches any handler.
//! [`IdeLive`] tracks what each token holds open, so a revocation closes its event streams
//! and ends the presence leases it created.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::body::Body;
use axum::extract::{MatchedPath, Path, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, post};
use axum::{Json, Router};
use futures::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::envelope::Data;
use super::error::ApiError;
use super::guard::loopback_peer;
use super::peer::guard::now_ms;
use super::serve::PeerAddr;
use super::{AppState, Transport};
use crate::runtime::RuntimeError;
use crate::runtime::ide_tokens::{self, IdeToken, IdeTokenError, NewToken};

/// The operations an IDE token may call, as `(method, path under /api/v1, operation ID)`.
/// Everything else, the `ide` routes included, is refused with `403 ide_token_scope`.
pub const SCOPE: &[(&str, &str, &str)] = &[
    ("GET", "/service", "v1.service.get"),
    ("GET", "/models", "v1.model.list"),
    ("GET", "/sessions", "v1.session.list"),
    ("POST", "/sessions", "v1.session.create"),
    ("GET", "/sessions/{sessionID}", "v1.session.get"),
    ("POST", "/sessions/{sessionID}/prompt", "v1.session.prompt"),
    (
        "POST",
        "/sessions/{sessionID}/interrupt",
        "v1.session.interrupt",
    ),
    ("POST", "/sessions/{sessionID}/mode", "v1.session.mode"),
    ("POST", "/sessions/{sessionID}/model", "v1.session.model"),
    ("GET", "/sessions/{sessionID}/messages", "v1.message.list"),
    ("GET", "/sessions/{sessionID}/events", "v1.session.events"),
    ("GET", "/sessions/{sessionID}/diff", "v1.session.diff"),
    (
        "POST",
        "/sessions/{sessionID}/interactive-clients",
        "v1.session.interactiveAttach",
    ),
    (
        "PUT",
        "/sessions/{sessionID}/interactive-clients/{clientID}",
        "v1.session.interactiveRenew",
    ),
    (
        "DELETE",
        "/sessions/{sessionID}/interactive-clients/{clientID}",
        "v1.session.interactiveRelease",
    ),
    ("GET", "/permissions/requests", "v1.permission.list"),
    (
        "POST",
        "/sessions/{sessionID}/permissions/{requestID}/reply",
        "v1.permission.reply",
    ),
    ("GET", "/questions/requests", "v1.question.list"),
    (
        "POST",
        "/sessions/{sessionID}/questions/{requestID}/reply",
        "v1.question.reply",
    ),
];

/// Modes an IDE token cannot select: both remove human approval.
const UNATTENDED_MODES: [&str; 2] = ["bypass", "dont-ask"];
/// The largest body read to check a Session create or Mode switch.
const CHECKED_BODY_LIMIT: usize = 1024 * 1024;

/// The IDE token a request authenticated with, for handlers that record what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeCaller {
    /// `itk_…`; never the token value.
    pub token_id: String,
}

/// What each token holds open: a cancellation that ends its event streams, and the
/// presence leases it created.
#[derive(Debug, Default)]
pub struct IdeLive {
    tokens: Mutex<HashMap<String, LiveToken>>,
}

#[derive(Debug, Default)]
struct LiveToken {
    cancel: CancellationToken,
    leases: Vec<Lease>,
}

#[derive(Debug, Clone)]
struct Lease {
    session: String,
    directory: String,
    client: String,
}

impl IdeLive {
    fn with<T>(&self, id: &str, f: impl FnOnce(&mut LiveToken) -> T) -> T {
        let mut tokens = self
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(tokens.entry(id.to_string()).or_default())
    }

    /// The cancellation of token `id`'s streams; already cancelled once it was revoked.
    fn cancellation(&self, id: &str) -> CancellationToken {
        self.with(id, |live| live.cancel.clone())
    }

    /// Record a presence lease token `id` created; `false` when the token was revoked
    /// meanwhile, so the caller ends the lease itself.
    pub fn add_lease(&self, id: &str, session: &str, directory: &str, client: &str) -> bool {
        self.with(id, |live| {
            if live.cancel.is_cancelled() {
                return false;
            }
            live.leases.push(Lease {
                session: session.into(),
                directory: directory.into(),
                client: client.into(),
            });
            true
        })
    }

    /// Cancel token `id`'s streams and hand back its leases. The cancelled entry stays, so
    /// a stream that passed authentication just before the revocation still closes.
    fn revoke(&self, id: &str) -> Vec<Lease> {
        self.with(id, |live| {
            live.cancel.cancel();
            std::mem::take(&mut live.leases)
        })
    }
}

/// End what revoked tokens `ids` hold: close their streams and release their leases.
pub async fn end(state: &AppState, ids: &[String]) {
    for id in ids {
        for lease in state.ide.revoke(id) {
            // A lease already released or expired is gone either way.
            let _ = state
                .runtime
                .release_interactive_client(&lease.session, &lease.directory, &lease.client)
                .await;
        }
        cyber_core::log::log(
            cyber_core::log::Level::Info,
            "http",
            "ide token revoked",
            serde_json::json!({ "token_id": id }),
        );
    }
}

/// The token of `Authorization: Bearer cyi_…`, when that is what the request carries.
pub fn presented(req: &Request) -> Option<&str> {
    req.headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| token.starts_with(ide_tokens::TOKEN_PREFIX))
}

/// Authenticate and scope a request presenting an IDE token, then run it. `401` off the
/// socket and loopback or for an inactive token; `403 ide_token_scope` out of scope.
pub async fn admit(state: AppState, req: Request, transport: Transport, next: Next) -> Response {
    let local = match transport {
        Transport::Unix => true,
        Transport::Tcp => req
            .extensions()
            .get::<PeerAddr>()
            .is_some_and(|peer| loopback_peer(peer.0.ip())),
        // In-process calls carry no credentials; a token there is never honored.
        Transport::Embedded => false,
    };
    let token = presented(&req).unwrap_or_default();
    let record = match local {
        true => ide_tokens::verify(&state.store, token, now_ms()),
        false => Ok(None),
    };
    let record = match record {
        Ok(Some(record)) => record,
        Ok(None) => return unauthorized(),
        Err(error) => return ApiError::from(RuntimeError::from(error)).into_response(),
    };
    let route = route(&req);
    let Some(operation) = operation(req.method(), route.as_deref()) else {
        return out_of_scope("this route is outside the IDE token scope");
    };
    let req = match checked_body(operation, req).await {
        Ok(req) => req,
        Err(response) => return *response,
    };
    cyber_core::log::log(
        cyber_core::log::Level::Info,
        "http",
        "ide token request",
        serde_json::json!({ "token_id": record.id, "operation": operation }),
    );
    let cancel = state.ide.cancellation(&record.id);
    let mut req = req;
    req.extensions_mut().insert(IdeCaller {
        token_id: record.id,
    });
    let response = next.run(req).await;
    closable(response, cancel)
}

/// The matched route template under `/api/v1`, or `None` for an unrouted path.
fn route(req: &Request) -> Option<String> {
    let matched = req.extensions().get::<MatchedPath>()?.as_str();
    Some(
        matched
            .strip_prefix("/api/v1")
            .unwrap_or(matched)
            .to_string(),
    )
}

/// The operation ID `(method, route)` names in [`SCOPE`]. Route parameters are compared
/// by position only, so `{session_id}` and `{sessionID}` match.
pub fn operation(method: &Method, route: Option<&str>) -> Option<&'static str> {
    let route = shape(route?);
    SCOPE
        .iter()
        .find(|(m, path, _)| *m == method.as_str() && shape(path) == route)
        .map(|(_, _, id)| *id)
}

fn shape(path: &str) -> String {
    path.split('/')
        .map(
            |segment| match segment.starts_with('{') && segment.ends_with('}') {
                true => "{}",
                false => segment,
            },
        )
        .collect::<Vec<_>>()
        .join("/")
}

/// Refuse a Session create or Mode switch that would remove human approval: `bypass` or
/// `dont-ask`, or a Session ruleset (which could allow every tool). The body is read here
/// and handed on unchanged; one the handler cannot parse is left for it to refuse.
async fn checked_body(operation: &str, req: Request) -> Result<Request, Box<Response>> {
    if !matches!(operation, "v1.session.create" | "v1.session.mode") {
        return Ok(req);
    }
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, CHECKED_BODY_LIMIT)
        .await
        .map_err(|_| Box::new(ApiError::invalid("request body is too large").into_response()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    let mode = value["mode"].as_str().unwrap_or_default();
    if UNATTENDED_MODES.contains(&mode) {
        return Err(Box::new(out_of_scope(format!(
            "an IDE token cannot select the {mode} mode"
        ))));
    }
    if operation == "v1.session.create" && !value["rules"].is_null() {
        return Err(Box::new(out_of_scope(
            "an IDE token cannot set Session rules",
        )));
    }
    Ok(Request::from_parts(parts, Body::from(bytes)))
}

/// End an event stream when its token is revoked; other responses pass unchanged.
fn closable(response: Response, cancel: CancellationToken) -> Response {
    let streaming = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    if !streaming {
        return response;
    }
    let (parts, body) = response.into_parts();
    let stream = body.into_data_stream().take_until(cancel.cancelled_owned());
    Response::from_parts(parts, Body::from_stream(stream))
}

fn unauthorized() -> Response {
    let mut response = ApiError::unauthorized().into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"cyber-ide\""),
    );
    response
}

fn out_of_scope(message: impl Into<String>) -> Response {
    ApiError::forbidden(message)
        .with_reason("ide_token_scope")
        .into_response()
}

/// `POST /api/v1/ide/tokens`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateIdeToken {
    /// The editor client; only `vscode`.
    pub client: String,
    /// A label shown by `cyber ide token list`.
    #[serde(default)]
    pub name: Option<String>,
    /// Lifetime in days, 1–90; default 30.
    #[serde(default)]
    pub expires_in_days: Option<u32>,
}

/// A created token. `token` appears only in this response.
#[derive(Debug, Serialize, JsonSchema)]
pub struct IssuedIdeToken {
    /// `itk_…`.
    pub id: String,
    /// `cyi_…`; store it, it is never shown again.
    pub token: String,
    pub client: String,
    pub name: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub expires_at: String,
}

/// A token record; never its value or hash.
#[derive(Debug, Serialize, JsonSchema)]
pub struct IdeTokenView {
    pub id: String,
    pub client: String,
    pub name: Option<String>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub expires_at: String,
    /// RFC 3339; recorded at most once a minute.
    pub last_used_at: Option<String>,
    /// RFC 3339.
    pub revoked_at: Option<String>,
    /// `active`, `revoked` or `expired`.
    pub status: String,
}

impl IdeTokenView {
    fn new(record: IdeToken, now_ms: i64) -> Self {
        let status = if record.revoked_ms.is_some() {
            "revoked"
        } else if record.active(now_ms) {
            "active"
        } else {
            "expired"
        };
        Self {
            id: record.id,
            client: record.client,
            name: record.name,
            created_at: rfc3339(record.created_ms),
            expires_at: rfc3339(record.expires_ms),
            last_used_at: record.last_used_ms.map(rfc3339),
            revoked_at: record.revoked_ms.map(rfc3339),
            status: status.into(),
        }
    }
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/ide/tokens", post(create).get(list))
        .route("/ide/tokens/{tokenID}", delete(revoke))
}

async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateIdeToken>,
) -> Result<(StatusCode, Json<Data<IssuedIdeToken>>), ApiError> {
    let new = NewToken {
        client: request.client,
        name: request.name,
        expires_in_days: request.expires_in_days,
    };
    let issued = ide_tokens::create(&state.store, new, now_ms()).map_err(issue_error)?;
    if let Some(replaced) = &issued.replaced {
        end(&state, std::slice::from_ref(replaced)).await;
    }
    let record = issued.record;
    let data = IssuedIdeToken {
        id: record.id,
        token: issued.token,
        client: record.client,
        name: record.name,
        created_at: rfc3339(record.created_ms),
        expires_at: rfc3339(record.expires_ms),
    };
    Ok((StatusCode::CREATED, Json(Data { data })))
}

async fn list(State(state): State<AppState>) -> Result<Json<Data<Vec<IdeTokenView>>>, ApiError> {
    let now = now_ms();
    let records = ide_tokens::list(&state.store, now).map_err(RuntimeError::from)?;
    Ok(Json(Data {
        data: records
            .into_iter()
            .map(|record| IdeTokenView::new(record, now))
            .collect(),
    }))
}

async fn revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<IdeTokenView>>, ApiError> {
    let now = now_ms();
    let record = ide_tokens::revoke(&state.store, &id, now)
        .map_err(RuntimeError::from)?
        .ok_or_else(|| ApiError::not_found("NotFoundError", format!("no IDE token {id}")))?;
    end(&state, std::slice::from_ref(&record.id)).await;
    Ok(Json(Data {
        data: IdeTokenView::new(record, now),
    }))
}

fn issue_error(error: IdeTokenError) -> ApiError {
    match error {
        IdeTokenError::Store(error) => RuntimeError::from(error).into(),
        IdeTokenError::Random(message) => {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "UnknownError", message)
        }
        other => ApiError::invalid(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_matches_route_templates_by_shape() {
        let get = Method::GET;
        assert_eq!(
            operation(&get, Some("/sessions/{session_id}")),
            Some("v1.session.get")
        );
        assert_eq!(
            operation(&Method::POST, Some("/sessions/{x}/permissions/{y}/reply")),
            Some("v1.permission.reply")
        );
        assert_eq!(operation(&Method::PATCH, Some("/sessions/{x}")), None);
        assert_eq!(operation(&get, Some("/config")), None);
        assert_eq!(operation(&Method::POST, Some("/ide/tokens")), None);
        assert_eq!(operation(&get, None), None);
        // A literal segment never matches a parameter.
        assert_eq!(operation(&get, Some("/sessions/status")), None);
    }

    #[test]
    fn scope_operations_are_documented_with_these_paths() {
        for (method, path, id) in SCOPE {
            let (documented, at) = super::super::openapi::lookup(id).expect(id);
            assert!(documented.eq_ignore_ascii_case(method), "{id}");
            assert_eq!(at, *path, "{id}");
        }
    }

    #[test]
    fn revocation_cancels_streams_and_returns_leases_once() {
        let live = IdeLive::default();
        let cancel = live.cancellation("itk_1");
        assert!(live.add_lease("itk_1", "ses_a", "/r", "icl_1"));
        let leases = live.revoke("itk_1");
        assert_eq!(leases.len(), 1);
        assert!(cancel.is_cancelled());
        assert!(live.cancellation("itk_1").is_cancelled());
        assert!(!live.add_lease("itk_1", "ses_a", "/r", "icl_2"));
        assert!(live.revoke("itk_1").is_empty());
        assert!(!live.cancellation("itk_2").is_cancelled());
    }
}
