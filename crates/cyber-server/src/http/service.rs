//! Authenticated, registration-bound listener shutdown.

use super::{ApiError, AppState};
use axum::extract::State;
use axum::routing::{get, post};
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

/// The listening server and its running work, so a client of another build can decide
/// whether restarting it interrupts anything.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ServiceInfo {
    pub id: String,
    pub version: String,
    pub build: String,
    pub pid: u32,
    /// Canonical path of the server executable, when the process could read it.
    pub binary: Option<String>,
    /// Sessions with a running Drain (a Turn in progress).
    pub running_sessions: usize,
    /// Workflow runs queued, running or waiting for input.
    pub active_workflow_runs: usize,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/service", get(info))
        .route("/service/stop", post(stop))
}

async fn info(State(state): State<AppState>) -> Result<Json<ServiceInfo>, ApiError> {
    let control = owned(&state)?;
    let active_workflow_runs = state
        .runtime
        .workflow_runs(None)?
        .iter()
        .filter(|run| run.status.is_active())
        .count();
    Ok(Json(ServiceInfo {
        id: control.id.clone(),
        version: state.options.version.clone(),
        build: cyber_core::version::build_info().build.into(),
        pid: std::process::id(),
        binary: cyber_core::version::binary().path.clone(),
        running_sessions: state.runtime.running_sessions().len(),
        active_workflow_runs,
    }))
}

fn owned(state: &AppState) -> Result<&ServiceControl, ApiError> {
    state.service.as_ref().ok_or_else(|| {
        ApiError::not_found(
            "ServiceUnavailableError",
            "this transport does not own server listeners",
        )
    })
}

async fn stop(
    State(state): State<AppState>,
    Json(request): Json<StopService>,
) -> Result<Json<StopAccepted>, ApiError> {
    let control = owned(&state)?;
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
