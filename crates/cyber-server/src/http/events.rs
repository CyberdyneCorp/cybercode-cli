//! Event streams (`server-api` → Instance event stream, Durable session replay).

use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use super::AppState;
use super::envelope::{location, query_value};
use super::error::ApiError;
use crate::runtime::{LiveEvent, Runtime};

const HEARTBEAT: Duration = Duration::from_secs(15);

/// Where a durable event sits in its aggregate's history.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DurableRef {
    #[serde(rename = "aggregateID")]
    pub aggregate_id: String,
    pub seq: i64,
    pub version: u32,
}

/// One event on any stream.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EventEnvelope {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
    /// The Location directory the event belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durable: Option<DurableRef>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HistoryPage {
    pub data: Vec<EventEnvelope>,
    #[serde(rename = "hasMore")]
    pub has_more: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct HistoryQuery {
    pub after: Option<i64>,
    pub limit: Option<u32>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/event", get(instance))
        .route("/sessions/{session_id}/events", get(session_events))
        .route("/sessions/{session_id}/history", get(history))
}

/// `session.prompt.admitted.1` → version 1.
fn version_of(kind: &str) -> u32 {
    kind.rsplit_once('.')
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(1)
}

fn durable(session_id: &str, seq: i64, kind: &str, data: Value) -> EventEnvelope {
    EventEnvelope {
        id: format!("{session_id}:{seq}"),
        kind: kind.to_string(),
        data,
        location: None,
        durable: Some(DurableRef {
            aggregate_id: session_id.into(),
            seq,
            version: version_of(kind),
        }),
    }
}

/// The envelope for a live event; the session ID is kept inside `data`.
pub fn envelope(event: &LiveEvent, counter: u64) -> EventEnvelope {
    if let LiveEvent::Durable {
        session_id,
        seq,
        kind,
        data,
    } = event
    {
        return durable(session_id, *seq, kind, data.clone());
    }
    let mut data = serde_json::to_value(event).unwrap_or_default();
    let kind = data
        .as_object_mut()
        .and_then(|m| m.remove("type"))
        .and_then(|t| t.as_str().map(str::to_string))
        .unwrap_or_default();
    EventEnvelope {
        id: format!("live:{counter}"),
        kind: format!("session.{}", kind.replace('_', ".")),
        data,
        location: None,
        durable: None,
    }
}

fn session_of(event: &LiveEvent) -> &str {
    match event {
        LiveEvent::Durable { session_id, .. }
        | LiveEvent::TextDelta { session_id, .. }
        | LiveEvent::ReasoningDelta { session_id, .. }
        | LiveEvent::ToolInputDelta { session_id, .. }
        | LiveEvent::Retry { session_id, .. }
        | LiveEvent::Usage { session_id, .. }
        | LiveEvent::Error { session_id, .. }
        | LiveEvent::Idle { session_id }
        | LiveEvent::Deleted { session_id } => session_id,
    }
}

fn sse_event(e: &EventEnvelope) -> Event {
    Event::default()
        .id(e.id.clone())
        .event(e.kind.clone())
        .json_data(e)
        .unwrap_or_default()
}

fn spawn_stream(
    runtime: Runtime,
    tx: mpsc::Sender<Event>,
    producer: impl std::future::Future<Output = ()> + Send + 'static,
) {
    tokio::spawn(async move {
        tokio::select! {
            biased;
            _ = runtime.shutting_down() => {},
            _ = tx.closed() => {},
            _ = producer => {},
        }
    });
}

fn stream_response(rx: mpsc::Receiver<Event>, runtime: Runtime) -> Response {
    let stream = tokio_stream::StreamExt::map(ReceiverStream::new(rx), Ok::<Event, Infallible>);
    let stream =
        futures::StreamExt::take_until(stream, async move { runtime.shutting_down().await });
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(HEARTBEAT).text(" heartbeat"))
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

