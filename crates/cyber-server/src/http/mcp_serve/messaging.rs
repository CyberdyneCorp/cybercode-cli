//! The messaging tools (`mcp-server` → Messaging over MCP; design section 16 of
//! `implement-cross-session-messaging`): `cyber_message_list`, `cyber_message_send` and
//! `cyber_message_history` over the `cross-session-messaging` Messaging API.
//!
//! The history tool calls its route in-process. `cyber_message_list` and
//! `cyber_message_send` call the pipeline their routes call ([`Messaging::send`] and
//! the reachable listing) directly: without a caller Session the sender is the MCP client
//! `mcp:<label>`, which no HTTP client label may claim, and the send route has no way to
//! carry the request depth: the message stores it, so the run that reads the message is
//! raised to it ("MCP recursion guard"). Addressing, inbound rules, rate limits, outcomes
//! and errors are therefore the routes' own.
//!
//! Order of checks for a send: schema, then a key claimed by this same call whose message
//! was recorded replays (the pipeline returns the recorded outcome); then the text
//! limits, the depth, the address and the self-target guard; only then the key claim, the
//! MCP serve record and the send, on a detached task so a cancelled call never
//! half-sends. A claimed key without a recorded message (a client's `rate_limited`
//! refusal records nothing, and a send can fail after the claim) is checked in full
//! again, at this call's depth and target.
//!
//! [`Messaging::send`]: crate::runtime::messaging::Messaging::send

use std::sync::Arc;

use cyber_store::mcp_serve::{self, McpServeInput};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::ApiError;
use super::super::idempotency::{PRINCIPAL, RETENTION_MS};
use super::caller::{self, Caller};
use super::connection::Connection;
use super::create;
use super::read::{self, Cuttable, ITEM_CAP, Pager};
use super::route;
use super::tools::{Outcome, ToolError, ToolOutput};
use crate::runtime::RuntimeError;
use crate::runtime::messaging::address::AddressError;
use crate::runtime::messaging::limits;
use crate::runtime::messaging::send::idempotent_id;
use crate::runtime::messaging::{SendRequest, Sender};

const SEND: &str = "cyber_message_send";

/// Who the call sends or lists as: the caller Session when known, else the MCP client,
/// whose records show `mcp:<label>`, whose rate window keys on it and whose inbound
/// addresses are `mcp:<label>` and `client`.
fn sender(connection: &Connection, caller: &Caller) -> Sender {
    match &caller.session_id {
        Some(id) => Sender::Session(id.clone()),
        None => Sender::Mcp(connection.label.clone()),
    }
}

// cyber_message_list

/// The listing `GET /messaging/sessions` returns, as the caller sees it. It is read from
/// the pipeline the route calls, because no HTTP client label may name an MCP client.
pub(super) async fn list(connection: &Connection, args: &Value, caller: &Caller) -> Outcome {
    let directory = args["directory"]
        .as_str()
        .map(read::absolute_directory)
        .transpose()?;
    if let Some(directory) = directory.as_deref()
        && !std::path::Path::new(directory).exists()
    {
        return Err(ApiError::invalid(format!("no such directory: {directory}")).into());
    }
    let sessions = connection
        .state
        .runtime
        .messaging()
        .reachable_as(&sender(connection, caller), directory.as_deref())
        .map_err(ApiError::from)?;
    Ok(ToolOutput {
        value: json!({ "sessions": sessions }),
        untrusted: true,
    })
}

// cyber_message_send

/// The tool-level identity of a send: its target address and text.
fn fingerprint(args: &Value) -> String {
    let fields = Value::from(vec![args["to"].clone(), args["text"].clone()]);
    let digest = Sha256::digest(fields.to_string().as_bytes());
    format!("{SEND}:{digest:x}")
}

/// Whether `key` was claimed by this same call (a retry), by another call (a conflict
/// before any write), or not at all.
fn claimed(connection: &Connection, key: &str, fingerprint: &str) -> Result<bool, ApiError> {
    let store = &connection.state.store;
    let known = mcp_serve::key_fingerprint(store, key, create::now_ms() - RETENTION_MS)
        .map_err(ApiError::unknown)?;
    match known {
        None => Ok(false),
        Some(_) => create::same_call(connection, key, fingerprint).map(|()| true),
    }
}

