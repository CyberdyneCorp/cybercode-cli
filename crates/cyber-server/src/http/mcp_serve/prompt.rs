//! `cyber_prompt` (`mcp-server` → Prompt admission over MCP): a prompt admitted through
//! the public user path `POST /api/v1/sessions/:id/prompt` (`source: "user"`, so
//! `UserPromptSubmit` hooks run and the text is never resolved as a slash command), to
//! an existing Session or to one created as `cyber_session_create` would.
//!
//! Order of checks: schema, `max_waits`, then a stored `:prompt` key replays; then the
//! exclusive fields, the size, the depth and self-target guard, `max_running_sessions`
//! (reserved atomically) and, for a new Session, its own replay or the creation checks.
//! Only then the writes, on a detached task, so a call cancelled meanwhile still creates
//! and admits.

use std::collections::HashSet;
use std::sync::Arc;

use cyber_store::mcp_serve::{self, McpServeInput};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::ApiError;
use super::caller::Caller;
use super::connection::{Connection, RunSlot, WaitSlot};
use super::create::{self, Prepared};
use super::progress::Progress;
use super::route::{self, Call};
use super::tools::{Outcome, ToolError, ToolOutput};
use super::wait::{self, observe, reserve_wait, result};

const TOOL: &str = "cyber_prompt";
/// Largest accepted prompt, in bytes.
const MAX_PROMPT: usize = 128 * 1024;
/// Arguments that make a different call under the same `idempotency_key`.
const IDENTITY: [&str; 8] = [
    "session_id",
    "directory",
    "title",
    "agent",
    "model",
    "mode",
    "prompt",
    "delivery",
];

/// The `Idempotency-Key`s of one keyed call: the call's identity and its two writes.
struct Keys {
    base: String,
    session: String,
    prompt: String,
}

impl Keys {
    fn new(base: String) -> Self {
        Self {
            session: format!("{base}:session"),
            prompt: format!("{base}:prompt"),
            base,
        }
    }

    /// The prompt's `message_id`, derived from the key so every retry names it.
    fn message_id(&self) -> String {
        let digest = Sha256::digest(self.prompt.as_bytes());
        format!("msg_mcp{}", &format!("{digest:x}")[..26])
    }
}

/// An admitted prompt.
struct Admitted {
    session_id: String,
    message_id: String,
    replayed: bool,
    /// The Session's pending requests before this call admitted the prompt; a replay
    /// waits from now.
    before: Option<HashSet<String>>,
    /// The request depth of the call that admitted it (a replay keeps the first record).
    depth: u32,
}

impl Admitted {
    fn from_receipt(receipt: &Value, replayed: bool) -> Self {
        let text = |key: &str| receipt[key].as_str().unwrap_or_default().to_owned();
        Self {
            session_id: text("session_id"),
            message_id: text("message_id"),
            replayed,
            before: None,
            depth: 0,
        }
    }
}

pub(super) async fn prompt(
    connection: &Arc<Connection>,
    args: &Value,
    caller: &Caller,
    progress: &Progress,
) -> Outcome {
    let keys = create::route_key(TOOL, args)?.map(Keys::new);
    let fingerprint = fingerprint(args);
    let slot = if args["wait"] == false {
        None
    } else {
        Some(reserve_wait(connection)?)
    };
    if let Some(keys) = &keys
        && let Some(admitted) = replayed(connection, args, keys, &fingerprint).await?
    {
        record(connection, &admitted, keys)?;
        return respond(connection, &admitted, args, slot, progress).await;
    }
    check(args)?;
    let depth = guard(connection, args, caller)?;
    let run = reserve_run(connection, args)?;
    let target = target(connection, args, caller, keys.as_ref(), &fingerprint).await?;
    let admission = Admission {
        connection: Arc::clone(connection),
        target,
        keys,
        depth,
        run,
        fingerprint,
        prompt: args["prompt"].as_str().unwrap_or_default().to_owned(),
        delivery: args["delivery"].as_str().unwrap_or("steer").to_owned(),
    };
    let admitted = tokio::spawn(admission.run())
        .await
        .map_err(ApiError::unknown)??;
    respond(connection, &admitted, args, slot, progress).await
}

/// A replay of the prompt write: its stored response, or its inbox row when the write
/// landed but its response is not stored yet.
async fn replayed(
    connection: &Connection,
    args: &Value,
    keys: &Keys,
    fingerprint: &str,
) -> Result<Option<Admitted>, ToolError> {
    if let Some(receipt) = create::replay(connection, &keys.prompt, &keys.base, fingerprint).await?
    {
        return Ok(Some(Admitted::from_receipt(&receipt, true)));
    }
    let session_id = match args["session_id"].as_str() {
        Some(id) => id.to_owned(),
        None => match create::replay(connection, &keys.session, &keys.base, fingerprint).await? {
            Some(session) => session["id"].as_str().unwrap_or_default().to_owned(),
            None => return Ok(None),
        },
    };
    let message_id = keys.message_id();
    let runtime = &connection.state.runtime;
    let admitted = runtime
        .state(&session_id)
        .await
        .is_ok_and(|state| state.input(&message_id).is_some());
    if !admitted {
        return Ok(None);
    }
    create::same_call(connection, &keys.base, fingerprint)?;
    Ok(Some(Admitted {
        session_id,
        message_id,
        replayed: true,
        before: None,
        depth: 0,
    }))
}

