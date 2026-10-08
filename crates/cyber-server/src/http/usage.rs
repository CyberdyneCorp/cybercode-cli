//! Scope-addressed usage, with unsupported scopes refused explicitly.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use super::envelope::Data;
use super::{ApiError, AppState};
use crate::runtime::UsageReport;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageQuery {
    scope: String,
    id: String,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/usage", get(report))
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "InvalidRequestError", message)
}

async fn report(
    State(state): State<AppState>,
    query: Result<Query<UsageQuery>, QueryRejection>,
) -> Result<Json<Data<UsageReport>>, ApiError> {
    let Query(query) = query.map_err(|e| invalid(e.body_text()))?;
    if query.id.trim().is_empty() {
        return Err(invalid("Usage id must not be empty"));
    }
    match query.scope.as_str() {
        "session" => Ok(Json(Data {
            data: state.runtime.session_usage(&query.id)?,
        })),
        "run" | "goal" | "loop" | "routine" | "project" => {
            let mut error = ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                format!("Usage scope {} is unavailable", query.scope),
            );
            error.body.service = Some("usage".into());
            Err(error)
        }
        _ => Err(invalid("Unknown usage scope")),
    }
}
