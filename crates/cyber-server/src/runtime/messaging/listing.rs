//! Peer Sessions in the reachable listing (`cross-session-messaging` → Peer watches and
//! listing; design section 6).
//!
//! The sending side: `list_sessions` adds, per enabled remote, the entries of that peer's
//! `GET /api/v1/peer/sessions`, fetched in parallel with a 5-second timeout and cached for
//! 15 seconds, as `X@<peer>` entries of kind `peer`; a peer that does not answer (or
//! cannot be asked) is one `*@<peer>` entry with state `offline` that cannot receive.
//!
//! The receiving side: a grant sees only the root Sessions it could message (their
//! inbound decision for `*@<grant>` is not `refuse`), at most 50, located by their
//! workspace name only unless `peers.share_paths` is set, and the status of one of them
//! for its watches. A Session the grant cannot see is not found.

use std::collections::HashMap;
use std::path::Path;
use std::sync::PoisonError;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::address::{AddressKind, SessionLocation};
use super::peer::{PeerClient, PeerTarget};
use super::send::Sender;
use super::{Messaging, PeerError, ReachableSession, ReachableState, RuntimeError};

/// How long the listing waits for one peer.
pub const LISTING_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a peer's entries are reused.
pub const LISTING_CACHE: Duration = Duration::from_secs(15);
/// The most entries a peer lists.
pub const PEER_LISTING_LIMIT: usize = 50;
/// The Session a grant-wide question is asked for: `*@<grant>`, which every pattern that
/// names the whole grant matches.
const ANY_SESSION: &str = "*";

/// Cached peer entries, keyed by the remote's name, URL and fingerprint (so an edited
/// remote is asked again).
pub(crate) type ListingCache = HashMap<String, (Instant, Vec<ReachableSession>)>;

/// `GET /api/v1/peer/sessions/:id/status`: what a peer Session is doing, for watches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PeerSessionStatus {
    pub session_id: String,
    pub state: ReachableState,
    /// When its last run ended (milliseconds since the epoch on that machine).
    pub last_idle_ms: Option<i64>,
}

/// The listing a peer answers.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PeerSessions {
    /// Addressed as this machine names them (`<name or ses_>`, no `@`); the asker adds
    /// `@<peer>`.
    #[schemars(with = "Vec<ReachableSession>")]
    pub sessions: Vec<Value>,
}

/// An enabled remote and whether it answered its last listing (`GET /api/v1/peers/remotes`,
/// the TUI sidebar's Peers line).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct RemoteReach {
    pub name: String,
    /// It answered within [`LISTING_TIMEOUT`] (the answer may be up to [`LISTING_CACHE`]
    /// old); `false` also when it has no token.
    pub online: bool,
}

/// The one entry of a peer that did not answer.
pub fn offline(peer: &str) -> ReachableSession {
    let address = format!("{ANY_SESSION}@{peer}");
    ReachableSession {
        address: address.clone(),
        session_id: address,
        name: peer.to_owned(),
        kind: AddressKind::Peer,
        state: ReachableState::Offline,
        location: SessionLocation {
            directory: String::new(),
            workspace: None,
        },
        can_receive: false,
    }
}

/// An entry a peer listed, addressed from here (`X@<peer>`). Entries this version cannot
/// read are skipped.
fn remote_entry(peer: &str, entry: Value) -> Option<ReachableSession> {
    let entry: ReachableSession = serde_json::from_value(entry).ok()?;
    if entry.state == ReachableState::Offline || entry.address.contains('@') {
        return None;
    }
    Some(ReachableSession {
        address: format!("{}@{peer}", entry.address),
        session_id: format!("{}@{peer}", entry.session_id),
        kind: AddressKind::Peer,
        ..entry
    })
}

/// The workspace name of a Location: its last path component.
fn workspace_name(location: &SessionLocation) -> String {
    location.workspace.clone().unwrap_or_else(|| {
        Path::new(&location.directory)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    })
}

/// A Location as a grant is shown it: the workspace name only, unless `share_paths`.
pub(super) fn shown_location(location: SessionLocation, share_paths: bool) -> SessionLocation {
    if share_paths {
        return location;
    }
    SessionLocation {
        directory: workspace_name(&location),
        workspace: None,
    }
}

/// The sender a grant-wide question is asked for: `*@<grant>`.
fn any_session_of(grant: &str) -> Sender {
    Sender::Peer {
        grant: grant.to_owned(),
        session_id: ANY_SESSION.into(),
        name: None,
    }
}

fn cache_key(target: &PeerTarget) -> String {
    format!("{}\0{}\0{}", target.name, target.url, target.fingerprint)
}

