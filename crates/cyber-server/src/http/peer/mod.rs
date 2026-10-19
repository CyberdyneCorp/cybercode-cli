//! The peer listener's router (`server-api` → Peer listener): only `/api/v1/peer/*`, for
//! other `cyber` servers the same person controls.
//!
//! It shares nothing with the main API's router: the local password, the Unix socket's
//! credentials and the main routes are not reachable here, every other path is `404`, and
//! any request with an `Origin` header is refused with `403` so a browser page on the LAN
//! cannot use the listener. Every peer route, known or not, first passes the peer guard
//! ([`guard`]): an active grant's `Bearer cyp_…` token, then the route's scope.

pub mod attach;
pub mod guard;
pub mod handle;
mod messages;
mod sessions;
pub mod tls;

use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use axum::extract::{Extension, Path, Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::Serialize;

use super::AppState;
use super::error::ApiError;
use crate::runtime::messaging::PeerScope;
use crate::runtime::peers::Grant;

/// Longest machine name, as for `peers.machine`.
const MACHINE_MAX: usize = 64;

/// What this listener tells peers about itself.
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// The name shown to peers ([`machine_name`]).
    pub machine: String,
    /// `sha256:<hex>` of the listener's SubjectPublicKeyInfo.
    pub fingerprint: String,
    /// `peers.share_paths`: listings show full Location paths, not only workspace names.
    pub share_paths: bool,
    /// How long an attach waits for `remote.attach` ([`ATTACH_WAIT`]).
    pub attach_wait: Duration,
}

pub use crate::runtime::peers::attach::ATTACH_WAIT;

pub use handle::PeerHandle;

/// The state of the peer routes.
#[derive(Clone)]
pub struct PeerState {
    pub app: AppState,
    /// Shared with the [`PeerHandle`], which replaces the fingerprint on a key rotation.
    pub info: Arc<RwLock<PeerInfo>>,
    limiter: Arc<guard::GrantLimiter>,
    /// Failed tokens per remote address.
    failures: Arc<guard::GrantLimiter>,
    attaches: Arc<attach::Attaches>,
}

impl PeerState {
    pub fn info(&self) -> PeerInfo {
        self.info
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The peer listener's router.
pub fn router(state: AppState, info: PeerInfo) -> Router {
    shared_router(state, Arc::new(RwLock::new(info)), Arc::default())
}

/// The router of a running listener, whose identity its [`PeerHandle`] can rotate.
/// Its attaches are the handle's, so a revocation through the main API ends them.
pub fn handle_router(state: AppState, handle: &PeerHandle) -> Router {
    shared_router(state, handle.shared_info(), handle.attaches())
}

fn shared_router(
    state: AppState,
    info: Arc<RwLock<PeerInfo>>,
    attaches: Arc<attach::Attaches>,
) -> Router {
    let peer = PeerState {
        app: state,
        info,
        limiter: Arc::default(),
        failures: Arc::default(),
        attaches,
    };
    let one = "/api/v1/peer/attach/{id}";
    let routes = Router::new()
        .route("/api/v1/peer/hello", get(hello))
        .route("/api/v1/peer/messages", post(messages::send))
        .route("/api/v1/peer/messages/{message_id}", get(messages::outcome))
        .route("/api/v1/peer/sessions", get(sessions::list))
        .route("/api/v1/peer/sessions/{id}/status", get(sessions::status))
        .route(one, post(attach::open).delete(attach::close))
        .route(&format!("{one}/session"), get(attach::session))
        .route(&format!("{one}/messages"), get(attach::messages))
        .route(&format!("{one}/events"), get(attach::events))
        .route(&format!("{one}/prompt"), post(attach::prompt))
        .route(&format!("{one}/interrupt"), post(attach::interrupt))
        .route(&format!("{one}/permissions"), get(attach::permissions))
        .route(
            &format!("{one}/permissions/{{request_id}}/reply"),
            post(attach::reply),
        )
        .route("/api/v1/peer/{*route}", any(peer_not_found))
        .route_layer(axum::middleware::from_fn_with_state(
            peer.clone(),
            guard::authenticate,
        ))
        .with_state(peer);
    routes
        .fallback(not_found)
        .layer(axum::middleware::from_fn(refuse_browsers))
}

/// `403` for any request with an `Origin` header, before routing or authentication.
async fn refuse_browsers(req: Request, next: Next) -> Response {
    if req.headers().contains_key(header::ORIGIN) {
        return ApiError::forbidden("the peer listener refuses browser requests").into_response();
    }
    next.run(req).await
}

/// `GET /api/v1/peer/hello`: who this machine is and what the caller's grant allows.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Hello {
    pub machine: String,
    pub version: String,
    pub fingerprint: String,
    pub scopes: Vec<PeerScope>,
    /// The caller's name on this machine.
    pub grant: String,
}

async fn hello(State(peer): State<PeerState>, Extension(grant): Extension<Grant>) -> Json<Hello> {
    let info = peer.info();
    Json(Hello {
        machine: info.machine,
        version: peer.app.options.version.clone(),
        fingerprint: info.fingerprint,
        scopes: grant.scopes,
        grant: grant.name,
    })
}

/// `404` for an unknown peer route; under `attach/` the scope is checked first.
async fn peer_not_found(
    Extension(grant): Extension<Grant>,
    Path(route): Path<String>,
) -> Result<Response, ApiError> {
    if route == "attach" || route.starts_with("attach/") {
        guard::require(&grant, PeerScope::Attach)?;
    }
    Err(ApiError::not_found("NotFoundError", "no such peer route"))
}

async fn not_found() -> Response {
    ApiError::not_found(
        "NotFoundError",
        "the peer listener serves only /api/v1/peer/*",
    )
    .into_response()
}

/// The name shown to peers: `peers.machine`, else this host's name reduced to the
/// characters a name after `@` may hold, without a `.local` suffix; `cyber` when the host
/// name is unknown.
pub fn machine_name(configured: Option<&str>) -> String {
    if let Some(name) = configured {
        return name.to_string();
    }
    sysinfo::System::host_name()
        .map(|host| host_label(&host))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "cyber".into())
}

