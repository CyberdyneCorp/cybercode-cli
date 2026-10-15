//! Authenticated Location-scoped MCP lifecycle operations.
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
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

/// The `mcp` route group: Location MCP lifecycle and the MCP serve endpoint.
pub fn routes(serve: std::sync::Arc<super::McpServeSessions>) -> Router<AppState> {
    Router::new()
        .route("/mcp", get(status))
        .route("/mcp/close", post(close))
        .route("/mcp/{name}/reconnect", post(reconnect))
        .merge(super::mcp_serve::routes(serve))
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

/// Reconnect a server settled `needs_auth` or `needs_client_registration` (for example after
/// `cyber mcp auth`), or one running on a credential `cyber mcp logout` removed, with a fresh
/// durable owner. The response is the status right after re-admission; later transitions
/// follow on the event stream.
async fn reconnect(
    State(state): State<AppState>,
    Path(name): Path<String>,
    parts: Parts,
    body: Bytes,
) -> Result<Json<Located<crate::runtime::McpServerStatus>>, ApiError> {
    if !body.is_empty() {
        return Err(ApiError::invalid("MCP reconnect takes no request body"));
    }
    let valid = (1..=48).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if !valid {
        return Err(ApiError::invalid("invalid MCP server name"));
    }
    let directory = location(&parts, &state.options.default_directory)?;
    let data = state
        .services
        .reconnect_mcp(directory.clone(), name)
        .await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}