/// One checked send, written by [`Send::run`].
struct Send {
    connection: Arc<Connection>,
    request: SendRequest,
    /// The resolved target; `None` for a replay, whose record exists already.
    target: Option<String>,
    key: Option<String>,
    /// Whether this call still claims `key` (an earlier attempt did not).
    claim: bool,
    fingerprint: String,
}

pub(super) async fn send(connection: &Arc<Connection>, args: &Value, caller: &Caller) -> Outcome {
    let key = create::route_key(SEND, args)?;
    let fingerprint = fingerprint(args);
    let claimed = match &key {
        Some(key) => claimed(connection, key, &fingerprint)?,
        None => false,
    };
    let to = args["to"].as_str().unwrap_or_default().to_owned();
    let text = args["text"].as_str().unwrap_or_default().to_owned();
    let mut request = SendRequest {
        id: Some(match &key {
            Some(key) => idempotent_id(PRINCIPAL, key),
            None => cyber_core::ids::new_id("msgx"),
        }),
        from: sender(connection, caller),
        to,
        text,
        depth: 0,
        expected_target: None,
        notify_when_idle: false,
    };
    let id = request.id.as_deref().unwrap_or_default();
    let replay = claimed
        && (connection.state.runtime.messaging())
            .recorded(id, &request.from)
            .map_err(ApiError::from)?;
    let target = if replay {
        None
    } else {
        Some(check(connection, caller, &mut request)?)
    };
    let send = Send {
        connection: Arc::clone(connection),
        target,
        request,
        key,
        claim: !claimed,
        fingerprint,
    };
    let value = tokio::spawn(send.run())
        .await
        .map_err(ApiError::unknown)??;
    Ok(ToolOutput {
        value,
        untrusted: false,
    })
}

/// The checks before any write: the text limits, the depth, the address and the
/// self-target guard. The result is the target Session, which the send must still name.
fn check(
    connection: &Connection,
    caller: &Caller,
    request: &mut SendRequest,
) -> Result<String, ApiError> {
    limits::check_text(&request.text).map_err(RuntimeError::from)?;
    request.depth = caller.check_depth(connection)?;
    let messaging = connection.state.runtime.messaging();
    let target = match messaging.resolve(request.from.session_id(), &request.to) {
        Ok(target) => target.candidate.session_id,
        Err(RuntimeError::Address(AddressError::SelfTarget)) => {
            return Err(caller::self_target());
        }
        Err(error) => return Err(error.into()),
    };
    caller.check_target(connection, &target)?;
    request.expected_target = Some(target.clone());
    Ok(target)
}

impl Send {
    async fn run(self) -> Result<Value, ToolError> {
        let connection = &self.connection;
        if let Some(target) = &self.target {
            if let Some(key) = self.key.as_ref().filter(|_| self.claim) {
                create::claim_key(connection, key, &self.fingerprint)?;
            }
            // Recorded before the admission, so the Turn the message starts is already
            // attributed to `mcp` when it begins.
            self.record(target)?;
        }
        let result = connection
            .state
            .runtime
            .messaging()
            .send(self.request)
            .await
            .map_err(ApiError::from)?;
        let mut value = json!({
            "message_id": result.message_id,
            "outcome": result.outcome,
            "to": result.to,
        });
        if let Some(reason) = result.reason {
            value["reason"] = json!(reason);
        }
        if let Some(limit) = result.limit {
            value["limit"] = json!(limit);
        }
        if result.replayed {
            value["replayed"] = json!(true);
        }
        Ok(value)
    }

    /// The MCP serve record of the message in its target, with the client label and the
    /// request depth. It replaces the record of an earlier attempt that sent nothing.
    fn record(&self, target: &str) -> Result<(), ApiError> {
        mcp_serve::replace_input(
            &self.connection.state.store,
            &McpServeInput {
                message_id: self.request.id.clone().unwrap_or_default(),
                session_id: target.to_owned(),
                mcp_client: self.connection.label.clone(),
                depth: self.request.depth,
                admitted_ms: create::now_ms(),
                idempotency_key: self.key.clone(),
            },
        )
        .map_err(ApiError::unknown)
    }
}

