//! Owner-only PR auto-fix controls. Worker observations are never public mutations.
use super::{
    ApiError, AppState,
    envelope::{Data, location},
};
use crate::autofix::records::Repository;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, request::Parts},
    routing::{get, post},
};
use cyber_core::autofix::{Config, Subscription};
use cyber_store::StoreError;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub expected_revision: u64,
    /// `pause`, `resume` or `disable`.
    pub action: String,
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/autofix/subscriptions", get(list).post(enable))
        .route("/autofix/subscriptions/{id}", get(show))
        .route("/autofix/subscriptions/{id}/control", post(control))
}
fn failure(error: StoreError) -> ApiError {
    match error {
        StoreError::InvalidEvent { reason, .. } => ApiError::conflict(reason),
        error => ApiError::from(crate::runtime::RuntimeError::from(error)),
    }
}
fn repository(state: &AppState) -> Repository {
    Repository::new(state.store.clone())
}
fn owned(state: &AppState, parts: &Parts, id: &str) -> Result<Subscription, ApiError> {
    let subscription = repository(state).get(id).map_err(|error| match error {
        StoreError::InvalidEvent { .. } => {
            ApiError::not_found("AutofixNotFoundError", "auto-fix subscription not found")
        }
        error => failure(error),
    })?;
    let requested = location(parts, &state.options.default_directory)?;
    if requested.to_string_lossy() != subscription.directory {
        return Err(ApiError::forbidden(
            "auto-fix subscription belongs to another Location",
        ));
    }
    Ok(subscription)
}
async fn list(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Data<Vec<Subscription>>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    Ok(Json(Data {
        data: repository(&state)
            .list(Some(&directory.to_string_lossy()))
            .map_err(failure)?,
    }))
}
async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Json<Data<Subscription>>, ApiError> {
    Ok(Json(Data {
        data: owned(&state, &parts, &id)?,
    }))
}
async fn enable(
    State(state): State<AppState>,
    parts: Parts,
    Json(config): Json<Config>,
) -> Result<(StatusCode, Json<Data<Subscription>>), ApiError> {
    config.validate().map_err(ApiError::invalid)?;
    let key = parts
        .headers
        .get("idempotency-key")
        .and_then(|s| s.to_str().ok())
        .filter(|s| !s.is_empty() && s.len() <= 200)
        .ok_or_else(|| ApiError::invalid("an Idempotency-Key of 1..200 bytes is required"))?
        .to_owned();
    let directory = location(&parts, &state.options.default_directory)?;
    let owner = state.runtime.state(&config.session_id).await?.info;
    if std::fs::canonicalize(&owner.directory).ok().as_ref() != Some(&directory) {
        return Err(ApiError::forbidden(
            "linked Session belongs to another Location",
        ));
    }
    let subscription = state
        .services
        .autofix_enable(directory, config, key)
        .await?;
    Ok((StatusCode::CREATED, Json(Data { data: subscription })))
}
async fn control(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
    Json(body): Json<Control>,
) -> Result<Json<Data<Subscription>>, ApiError> {
    let subscription = owned(&state, &parts, &id)?;
    if subscription.revision != body.expected_revision {
        return Err(ApiError::conflict("auto-fix subscription revision changed"));
    }
    if !matches!(body.action.as_str(), "pause" | "resume" | "disable") {
        return Err(ApiError::invalid("expected pause, resume or disable"));
    }
    let next = repository(&state)
        .update(
            &id,
            body.expected_revision,
            chrono::Utc::now().timestamp_millis(),
            move |s| s.control(&body.action),
        )
        .map_err(failure)?;
    Ok(Json(Data { data: next }))
}
