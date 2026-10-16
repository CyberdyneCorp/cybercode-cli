//! Scope-addressed usage, with unsupported scopes refused explicitly.

use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use super::envelope::Data;
use super::{ApiError, AppState};
use crate::runtime::{
    InsightWindow, InsightsRequest, Period, RuntimeError, ScopeUsage, StatsRequest, UsageInsights,
    UsageLimits, UsageStats,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageQuery {
    scope: String,
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatsQuery {
    #[serde(default)]
    period: Option<String>,
    #[serde(default)]
    directory: Option<String>,
    #[serde(default)]
    tz: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InsightsQuery {
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    directory: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/usage", get(report))
        .route("/usage/stats", get(stats))
        .route("/usage/insights", get(insights))
        .route("/usage/limits", get(limits))
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "InvalidRequestError", message)
}

async fn report(
    State(state): State<AppState>,
    query: Result<Query<UsageQuery>, QueryRejection>,
) -> Result<Json<Data<ScopeUsage>>, ApiError> {
    let Query(query) = query.map_err(|e| invalid(e.body_text()))?;
    if query.id.trim().is_empty() {
        return Err(invalid("Usage id must not be empty"));
    }
    match query.scope.as_str() {
        "session" => Ok(Json(Data {
            data: ScopeUsage::Session(Box::new(
                state.runtime.session_usage_breakdown(&query.id).await?,
            )),
        })),
        "run" => match state.runtime.run_usage(&query.id) {
            Ok(report) => Ok(Json(Data {
                data: ScopeUsage::Run(report),
            })),
            Err(RuntimeError::Invalid(message)) => {
                Err(ApiError::not_found("WorkflowRunNotFoundError", message))
            }
            Err(error) => Err(error.into()),
        },
        "goal" | "loop" | "routine" | "project" => {
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

/// `GET /usage/stats` (`observability-costs` → Usage statistics API).
async fn stats(
    State(state): State<AppState>,
    query: Result<Query<StatsQuery>, QueryRejection>,
) -> Result<Json<Data<UsageStats>>, ApiError> {
    let Query(query) = query.map_err(|e| invalid(e.body_text()))?;
    let period = match query.period.as_deref() {
        None => Period::All,
        Some(value) => {
            Period::parse(value).ok_or_else(|| invalid("period must be `all` or 1d to 371d"))?
        }
    };
    let project = project(query.directory)?;
    let request = StatsRequest {
        period,
        project,
        zone: query.tz,
    };
    let runtime = state.runtime.clone();
    let data = tokio::task::spawn_blocking(move || runtime.usage_stats(&request))
        .await
        .map_err(|_| invalid("Usage statistics were interrupted"))??;
    Ok(Json(Data { data }))
}

/// The project root of an absolute `directory` query value.
fn project(directory: Option<String>) -> Result<Option<std::path::PathBuf>, ApiError> {
    directory
        .filter(|d| !d.is_empty())
        .map(|d| {
            let directory = std::path::PathBuf::from(d);
            if !directory.is_absolute() {
                return Err(invalid("directory must be an absolute path"));
            }
            let directory = std::fs::canonicalize(&directory).unwrap_or(directory);
            Ok(cyber_core::config::project_root(&directory))
        })
        .transpose()
}

/// `GET /usage/insights` (`observability-costs` → Usage insights).
async fn insights(
    State(state): State<AppState>,
    query: Result<Query<InsightsQuery>, QueryRejection>,
) -> Result<Json<Data<UsageInsights>>, ApiError> {
    let Query(query) = query.map_err(|e| invalid(e.body_text()))?;
    let window = match query.window.as_deref() {
        None => InsightWindow::Day,
        Some(value) => {
            InsightWindow::parse(value).ok_or_else(|| invalid("window must be `24h` or `7d`"))?
        }
    };
    let request = InsightsRequest {
        window,
        project: project(query.directory)?,
    };
    let runtime = state.runtime.clone();
    let data = tokio::task::spawn_blocking(move || runtime.usage_insights(&request))
        .await
        .map_err(|_| invalid("Usage insights were interrupted"))??;
    Ok(Json(Data { data }))
}

/// `GET /usage/limits` (`observability-costs` → Provider-reported limits).
async fn limits(State(state): State<AppState>) -> Json<Data<UsageLimits>> {
    Json(Data {
        data: state.runtime.usage_limits(),
    })
}