/// `/sessions/<id>/status` with `id` percent-encoded as one path segment.
fn status_path(id: &str) -> String {
    let mut path = String::from("/sessions/");
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            path.push(char::from(byte));
        } else {
            path.push_str(&format!("%{byte:02X}"));
        }
    }
    path.push_str("/status");
    path
}

/// Ask `target` for its Sessions.
async fn fetch(target: PeerTarget) -> Result<Vec<ReachableSession>, PeerError> {
    let peer = target.name.clone();
    let client = PeerClient::new(target)?;
    let path = format!("/sessions?limit={PEER_LISTING_LIMIT}");
    let (status, body) = client.request(reqwest::Method::GET, &path, None).await?;
    let listed = (status == 200)
        .then(|| serde_json::from_value::<PeerSessions>(body).ok())
        .flatten()
        .ok_or_else(|| PeerError::Unreachable {
            peer: peer.clone(),
            cause: format!("the Session listing answered HTTP {status}"),
        })?;
    Ok(listed
        .sessions
        .into_iter()
        .filter_map(|entry| remote_entry(&peer, entry))
        .take(PEER_LISTING_LIMIT)
        .collect())
}

impl Messaging {
    /// [`Self::reachable_as`] with the Sessions of every enabled peer after this server's
    /// own (`list_sessions`, `GET /messaging/sessions`). A `directory` filter names a
    /// Location of this machine, so it lists no peer Sessions.
    pub async fn list_as(
        &self,
        sender: &Sender,
        directory: Option<&str>,
    ) -> Result<Vec<ReachableSession>, RuntimeError> {
        let mut sessions = self.reachable_as(sender, directory)?;
        if directory.is_none() {
            sessions.extend(self.peer_sessions().await);
        }
        Ok(sessions)
    }

    /// [`Self::list_as`] for a Session of this server, or the `api` client.
    pub async fn list(
        &self,
        from: Option<&str>,
        directory: Option<&str>,
    ) -> Result<Vec<ReachableSession>, RuntimeError> {
        let sender = match from {
            Some(id) => Sender::Session(id.to_owned()),
            None => Sender::Client(super::send::DEFAULT_CLIENT.into()),
        };
        self.list_as(&sender, directory).await
    }

    /// Every enabled remote, in config order, online when its listing answered. It shares
    /// the listing's cache, so it asks a peer at most once per [`LISTING_CACHE`].
    pub async fn remotes(&self) -> Vec<RemoteReach> {
        let names = self.enabled_peers();
        let listings = names.iter().map(|name| self.peer_listing(name));
        let listings = futures::future::join_all(listings).await;
        names
            .into_iter()
            .zip(listings)
            .map(|(name, entries)| {
                let silent = offline(&name).address;
                let online = !entries
                    .iter()
                    .any(|e| e.state == ReachableState::Offline && e.address == silent);
                RemoteReach { name, online }
            })
            .collect()
    }