/// Report the admission, then wait (holding the wait slot) or return the receipt.
async fn respond(
    connection: &Arc<Connection>,
    admitted: &Admitted,
    args: &Value,
    slot: Option<WaitSlot>,
    progress: &Progress,
) -> Outcome {
    let (session_id, message_id) = (&admitted.session_id, &admitted.message_id);
    progress.admitted(session_id, message_id);
    let mut value = match slot {
        Some(_slot) => {
            let limit = connection
                .state
                .services
                .mcp_serve()?
                .limits
                .max_wait_seconds;
            let seconds = wait::wait_seconds(args, limit);
            let before = admitted.before.clone();
            wait::wait(
                connection, session_id, message_id, seconds, progress, before,
            )
            .await?
        }
        None => result(&observe(connection, session_id, message_id).await?),
    };
    if admitted.replayed {
        value["replayed"] = json!(true);
    }
    Ok(ToolOutput {
        value,
        untrusted: true,
    })
}

/// The tool-level identity of a call.
fn fingerprint(args: &Value) -> String {
    let mut fields: Vec<Value> = IDENTITY.iter().map(|key| args[*key].clone()).collect();
    if fields[7].is_null() {
        fields[7] = json!("steer");
    }
    let digest = Sha256::digest(Value::from(fields).to_string().as_bytes());
    format!("{TOOL}:{digest:x}")
}

/// The argument rules a schema cannot state.
fn check(args: &Value) -> Result<(), ApiError> {
    let existing = args["session_id"].is_string();
    if existing == args["directory"].is_string() {
        return Err(
            ApiError::invalid("give exactly one of session_id and directory")
                .with_reason("exclusive_fields"),
        );
    }
    if existing
        && let Some(key) = ["title", "agent", "model", "mode"]
            .into_iter()
            .find(|key| !args[*key].is_null())
    {
        return Err(ApiError::invalid(format!(
            "{key} applies only to a new Session (with directory)"
        ))
        .with_reason("session_settings_on_existing"));
    }
    let size = args["prompt"].as_str().map_or(0, str::len);
    if size > MAX_PROMPT {
        return Err(
            ApiError::invalid(format!("the prompt has {size} bytes; at most 128 KiB"))
                .with_reason("too_large"),
        );
    }
    Ok(())
}

/// The recursion guard: the request depth below `max_depth`, and never the caller
/// Session or one of its ancestors as the target.
fn guard(connection: &Connection, args: &Value, caller: &Caller) -> Result<u32, ApiError> {
    let depth = caller.check_depth(connection)?;
    if let Some(id) = args["session_id"].as_str() {
        caller.check_target(connection, id)?;
    }
    Ok(depth)
}

/// `max_running_sessions`: a prompt that would start a run (on an idle or a new
/// Session) reserves one of the runs of Sessions created through MCP serve or running a
/// run an MCP prompt started, and is refused when none is left. Steering a running
/// Session never is.
fn reserve_run(connection: &Connection, args: &Value) -> Result<Option<RunSlot>, ApiError> {
    let state = &connection.state;
    let runtime = &state.runtime;
    if args["session_id"]
        .as_str()
        .is_some_and(|id| runtime.is_running(id))
    {
        return Ok(None);
    }
    let limit = state.services.mcp_serve()?.limits.max_running_sessions;
    let running = runtime.running_sessions();
    let created: HashSet<String> =
        mcp_serve::session_clients(&state.store, running.iter().cloned().collect())
            .map_err(ApiError::unknown)?
            .into_keys()
            .collect();
    let bound = usize::try_from(limit).unwrap_or(usize::MAX);
    match connection
        .shared
        .reserve_run(runtime, &running, &created, bound)
    {
        Ok(slot) => Ok(Some(slot)),
        Err(count) => Err(ApiError::rate_limited(
            "concurrency",
            5000,
            format!("{count} Sessions driven through MCP serve are running; at most {limit}"),
        )),
    }
}

/// Where the prompt goes.
enum Target {
    Existing(String),
    New(Prepared),
}

async fn target(
    connection: &Arc<Connection>,
    args: &Value,
    caller: &Caller,
    keys: Option<&Keys>,
    fingerprint: &str,
) -> Result<Target, ToolError> {
    if let Some(id) = args["session_id"].as_str() {
        return Ok(Target::Existing(id.to_owned()));
    }
    if let Some(keys) = keys
        && let Some(session) =
            create::replay(connection, &keys.session, &keys.base, fingerprint).await?
    {
        create::record(connection, &session)?;
        let id = session["id"].as_str().unwrap_or_default().to_owned();
        return Ok(Target::Existing(id));
    }
    Ok(Target::New(create::prepare(connection, args, caller)?))
}

