//! Peer grant management on the main API (`cli-commands` → Peers command): what
//! `cyber peers invite`, `revoke` and `list` call on the local server. These routes are
//! local (the main API never leaves loopback); the peer listener serves none of them.

use std::net::IpAddr;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::AppState;
use super::envelope::Data;
use super::error::ApiError;
use super::peer::PeerHandle;
use super::peer::guard::now_ms;
use crate::runtime::RuntimeError;
use crate::runtime::messaging::{PeerScope, RemoteReach};
use crate::runtime::peers::{Grant, GrantError, NewGrant, PairingCode, grants};

/// The running peer listener.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PeerListenerInfo {
    /// `https://<bound address>`, or `http://` for a plain listener on loopback.
    pub url: String,
    pub machine: String,
    /// `sha256:<hex>` of the listener's SubjectPublicKeyInfo.
    pub fingerprint: String,
    pub tls: bool,
}

/// A grant as listed; never its token or hash.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GrantView {
    pub id: String,
    /// The peer's name on this machine; its senders are named `<session>@<name>`.
    pub name: String,
    pub scopes: Vec<PeerScope>,
    pub created_ms: i64,
    pub expires_ms: Option<i64>,
    pub last_used_ms: Option<i64>,
    pub revoked_ms: Option<i64>,
    /// Neither revoked nor expired.
    pub active: bool,
}

impl GrantView {
    fn new(grant: Grant, now_ms: i64) -> Self {
        Self {
            active: grant.active(now_ms),
            id: grant.id,
            name: grant.name,
            scopes: grant.scopes,
            created_ms: grant.created_ms,
            expires_ms: grant.expires_ms,
            last_used_ms: grant.last_used_ms,
            revoked_ms: grant.revoked_ms,
        }
    }
}

/// `GET /api/v1/peers`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PeersOverview {
    /// `None` when no peer listener runs (`peers.listen` unset, or it could not open).
    pub listener: Option<PeerListenerInfo>,
    /// Every grant, oldest first, revoked and expired ones included.
    pub grants: Vec<GrantView>,
}

/// `POST /api/v1/peers/grants`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvitePeer {
    /// The peer's name on this machine (what follows `@` in its senders' addresses).
    pub name: String,
    /// Default `["message"]`.
    #[serde(default)]
    pub scopes: Vec<PeerScope>,
    /// Unix milliseconds; without it the grant does not expire.
    #[serde(default)]
    pub expires_ms: Option<i64>,
    /// Replace the listener's key first; every existing pairing must then be redone.
    #[serde(default)]
    pub rotate_key: bool,
    /// The host the pairing code names; default the bound IP, or this host's LAN name
    /// (its host name on `.local`) for a listener on all interfaces.
    #[serde(default)]
    pub host: Option<String>,
}

/// The created grant and its pairing code, which carries the token and is shown only here.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PeerInvitation {
    pub grant: GrantView,
    /// `cyberpeer://<host>:<port>/?machine=…&fp=…&t=cyp_…`, for `cyber peers add`.
    pub code: String,
    pub listener: PeerListenerInfo,
}

/// `DELETE /api/v1/peers/grants/:name`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PeerRevoked {
    pub name: String,
    /// `false` when no active grant had that name.
    pub revoked: bool,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/peers", get(overview))
        .route("/peers/remotes", get(remotes))
        .route("/peers/grants", post(invite))
        .route("/peers/grants/{name}", delete(revoke))
}

fn listener(state: &AppState) -> Option<&PeerHandle> {
    state.service.as_ref()?.peer.as_deref()
}

fn listener_info(handle: &PeerHandle) -> PeerListenerInfo {
    let info = handle.info();
    PeerListenerInfo {
        url: handle.url(),
        machine: info.machine,
        fingerprint: info.fingerprint,
        tls: handle.tls,
    }
}

