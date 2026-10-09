//! Authenticated Location-scoped MCP lifecycle operations.
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::request::Parts,
    routing::{get, post},
};
use schemars::JsonSchema;
use serde::Serialize;

use super::{
    ApiError, AppState,
    envelope::{Located, LocationInfo, location},
};

#[derive(Debug, Serialize, JsonSchema)]
pub struct McpClosed {
    /// All MCP owners observed for this Location have settled; a later open may reconnect.
    pub closed: bool,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mcp", get(status))
        .route("/mcp/close", post(close))
}

async fn status(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Vec<crate::runtime::McpServerStatus>>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    let data = state.services.mcp_status(&directory)?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn close(
    State(state): State<AppState>,
    parts: Parts,
    body: Bytes,
) -> Result<Json<Located<McpClosed>>, ApiError> {
    if !body.is_empty() {
        return Err(ApiError::invalid("MCP close takes no request body"));
    }
    let directory = location(&parts, &state.options.default_directory)?;
    state.services.close_mcp(directory.clone()).await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: McpClosed { closed: true },
    }))
}
