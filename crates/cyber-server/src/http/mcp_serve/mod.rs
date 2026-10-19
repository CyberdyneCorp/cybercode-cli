//! MCP serve (`mcp-server`): Cyber Code Sessions for external MCP clients at
//! `/api/v1/mcp/serve`, Streamable HTTP `2025-06-18` in the `mcp` route group.
//!
//! MCP clients are trusted local clients of the server API: `guard` applies the local
//! authentication, the loopback-only and loopback-`Host` checks before this module runs,
//! and every tool maps onto existing routes in-process, so idempotency, tagged errors and
//! admission are the routes' own.

mod caller;
mod connection;
mod create;
mod messaging;
mod progress;
mod prompt;
mod protocol;
mod read;
mod route;
mod schema;
mod tools;
mod wait;

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Router};
use futures::StreamExt;

pub use connection::{Clock as McpServeClock, McpServeSessions};

use super::{ApiError, AppState};

const PATH: &str = "/mcp/serve";
/// Largest accepted request body.
const MAX_BODY: usize = 1024 * 1024;
/// An oversized body is read up to this many bytes and dropped, so the client receives
/// the `too_large` answer instead of a reset connection.
const MAX_DRAIN: usize = 16 * 1024 * 1024;

/// Whether a request path (inside `/api/v1` or not) is the MCP serve endpoint.
pub fn is_endpoint(path: &str) -> bool {
    path == PATH || path.strip_prefix("/api/v1") == Some(PATH)
}

pub fn routes(sessions: Arc<McpServeSessions>) -> Router<AppState> {
    Router::new()
        .route(PATH, post(message).get(no_stream).delete(end))
        .layer(Extension(sessions))
}

/// MCP serve configuration, read for every request; `enabled: false` turns it off.
fn enabled(state: &AppState) -> Result<(), ApiError> {
    if state.services.mcp_serve()?.enabled {
        Ok(())
    } else {
        Err(ApiError::service_unavailable(
            "mcp_serve",
            "MCP serve is disabled",
        ))
    }
}

async fn message(
    State(state): State<AppState>,
    Extension(sessions): Extension<Arc<McpServeSessions>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if let Err(error) = enabled(&state) {
        return error.into_response();
    }
    let body = match read_body(body).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    let cx = protocol::Context {
        state: &state,
        sessions: &sessions,
        headers: &headers,
    };
    protocol::handle(cx, &body).await
}

/// The server sends no unsolicited messages, so there is no GET stream.
async fn no_stream() -> Response {
    let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST, DELETE"));
    response
}

/// `DELETE` ends an MCP session; its Sessions keep running.
async fn end(
    State(state): State<AppState>,
    Extension(sessions): Extension<Arc<McpServeSessions>>,
    headers: HeaderMap,
) -> Response {
    if let Err(error) = enabled(&state) {
        return error.into_response();
    }
    match connection::session_id(&headers) {
        Ok(id) if sessions.close(id) => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => connection::unknown().into_response(),
        Err(error) => error.into_response(),
    }
}

async fn read_body(body: Body) -> Result<Bytes, ApiError> {
    let mut stream = body.into_data_stream();
    let mut kept = Vec::new();
    let mut total = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ApiError::invalid("could not read the request body"))?;
        total += chunk.len();
        if total <= MAX_BODY {
            kept.extend_from_slice(&chunk);
        } else if total > MAX_DRAIN {
            break;
        }
    }
    if total > MAX_BODY {
        return Err(ApiError::invalid("the request body exceeds 1 MiB").with_reason("too_large"));
    }
    Ok(Bytes::from(kept))
}
