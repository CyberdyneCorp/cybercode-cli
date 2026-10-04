//! `Idempotency-Key` handling (`server-api` → Idempotency keys).

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use cyber_store::Store;
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};

use super::AppState;
use super::error::ApiError;

const RETENTION_MS: i64 = 24 * 3600 * 1000;
const MAX_BODY: usize = 32 * 1024 * 1024;
/// Single-user server: every authenticated caller is the same principal.
const PRINCIPAL: &str = "cyber";

struct Stored {
    hash: String,
    status: u16,
    body: Vec<u8>,
    content_type: String,
}

/// Keys whose first request is still running.
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static KEYS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    KEYS.get_or_init(Mutex::default)
}

pub async fn layer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }
    let Some(key) = req
        .headers()
        .get("idempotency-key")
        .map(|v| v.to_str().map(str::to_string))
    else {
        return next.run(req).await;
    };
    let Some(key) = key
        .ok()
        .filter(|k| (1..=128).contains(&k.len()) && k.bytes().all(|b| (0x21..=0x7e).contains(&b)))
    else {
        return ApiError::invalid("Idempotency-Key must be 1-128 printable characters")
            .into_response();
    };
    let (parts, body) = req.into_parts();
    let Ok(bytes) = body.collect().await.map(|b| b.to_bytes()) else {
        return ApiError::invalid("could not read the request body").into_response();
    };
    if bytes.len() > MAX_BODY {
        return ApiError::invalid("request body too large").into_response();
    }
    let hash = request_hash(parts.method.as_str(), parts.uri.path(), &bytes);
    match lookup(&state.store, &key).await {
        Ok(Some(stored)) if stored.hash == hash => return replay(stored),
        Ok(Some(_)) => {
            return ApiError::conflict("Idempotency-Key was used with a different request")
                .into_response();
        }
        Ok(None) => {}
        Err(e) => return ApiError::unknown(e).into_response(),
    }
    if !in_flight()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key.clone())
    {
        return ApiError::conflict("a request with this Idempotency-Key is in progress")
            .into_response();
    }
    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    let response = remember(&state.store, &key, hash, response).await;
    in_flight()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&key);
    response
}

fn request_hash(method: &str, path: &str, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(method.as_bytes());
    h.update([0]);
    h.update(path.as_bytes());
    h.update([0]);
    h.update(body);
    format!("{:x}", h.finalize())
}

fn replay(stored: Stored) -> Response {
    let status = StatusCode::from_u16(stored.status).unwrap_or(StatusCode::OK);
    let mut response = (status, stored.body).into_response();
    if let Ok(ct) = HeaderValue::from_str(&stored.content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, ct);
    }
    response
        .headers_mut()
        .insert("idempotent-replayed", HeaderValue::from_static("true"));
    response
}

/// Store non-5xx responses so a retry replays them.
async fn remember(store: &Arc<Store>, key: &str, hash: String, response: Response) -> Response {
    if response.status().is_server_error() {
        return response;
    }
    let (parts, body) = response.into_parts();
    let Ok(bytes) = body.collect().await.map(|b| b.to_bytes()) else {
        return ApiError::unknown("response body").into_response();
    };
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let stored = Stored {
        hash,
        status: parts.status.as_u16(),
        body: bytes.to_vec(),
        content_type,
    };
    if let Err(e) = insert(store, key, stored).await {
        eprintln!("cyber server: could not store idempotency key: {e}");
    }
    Response::from_parts(parts, Body::from(bytes))
}

async fn lookup(store: &Arc<Store>, key: &str) -> Result<Option<Stored>, String> {
    let (store, key) = (Arc::clone(store), key.to_string());
    tokio::task::spawn_blocking(move || {
        let cutoff = now_ms() - RETENTION_MS;
        store
            .read(move |conn| {
                use rusqlite::OptionalExtension;
                Ok(conn
                    .query_row(
                        "SELECT request_hash, status, body, content_type FROM idempotency_key WHERE principal = ?1 AND key = ?2 AND created_ms > ?3",
                        rusqlite::params![PRINCIPAL, key, cutoff],
                        |r| Ok(Stored { hash: r.get(0)?, status: r.get(1)?, body: r.get(2)?, content_type: r.get(3)? }),
                    )
                    .optional()?)
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn insert(store: &Arc<Store>, key: &str, s: Stored) -> Result<(), String> {
    let (store, key) = (Arc::clone(store), key.to_string());
    tokio::task::spawn_blocking(move || {
        let now = now_ms();
        store
            .transaction(move |tx| {
                tx.execute("DELETE FROM idempotency_key WHERE created_ms <= ?1", [now - RETENTION_MS])?;
                tx.execute(
                    "INSERT OR REPLACE INTO idempotency_key (principal, key, request_hash, status, body, content_type, created_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![PRINCIPAL, key, s.hash, s.status, s.body, s.content_type, now],
                )?;
                Ok(())
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