/// The name a listener bound to every interface is reached by on the local network, for
/// its pairing codes: this host's own name, with `.local` when it has no domain (the name
/// the operating system's mDNS responder answers for), else `<machine>.local`.
/// `peers.machine` names this machine to its peers and may differ from the host name, so
/// it is used only when the host name is unknown or not a host name.
pub fn lan_host(machine: &str) -> String {
    sysinfo::System::host_name()
        .and_then(|host| lan_name(&host))
        .unwrap_or_else(|| format!("{machine}.local"))
}

/// `host` as a LAN host name: `.local` added when it has no domain; `None` when it is not
/// a DNS host name (labels of letters, digits and `-`).
pub fn lan_name(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.');
    let label = |label: &str| {
        (1..=63).contains(&label.len())
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
    };
    if host.len() > 253 || !host.split('.').all(label) {
        return None;
    }
    Some(if host.contains('.') {
        host.to_string()
    } else {
        format!("{host}.local")
    })
}

fn host_label(host: &str) -> String {
    let host = host.strip_suffix(".local").unwrap_or(host);
    let label: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(MACHINE_MAX)
        .collect();
    label
        .trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_names_become_machine_names() {
        assert_eq!(host_label("Leos-MacBook.local"), "Leos-MacBook");
        assert_eq!(host_label("my desk"), "my-desk");
        assert_eq!(host_label("-_box"), "box");
        assert_eq!(host_label(&"a".repeat(80)).len(), MACHINE_MAX);
        assert_eq!(machine_name(Some("desk")), "desk");
        assert!(!machine_name(None).is_empty());
    }

    #[test]
    fn the_lan_host_is_the_host_name_on_the_local_domain() {
        assert_eq!(
            lan_name("Leos-MacBook").as_deref(),
            Some("Leos-MacBook.local")
        );
        assert_eq!(lan_name("box.local.").as_deref(), Some("box.local"));
        assert_eq!(lan_name("desk.lan").as_deref(), Some("desk.lan"));
        for bad in ["", "my desk", "a..b", "-x", "a_b", "x/y"] {
            assert_eq!(lan_name(bad), None, "{bad}");
        }
        let host = lan_host("desk");
        assert!(host.contains('.'), "{host}");
        assert_eq!(lan_name(&host).as_deref(), Some(host.as_str()));
    }
}