/// Live events for one Location (`scope=all` for every Location).
async fn instance(State(state): State<AppState>, parts: Parts) -> Result<Response, ApiError> {
    let all = parts
        .uri
        .query()
        .and_then(|q| query_value(q, "scope"))
        .as_deref()
        == Some("all");
    let directory = location(&parts, &state.options.default_directory)?
        .display()
        .to_string();
    let (tx, rx) = mpsc::channel(256);
    let mut live = state.runtime.subscribe();
    let runtime = state.runtime.clone();
    spawn_stream(runtime.clone(), tx.clone(), async move {
        let hello = EventEnvelope {
            id: "live:0".into(),
            kind: "server.connected".into(),
            data: json!({}),
            location: None,
            durable: None,
        };
        if tx.send(sse_event(&hello)).await.is_err() {
            return;
        }
        let mut directories: HashMap<String, String> = HashMap::new();
        let mut counter = 0u64;
        loop {
            let event = match live.recv().await {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            counter += 1;
            let session = session_of(&event).to_string();
            let dir = match directories.get(&session) {
                Some(d) => d.clone(),
                None => {
                    let d = state
                        .runtime
                        .state(&session)
                        .await
                        .map(|s| s.info.directory)
                        .unwrap_or_default();
                    directories.insert(session.clone(), d.clone());
                    d
                }
            };
            if !all && dir != directory {
                continue;
            }
            let mut e = envelope(&event, counter);
            e.location = Some(dir);
            if tx.send(sse_event(&e)).await.is_err() {
                return;
            }
        }
    });
    Ok(stream_response(rx, runtime))
}

/// Replay durable events after `after` (or `Last-Event-ID`), then follow new ones without gaps.
async fn session_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    parts: Parts,
) -> Result<Response, ApiError> {
    let after = parts
        .uri
        .query()
        .and_then(|q| query_value(q, "after"))
        .or_else(|| {
            parts
                .headers
                .get("last-event-id")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.rsplit(':').next().unwrap_or(v).to_string())
        });
    let after = match after {
        Some(a) => a.parse::<i64>().map_err(|_| {
            ApiError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "InvalidCursorError",
                "after must be a sequence number",
            )
        })?,
        None => -1,
    };
    let directory = state.runtime.state(&id).await?.info.directory;
    // Subscribe before replaying so nothing committed in between is missed.
    let live = state.runtime.subscribe();
    let (tx, rx) = mpsc::channel(256);
    let runtime = state.runtime.clone();
    spawn_stream(
        runtime.clone(),
        tx.clone(),
        follow(state, id, directory, after, live, tx),
    );
    Ok(stream_response(rx, runtime))
}

async fn follow(
    state: AppState,
    id: String,
    directory: String,
    mut last: i64,
    mut live: broadcast::Receiver<LiveEvent>,
    tx: mpsc::Sender<Event>,
) {
    if !replay(&state, &id, &directory, &mut last, &tx).await {
        return;
    }
    loop {
        match live.recv().await {
            Ok(LiveEvent::Durable {
                session_id,
                seq,
                kind,
                data,
            }) if session_id == id && seq > last => {
                if seq > last + 1 && !replay(&state, &id, &directory, &mut last, &tx).await {
                    return;
                }
                if seq > last {
                    last = seq;
                    let mut e = durable(&id, seq, &kind, data);
                    e.location = Some(directory.clone());
                    if tx.send(sse_event(&e)).await.is_err() {
                        return;
                    }
                }
            }
            Ok(LiveEvent::Deleted { session_id }) if session_id == id => return,
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => {
                if !replay(&state, &id, &directory, &mut last, &tx).await {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Send stored events after `last`; false when the client went away.
async fn replay(
    state: &AppState,
    id: &str,
    directory: &str,
    last: &mut i64,
    tx: &mpsc::Sender<Event>,
) -> bool {
    loop {
        let Ok(page) = read_page(state, id, *last, 500).await else {
            return false;
        };
        for stored in page.events {
            *last = stored.seq;
            let mut e = durable(id, stored.seq, &stored.kind, stored.data);
            e.location = Some(directory.to_string());
            if tx.send(sse_event(&e)).await.is_err() {
                return false;
            }
        }
        if !page.has_more {
            return true;
        }
    }
}

async fn read_page(
    state: &AppState,
    id: &str,
    after: i64,
    limit: u32,
) -> Result<cyber_store::EventPage, ApiError> {
    let (store, id) = (std::sync::Arc::clone(&state.store), id.to_string());
    tokio::task::spawn_blocking(move || store.read_events(&id, after, limit))
        .await
        .map_err(ApiError::unknown)?
        .map_err(ApiError::unknown)
}

async fn history(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<Json<HistoryPage>, ApiError> {
    state.runtime.state(&id).await?;
    let limit = q.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(ApiError::invalid("limit must be between 1 and 500"));
    }
    let page = read_page(&state, &id, q.after.unwrap_or(-1), limit).await?;
    let data = page
        .events
        .into_iter()
        .map(|e| durable(&id, e.seq, &e.kind, e.data))
        .collect();
    Ok(Json(HistoryPage {
        data,
        has_more: page.has_more,
    }))
}