// cyber_message_history

/// One history record as a page item; its text is the part cut to fit.
fn item(record: &Value) -> Cuttable {
    let created_at = record["created_at"]
        .as_str()
        .and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
        .map(|time| time.timestamp_millis());
    let mut base = json!({
        "message_id": record["message_id"],
        "direction": record["direction"],
        "from": record["from"],
        "to": record["to"],
        "outcome": record["outcome"],
        "size": record["size"],
        "created_at": created_at,
    });
    if record["reason"].is_string() {
        base["reason"] = record["reason"].clone();
    }
    let fields = match record["text"].as_str() {
        Some(text) => vec![(None, "text", text.to_owned())],
        None => Vec::new(),
    };
    Cuttable { base, fields }
}

/// One page of `GET /sessions/:id/messaging`, oldest first.
async fn page(
    connection: &Connection,
    session_id: &str,
    cursor: Option<&str>,
    limit: usize,
) -> Result<Value, ToolError> {
    let limit = limit.to_string();
    let mut query = vec![("order", "asc"), ("limit", limit.as_str())];
    if let Some(cursor) = cursor {
        query.push(("cursor", cursor));
    }
    route::route(
        connection,
        "v1.session.messaging",
        &[("sessionID", session_id)],
        &query,
    )
    .await
}

pub(super) async fn history(connection: &Connection, args: &Value) -> Outcome {
    let session_id = args["session_id"].as_str().unwrap_or_default();
    let cursor = args["cursor"].as_str();
    let limit = read::count(args, "limit", 50);
    let mut pager = Pager::new(read::count(args, "max_bytes", read::DEFAULT_MAX_BYTES));
    let fetched = page(connection, session_id, cursor, limit).await?;
    let records = fetched["data"].as_array().map_or(&[][..], Vec::as_slice);
    let kept = records
        .iter()
        .take_while(|record| pager.push(&item(record), ITEM_CAP))
        .count();
    let next = if kept == records.len() {
        fetched["cursor"]["next"].as_str().map(str::to_owned)
    } else {
        // `max_bytes` ended the page early: the cursor after the kept records is that of
        // the same page asked for only them (oldest first, so the prefix is stable).
        let shorter = page(connection, session_id, cursor, kept).await?;
        shorter["cursor"]["next"].as_str().map(str::to_owned)
    };
    let mut value = json!({ "messages": pager.items, "truncated": pager.truncated });
    if let Some(next) = next {
        value["next_cursor"] = json!(next);
    }
    Ok(ToolOutput {
        value,
        untrusted: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_send_identity_is_the_target_and_the_text() {
        let base = json!({"to": "beta", "text": "schema v2 is merged", "idempotency_key": "k"});
        let mut other = base.clone();
        other["idempotency_key"] = json!("other");
        assert_eq!(fingerprint(&base), fingerprint(&other));
        other["to"] = json!("gamma");
        assert_ne!(fingerprint(&base), fingerprint(&other));
        let mut text = base.clone();
        text["text"] = json!("schema v3");
        assert_ne!(fingerprint(&base), fingerprint(&text));
    }

    #[test]
    fn history_items_keep_the_spec_fields_and_cut_only_the_text() {
        let record = json!({"message_id": "msgx_1", "session_id": "ses_b", "direction": "in",
            "from": "mcp:zed", "to": "ses_b", "origin_kind": "client",
            "outcome": "refused", "reason": "non_interactive", "size": 5, "sha256": "ab",
            "depth": 0, "hops": 1, "created_at": "2026-10-19T10:00:00.250Z",
            "updated_at": "2026-10-19T10:00:00.250Z", "seq": 4});
        let item = item(&record);
        assert!(item.fields.is_empty(), "a refused record has no text");
        assert_eq!(item.base["created_at"], 1_792_404_000_250_i64);
        assert_eq!(item.base["reason"], "non_interactive");
        assert!(item.base.get("sha256").is_none());
        let mut delivered = record;
        delivered["text"] = json!("hello");
        assert_eq!(super::item(&delivered).fields.len(), 1);
    }
}
