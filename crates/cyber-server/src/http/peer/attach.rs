//! `/api/v1/peer/attach/:id/*` (scope `attach`): one Session of this machine, opened by a
//! peer's `cyber attach <session>@<machine>` (`cross-session-messaging` → Peer attach).
//!
//! `POST /attach/:id` raises `remote.attach` in that Session (resource `peer:<grant>`) and
//! waits for the user's answer; only an approval issues an attach ID, which every other
//! attach route needs in `X-Cyber-Attach` for that Session and that grant. Before it,
//! nothing of the Session is sent. With it the peer reaches only the Session's state and
//! transcript, its event stream, the prompt (text and images, as the user), interrupt and
//! the replies to its permission requests: never other Sessions, config or the shell, and
//! never an answer to an attach request. A reply approves `once` or rejects: `always`
//! would save a project-wide approval that outlives the attach and its grant, so it is
//! refused. The Session keeps its own rules and Mode.
//! An attach ends when the peer closes it, after an hour without requests while no event
//! stream of it is open, when its grant is revoked or expires, or when the server stops.
//! Ending it also ends its open event streams at once: a revocation through the main API
//! (`DELETE /api/v1/peers/grants/:name`) cancels them, and every stream re-reads its grant
//! every [`GRANT_RECHECK`] so an expiry or any other revocation ends it too.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use cyber_llm::Content;
use futures::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{PeerState, guard};
use crate::http::envelope::{Data, query_value};
use crate::http::error::ApiError;
use crate::http::sessions::{MessagesQuery, PermissionReplyBody, PromptBody, ReplyKind};
use crate::runtime::messaging::PeerScope;
use crate::runtime::peers::attach::REMOTE_ATTACH;
use crate::runtime::peers::{Grant, grants};
use crate::runtime::{Delivery, PendingKind, PendingRequest, RuntimeError};

/// The header carrying the attach ID.
pub const ATTACH_HEADER: &str = "x-cyber-attach";
/// An attach without requests (and without an open event stream) for this long ends.
const IDLE: Duration = Duration::from_secs(60 * 60);
/// How often an open event stream re-reads its grant.
pub const GRANT_RECHECK: Duration = Duration::from_secs(2);

/// The approved attaches of this listener.
#[derive(Default)]
pub struct Attaches {
    open: Mutex<HashMap<String, Attached>>,
}

struct Attached {
    grant_id: String,
    grant_name: String,
    session_id: String,
    last_used: Instant,
    /// Open event streams: an attach being watched is in use.
    streams: usize,
    /// Cancelled when the attach ends; its event streams end with it.
    ended: CancellationToken,
}

impl Attached {
    fn end(&self) {
        self.ended.cancel();
    }
}

impl Attaches {
    fn open(&self, grant: &Grant, session_id: &str) -> String {
        let id = cyber_core::ids::new_id("pat");
        let attached = Attached {
            grant_id: grant.id.clone(),
            grant_name: grant.name.clone(),
            session_id: session_id.to_owned(),
            last_used: Instant::now(),
            streams: 0,
            ended: CancellationToken::new(),
        };
        self.lock().insert(id.clone(), attached);
        id
    }

    /// Whether attach `id` is open for `grant` on `session_id`; a use keeps it open.
    fn admits(&self, id: &str, grant: &Grant, session_id: &str, now: Instant) -> bool {
        self.admit(id, grant, session_id, now).is_some()
    }

    /// [`Self::admits`], returning the attach's end token.
    fn admit(
        &self,
        id: &str,
        grant: &Grant,
        session_id: &str,
        now: Instant,
    ) -> Option<CancellationToken> {
        let mut open = self.lock();
        open.retain(|_, attached| {
            let kept = attached.streams > 0 || now.duration_since(attached.last_used) < IDLE;
            if !kept {
                attached.end();
            }
            kept
        });
        let attached = open.get_mut(id)?;
        if attached.grant_id != grant.id || attached.session_id != session_id {
            return None;
        }
        attached.last_used = now;
        Some(attached.ended.clone())
    }

    /// Count an event stream of attach `id` as open until the returned guard drops.
    fn watch(self: &Arc<Self>, id: &str, grant: &Grant, session_id: &str) -> Option<Watching> {
        let ended = self.admit(id, grant, session_id, Instant::now())?;
        if let Some(attached) = self.lock().get_mut(id) {
            attached.streams += 1;
        }
        Some(Watching {
            attaches: Arc::clone(self),
            id: id.to_owned(),
            stream: ended.child_token(),
        })
    }

