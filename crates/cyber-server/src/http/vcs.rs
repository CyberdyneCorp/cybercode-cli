//! The `vcs` route group (`vcs-integration` → Git awareness): the Location's git state.

use axum::{Json, Router, extract::State, http::request::Parts, routing::get};
use schemars::JsonSchema;
use serde::Serialize;

use super::{
    ApiError, AppState,
    envelope::{Located, LocationInfo, location},
};

/// `{ "vcs": null }` outside a git repository.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Vcs {
    pub vcs: Option<cyber_core::vcs::GitState>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/vcs", get(vcs))
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
