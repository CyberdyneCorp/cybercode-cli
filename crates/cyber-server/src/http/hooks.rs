//! Authenticated, Session-addressed durable hook execution observations.
use super::{
    ApiError, AppState,
    envelope::{Cursor, Page},
};
use crate::runtime::{HookExecutionRecord, RuntimeError};
use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    routing::{get, post},
};
use serde::Deserialize;
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Filter {
    limit: Option<u32>,
    cursor: Option<String>,
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sessions/{sessionID}/hook-executions", get(list))
        .route("/hooks", get(review))
        .route("/hooks/trust", post(trust))
        .route("/hooks/untrust", post(untrust))
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

async fn review(
    State(state): State<AppState>,
    parts: axum::http::request::Parts,
) -> Result<Json<super::envelope::Located<cyber_core::hooks::HookReview>>, ApiError> {
    let directory = super::envelope::location(&parts, &state.options.default_directory)?;
    let mut review = state.services.review_hooks(&directory)?;
    for hook in &mut review.hooks {
        hook.last_run = crate::runtime::hook_last_run(&state.store, &directory, &hook.definition)?;
    }
    Ok(Json(super::envelope::Located {
        data: review,
        location: super::envelope::LocationInfo::of(&directory),
    }))
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HookTrustBody {
    pub digest: String,
}
#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct HookRevocation {
    pub digest: String,
    pub revoked: bool,
}
async fn trust(
    State(state): State<AppState>,
    parts: axum::http::request::Parts,
    body: Result<Json<HookTrustBody>, JsonRejection>,
) -> Result<Json<super::envelope::Located<HookApproval>>, ApiError> {
    let Json(body) = body.map_err(|error| ApiError::invalid(error.body_text()))?;
    let directory = super::envelope::location(&parts, &state.options.default_directory)?;
    state.services.trust_hook(&directory, &body.digest)?;
    Ok(Json(super::envelope::Located {
        data: HookApproval {
            digest: body.digest,
        },
        location: super::envelope::LocationInfo::of(&directory),
    }))
}
#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct HookApproval {
    pub digest: String,
}
async fn untrust(
    State(state): State<AppState>,
    parts: axum::http::request::Parts,
    body: Result<Json<HookTrustBody>, JsonRejection>,
) -> Result<Json<super::envelope::Located<HookRevocation>>, ApiError> {
    let Json(body) = body.map_err(|error| ApiError::invalid(error.body_text()))?;
    let directory = super::envelope::location(&parts, &state.options.default_directory)?;
    let revoked = state.services.untrust_hook(&directory, &body.digest)?;
    Ok(Json(super::envelope::Located {
        data: HookRevocation {
            digest: body.digest,
            revoked,
        },
        location: super::envelope::LocationInfo::of(&directory),
    }))
}