    fn close(&self, id: &str, grant: &Grant) -> bool {
        let mut open = self.lock();
        let owned = open
            .get(id)
            .is_some_and(|attached| attached.grant_id == grant.id);
        if !owned {
            return false;
        }
        open.remove(id).inspect(Attached::end).is_some()
    }

    /// End every attach of the grant named `name` (it was revoked), with its open event
    /// streams; returns how many ended.
    pub fn end_grant(&self, name: &str) -> usize {
        let mut open = self.lock();
        let before = open.len();
        open.retain(|_, attached| {
            let kept = attached.grant_name != name;
            if !kept {
                attached.end();
            }
            kept
        });
        before - open.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Attached>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One open event stream of an attach. Dropping it (the stream ended or the peer hung up)
/// stops its grant checks and counts the attach as used from then on.
struct Watching {
    attaches: Arc<Attaches>,
    id: String,
    /// Cancelled with the attach, by a failed grant check, or on drop.
    stream: CancellationToken,
}

impl Drop for Watching {
    fn drop(&mut self) {
        self.stream.cancel();
        if let Some(attached) = self.attaches.lock().get_mut(&self.id) {
            attached.streams = attached.streams.saturating_sub(1);
            attached.last_used = Instant::now();
        }
    }
}

/// End `stream` once grant `grant_id` is no longer active (revoked, expired, removed or
/// unreadable), checking every [`GRANT_RECHECK`] until the stream ends.
fn recheck_grant(peer: &PeerState, grant_id: String, stream: CancellationToken) {
    let store = Arc::clone(&peer.app.store);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = stream.cancelled() => return,
                () = tokio::time::sleep(GRANT_RECHECK) => {}
            }
            let (store, id) = (Arc::clone(&store), grant_id.clone());
            let now = guard::now_ms();
            let active = tokio::task::spawn_blocking(move || grants::active(&store, &id, now))
                .await
                .is_ok_and(|active| matches!(active, Ok(true)));
            if !active {
                stream.cancel();
                return;
            }
        }
    });
}

/// `POST /api/v1/peer/attach/:id`: an approved attach.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Attach {
    /// Sent as `X-Cyber-Attach` on the other attach routes.
    pub attach_id: String,
    pub session_id: String,
}

/// Ask the Session's user; `PermissionDeniedError` unless approved within the wait.
pub(super) async fn open(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<String>,
) -> Result<Json<Attach>, ApiError> {
    guard::require(&grant, PeerScope::Attach)?;
    let runtime = &peer.app.runtime;
    let shown = runtime
        .messaging()
        .granted_status(&grant.name, &id)
        .await?
        .ok_or_else(|| hidden(&id))?;
    let wait = peer.info().attach_wait;
    runtime
        .approve_attach(&shown.session_id, &grant.name, wait)
        .await?;
    let attach_id = peer.attaches.open(&grant, &shown.session_id);
    Ok(Json(Attach {
        attach_id,
        session_id: shown.session_id,
    }))
}

/// `DELETE /api/v1/peer/attach/:id`: end this attach.
pub(super) async fn close(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    guard::require(&grant, PeerScope::Attach)?;
    let id = attach_id(&headers).unwrap_or_default();
    if peer.attaches.close(id, &grant) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_attached())
    }
}

/// The scope, then an open attach of this grant to Session `session_id`.
fn admitted(
    peer: &PeerState,
    grant: &Grant,
    headers: &HeaderMap,
    session_id: &str,
) -> Result<(), ApiError> {
    guard::require(grant, PeerScope::Attach)?;
    let id = attach_id(headers).unwrap_or_default();
    if peer.attaches.admits(id, grant, session_id, Instant::now()) {
        Ok(())
    } else {
        Err(not_attached())
    }
}

/// `GET /attach/:id/session`: the Session as `GET /sessions/:id` shows it.
pub(super) async fn session(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    admitted(&peer, &grant, &headers, &id)?;
    let shown = crate::http::sessions::show(State(peer.app), Path(id)).await?;
    Ok(shown.into_response())
}

/// `GET /attach/:id/messages`: the transcript, as `GET /sessions/:id/messages`.
pub(super) async fn messages(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: Query<MessagesQuery>,
) -> Result<Response, ApiError> {
    admitted(&peer, &grant, &headers, &id)?;
    let page = crate::http::sessions::messages(State(peer.app), Path(id), query).await?;
    Ok(page.into_response())
}

