//! MCP serve records (`mcp-server`): which MCP client created a Session or admitted an
//! input, at what depth, and the argument fingerprint stored with each idempotency key.
//! They feed `SessionSummary.mcp_client`, the `mcp_only` list filter, `recent_prompts`
//! and usage attribution; the Sessions themselves stay ordinary Sessions.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params, params_from_iter};

use crate::{Store, StoreError};

/// A Session created through MCP serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServeSession {
    pub session_id: String,
    pub mcp_client: String,
    pub created_ms: i64,
}

/// An input (prompt or message) admitted through MCP serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServeInput {
    pub message_id: String,
    pub session_id: String,
    pub mcp_client: String,
    pub depth: u32,
    pub admitted_ms: i64,
    pub idempotency_key: Option<String>,
}

/// Keep the first record of a Session; recording it again is a no-op.
pub fn record_session(store: &Store, record: &McpServeSession) -> Result<(), StoreError> {
    let record = record.clone();
    store.transaction(move |tx| {
        tx.execute(
            "INSERT OR IGNORE INTO mcp_serve_session (session_id, mcp_client, created_ms) VALUES (?1, ?2, ?3)",
            params![record.session_id, record.mcp_client, record.created_ms],
        )?;
        Ok(())
    })
}

/// The `mcp_client` of each of `ids` created through MCP serve; others are absent.
pub fn session_clients(
    store: &Store,
    ids: Vec<String>,
) -> Result<HashMap<String, String>, StoreError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    store.read(move |conn| {
        let mut clients = HashMap::new();
        // SQLite bounds host parameters; 500 per query stays well inside every build.
        for chunk in ids.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let mut stmt = conn.prepare(&format!(
                "SELECT session_id, mcp_client FROM mcp_serve_session WHERE session_id IN ({marks})"
            ))?;
            let rows = stmt.query_map(params_from_iter(chunk), |r| Ok((r.get(0)?, r.get(1)?)))?;
            for row in rows {
                let (id, client): (String, String) = row?;
                clients.insert(id, client);
            }
        }
        Ok(clients)
    })
}

/// Keep the first record of an admitted input.
pub fn record_input(store: &Store, input: &McpServeInput) -> Result<(), StoreError> {
    let input = input.clone();
    store.transaction(move |tx| {
        tx.execute(
            "INSERT OR IGNORE INTO mcp_serve_input (message_id, session_id, mcp_client, depth, admitted_ms, idempotency_key) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                input.message_id,
                input.session_id,
                input.mcp_client,
                input.depth,
                input.admitted_ms,
                input.idempotency_key
            ],
        )?;
        Ok(())
    })
}

/// The record of one input admitted through MCP serve.
pub fn input(store: &Store, message_id: &str) -> Result<Option<McpServeInput>, StoreError> {
    let message_id = message_id.to_owned();
    store.read(move |conn| {
        Ok(conn
            .query_row(
                "SELECT message_id, session_id, mcp_client, depth, admitted_ms, idempotency_key FROM mcp_serve_input WHERE message_id = ?1",
                [message_id],
                input_row,
            )
            .optional()?)
    })
}

fn input_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<McpServeInput> {
    Ok(McpServeInput {
        message_id: r.get(0)?,
        session_id: r.get(1)?,
        mcp_client: r.get(2)?,
        depth: r.get(3)?,
        admitted_ms: r.get(4)?,
        idempotency_key: r.get(5)?,
    })
}

/// The most recent inputs admitted to a Session through MCP serve, newest first.
pub fn recent_inputs(
    store: &Store,
    session_id: &str,
    limit: usize,
) -> Result<Vec<McpServeInput>, StoreError> {
    let session_id = session_id.to_owned();
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    store.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT message_id, session_id, mcp_client, depth, admitted_ms, idempotency_key FROM mcp_serve_input WHERE session_id = ?1 ORDER BY admitted_ms DESC, message_id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![session_id, limit], input_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    })
}

/// Store the argument fingerprint of an idempotency key before its first write, and drop
/// fingerprints created at or before `expired_ms`. A live fingerprint is never replaced:
/// the result is whether the key now holds `fingerprint` (false: another call owns it).
pub fn claim_key(
    store: &Store,
    key: &str,
    fingerprint: &str,
    now_ms: i64,
    expired_ms: i64,
) -> Result<bool, StoreError> {
    let (key, fingerprint) = (key.to_owned(), fingerprint.to_owned());
    store.transaction(move |tx| {
        tx.execute(
            "DELETE FROM mcp_serve_key WHERE created_ms <= ?1",
            [expired_ms],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO mcp_serve_key (key, fingerprint, created_ms) VALUES (?1, ?2, ?3)",
            params![key, fingerprint, now_ms],
        )?;
        let stored: String = tx.query_row(
            "SELECT fingerprint FROM mcp_serve_key WHERE key = ?1",
            [&key],
            |r| r.get(0),
        )?;
        Ok(stored == fingerprint)
    })
}

/// The fingerprint stored with `key` after `expired_ms`.
pub fn key_fingerprint(
    store: &Store,
    key: &str,
    expired_ms: i64,
) -> Result<Option<String>, StoreError> {
    let key = key.to_owned();
    store.read(move |conn| {
        Ok(conn
            .query_row(
                "SELECT fingerprint FROM mcp_serve_key WHERE key = ?1 AND created_ms > ?2",
                params![key, expired_ms],
                |r| r.get(0),
            )
            .optional()?)
    })
}
