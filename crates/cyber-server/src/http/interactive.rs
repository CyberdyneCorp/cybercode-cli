//! Explicit browser attachments. Data streams do not renew these owner-authenticated
//! leases, and idempotency replay returns a receipt without recreating authority.
use super::{
    ApiError, AppState,
    envelope::{Data, location},
};
use crate::runtime::presence::{InteractiveClient, InteractiveClientError};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{post, put},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sessions/{sessionID}/interactive-clients", post(create))
        .route(
            "/sessions/{sessionID}/interactive-clients/{clientID}",
            put(renew).delete(release),
        )
}
async fn binding(state: &AppState, id: &str, parts: &Parts) -> Result<String, ApiError> {
    let directory = location(parts, &state.options.default_directory)?;
    let info = state.runtime.state(id).await?.info;
    let actual = std::fs::canonicalize(&info.directory).map_err(|_| {
        ApiError::not_found(
            "InteractiveClientNotFoundError",
            "Session Location is unavailable",
        )
    })?;
    if actual != directory {
        return Err(ApiError::not_found(
            "InteractiveClientNotFoundError",
            "interactive client is not bound to this Session and Location",
        ));
    }
    Ok(directory.display().to_string())
}
async fn create(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    let directory = binding(&state, &id, &parts).await?;
    let receipt = state
        .runtime
        .add_interactive_client(&id, &directory)
        .await
        .map_err(error)?;
    Ok((
        StatusCode::CREATED,
        [("cache-control", "no-store")],
        Json(Data { data: receipt }),
    )
        .into_response())
}
async fn renew(
    State(state): State<AppState>,
    Path((session, client)): Path<(String, String)>,
    parts: Parts,
) -> Result<Response, ApiError> {
    let directory = binding(&state, &session, &parts).await?;
    let receipt = state
        .runtime
        .renew_interactive_client(&session, &directory, &client)
        .map_err(error)?;
    Ok((
        [("cache-control", "no-store")],
        Json(Data { data: receipt }),
    )
        .into_response())
}
async fn release(
    State(state): State<AppState>,
    Path((session, client)): Path<(String, String)>,
    parts: Parts,
) -> Result<Response, ApiError> {
    let directory = binding(&state, &session, &parts).await?;
    state
        .runtime
        .release_interactive_client(&session, &directory, &client)
        .await
        .map_err(error)?;
    Ok((StatusCode::NO_CONTENT, [("cache-control", "no-store")]).into_response())
}
fn error(error: InteractiveClientError) -> ApiError {
    match error {
        InteractiveClientError::Missing => ApiError::not_found(
            "InteractiveClientNotFoundError",
            "interactive client is unknown, expired, or bound to another Session/Location",
        ),
        InteractiveClientError::Unavailable => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "InteractiveClientUnavailableError",
            "interactive client authority could not be recorded",
        ),
        InteractiveClientError::Limit => ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "InteractiveClientLimitError",
            "interactive client capacity reached",
        ),
    }
}
// Keep the public DTO reachable from this operation's schema owner.
pub type InteractiveClientLease = InteractiveClient;
