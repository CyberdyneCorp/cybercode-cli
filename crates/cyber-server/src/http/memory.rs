//! Explicit authenticated memory review; model permissions govern model dispatch separately.
use axum::{
    Json, Router,
    extract::{Path, State},
    http::request::Parts,
    routing::get,
};
use cyber_core::memory::{MemoryCatalog, MemoryDocument};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    ApiError, AppState,
    envelope::{Located, LocationInfo, location, query_value},
};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemoryScope {
    #[default]
    Project,
    Global,
}

impl MemoryScope {
    pub fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "project" => Ok(Self::Project),
            "global" => Ok(Self::Global),
            _ => Err(ApiError::invalid("Memory scope must be project or global")),
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/memory", get(list))
        .route("/memory/{scope}/{name}", get(read))
}

async fn list(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<MemoryCatalog>>, ApiError> {
    let scope = query_value(parts.uri.query().unwrap_or_default(), "scope")
        .map(|value| MemoryScope::parse(&value))
        .transpose()?
        .unwrap_or_default();
    let directory = location(&parts, &state.options.default_directory)?;
    let data = state.services.memory_list(directory.clone(), scope).await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn read(
    State(state): State<AppState>,
    Path((scope, name)): Path<(String, String)>,
    parts: Parts,
) -> Result<Json<Located<MemoryDocument>>, ApiError> {
    let scope = MemoryScope::parse(&scope)?;
    cyber_core::memory::validate_name(&name)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    let directory = location(&parts, &state.options.default_directory)?;
    let data = state
        .services
        .memory_read(directory.clone(), scope, name)
        .await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

pub(super) fn unavailable() -> ApiError {
    let mut error = ApiError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "ServiceUnavailableError",
        "Memory review is unavailable in this host",
    );
    error.body.service = Some("memory".into());
    error
}
