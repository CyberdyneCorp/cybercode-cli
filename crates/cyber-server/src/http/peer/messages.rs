//! `POST /api/v1/peer/messages` and `GET /api/v1/peer/messages/:id` (scope `message`):
//! messages from the calling machine's Sessions, named after the grant (design section 4).

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, State};

use super::{PeerState, guard};
use crate::http::error::ApiError;
use crate::runtime::messaging::{PeerMessage, PeerOutcome, PeerScope, PeerSendResult};
use crate::runtime::peers::Grant;

/// Receive one message; a retry with the same ID returns the first outcome.
pub(super) async fn send(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    body: Result<Json<PeerMessage>, JsonRejection>,
) -> Result<Json<PeerSendResult>, ApiError> {
    guard::require(&grant, PeerScope::Message)?;
    let Json(body) = body.map_err(|error| ApiError::invalid(error.body_text()))?;
    let share_paths = peer.info().share_paths;
    let result = peer
        .app
        .runtime
        .messaging()
        .receive(&grant.name, share_paths, body)
        .await?;
    Ok(Json(result))
}

/// The current outcome of a message this grant sent; `404` for any other message.
pub(super) async fn outcome(
    State(peer): State<PeerState>,
    Extension(grant): Extension<Grant>,
    Path(message_id): Path<String>,
) -> Result<Json<PeerOutcome>, ApiError> {
    guard::require(&grant, PeerScope::Message)?;
    peer.app
        .runtime
        .messaging()
        .peer_outcome(&grant.name, &message_id)?
        .map(Json)
        .ok_or_else(|| {
            ApiError::not_found(
                "NotFoundError",
                format!("this grant sent no message {message_id} that was recorded here"),
            )
        })
}