/// The writes of one call, run to completion even when the call is cancelled.
struct Admission {
    connection: Arc<Connection>,
    target: Target,
    keys: Option<Keys>,
    /// The request depth, recorded with the input and raised into its run.
    depth: u32,
    /// The reserved run start, when the prompt would start a run.
    run: Option<RunSlot>,
    fingerprint: String,
    prompt: String,
    delivery: String,
}

impl Admission {
    async fn run(self) -> Result<Admitted, ToolError> {
        let connection = &self.connection;
        let keys = self.keys.as_ref();
        if let Some(keys) = keys {
            create::claim_key(connection, &keys.base, &self.fingerprint)?;
        }
        let session_id = match self.target {
            Target::Existing(id) => id,
            Target::New(prepared) => {
                let key = keys.map(|keys| keys.session.as_str());
                let created = create::write(connection, prepared, key, None).await?;
                create::record(connection, &created)?;
                created["id"].as_str().unwrap_or_default().to_owned()
            }
        };
        let starts_run = !connection.state.runtime.is_running(&session_id);
        let before = wait::request_ids(connection, &session_id);
        let mut body = json!({
            "parts": [{ "type": "text", "text": self.prompt }],
            "delivery": self.delivery,
            "source": "user",
        });
        let mut headers = Vec::new();
        if let Some(keys) = keys {
            body["id"] = json!(keys.message_id());
            headers.push(("idempotency-key", keys.prompt.as_str()));
        }
        let mut receipt = route::send(
            connection,
            "v1.session.prompt",
            Call {
                path: &[("sessionID", session_id.as_str())],
                body: Some(&body),
                headers: &headers,
                ..Call::default()
            },
        )
        .await?;
        let runtime = &connection.state.runtime;
        if let Some(run) = self.run
            && starts_run
        {
            run.started(runtime, &session_id);
        }
        runtime.raise_mcp_depth(&session_id, self.depth);
        let mut admitted = Admitted::from_receipt(&receipt["data"].take(), false);
        admitted.before = Some(before);
        admitted.depth = self.depth;
        record_input(connection, &admitted, keys.map(|keys| keys.base.as_str()))?;
        Ok(admitted)
    }
}

fn record(connection: &Connection, admitted: &Admitted, keys: &Keys) -> Result<(), ApiError> {
    record_input(connection, admitted, Some(&keys.base))
}

/// Keep the first MCP serve record of an admitted prompt (its `mcp_client` and
/// `admitted_at`).
fn record_input(
    connection: &Connection,
    admitted: &Admitted,
    key: Option<&str>,
) -> Result<(), ApiError> {
    mcp_serve::record_input(
        &connection.state.store,
        &McpServeInput {
            message_id: admitted.message_id.clone(),
            session_id: admitted.session_id.clone(),
            mcp_client: connection.label.clone(),
            depth: admitted.depth,
            admitted_ms: create::now_ms(),
            idempotency_key: key.map(str::to_owned),
        },
    )
    .map_err(ApiError::unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_derive_both_writes_and_the_message_id() {
        let keys = Keys::new("mcp:cyber_prompt:k1".into());
        assert_eq!(keys.session, "mcp:cyber_prompt:k1:session");
        assert_eq!(keys.prompt, "mcp:cyber_prompt:k1:prompt");
        let id = keys.message_id();
        assert!(cyber_core::ids::has_prefix(&id, "msg"), "{id}");
        assert_eq!(id, Keys::new("mcp:cyber_prompt:k1".into()).message_id());
        assert_ne!(id, Keys::new("mcp:cyber_prompt:k2".into()).message_id());
    }

    #[test]
    fn wait_and_the_default_delivery_never_change_the_identity() {
        let base = json!({"directory": "/r", "prompt": "p"});
        let mut other = base.clone();
        other["wait"] = json!(false);
        other["wait_seconds"] = json!(9);
        other["delivery"] = json!("steer");
        assert_eq!(fingerprint(&base), fingerprint(&other));
        other["delivery"] = json!("queue");
        assert_ne!(fingerprint(&base), fingerprint(&other));
        let mut existing = base.clone();
        existing["session_id"] = json!("ses_1");
        assert_ne!(fingerprint(&base), fingerprint(&existing));
    }

    #[test]
    fn argument_rules() {
        let reason = |args: Value| {
            check(&args)
                .err()
                .map(|e| serde_json::to_value(&e.body).unwrap()["reason"].clone())
        };
        assert_eq!(reason(json!({"prompt": "p", "directory": "/r"})), None);
        assert_eq!(
            reason(json!({"prompt": "p"})),
            Some(json!("exclusive_fields"))
        );
        assert_eq!(
            reason(json!({"prompt": "p", "session_id": "s", "directory": "/r"})),
            Some(json!("exclusive_fields"))
        );
        assert_eq!(
            reason(json!({"prompt": "p", "session_id": "s", "model": "a/b"})),
            Some(json!("session_settings_on_existing"))
        );
        let large = "x".repeat(MAX_PROMPT + 1);
        assert_eq!(
            reason(json!({"prompt": large, "session_id": "s"})),
            Some(json!("too_large"))
        );
        let fits = "x".repeat(MAX_PROMPT);
        assert_eq!(reason(json!({"prompt": fits, "session_id": "s"})), None);
    }
}
