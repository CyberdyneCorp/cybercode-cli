//! Output style routes (`system-context` → Output style definitions and selection,
//! `server-api` → Output style routes).

use std::path::Path as FsPath;

use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::{
    Json, Router,
    routing::{get, post, put},
};
use cyber_core::output_styles::StyleListing;
use schemars::JsonSchema;
use serde::Deserialize;

use super::sessions::{Session, show};
use super::{
    ApiError, AppState,
    envelope::{Data, Located, LocationInfo, location},
};

/// `{ style: string | null }`: `null` returns a Session to its configured style, and
/// sets the global default to `default`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StyleBody {
    #[serde(default)]
    pub style: Option<String>,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/config/output-styles", get(list))
        .route("/config/output-style", put(set_default))
        .route("/sessions/{session_id}/output-style", post(switch))
}

/// Refuse an explicit selection that is invalid, unknown or held at `directory`.
pub(super) fn check_defined(
    state: &AppState,
    directory: &FsPath,
    style: &str,
) -> Result<(), ApiError> {
    let listing = state.services.output_styles(directory)?;
    listing
        .styles
        .select(style)
        .map(drop)
        .map_err(ApiError::invalid)
}

async fn list(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<StyleListing>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: state.services.output_styles(&directory)?,
    }))
}

/// Set `output_style` in the user's global configuration: built-in and user styles only.
async fn set_default(
    State(state): State<AppState>,
    parts: Parts,
    Json(body): Json<StyleBody>,
) -> Result<Json<Located<StyleListing>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    let style = body
        .style
        .as_deref()
        .unwrap_or(cyber_core::output_styles::DEFAULT);
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: state.services.set_output_style(&directory, style)?,
    }))
}

/// Switch a primary Session's own style, checked at the Session's Location; it applies
/// from the next Turn.
async fn switch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<StyleBody>,
) -> Result<Json<Data<Session>>, ApiError> {
    let info = state.runtime.state(&id).await?.info;
    if !info.takes_output_style() {
        return Err(ApiError::invalid("output styles apply to primary Sessions"));
    }
    if let Some(style) = &body.style {
        if !cyber_core::output_styles::valid_name(style) {
            return Err(ApiError::invalid(format!(
                "invalid output style name \"{style}\""
            )));
        }
        check_defined(&state, FsPath::new(&info.directory), style)?;
    }
    state
        .runtime
        .switch_output_style(&id, body.style.as_deref())
        .await?;
    show(State(state), Path(id)).await
}
