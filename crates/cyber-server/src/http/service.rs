//! Authenticated, registration-bound listener shutdown.

use super::{ApiError, AppState};
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Clone)]
pub struct ServiceControl {
    pub id: String,
    pub stop: Arc<Notify>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopService {
    pub id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StopAccepted {
    pub id: String,
    pub stopping: bool,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/service/stop", post(stop))
}

async fn stop(
    State(state): State<AppState>,
    Json(request): Json<StopService>,
) -> Result<Json<StopAccepted>, ApiError> {
    let control = state.service.as_ref().ok_or_else(|| {
        ApiError::not_found(
            "ServiceUnavailableError",
            "this transport does not own server listeners",
        )
    })?;
    if request.id != control.id {
        return Err(ApiError::conflict(
            "server registration identity does not match",
        ));
    }
    // Wake every active listener waiter and retain a permit during startup.
    control.stop.notify_waiters();
    control.stop.notify_one();
    Ok(Json(StopAccepted {
        id: control.id.clone(),
        stopping: true,
    }))
}
