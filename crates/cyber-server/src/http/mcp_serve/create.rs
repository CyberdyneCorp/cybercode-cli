//! `cyber_session_create` (`mcp-server` → Session creation over MCP): an ordinary
//! Session through `POST /api/v1/sessions`, bounded by the Mode ceiling and the
//! connection's quota, carrying the `exec-mode` non-interactive rules, and attributed to
//! the connection's `mcp_client` in the MCP serve records.
//!
//! Order of checks: a stored idempotency key replays first (only schema runs before it);
//! then the directory, the Mode ceiling, the depth and the quota; only then the first
//! write, which runs detached so that cancellation ends only the answer.

use std::path::Path;
use std::sync::Arc;

use cyber_store::mcp_serve::{self, McpServeSession};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::ApiError;
use super::super::idempotency::{self, RETENTION_MS};
use super::caller::Caller;
use super::connection::{Connection, Creation};
use super::read::{session_state, summary};
use super::route::{self, Call};
use super::schema::Violation;
use super::tools::{Outcome, ToolError, ToolOutput};

const TOOL: &str = "cyber_session_create";

pub(super) async fn create(connection: &Arc<Connection>, args: &Value, caller: &Caller) -> Outcome {
    let key = route_key(TOOL, args)?;
    let fingerprint = fingerprint(args);
    if let Some(key) = &key
        && let Some(stored) = replay(connection, key, key, &fingerprint).await?
    {
        let client = record(connection, &stored)?;
        return Ok(output(connection, &stored, client, true));
    }
    let prepared = prepare(connection, args, caller)?;
    let claim = key.clone().map(|key| (key, fingerprint));
    // The write and its record run detached: a cancelled call, an ended MCP session or a
    // closed connection ends only the answer, never a half-recorded creation.
    let task = Arc::clone(connection);
    let (created, client) = tokio::spawn(async move {
        let created = write(&task, prepared, key.as_deref(), claim).await?;
        let client = record(&task, &created)?;
        Ok::<_, ToolError>((created, client))
    })
    .await
    .map_err(ApiError::unknown)??;
    Ok(output(connection, &created, client, false))
}

/// A Session creation that passed every check (directory, Mode ceiling, quota) and
/// holds its quota slot until written.
pub(super) struct Prepared {
    directory: String,
    body: Value,
    creation: Creation,
}

/// Run the checks of a creation, in order, without writing.
pub(super) fn prepare(
    connection: &Arc<Connection>,
    args: &Value,
    caller: &Caller,
) -> Result<Prepared, ApiError> {
    let directory = directory(args)?;
    let settings = connection.state.services.mcp_serve()?;
    let plan = plan(connection, args, directory, &settings.max_mode)?;
    caller.check_depth(connection)?;
    let limit = settings.limits.sessions_per_connection;
    let creation = connection.reserve_creation(limit).ok_or_else(|| {
        ApiError::forbidden(format!(
            "this MCP connection has created its {limit} Sessions"
        ))
        .with_reason("session_quota")
        .with_field("limit", limit)
    })?;
    Ok(Prepared {
        directory: directory.display().to_string(),
        body: plan.body(args),
        creation,
    })
}

/// Create the prepared Session through `POST /api/v1/sessions` keyed `key`, after
/// storing `claim` (the key whose fingerprint identifies the call, and that fingerprint)
/// before the write. The result is the created `Session`.
pub(super) async fn write(
    connection: &Connection,
    prepared: Prepared,
    key: Option<&str>,
    claim: Option<(String, String)>,
) -> Result<Value, ToolError> {
    if let Some((key, fingerprint)) = claim {
        claim_key(connection, &key, &fingerprint)?;
    }
    let location = route::encode(&prepared.directory);
    let mut headers = vec![("x-cyber-directory", location.as_str())];
    if let Some(key) = key {
        headers.push(("idempotency-key", key));
    }
    let mut created = route::send(
        connection,
        "v1.session.create",
        Call {
            body: Some(&prepared.body),
            headers: &headers,
            ..Call::default()
        },
    )
    .await?;
    prepared.creation.keep();
    Ok(created["data"].take())
}

/// Store the argument fingerprint of `key` before its first write; a key already
/// claimed with other arguments refuses the call before it writes.
pub(super) fn claim_key(
    connection: &Connection,
    key: &str,
    fingerprint: &str,
) -> Result<(), ApiError> {
    let now = now_ms();
    let claimed = mcp_serve::claim_key(
        &connection.state.store,
        key,
        fingerprint,
        now,
        now - RETENTION_MS,
    )
    .map_err(ApiError::unknown)?;
    if claimed { Ok(()) } else { Err(mismatch()) }
}

fn mismatch() -> ApiError {
    ApiError::conflict("idempotency_key was used with different arguments")
        .with_reason("idempotency_mismatch")
}

/// The `server-api` `Idempotency-Key` of a tool call, from its 1..64 printable
/// `idempotency_key`.
pub(super) fn route_key(tool: &str, args: &Value) -> Result<Option<String>, ApiError> {
    let Some(key) = args["idempotency_key"].as_str() else {
        return Ok(None);
    };
    if !key.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(Violation {
            path: "/idempotency_key".into(),
            message: "must be printable ASCII without spaces".into(),
        }
        .into());
    }
    Ok(Some(format!("mcp:{tool}:{key}")))
}

