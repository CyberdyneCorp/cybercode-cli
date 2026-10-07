//! Background task registry and cancellation, shared by clients and model tools.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use serde::Deserialize;

use super::{
    ApiError, AppState,
    envelope::{Cursor, Data, Page},
};
use crate::runtime::Job;

#[derive(Default, Deserialize)]
struct Filter {
    session_id: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list))
        .route("/jobs/{id}", get(show))
        .route("/jobs/{id}/stop", post(stop))
}
async fn list(
    State(state): State<AppState>,
    Query(filter): Query<Filter>,
) -> Result<Json<Page<Job>>, ApiError> {
    if let Some(id) = &filter.session_id {
        state.runtime.state(id).await?;
    }
    let (jobs, next) = state.runtime.jobs_page(
        filter.session_id.as_deref(),
        filter.limit.unwrap_or(50),
        filter.cursor.as_deref(),
    )?;
    Ok(Json(Page {
        data: jobs,
        cursor: Cursor {
            next,
            ..Default::default()
        },
    }))
}
fn known(state: &AppState, id: &str) -> Result<Job, ApiError> {
    state
        .runtime
        .find_job(id)?
        .ok_or_else(|| ApiError::not_found("JobNotFoundError", format!("No job {id}")))
}
async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Job>>, ApiError> {
    Ok(Json(Data {
        data: known(&state, &id)?,
    }))
}
async fn stop(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Job>>, ApiError> {
    known(&state, &id)?;
    Ok(Json(Data {
        data: state.runtime.cancel_job(&id).await?,
    }))
}
