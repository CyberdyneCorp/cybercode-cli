//! PTY attach (`background-tasks` → PTY sessions): a client asks for a single-use ticket
//! with its normal credentials, then opens the WebSocket with it, since browsers cannot
//! set headers on one. The socket replays the retained output from `cursor` (omitted:
//! the whole buffer; `-1`: none), then streams live output; client frames are input.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{SinkExt as _, StreamExt as _};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;

use super::{ApiError, AppState, envelope::Data};
use crate::runtime::{Pty, PtyChunk, PtyCursor};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/jobs/{id}/pty/ticket", post(ticket))
        .route("/jobs/{id}/pty/connect", get(connect))
}

/// Whether `path` is the PTY attach socket, which authenticates with its ticket.
pub(super) fn attach_route(path: &str) -> bool {
    path.contains("/jobs/") && path.ends_with("/pty/connect")
}

/// A single-use ticket for `GET /jobs/{id}/pty/connect`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PtyTicket {
    pub ticket: String,
    /// Seconds until the ticket expires.
    pub expires_in: u64,
}

fn running(state: &AppState, id: &str) -> Result<std::sync::Arc<Pty>, ApiError> {
    state
        .runtime
        .pty(id)
        .ok_or_else(|| ApiError::not_found("PtyNotFoundError", format!("No running PTY {id}")))
}

async fn ticket(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<PtyTicket>>, ApiError> {
    running(&state, &id)?;
    Ok(Json(Data {
        data: PtyTicket {
            ticket: state.runtime.issue_pty_ticket(&id)?,
            expires_in: crate::runtime::pty::TICKET_TTL.as_secs(),
        },
    }))
}

#[derive(Deserialize)]
struct Attach {
    #[serde(default)]
    ticket: String,
    cursor: Option<i64>,
}

async fn connect(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<Attach>,
    ws: WebSocketUpgrade,
) -> Response {
    if !state.runtime.redeem_pty_ticket(&query.ticket, &id) {
        return ApiError::unauthorized()
            .with_reason("invalid_ticket")
            .into_response();
    }
    let cursor = match PtyCursor::parse(query.cursor) {
        Ok(cursor) => cursor,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let pty = match running(&state, &id) {
        Ok(pty) => pty,
        Err(error) => return error.into_response(),
    };
    ws.on_upgrade(move |socket| serve(state, pty, cursor, socket))
}

fn control(value: serde_json::Value) -> Message {
    Message::Text(value.to_string().into())
}

async fn serve(state: AppState, pty: std::sync::Arc<Pty>, cursor: PtyCursor, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let input = std::sync::Arc::clone(&pty);
    let reader = tokio::spawn(async move {
        while let Some(Ok(message)) = stream.next().await {
            let bytes = match message {
                Message::Binary(bytes) => bytes.to_vec(),
                Message::Text(text) => text.as_bytes().to_vec(),
                Message::Close(_) => break,
                _ => continue,
            };
            if input.write(&bytes).is_err() {
                break;
            }
        }
    });
    let _ = tokio::select! {
        biased;
        _ = state.runtime.shutting_down() => Ok(()),
        sent = stream_output(&pty, cursor, &mut sink) => sent,
    };
    reader.abort();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        sink.send(Message::Close(None)),
    )
    .await;
}

/// Replay from `cursor`, then send live output until the program exits. A subscriber
/// that falls behind resumes from the buffer at the cursor it reached.
async fn stream_output(
    pty: &Pty,
    cursor: PtyCursor,
    sink: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), axum::Error> {
    let mut attach = pty.attach(cursor);
    sink.send(control(
        json!({"type": "attached", "job_id": pty.job_id, "cursor": attach.start}),
    ))
    .await?;
    loop {
        let mut next = attach.end();
        if !attach.replay.is_empty() {
            sink.send(Message::Binary(std::mem::take(&mut attach.replay).into()))
                .await?;
        }
        if attach.closed {
            break;
        }
        loop {
            match attach.live.recv().await {
                Ok(PtyChunk::Output { cursor, bytes }) => {
                    // Chunks already replayed are skipped; a partial overlap is trimmed.
                    let end = cursor + bytes.len() as u64;
                    if end <= next {
                        continue;
                    }
                    let skip = usize::try_from(next.saturating_sub(cursor)).unwrap_or(0);
                    sink.send(Message::Binary(bytes[skip..].to_vec().into()))
                        .await?;
                    next = end;
                }
                Ok(PtyChunk::Closed) | Err(RecvError::Closed) => {
                    return sink
                        .send(control(json!({"type": "exit", "cursor": next})))
                        .await;
                }
                Err(RecvError::Lagged(_)) => break,
            }
        }
        attach = pty.attach(PtyCursor::At(next));
    }
    sink.send(control(json!({"type": "exit", "cursor": attach.end()})))
        .await
}
