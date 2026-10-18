//! Background task registry and cancellation, shared by clients and model tools.
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
        .route("/jobs/{id}/output", get(output))
}

/// Bytes of a Job log returned by default, and at most.
const DEFAULT_TAIL: u64 = 64 * 1024;
const MAX_TAIL: u64 = 1024 * 1024;

#[derive(Default, Deserialize)]
struct OutputQuery {
    tail_bytes: Option<u64>,
}

/// The end of a process Job's log (`background-tasks` → Tasks view: open output).
#[derive(Debug, Serialize, JsonSchema)]
pub struct JobOutput {
    /// The last `tail_bytes` of the log, lossily decoded.
    pub text: String,
    /// The log's size in bytes.
    pub size: u64,
    /// Whether earlier bytes were left out.
    pub truncated: bool,
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

async fn output(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<OutputQuery>,
) -> Result<Json<Data<JobOutput>>, ApiError> {
    let job = known(&state, &id)?;
    let path = job.output_path.ok_or_else(|| {
        ApiError::not_found("JobOutputNotFoundError", format!("Job {id} has no log"))
    })?;
    let tail = query.tail_bytes.unwrap_or(DEFAULT_TAIL).clamp(1, MAX_TAIL);
    let read = tokio::task::spawn_blocking(move || read_tail(std::path::Path::new(&path), tail))
        .await
        .map_err(ApiError::unknown)?;
    let (text, size) = read
        .map_err(|e| ApiError::not_found("JobOutputNotFoundError", format!("Job {id} log: {e}")))?;
    Ok(Json(Data {
        data: JobOutput {
            truncated: size > tail,
            text,
            size,
        },
    }))
}

fn read_tail(path: &std::path::Path, tail: u64) -> std::io::Result<(String, u64)> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(tail)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok((String::from_utf8_lossy(&bytes).into_owned(), size))
}
