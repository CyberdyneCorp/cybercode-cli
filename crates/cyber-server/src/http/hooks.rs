//! Authenticated, Session-addressed durable hook execution observations.
use super::{
    ApiError, AppState,
    envelope::{Cursor, Page},
};
use crate::runtime::{HookExecutionRecord, RuntimeError};
use axum::{
    Json, Router,
    extract::{Path, Query, State, rejection::QueryRejection},
    routing::get,
};
use serde::Deserialize;
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    limit: Option<u32>,
    cursor: Option<String>,
}
pub fn routes() -> Router<AppState> {
    Router::new().route("/sessions/{sessionID}/hook-executions", get(list))
}
async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<Filter>, QueryRejection>,
) -> Result<Json<Page<HookExecutionRecord>>, ApiError> {
    let Query(filter) = query.map_err(|error| ApiError::invalid(error.body_text()))?;
    state.runtime.state(&id).await?;
    let (data, next) = state
        .runtime
        .hook_executions_page(&id, filter.limit.unwrap_or(50), filter.cursor.as_deref())
        .map_err(|error| match error {
            RuntimeError::Invalid(message) if message.starts_with("InvalidCursorError:") => {
                ApiError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "InvalidCursorError",
                    message,
                )
            }
            other => other.into(),
        })?;
    Ok(Json(Page {
        data,
        cursor: Cursor {
            next,
            ..Default::default()
        },
    }))
}