/// The tool-level identity of a call: the arguments that make a different Session.
fn fingerprint(args: &Value) -> String {
    let fields = ["directory", "title", "agent", "model", "mode"].map(|key| args[key].clone());
    let digest = Sha256::digest(Value::from(fields.to_vec()).to_string().as_bytes());
    format!("{TOOL}:{digest:x}")
}

/// The stored 2xx body of the write keyed `key`, replayed before any other check; the
/// call is identified by the fingerprint stored with `identity`, and a stored response
/// without a matching fingerprint is a conflict.
pub(super) async fn replay(
    connection: &Connection,
    key: &str,
    identity: &str,
    fingerprint: &str,
) -> Result<Option<Value>, ToolError> {
    let store = &connection.state.store;
    let Some(stored) = idempotency::stored(store, key)
        .await
        .map_err(ApiError::unknown)?
    else {
        return Ok(None);
    };
    same_call(connection, identity, fingerprint)?;
    let body = serde_json::from_slice(&stored.body).unwrap_or(Value::Null);
    let mut answered = route::answer(key, stored.status, body)?;
    Ok(Some(answered["data"].take()))
}

/// Whether the call keyed `identity` is this one (its stored fingerprint matches); a
/// key without a stored fingerprint is another call.
pub(super) fn same_call(
    connection: &Connection,
    identity: &str,
    fingerprint: &str,
) -> Result<(), ApiError> {
    let store = &connection.state.store;
    let known = mcp_serve::key_fingerprint(store, identity, now_ms() - RETENTION_MS)
        .map_err(ApiError::unknown)?;
    if known.as_deref() == Some(fingerprint) {
        return Ok(());
    }
    Err(mismatch())
}

/// An existing absolute directory, as given (never canonicalized here: the route
/// resolves the Location itself).
fn directory(args: &Value) -> Result<&Path, ApiError> {
    let text = args["directory"].as_str().unwrap_or_default();
    let path = Path::new(text);
    if path.is_absolute() && path.is_dir() {
        Ok(path)
    } else {
        Err(
            ApiError::invalid(format!("{text} is not an existing absolute directory"))
                .with_reason("not_a_directory"),
        )
    }
}

/// What the Session will be created with: its agent and its bounded Mode.
struct Plan {
    agent: Option<String>,
    mode: String,
}

impl Plan {
    fn body(&self, args: &Value) -> Value {
        let mut body = json!({
            "mode": self.mode,
            "rules": cyber_core::config::never_ask_rules(),
        });
        for key in ["title", "model"] {
            if args[key].is_string() {
                body[key] = args[key].clone();
            }
        }
        if let Some(agent) = &self.agent {
            body["agent"] = json!(agent);
        }
        body
    }
}

/// Predict the Mode the server would resolve (explicit `mode`, else the agent's
/// `permission_mode`, else the Location's `mode`) and bound it by `ceiling`: refused
/// when it came from an explicit `mode` or `agent`, clamped otherwise.
fn plan(
    connection: &Connection,
    args: &Value,
    directory: &Path,
    ceiling: &str,
) -> Result<Plan, ApiError> {
    let state = &connection.state;
    let defaults = state.services.location_defaults(directory)?;
    let explicit_agent = args["agent"].as_str().map(str::to_owned);
    let agent = explicit_agent.clone().or(defaults.default_agent);
    let (requested, explicit) = match args["mode"].as_str() {
        Some(mode) => (mode.to_owned(), true),
        None => {
            let directory = directory.display().to_string();
            let from_agent = state
                .runtime
                .agent_permission_mode(&directory, agent.as_deref().unwrap_or("build"))?;
            match from_agent {
                Some(mode) => (mode, explicit_agent.is_some()),
                None => (defaults.mode.unwrap_or_else(|| "default".into()), false),
            }
        }
    };
    let mode = cyber_core::config::bound_mode(&requested, ceiling, explicit).ok_or_else(|| {
        ApiError::forbidden(format!(
            "Mode {requested} is above the MCP serve ceiling {ceiling}"
        ))
        .with_reason("mode_ceiling")
        .with_field("ceiling", ceiling)
    })?;
    Ok(Plan { agent, mode })
}

/// Keep (or heal) the Session's MCP serve record; the result is its recorded client.
pub(super) fn record(connection: &Connection, session: &Value) -> Result<String, ApiError> {
    let id = session["id"].as_str().unwrap_or_default().to_owned();
    let store = &connection.state.store;
    mcp_serve::record_session(
        store,
        &McpServeSession {
            session_id: id.clone(),
            mcp_client: connection.label.clone(),
            created_ms: session["created_ms"].as_i64().unwrap_or_else(now_ms),
        },
    )
    .map_err(ApiError::unknown)?;
    let mut clients =
        mcp_serve::session_clients(store, vec![id.clone()]).map_err(ApiError::unknown)?;
    Ok(clients
        .remove(&id)
        .unwrap_or_else(|| connection.label.clone()))
}

/// `{ session: SessionSummary, effective_mode, replayed? }`.
fn output(connection: &Connection, session: &Value, client: String, replayed: bool) -> ToolOutput {
    let id = session["id"].as_str().unwrap_or_default();
    let created_at = session["created_ms"].as_i64().unwrap_or_default();
    let runtime = &connection.state.runtime;
    let state = (session_state(runtime, id), runtime.run_mcp_depth(id));
    let mut summary = summary(session, created_at, None, state);
    summary["mcp_client"] = json!(client);
    let mut value = json!({ "session": summary, "effective_mode": session["mode"] });
    if replayed {
        value["replayed"] = json!(true);
    }
    ToolOutput {
        value,
        untrusted: true,
    }
}

pub(super) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