/// `GET /attach/:id/events?after=`: the Session's durable event stream after `after`;
/// without it, its durable and live events from now on, as the instance stream carries
/// them to a local TUI. The stream ends when the attach does, or its grant stops being
/// active.
pub(super) async fn events(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    guard::require(&grant, PeerScope::Attach)?;
    let attach = attach_id(&parts.headers).unwrap_or_default();
    let watching = peer
        .attaches
        .watch(attach, &grant, &id)
        .ok_or_else(not_attached)?;
    let response = match parts.uri.query().and_then(|q| query_value(q, "after")) {
        // A replay: the Session stream after `after`.
        Some(after) => {
            let (mut forwarded, ()) = axum::http::Request::builder()
                .uri(format!("/sessions/{id}/events?after={after}"))
                .body(())
                .map_err(|error| ApiError::invalid(error.to_string()))?
                .into_parts();
            forwarded.extensions = parts.extensions;
            crate::http::events::session_events(State(peer.app.clone()), Path(id), forwarded)
                .await?
        }
        // The attached TUI's instance stream: live events too, or a run never ends there.
        None => crate::http::events::session_instance_events(peer.app.clone(), id).await?,
    };
    recheck_grant(&peer, grant.id, watching.stream.clone());
    let (head, body) = response.into_parts();
    let ended = watching.stream.clone().cancelled_owned();
    let body = body
        .into_data_stream()
        .take_until(ended)
        // The guard lives as long as the body.
        .map(move |chunk| {
            let _watching = &watching;
            chunk
        });
    Ok(Response::from_parts(head, Body::from_stream(body)))
}

/// What a peer may prompt: text and images, as the Session's user.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PeerPrompt {
    id: Option<String>,
    /// Text and image parts only.
    parts: Vec<Content>,
    /// Default `steer`.
    delivery: Option<Delivery>,
}

/// `POST /attach/:id/prompt`, as `POST /sessions/:id/prompt` from the user.
pub(super) async fn prompt(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<PeerPrompt>, JsonRejection>,
) -> Result<Response, ApiError> {
    admitted(&peer, &grant, &headers, &id)?;
    let Json(body) = body.map_err(|error| ApiError::invalid(error.body_text()))?;
    let allowed = body
        .parts
        .iter()
        .all(|part| matches!(part, Content::Text { .. } | Content::Image { .. }));
    if !allowed {
        return Err(ApiError::invalid(
            "an attached peer may prompt with text and image parts only",
        ));
    }
    let prompt = PromptBody {
        id: body.id,
        parts: body.parts,
        delivery: body.delivery.unwrap_or(Delivery::Steer),
        resume: true,
        source: None,
    };
    crate::http::sessions::prompt(State(peer.app), Path(id), Json(prompt)).await
}

/// `POST /attach/:id/interrupt`.
pub(super) async fn interrupt(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    admitted(&peer, &grant, &headers, &id)?;
    crate::http::sessions::interrupt(State(peer.app), Path(id)).await
}