async fn overview(State(state): State<AppState>) -> Result<Json<Data<PeersOverview>>, ApiError> {
    let now = now_ms();
    let grants = grants::list(&state.store).map_err(RuntimeError::from)?;
    Ok(Json(Data {
        data: PeersOverview {
            listener: listener(&state).map(listener_info),
            grants: grants
                .into_iter()
                .map(|grant| GrantView::new(grant, now))
                .collect(),
        },
    }))
}

/// `GET /api/v1/peers/remotes`: the enabled remotes and whether each answered.
async fn remotes(State(state): State<AppState>) -> Json<Data<Vec<RemoteReach>>> {
    Json(Data {
        data: state.runtime.messaging().remotes().await,
    })
}

async fn invite(
    State(state): State<AppState>,
    Json(request): Json<InvitePeer>,
) -> Result<(StatusCode, Json<Data<PeerInvitation>>), ApiError> {
    let handle = listener(&state).ok_or_else(|| {
        ApiError::conflict(
            "no peer listener is running: set peers.listen in user config, then run \
             `cyber service restart`",
        )
    })?;
    let host = match request.host.as_deref() {
        Some(host) => valid_host(host)?,
        None => default_host(handle),
    };
    let new = NewGrant {
        name: request.name,
        scopes: request.scopes,
        expires_ms: request.expires_ms,
    };
    let now = now_ms();
    let issued = grants::create(&state.store, new, now).map_err(grant_error)?;
    // After the grant, so a refused grant never costs the existing pairings their key.
    if request.rotate_key
        && let Err(error) = handle.rotate_key()
    {
        grants::revoke(&state.store, &issued.grant.name, now).map_err(RuntimeError::from)?;
        return Err(ApiError::unknown(error));
    }
    let listener = listener_info(handle);
    let code = PairingCode {
        authority: format!("{host}:{}", handle.address.port()),
        tls: handle.tls,
        machine: listener.machine.clone(),
        fingerprint: listener.fingerprint.clone(),
        token: issued.token,
    };
    let invitation = PeerInvitation {
        grant: GrantView::new(issued.grant, now),
        code: code.to_string(),
        listener,
    };
    Ok((StatusCode::CREATED, Json(Data { data: invitation })))
}

async fn revoke(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Data<PeerRevoked>>, ApiError> {
    let revoked = grants::revoke(&state.store, &name, now_ms()).map_err(RuntimeError::from)?;
    // Its attaches end now, open event streams included, not at their next request.
    if let Some(handle) = listener(&state) {
        handle.end_attaches(&name);
    }
    Ok(Json(Data {
        data: PeerRevoked { name, revoked },
    }))
}

fn grant_error(error: GrantError) -> ApiError {
    match error {
        GrantError::NameInUse(_) => ApiError::conflict(error.to_string()),
        GrantError::InvalidName(_) | GrantError::ExpiryInPast => {
            ApiError::invalid(error.to_string())
        }
        GrantError::Random(_) => ApiError::unknown(error),
        GrantError::Store(error) => RuntimeError::from(error).into(),
    }
}

/// The bound IP, or this host's LAN name ([`super::peer::lan_host`], what its mDNS
/// record also advertises) for a listener on all interfaces.
fn default_host(handle: &PeerHandle) -> String {
    match handle.address.ip() {
        ip if ip.is_unspecified() => super::peer::lan_host(&handle.info().machine),
        IpAddr::V6(ip) => format!("[{ip}]"),
        IpAddr::V4(ip) => ip.to_string(),
    }
}

/// A host name or IP for the pairing code; an IPv6 address is bracketed.
fn valid_host(host: &str) -> Result<String, ApiError> {
    let host = match host.parse::<std::net::Ipv6Addr>() {
        Ok(ip) => format!("[{ip}]"),
        Err(_) => host.to_string(),
    };
    let parsed = url::Url::parse(&format!("https://{host}:1/"));
    let valid = !host.contains(['/', '@', '?', '#', ':']) || host.starts_with('[');
    match parsed {
        Ok(url) if valid && url.port() == Some(1) && url.path() == "/" => Ok(host),
        _ => Err(ApiError::invalid(format!(
            "host {host:?} is not a host name or IP address"
        ))),
    }
}