    /// The entries of every enabled peer, asked in parallel.
    async fn peer_sessions(&self) -> Vec<ReachableSession> {
        let names = self.enabled_peers();
        let listings = names.iter().map(|name| self.peer_listing(name));
        futures::future::join_all(listings)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// The enabled remotes' names; unreadable `peers` settings list none.
    fn enabled_peers(&self) -> Vec<String> {
        match self.runtime.inner.tools.peer_names() {
            Ok(names) => names,
            Err(error) => {
                cyber_core::log::warn(
                    "messaging",
                    "peer listing skipped",
                    serde_json::json!({ "error": error }),
                );
                Vec::new()
            }
        }
    }

    /// One peer's entries: cached for [`LISTING_CACHE`], else asked with a
    /// [`LISTING_TIMEOUT`]; [`offline`] when it does not answer or has no token.
    async fn peer_listing(&self, name: &str) -> Vec<ReachableSession> {
        let Ok(target) = self.peer_target(name) else {
            return vec![offline(name)];
        };
        let key = cache_key(&target);
        if let Some(cached) = self.cached_listing(&key) {
            return cached;
        }
        let entries = match tokio::time::timeout(LISTING_TIMEOUT, fetch(target)).await {
            Ok(Ok(entries)) => entries,
            Ok(Err(_)) | Err(_) => vec![offline(name)],
        };
        let mut cache = self
            .runtime
            .inner
            .peer_listings
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache.retain(|_, (at, _)| at.elapsed() < LISTING_CACHE);
        cache.insert(key, (Instant::now(), entries.clone()));
        entries
    }

    fn cached_listing(&self, key: &str) -> Option<Vec<ReachableSession>> {
        let cache = self
            .runtime
            .inner
            .peer_listings
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        cache
            .get(key)
            .filter(|(at, _)| at.elapsed() < LISTING_CACHE)
            .map(|(_, entries)| entries.clone())
    }

    /// `GET /api/v1/peer/sessions` for grant `grant`: the root Sessions it could message,
    /// newest first, at most `limit` (and [`PEER_LISTING_LIMIT`]). Locations show only
    /// the workspace name unless `share_paths`.
    pub fn granted_sessions(
        &self,
        grant: &str,
        share_paths: bool,
        limit: usize,
    ) -> Result<Vec<ReachableSession>, RuntimeError> {
        Ok(self
            .reachable_as(&any_session_of(grant), None)?
            .into_iter()
            .filter(|session| session.can_receive && session.kind == AddressKind::Local)
            .take(limit.min(PEER_LISTING_LIMIT))
            .map(|mut session| {
                session.location = shown_location(session.location, share_paths);
                session
            })
            .collect())
    }

    /// The root Session `id` names, when grant `grant` is shown it: one it could message
    /// (its inbound decision for `*@<grant>` is not `refuse`), listed or not.
    pub(super) fn shown_to(
        &self,
        grant: &str,
        id: &str,
    ) -> Result<Option<ReachableSession>, RuntimeError> {
        Ok(self
            .reachable_one_as(&any_session_of(grant), id)?
            .filter(|session| session.can_receive))
    }

    /// `GET /api/v1/peer/sessions/:id/status` for grant `grant`: Session `id` (its ID or a
    /// name that resolves to one root Session) and what it is doing; `None` when the grant
    /// cannot see it. Any root Session the grant could message is found, not only those
    /// [`Self::granted_sessions`] lists.
    pub async fn granted_status(
        &self,
        grant: &str,
        id: &str,
    ) -> Result<Option<PeerSessionStatus>, RuntimeError> {
        let Some(session) = self.shown_to(grant, id)? else {
            return Ok(None);
        };
        let handle = self.runtime.inner.handle(&session.session_id).await?;
        let last_idle_ms = super::watch::last_run(&*handle.state.lock().await).1;
        Ok(Some(PeerSessionStatus {
            session_id: session.session_id,
            state: session.state,
            last_idle_ms,
        }))
    }

    /// The status of Session `id` of peer `peer` (`None`: the peer does not show it to
    /// this machine).
    pub(super) async fn remote_status(
        &self,
        peer: &str,
        id: &str,
    ) -> Result<Option<PeerSessionStatus>, RuntimeError> {
        let target = self.peer_target(peer)?;
        let client = PeerClient::new(target).map_err(RuntimeError::Peer)?;
        let (status, body) = client
            .request(reqwest::Method::GET, &status_path(id), None)
            .await
            .map_err(RuntimeError::Peer)?;
        match status {
            200 => serde_json::from_value(body).map(Some).map_err(|_| {
                RuntimeError::Peer(PeerError::Unreachable {
                    peer: peer.to_owned(),
                    cause: "the Session status answered an unexpected body".into(),
                })
            }),
            404 => Ok(None),
            _ => Err(RuntimeError::Peer(PeerError::Unreachable {
                peer: peer.to_owned(),
                cause: format!("the Session status answered HTTP {status}"),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_entries_are_addressed_through_the_peer() {
        let entry = serde_json::json!({"address": "migration", "session_id": "ses_1",
            "name": "migration", "kind": "local", "state": "running",
            "location": {"directory": "api"}, "canReceive": true});
        let listed = remote_entry("desk", entry).unwrap();
        assert_eq!(
            (listed.address.as_str(), listed.session_id.as_str()),
            ("migration@desk", "ses_1@desk")
        );
        assert_eq!(
            (listed.kind, listed.state),
            (AddressKind::Peer, ReachableState::Running)
        );
        let further = serde_json::json!({"address": "x@nas", "session_id": "ses_2",
            "name": "x", "kind": "peer", "state": "idle",
            "location": {"directory": ""}, "canReceive": true});
        assert!(remote_entry("desk", further).is_none());
        assert!(remote_entry("desk", serde_json::json!({"address": "y"})).is_none());
        let down = offline("desk");
        assert_eq!(
            (
                down.address.as_str(),
                down.state,
                down.can_receive,
                down.kind
            ),
            ("*@desk", ReachableState::Offline, false, AddressKind::Peer)
        );
    }

    #[test]
    fn locations_keep_only_the_workspace_name_and_ids_are_one_segment() {
        let location = SessionLocation {
            directory: "/home/me/work/api".into(),
            workspace: None,
        };
        assert_eq!(workspace_name(&location), "api");
        assert_eq!(status_path("ses_1"), "/sessions/ses_1/status");
        assert_eq!(status_path("a b/c"), "/sessions/a%20b%2Fc/status");
    }
}
