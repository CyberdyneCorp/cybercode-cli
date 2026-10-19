//! Messaging routes (`cross-session-messaging` → Messaging API).

use axum::{
    Json, Router,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{
    AppState,
    envelope::{Cursor, Data, Page},
    error::ApiError,
    idempotency::PRINCIPAL,
};
use crate::runtime::RuntimeError;
use crate::runtime::messaging::send::{DEFAULT_CLIENT, idempotent_id};
use crate::runtime::messaging::{
    Direction, HistoryQuery, MessageRecord, Order, ReachableSession, SendRequest, SendResult,
    Sender, Watch, WatchRequest,
};

#[derive(Debug, Default, Deserialize)]
struct ReachableQuery {
    /// The listing caller's Session; without it the caller is a client.
    from: Option<String>,
    /// Only Sessions in this directory.
    directory: Option<String>,
}

/// `GET /api/v1/sessions/:id/messaging` (`server-api` → Pagination).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryParams {
    cursor: Option<String>,
    /// 1-200, default 50.
    limit: Option<u32>,
    /// `in` or `out`; both without it.
    direction: Option<Direction>,
    /// `asc` or `desc` (default) by creation time.
    #[serde(default)]
    order: Order,
}

/// `POST /api/v1/messaging/send`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendMessage {
    /// A `msgx_` ID; a repeated ID returns the original result with `replayed: true`.
    /// Without it, an `Idempotency-Key` derives one.
    #[serde(default)]
    pub id: Option<String>,
    /// The sending Session; without it the sender is the client (origin `client`).
    #[serde(default)]
    pub from: Option<String>,
    /// The target's address (`ses_` ID, subagent or Session name).
    pub to: String,
    /// Plain UTF-8 text, at most 16384 bytes.
    pub text: String,
    /// Also subscribe `from` to one notice when the target next goes idle; needs `from`.
    #[serde(default)]
    pub notify_when_idle: bool,
}

/// `POST /api/v1/messaging/watch`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WatchSession {
    /// The subscribing Session, which receives the notice.
    pub from: String,
    /// The watched Session's address (`ses_` ID, subagent or Session name).
    pub target: String,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/messaging/sessions", get(sessions))
        .route("/messaging/send", post(send))
        .route("/messaging/watch", post(watch))
        .route("/sessions/{session_id}/messaging", get(history))
}

/// The client label from `X-Cyber-Client`: 1-64 printable ASCII characters, else `api`.
/// A label starting with `mcp:` is refused: that key space (`mcp:<label>`) belongs to
/// MCP serve clients, so no HTTP client can match their inbound rules or rate window.
fn client_label(headers: &HeaderMap) -> Result<String, ApiError> {
    let Some(value) = headers.get("x-cyber-client") else {
        return Ok(DEFAULT_CLIENT.into());
    };
    let label = value
        .to_str()
        .ok()
        .filter(|label| {
            (1..=64).contains(&label.len()) && label.bytes().all(|b| b.is_ascii_graphic())
        })
        .ok_or_else(|| ApiError::invalid("X-Cyber-Client must be 1-64 printable characters"))?;
    if label
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("mcp:"))
    {
        return Err(ApiError::invalid(
            "X-Cyber-Client cannot start with `mcp:`, which names MCP serve clients",
        ));
    }
    Ok(label.to_owned())
}

async fn send(
    State(app): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SendMessage>,
) -> Result<Json<Data<SendResult>>, ApiError> {
    let from = match body.from {
        Some(session) => Sender::Session(session),
        None => Sender::Client(client_label(&headers)?),
    };
    let id = body.id.or_else(|| {
        headers
            .get("idempotency-key")
            .and_then(|key| key.to_str().ok())
            .map(|key| idempotent_id(PRINCIPAL, key))
    });
    let data = app
        .runtime
        .messaging()
        .send(SendRequest {
            id,
            from,
            to: body.to,
            text: body.text,
            depth: 0,
            expected_target: None,
            notify_when_idle: body.notify_when_idle,
        })
        .await?;
    Ok(Json(Data { data }))
}

async fn watch(
    State(app): State<AppState>,
    Json(body): Json<WatchSession>,
) -> Result<(StatusCode, Json<Data<Watch>>), ApiError> {
    let data = app
        .runtime
        .messaging()
        .watch(WatchRequest {
            id: None,
            subscriber: body.from,
            target: body.target,
            expected_target: None,
            address: None,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(Data { data })))
}

async fn sessions(
    State(app): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ReachableQuery>,
) -> Result<Json<Data<Vec<ReachableSession>>>, ApiError> {
    // Sessions store canonical directories, so the filter is canonicalized the same way.
    let directory = query
        .directory
        .map(|d| {
            std::fs::canonicalize(&d)
                .map(|path| path.display().to_string())
                .map_err(|_| ApiError::invalid(format!("no such directory: {d}")))
        })
        .transpose()?;
    let sender = match query.from {
        Some(session) => Sender::Session(session),
        None => Sender::Client(client_label(&headers)?),
    };
    let data = app
        .runtime
        .messaging()
        .reachable_as(&sender, directory.as_deref())?;
    Ok(Json(Data { data }))
}

async fn history(
    State(app): State<AppState>,
    Path(session): Path<String>,
    query: Result<Query<HistoryParams>, QueryRejection>,
) -> Result<Json<Page<MessageRecord>>, ApiError> {
    let Query(params) = query.map_err(|error| ApiError::invalid(error.body_text()))?;
    let page = app
        .runtime
        .messaging()
        .history(
            &session,
            HistoryQuery {
                direction: params.direction,
                order: params.order,
                limit: params.limit,
                cursor: params.cursor,
            },
        )
        .map_err(|error| match error {
            RuntimeError::Invalid(message) if message.starts_with("InvalidCursorError:") => {
                ApiError::new(StatusCode::BAD_REQUEST, "InvalidCursorError", message)
            }
            other => other.into(),
        })?;
    Ok(Json(Page {
        data: page.records,
        cursor: Cursor {
            next: page.next,
            ..Cursor::default()
        },
    }))
}
