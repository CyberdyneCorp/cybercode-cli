//! The `vcs` route group (`vcs-integration` → Git awareness): the Location's git state,
//! and generated commit and PR text (`vcs-integration` → Commit and PR text generation).

use axum::{
    Json, Router,
    extract::State,
    http::request::Parts,
    routing::{get, post},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ApiError, AppState,
    envelope::{Located, LocationInfo, location},
};

/// `{ "vcs": null }` outside a git repository.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Vcs {
    pub vcs: Option<cyber_core::vcs::GitState>,
}

/// A conventional commit message for the staged diff.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommitMessage {
    /// Ends with a `Co-authored-by` trailer only when `git.co_author` is configured.
    pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrDescriptionRequest {
    /// The base branch or commit (default: `origin/HEAD`, else `main`, else `master`).
    #[serde(default)]
    pub base: Option<String>,
}

/// A drafted pull request for `base..HEAD`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PrDescription {
    pub base: String,
    pub title: String,
    pub body: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/vcs", get(vcs))
        .route("/git/commit-message", post(commit_message))
        .route("/git/pr-description", post(pr_description))
}

async fn commit_message(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<CommitMessage>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    let data = state.services.commit_message(&directory).await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn pr_description(
    State(state): State<AppState>,
    parts: Parts,
    body: Option<Json<PrDescriptionRequest>>,
) -> Result<Json<Located<PrDescription>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    let request = body.map(|Json(body)| body).unwrap_or_default();
    let data = state
        .services
        .pr_description(&directory, request.base)
        .await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn vcs(State(state): State<AppState>, parts: Parts) -> Result<Json<Located<Vcs>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    if !directory.is_dir() {
        return Err(ApiError::invalid("VCS Location must be a directory"));
    }
    let probe = directory.clone();
    let vcs = tokio::task::spawn_blocking(move || cyber_core::vcs::status(&probe))
        .await
        .map_err(|_| ApiError::invalid("VCS status was interrupted"))?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: Vcs { vcs },
    }))
}
