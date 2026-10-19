//! `GET /api/v1/peer/sessions` and `GET /api/v1/peer/sessions/:id/status` (scope
//! `message`): the Sessions the calling grant could message, for its `list_sessions` and
//! its watches (design section 6).

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Extension, Path, Query, State};
use serde::Deserialize;

use super::{PeerState, guard};
use crate::http::error::ApiError;
use crate::runtime::messaging::listing::{PEER_LISTING_LIMIT, PeerSessions};
use crate::runtime::messaging::{PeerScope, PeerSessionStatus};
use crate::runtime::peers::Grant;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListQuery {
    limit: Option<usize>,
}

/// The root Sessions this grant could message, newest first, at most 50.
pub(super) async fn list(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<PeerSessions>, ApiError> {
    guard::require(&grant, PeerScope::Message)?;
    let Query(query) = query.map_err(|error| ApiError::invalid(error.body_text()))?;
    let limit = query.limit.unwrap_or(PEER_LISTING_LIMIT);
    let sessions = peer.app.runtime.messaging().granted_sessions(
        &grant.name,
        peer.info().share_paths,
        limit,
    )?;
    let sessions = sessions
        .into_iter()
        .map(|session| serde_json::to_value(session).unwrap_or_default())
        .collect();
    Ok(Json(PeerSessions { sessions }))
}

/// What one listed Session is doing; `404` for a Session this grant cannot see.
pub(super) async fn status(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    Path(id): Path<String>,
) -> Result<Json<PeerSessionStatus>, ApiError> {
    guard::require(&grant, PeerScope::Message)?;
    peer.app
        .runtime
        .messaging()
        .granted_status(&grant.name, &id)
        .await?
        .map(Json)
        .ok_or_else(|| {
            ApiError::not_found(
                "NotFoundError",
                format!("no Session {id} is shown to this grant"),
            )
        })
}
