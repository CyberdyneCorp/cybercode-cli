//! Schedule routes (`loops-scheduling` → Persistent local schedules): `/api/v1/schedules`.
//! Owner credential only; IDE tokens do not reach them (their scope excludes them), and
//! model subprocesses never hold the owner credential.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, request::Parts},
    routing::{get, post},
};
use schemars::JsonSchema;
use serde::Serialize;

use super::{AppState, envelope::Data, error::ApiError};
use crate::runtime::schedules::{Schedule, ScheduleRequest};

type Result<T> = std::result::Result<T, ApiError>;

/// `POST /api/v1/schedules/{id}/run`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScheduleRunStarted {
    pub run_id: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/schedules", get(list).post(create))
        .route("/schedules/{schedule_id}", get(show).delete(remove))
        .route("/schedules/{schedule_id}/pause", post(pause))
        .route("/schedules/{schedule_id}/resume", post(resume))
        .route("/schedules/{schedule_id}/run", post(run_now))
}

async fn list(State(state): State<AppState>) -> Result<Json<Data<Vec<Schedule>>>> {
    Ok(Json(Data {
        data: state.runtime.schedules()?,
    }))
}

async fn create(
    State(state): State<AppState>,
    parts: Parts,
    Json(mut body): Json<ScheduleRequest>,
) -> Result<(StatusCode, Json<Data<Schedule>>)> {
    let directory = match body.directory.take() {
        Some(directory) => std::fs::canonicalize(&directory)
            .map_err(|_| ApiError::invalid(format!("no such directory: {directory}")))?,
        None => super::envelope::location(&parts, &state.options.default_directory)?,
    };
    if body.model.is_none() {
        body.model = state.services.default_model(&directory);
    }
    body.directory = Some(directory.display().to_string());
    let created = state.runtime.create_schedule(body).await?;
    Ok((StatusCode::CREATED, Json(Data { data: created })))
}

async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Schedule>>> {
    Ok(Json(Data {
        data: state.runtime.schedule(&id, true)?,
    }))
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Vec<String>>>> {
    state.runtime.remove_schedule(&id)?;
    Ok(Json(Data { data: vec![id] }))
}

async fn pause(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Schedule>>> {
    Ok(Json(Data {
        data: state.runtime.pause_schedule(&id)?,
    }))
}

async fn resume(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Schedule>>> {
    Ok(Json(Data {
        data: state.runtime.resume_schedule(&id)?,
    }))
}

async fn run_now(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<ScheduleRunStarted>>> {
    let run_id = state.runtime.run_schedule_now(&id).await?;
    Ok(Json(Data {
        data: ScheduleRunStarted { run_id },
    }))
}
