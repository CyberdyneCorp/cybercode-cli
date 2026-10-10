//! Authenticated, read-only Location code-intelligence status.
use super::{
    ApiError, AppState,
    envelope::{Located, LocationInfo, location},
};
use axum::{Json, Router, extract::State, http::request::Parts, routing::get};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LspState {
    Starting,
    Connected,
    Broken,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LspStatus {
    pub id: String,
    pub root: std::path::PathBuf,
    pub status: LspState,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FormatterStatus {
    pub id: String,
    pub extensions: Vec<String>,
    pub enabled: bool,
    pub installed: bool,
    pub detected_by: String,
}
impl From<cyber_core::intelligence::DetectedFormatter> for FormatterStatus {
    fn from(formatter: cyber_core::intelligence::DetectedFormatter) -> Self {
        Self {
            id: formatter.definition.id,
            extensions: formatter.definition.extensions,
            enabled: formatter.enabled,
            installed: formatter.installed,
            detected_by: formatter.detected_by,
        }
    }
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/formatters", get(formatters))
        .route("/lsp", get(lsp))
}
async fn lsp(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Vec<LspStatus>>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    if !directory.is_dir() {
        return Err(ApiError::invalid("LSP Location must be a directory"));
    }
    let data = state.services.lsp_status(directory.clone()).await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}
async fn formatters(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Vec<FormatterStatus>>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    if !directory.is_dir() {
        return Err(ApiError::invalid("Formatter Location must be a directory"));
    }
    let data = state.services.formatters(directory.clone()).await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}
