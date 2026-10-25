//! Owner review and Stop are distinct from the model tool's scoped action bridge.
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{
    ApiError, AppState,
    envelope::{Data, location},
};
use crate::runtime::computer::{ComputerError, GrantView, HealthReport, Preview, Status};
use cyber_computer::backend::Selector;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub confirm: bool,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Allow {
    pub preview_id: String,
    pub confirm: bool,
    pub ttl_seconds: Option<u64>,
    #[serde(default)]
    pub acknowledge_unknown: bool,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/tools/computer/health", get(health))
        .route("/tools/computer/setup", post(setup))
        .route("/tools/computer/stop", post(stop_all))
        .route("/sessions/{id}/computer", get(status))
        .route("/sessions/{id}/computer/preview", post(preview))
        .route("/sessions/{id}/computer/allow", post(allow))
        .route("/sessions/{id}/computer/stop", post(stop))
        .route("/sessions/{id}/computer/frames/{frame}", get(frame))
        .layer(axum::extract::DefaultBodyLimit::max(4096))
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                response.headers_mut().insert(
                    "cache-control",
                    axum::http::HeaderValue::from_static("no-store"),
                );
                response
            },
        ))
}
async fn bound(state: &AppState, id: &str, parts: &Parts) -> Result<(), ApiError> {
    let requested = location(parts, &state.options.default_directory)?;
    let info = state.runtime.state(id).await?.info;
    let actual = std::fs::canonicalize(info.directory).map_err(|_| {
        ApiError::not_found("SessionNotFoundError", "Session Location is unavailable")
    })?;
    if actual != requested {
        return Err(ApiError::not_found(
            "SessionNotFoundError",
            "Session is not bound to this Location",
        ));
    }
    Ok(())
}
async fn health(State(state): State<AppState>) -> Result<Json<Data<HealthReport>>, ApiError> {
    let computer = state.runtime.computer().map_err(error)?;
    let report = tokio::task::spawn_blocking(move || computer.health(false))
        .await
        .map_err(|_| ApiError::unknown("computer worker failed"))?
        .map_err(error)?;
    Ok(Json(Data { data: report }))
}
async fn setup(
    State(state): State<AppState>,
    Json(input): Json<Setup>,
) -> Result<Json<Data<HealthReport>>, ApiError> {
    if !input.confirm {
        return Err(ApiError::invalid("OS setup requires explicit confirmation"));
    }
    let computer = state.runtime.computer().map_err(error)?;
    let report = tokio::task::spawn_blocking(move || computer.health(true))
        .await
        .map_err(|_| ApiError::unknown("computer worker failed"))?
        .map_err(error)?;
    Ok(Json(Data { data: report }))
}
async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Json<Data<Status>>, ApiError> {
    bound(&state, &id, &parts).await?;
    Ok(Json(Data {
        data: state.runtime.computer_status(&id).await.map_err(error)?,
    }))
}
async fn preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
    Json(input): Json<Selector>,
) -> Result<Json<Data<Preview>>, ApiError> {
    bound(&state, &id, &parts).await?;
    Ok(Json(Data {
        data: state
            .runtime
            .computer_preview(&id, input)
            .await
            .map_err(error)?,
    }))
}
async fn allow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
    Json(input): Json<Allow>,
) -> Result<Json<Data<GrantView>>, ApiError> {
    bound(&state, &id, &parts).await?;
    if !input.confirm {
        return Err(ApiError::invalid(
            "review and confirm the exact native target before arming",
        ));
    }
    let computer = state.runtime.computer().map_err(error)?;
    let grant = tokio::task::spawn_blocking(move || {
        computer.allow(
            &id,
            &input.preview_id,
            input.ttl_seconds,
            input.acknowledge_unknown,
        )
    })
    .await
    .map_err(|_| ApiError::unknown("computer worker failed"))?
    .map_err(error)?;
    Ok(Json(Data { data: grant }))
}
async fn stop(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<StatusCode, ApiError> {
    bound(&state, &id, &parts).await?;
    state.runtime.computer_stop(&id);
    Ok(StatusCode::NO_CONTENT)
}
async fn stop_all(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    state.runtime.computer().map_err(error)?.stop_all();
    Ok(StatusCode::NO_CONTENT)
}
async fn frame(
    State(state): State<AppState>,
    Path((id, frame)): Path<(String, String)>,
    parts: Parts,
) -> Result<Response, ApiError> {
    bound(&state, &id, &parts).await?;
    let computer = state.runtime.computer().map_err(error)?;
    let bytes = tokio::task::spawn_blocking(move || computer.frame_bytes(&id, &frame))
        .await
        .map_err(|_| ApiError::unknown("computer worker failed"))?
        .map_err(error)?;
    Ok((
        [
            ("content-type", "image/png"),
            ("x-content-type-options", "nosniff"),
            ("cache-control", "no-store"),
        ],
        bytes,
    )
        .into_response())
}
pub(super) fn error(error: ComputerError) -> ApiError {
    use cyber_computer::Error as Native;
    let (status, tag) = match &error {
        ComputerError::Native(Native::UnsupportedPlatform) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "ComputerUnsupportedPlatformError",
        ),
        ComputerError::Native(Native::Disabled) => (StatusCode::FORBIDDEN, "ComputerDisabledError"),
        ComputerError::Native(Native::NotArmed | Native::Consent(_)) => {
            (StatusCode::FORBIDDEN, "ComputerConsentError")
        }
        ComputerError::Native(Native::Busy) => (StatusCode::CONFLICT, "ComputerBusyError"),
        ComputerError::Native(Native::StaleFrame) => {
            (StatusCode::CONFLICT, "ComputerStaleFrameError")
        }
        ComputerError::Native(Native::Limit(_)) => {
            (StatusCode::TOO_MANY_REQUESTS, "ComputerLimitError")
        }
        ComputerError::FrameExpired => (StatusCode::NOT_FOUND, "ComputerFrameExpiredError"),
        ComputerError::Conflict => (StatusCode::CONFLICT, "ComputerConflictError"),
        ComputerError::Unknown => (StatusCode::CONFLICT, "ComputerUnknownOutcomeError"),
        ComputerError::Storage => (StatusCode::SERVICE_UNAVAILABLE, "ComputerStorageError"),
        _ => (StatusCode::BAD_REQUEST, "ComputerInvalidRequestError"),
    };
    ApiError::new(status, tag, error.to_string())
}