/// `GET /attach/:id/permissions`: the Session's pending permission requests, without
/// attach requests.
pub(super) async fn permissions(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Data<Vec<PendingRequest>>>, ApiError> {
    admitted(&peer, &grant, &headers, &id)?;
    Ok(Json(Data {
        data: answerable(&peer, &id),
    }))
}

/// `POST /attach/:id/permissions/:request/reply`, for a request [`permissions`] lists:
/// `once` or `reject`, never `always` (a saved approval is this machine's user's to make).
pub(super) async fn reply(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    Path((id, request_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Json<PermissionReplyBody>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    admitted(&peer, &grant, &headers, &id)?;
    let Json(body) = body.map_err(|error| ApiError::invalid(error.body_text()))?;
    if matches!(body.reply, ReplyKind::Always) {
        return Err(ApiError::invalid(
            "an attached peer may approve once; `always` is answered on this machine",
        ));
    }
    if !answerable(&peer, &id).iter().any(|r| r.id == request_id) {
        return Err(ApiError::not_found(
            "RequestNotFoundError",
            format!("no pending request {request_id} in session {id}"),
        ));
    }
    crate::http::sessions::reply_permission(State(peer.app), Path((id, request_id)), Json(body))
        .await
}

/// Pending permission requests of Session `id` an attached peer may answer.
fn answerable(peer: &PeerState, id: &str) -> Vec<PendingRequest> {
    peer.app
        .runtime
        .pending_requests(Some(id))
        .into_iter()
        .filter(|request| match &request.kind {
            PendingKind::Permission(ask) => ask.action != REMOTE_ATTACH,
            PendingKind::Question { .. } => false,
        })
        .collect()
}

fn attach_id(headers: &HeaderMap) -> Option<&str> {
    headers.get(ATTACH_HEADER)?.to_str().ok()
}

fn hidden(id: &str) -> ApiError {
    ApiError::not_found(
        "NotFoundError",
        format!("no Session {id} is shown to this grant"),
    )
}

/// No approved attach covers the request; nothing of the Session is sent.
fn not_attached() -> ApiError {
    RuntimeError::PermissionDenied(format!(
        "no approved attach: POST /api/v1/peer/attach/<session> and send its attach_id in {ATTACH_HEADER}"
    ))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(id: &str) -> Grant {
        Grant {
            id: id.into(),
            name: "laptop".into(),
            scopes: vec![PeerScope::Attach],
            created_ms: 0,
            expires_ms: None,
            last_used_ms: None,
            revoked_ms: None,
        }
    }

    /// An attach admits only its grant on its Session, ends when closed by its grant and
    /// after an hour without requests.
    #[test]
    fn an_attach_admits_its_grant_on_its_session() {
        let attaches = Attaches::default();
        let (laptop, other) = (grant("pgr_a"), grant("pgr_b"));
        let id = attaches.open(&laptop, "ses_a");
        let now = Instant::now();
        assert!(attaches.admits(&id, &laptop, "ses_a", now));
        assert!(!attaches.admits(&id, &laptop, "ses_b", now));
        assert!(!attaches.admits(&id, &other, "ses_a", now));
        assert!(!attaches.admits("pat_unknown", &laptop, "ses_a", now));
        assert!(!attaches.close(&id, &other));
        assert!(attaches.close(&id, &laptop));
        assert!(!attaches.admits(&id, &laptop, "ses_a", now));

        let idle = attaches.open(&laptop, "ses_a");
        let used = now + IDLE / 2;
        assert!(attaches.admits(&idle, &laptop, "ses_a", used));
        assert!(attaches.admits(
            &idle,
            &laptop,
            "ses_a",
            used + IDLE - Duration::from_secs(1)
        ));
        let later = used + IDLE - Duration::from_secs(1) + IDLE;
        assert!(!attaches.admits(&idle, &laptop, "ses_a", later));
    }

    /// Regression: an open event stream outlived its attach (a close, a revocation, the
    /// idle expiry), and a watched attach went idle. Ending an attach now cancels its
    /// streams, and an open stream keeps it in use.
    #[test]
    fn ending_an_attach_ends_its_streams_and_a_stream_keeps_it_open() {
        let attaches = Arc::new(Attaches::default());
        let (laptop, tablet) = (
            grant("pgr_a"),
            Grant {
                name: "tablet".into(),
                ..grant("pgr_b")
            },
        );
        let closed = attaches.open(&laptop, "ses_a");
        let stream = attaches.watch(&closed, &laptop, "ses_a").unwrap();
        assert!(attaches.watch(&closed, &laptop, "ses_b").is_none());
        assert!(attaches.close(&closed, &laptop));
        assert!(stream.stream.is_cancelled());

        let first = attaches.open(&laptop, "ses_a");
        let second = attaches.open(&laptop, "ses_b");
        let kept = attaches.open(&tablet, "ses_a");
        let streams = [
            attaches.watch(&first, &laptop, "ses_a").unwrap(),
            attaches.watch(&second, &laptop, "ses_b").unwrap(),
        ];
        let other = attaches.watch(&kept, &tablet, "ses_a").unwrap();
        assert_eq!(attaches.end_grant("laptop"), 2);
        assert!(streams.iter().all(|s| s.stream.is_cancelled()));
        assert!(!other.stream.is_cancelled());
        assert!(!attaches.admits(&first, &laptop, "ses_a", Instant::now()));

        // Watched past the idle limit: still open; idle after the stream ends.
        let much_later = Instant::now() + IDLE * 3;
        assert!(attaches.admits(&kept, &tablet, "ses_a", much_later));
        drop(other);
        let watched = attaches.open(&tablet, "ses_b");
        let stream = attaches.watch(&watched, &tablet, "ses_b").unwrap();
        drop(stream);
        assert!(!attaches.admits(&watched, &tablet, "ses_b", Instant::now() + IDLE * 2));
        let idle = attaches.open(&tablet, "ses_c");
        let stream = attaches.watch(&idle, &tablet, "ses_c").unwrap();
        // The idle sweep ends an idle attach without streams, cancelling its token.
        let token = attaches
            .admit(&idle, &tablet, "ses_c", Instant::now())
            .unwrap();
        drop(stream);
        assert!(!attaches.admits(&idle, &tablet, "ses_c", Instant::now() + IDLE * 2));
        assert!(token.is_cancelled());
    }
}
