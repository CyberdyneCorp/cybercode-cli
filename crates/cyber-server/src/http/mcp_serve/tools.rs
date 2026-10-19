//! The served tools (`mcp-server` → Served tools and errors): strict input schemas,
//! output schemas, annotations, and result rendering. These are MCP tools for external
//! clients, never `builtin-tools` offered to Cyber's own agents.

use std::sync::Arc;

use serde_json::{Value, json};

use super::super::ApiError;
use super::super::jsonrpc::INVALID_PARAMS;
use super::caller::Caller;
use super::connection::Connection;
use super::progress::Progress;
use super::{create, messaging, prompt, read, schema, wait};

/// The `_meta` key marking results that carry model, tool or message text.
const UNTRUSTED: &str = "dev.cyber-code/untrusted";

/// A tool success: `structuredContent`, and whether it carries untrusted text.
pub(super) struct ToolOutput {
    pub value: Value,
    pub untrusted: bool,
}

/// A tool failure: the `server-api` tagged error body, unchanged.
#[derive(Debug)]
pub(super) struct ToolError(pub Value);

impl From<ApiError> for ToolError {
    fn from(error: ApiError) -> Self {
        let mut body = serde_json::to_value(&error.body).unwrap_or_default();
        if let Some(code) = error.code {
            body["code"] = json!(code);
        }
        Self(body)
    }
}

pub(super) type Outcome = Result<ToolOutput, ToolError>;

struct Tool {
    name: &'static str,
    description: &'static str,
    /// `(readOnlyHint, destructiveHint, idempotentHint, openWorldHint)`.
    hints: [bool; 4],
    input: fn() -> Value,
    output: fn() -> Value,
}

/// Every served tool, sorted by name.
const TOOLS: [Tool; 10] = [
    Tool {
        name: "cyber_interrupt",
        description: "Interrupt a Session's running Turn (inbox rows are kept). Interrupting an idle Session succeeds with was_running false. stop_background also stops its background tasks.",
        hints: [false, true, true, false],
        input: interrupt_input,
        output: interrupt_output,
    },
    Tool {
        name: "cyber_message_history",
        description: "Read a Session's cross-session message history (received and sent), oldest first, paged by cursor and bounded by max_bytes.",
        hints: [true, false, true, false],
        input: message_history_input,
        output: message_history_output,
    },
    Tool {
        name: "cyber_message_list",
        description: "List the Sessions a message can be sent to, newest first, optionally in one directory; canReceive false means a message would be refused.",
        hints: [true, false, true, false],
        input: message_list_input,
        output: message_list_output,
    },
    Tool {
        name: "cyber_message_send",
        description: "Send a plain-text message (at most 16 KiB) to a Session by address (ses_ ID, subagent or Session name). It is read at the target's next safe boundary as untrusted data; the outcome is delivered, held or refused with a reason. You have no message address: read replies through the transcript. Pass a random unique idempotency_key so a retry never sends twice.",
        hints: [false, false, false, true],
        input: message_send_input,
        output: message_send_output,
    },
    Tool {
        name: "cyber_prompt",
        description: "Send a prompt (literal text, never a slash command) to a Session (session_id) or to a new Session in a directory, then wait up to wait_seconds for the result or return the receipt. A timeout is not an error: re-attach with cyber_prompt_wait. Pass a random unique idempotency_key so a retry never runs twice.",
        hints: [false, true, false, true],
        input: prompt_input,
        output: prompt_output,
    },
    Tool {
        name: "cyber_prompt_wait",
        description: "Wait for the result of a prompt admitted earlier, by session_id and message_id (also after a server restart); wait_seconds 0 polls.",
        hints: [true, false, true, false],
        input: prompt_wait_input,
        output: prompt_output,
    },
    Tool {
        name: "cyber_session_create",
        description: "Create a Session in an absolute directory, bounded by the MCP serve Mode ceiling.",
        hints: [false, false, false, false],
        input: create_input,
        output: create_output,
    },
    Tool {
        name: "cyber_session_list",
        description: "List the server's Sessions, most recently updated first, optionally in one directory or state.",
        hints: [true, false, true, false],
        input: list_input,
        output: list_output,
    },
    Tool {
        name: "cyber_session_read",
        description: "Read one Session: its state, last Turn, usage, pending approvals and inbox.",
        hints: [true, false, true, false],
        input: read_input,
        output: read_output,
    },
    Tool {
        name: "cyber_session_transcript",
        description: "Read a Session's durable transcript in ascending order, paged by cursor and bounded by max_bytes.",
        hints: [true, false, true, false],
        input: transcript_input,
        output: transcript_output,
    },
];

/// Every tool name, for the fixed `initialize` instructions.
#[cfg(test)]
pub(super) fn names() -> Vec<&'static str> {
    TOOLS.iter().map(|tool| tool.name).collect()
}

/// `tools/list`.
pub(super) fn list() -> Value {
    let tools: Vec<Value> = TOOLS
        .iter()
        .map(|tool| {
            let [read_only, destructive, idempotent, open_world] = tool.hints;
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": (tool.input)(),
                "outputSchema": (tool.output)(),
                "annotations": {
                    "readOnlyHint": read_only,
                    "destructiveHint": destructive,
                    "idempotentHint": idempotent,
                    "openWorldHint": open_world,
                },
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// Whether a `tools/call` waits (`cyber_prompt` unless `wait: false`, `cyber_prompt_wait`
/// unless `wait_seconds: 0`), so it is answered as an SSE stream.
pub(super) fn waits(params: &Value) -> bool {
    let arguments = &params["arguments"];
    match params["name"].as_str() {
        Some("cyber_prompt") => arguments["wait"] != false,
        Some("cyber_prompt_wait") => arguments["wait_seconds"] != 0,
        _ => false,
    }
}

/// `tools/call`: an unknown tool is a JSON-RPC error; everything after dispatch is a
/// tool result, so the calling model can correct its arguments.
pub(super) async fn call(
    connection: &Arc<Connection>,
    params: &Value,
    caller: &Caller,
    progress: &Progress,
) -> Result<Value, (i64, String)> {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err((INVALID_PARAMS, "tools/call requires a tool name".into()));
    };
    let Some(tool) = TOOLS.iter().find(|tool| tool.name == name) else {
        return Err((INVALID_PARAMS, format!("unknown tool {name}")));
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let outcome = match schema::validate(&(tool.input)(), &arguments) {
        Ok(()) => run(connection, tool.name, &arguments, caller, progress).await,
        Err(violation) => Err(ApiError::from(violation).into()),
    };
    Ok(render(outcome))
}

async fn run(
    connection: &Arc<Connection>,
    name: &str,
    arguments: &Value,
    caller: &Caller,
    progress: &Progress,
) -> Outcome {
    match name {
        "cyber_session_list" => read::list(connection, arguments).await,
        "cyber_session_read" => read::read(connection, arguments).await,
        "cyber_session_transcript" => read::transcript(connection, arguments).await,
        "cyber_interrupt" => read::interrupt(connection, arguments, caller).await,
        "cyber_session_create" => create::create(connection, arguments, caller).await,
        "cyber_prompt" => prompt::prompt(connection, arguments, caller, progress).await,
        "cyber_prompt_wait" => wait::prompt_wait(connection, arguments, progress).await,
        "cyber_message_list" => messaging::list(connection, arguments, caller).await,
        "cyber_message_send" => messaging::send(connection, arguments, caller).await,
        "cyber_message_history" => messaging::history(connection, arguments).await,
        _ => Err(ApiError::service_unavailable(
            "mcp_serve",
            format!("{name} is not available on this server"),
        )
        .into()),
    }
}

/// A success carries `structuredContent` and its JSON as `content[0]`; a failure only the
/// error JSON, with `isError`.
fn render(outcome: Outcome) -> Value {
    match outcome {
        Ok(output) => {
            let mut result = json!({
                "content": [{ "type": "text", "text": output.value.to_string() }],
                "structuredContent": output.value,
                "isError": false,
            });
            if output.untrusted {
                let mut meta = serde_json::Map::new();
                meta.insert(UNTRUSTED.into(), Value::Bool(true));
                result["_meta"] = Value::Object(meta);
            }
            result
        }
        Err(ToolError(body)) => json!({
            "content": [{ "type": "text", "text": body.to_string() }],
            "isError": true,
        }),
    }
}

// Schemas. Byte-size limits are enforced by the tools, never in `inputSchema`.

fn strict(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn shape(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn id(description: &str) -> Value {
    json!({ "type": "string", "minLength": 1, "maxLength": 128, "description": description })
}

fn integer(minimum: i64, maximum: i64, default: i64) -> Value {
    json!({ "type": "integer", "minimum": minimum, "maximum": maximum, "default": default })
}

fn directory() -> Value {
    json!({ "type": "string", "minLength": 1, "description": "An absolute directory path." })
}

fn mode() -> Value {
    json!({ "type": "string", "enum": ["plan", "dont-ask", "default", "accept-edits", "auto", "bypass"] })
}

fn idempotency_key() -> Value {
    json!({ "type": "string", "minLength": 1, "maxLength": 64, "pattern": "^[!-~]+$",
        "description": "A random unique value (for example a UUID) of printable characters that makes a retry replay instead of repeating." })
}

fn max_bytes() -> Value {
    json!({ "type": "integer", "minimum": 1024, "maximum": 196_608, "default": 65_536,
        "description": "Upper bound of the serialized page; at least one item is returned while items remain." })
}

fn state() -> Value {
    json!({ "type": "string", "enum": ["idle", "running", "waiting_for_approval"] })
}

fn timestamp() -> Value {
    json!({ "type": "integer", "description": "Unix time in milliseconds." })
}

fn session_summary() -> Value {
    shape(
        json!({
            "session_id": { "type": "string" },
            "title": { "type": "string" },
            "directory": { "type": "string" },
            "parent_id": { "type": "string" },
            "agent": { "type": "string" },
            "model": { "type": "string" },
            "mode": { "type": "string" },
            "state": state(),
            "mcp_client": { "type": "string" },
            "mcp_depth": { "type": "integer", "minimum": 0 },
            "created_at": timestamp(),
            "updated_at": timestamp(),
        }),
        &[
            "session_id",
            "title",
            "directory",
            "agent",
            "model",
            "mode",
            "state",
            "mcp_depth",
            "created_at",
            "updated_at",
        ],
    )
}

fn usage() -> Value {
    let tokens = json!({ "type": "integer", "minimum": 0 });
    shape(
        json!({
            "input_tokens": tokens, "output_tokens": tokens, "reasoning_tokens": tokens,
            "cache_read_tokens": tokens, "cache_write_tokens": tokens,
            "cost_usd": { "type": ["number", "null"], "description": "null when any counted step is unpriced." },
        }),
        &[
            "input_tokens",
            "output_tokens",
            "reasoning_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
            "cost_usd",
        ],
    )
}

fn pending_request() -> Value {
    shape(
        json!({
            "request_id": { "type": "string" },
            "kind": { "type": "string", "enum": ["permission", "question"] },
            "session_id": { "type": "string" },
            "summary": { "type": "string", "maxLength": 512 },
            "created_at": timestamp(),
        }),
        &["request_id", "kind", "session_id", "summary"],
    )
}

fn page(key: &str, item: Value) -> Value {
    let mut properties = json!({
        "next_cursor": { "type": "string" },
        "truncated": { "type": "boolean" },
    });
    properties[key] = json!({ "type": "array", "items": item });
    shape(properties, &[key, "truncated"])
}

fn list_input() -> Value {
    strict(
        json!({
            "directory": directory(),
            "mcp_only": { "type": "boolean", "default": false,
                "description": "Only Sessions created through MCP serve." },
            "state": state(),
            "cursor": { "type": "string" },
            "limit": integer(1, 200, 50),
            "max_bytes": max_bytes(),
        }),
        &[],
    )
}

fn list_output() -> Value {
    page("sessions", session_summary())
}

fn read_input() -> Value {
    strict(
        json!({ "session_id": id("The Session to read.") }),
        &["session_id"],
    )
}

fn read_output() -> Value {
    shape(
        json!({
            "session": session_summary(),
            "last_turn": shape(
                json!({
                    "finished_at": timestamp(),
                    "stop_reason": { "type": "string", "enum": ["completed", "error", "budget_exceeded", "interrupted"] },
                    "final_text_preview": { "type": "string", "description": "At most 2 KiB." },
                }),
                &["finished_at", "stop_reason", "final_text_preview"],
            ),
            "usage": usage(),
            "pending": { "type": "array", "maxItems": 20, "items": pending_request() },
            "inbox_pending": { "type": "integer", "minimum": 0 },
            "recent_prompts": { "type": "array", "maxItems": 20, "items": shape(
                json!({
                    "message_id": { "type": "string" },
                    "admitted_at": timestamp(),
                    "status": prompt_status(),
                }),
                &["message_id", "admitted_at", "status"],
            ) },
        }),
        &[
            "session",
            "usage",
            "pending",
            "inbox_pending",
            "recent_prompts",
        ],
    )
}

fn transcript_input() -> Value {
    strict(
        json!({
            "session_id": id("The Session whose transcript to read."),
            "cursor": { "type": "string" },
            "from": { "type": "string", "enum": ["start", "end"], "default": "start",
                "description": "Where a first page starts; ignored with a cursor." },
            "limit": integer(1, 100, 20),
            "include_tool_output": { "type": "boolean", "default": false },
            "max_bytes": max_bytes(),
        }),
        &["session_id"],
    )
}

fn transcript_output() -> Value {
    page(
        "items",
        shape(
            json!({
                "seq": { "type": "integer" },
                "message_id": { "type": "string" },
                "role": { "type": "string", "enum": ["user", "assistant", "tool", "notice"] },
                "origin": { "type": "string" },
                "created_at": timestamp(),
                "text": { "type": "string" },
                "tool": shape(
                    json!({
                        "name": { "type": "string" },
                        "call_id": { "type": "string" },
                        "status": { "type": "string" },
                        "input_summary": { "type": "string", "maxLength": 512 },
                        "output": { "type": "string" },
                    }),
                    &["name", "call_id", "status", "input_summary"],
                ),
            }),
            &["seq", "message_id", "role"],
        ),
    )
}

fn interrupt_input() -> Value {
    strict(
        json!({
            "session_id": id("The Session to interrupt."),
            "stop_background": { "type": "boolean", "default": false },
        }),
        &["session_id"],
    )
}

fn interrupt_output() -> Value {
    shape(
        json!({ "session_id": { "type": "string" }, "was_running": { "type": "boolean" } }),
        &["session_id", "was_running"],
    )
}

fn create_input() -> Value {
    strict(
        json!({
            "directory": directory(),
            "title": { "type": "string", "maxLength": 200 },
            "agent": { "type": "string", "minLength": 1 },
            "model": { "type": "string", "minLength": 3, "description": "provider/model[#variant]" },
            "mode": mode(),
            "idempotency_key": idempotency_key(),
        }),
        &["directory"],
    )
}

fn create_output() -> Value {
    shape(
        json!({
            "session": session_summary(),
            "effective_mode": { "type": "string" },
            "replayed": { "type": "boolean" },
        }),
        &["session", "effective_mode"],
    )
}

fn prompt_input() -> Value {
    strict(
        json!({
            "session_id": id("An existing Session; exclusive with directory."),
            "directory": directory(),
            "prompt": { "type": "string", "minLength": 1, "description": "Literal text; never run as a slash command." },
            "delivery": { "type": "string", "enum": ["steer", "queue"], "default": "steer" },
            "title": { "type": "string", "maxLength": 200 },
            "agent": { "type": "string", "minLength": 1 },
            "model": { "type": "string", "minLength": 3 },
            "mode": mode(),
            "idempotency_key": idempotency_key(),
            "wait": { "type": "boolean", "default": true },
            "wait_seconds": integer(1, 3600, 50),
        }),
        &["prompt"],
    )
}

fn prompt_wait_input() -> Value {
    strict(
        json!({
            "session_id": id("The Session the prompt was admitted to."),
            "message_id": id("The prompt's message_id from its receipt."),
            "wait_seconds": integer(0, 3600, 50),
        }),
        &["session_id", "message_id"],
    )
}

fn prompt_status() -> Value {
    json!({ "type": "string", "enum": ["queued", "running", "waiting_for_approval", "completed", "failed", "interrupted", "budget_exceeded", "refused"] })
}

fn prompt_output() -> Value {
    shape(
        json!({
            "session_id": { "type": "string" },
            "message_id": { "type": "string" },
            "status": prompt_status(),
            "done": { "type": "boolean" },
            "stop_reason": { "type": "string", "enum": ["completed", "error", "budget_exceeded", "interrupted"] },
            "final_text": { "type": "string" },
            "truncated": { "type": "boolean" },
            "usage": usage(),
            "turns": { "type": "integer", "minimum": 0 },
            "pending": { "type": "array", "items": pending_request() },
            "error": shape(json!({ "_tag": { "type": "string" }, "message": { "type": "string" } }), &["_tag", "message"]),
            "replayed": { "type": "boolean" },
            "admitted_at": timestamp(),
            "finished_at": timestamp(),
        }),
        &["session_id", "message_id", "status", "done", "admitted_at"],
    )
}

fn message_list_input() -> Value {
    strict(json!({ "directory": directory() }), &[])
}

fn message_list_output() -> Value {
    let location = shape(
        json!({ "directory": { "type": "string" }, "workspace": { "type": "string" } }),
        &["directory"],
    );
    let session = shape(
        json!({
            "address": { "type": "string", "description": "What to pass as to." },
            "session_id": { "type": "string" },
            "name": { "type": "string" },
            "kind": { "type": "string", "description": "local, subagent or teammate; an open set." },
            "state": { "type": "string", "enum": ["idle", "running", "waiting_input"] },
            "location": location,
            "canReceive": { "type": "boolean" },
        }),
        &["address", "name", "kind", "state", "location", "canReceive"],
    );
    shape(
        json!({ "sessions": { "type": "array", "items": session } }),
        &["sessions"],
    )
}

fn message_send_input() -> Value {
    strict(
        json!({
            "to": { "type": "string", "minLength": 1,
                "description": "The target's address: a ses_ ID, a subagent name or a Session name." },
            "text": { "type": "string", "minLength": 1, "description": "Plain text, at most 16 KiB." },
            "idempotency_key": idempotency_key(),
        }),
        &["to", "text"],
    )
}

fn message_outcome() -> Value {
    json!({ "type": "string", "enum": ["delivered", "held", "refused"] })
}

fn message_send_output() -> Value {
    shape(
        json!({
            "message_id": { "type": "string" },
            "outcome": message_outcome(),
            "reason": { "type": "string" },
            "limit": { "type": "string", "description": "The limit a rate_limited refusal hit." },
            "to": shape(
                json!({ "address": { "type": "string" }, "session_id": { "type": "string" } }),
                &["address"],
            ),
            "replayed": { "type": "boolean" },
        }),
        &["message_id", "outcome"],
    )
}

fn message_history_input() -> Value {
    strict(
        json!({
            "session_id": id("The Session whose messages to read."),
            "cursor": { "type": "string" },
            "limit": integer(1, 100, 50),
            "max_bytes": max_bytes(),
        }),
        &["session_id"],
    )
}

fn message_history_output() -> Value {
    page(
        "messages",
        shape(
            json!({
                "message_id": { "type": "string" },
                "direction": { "type": "string", "enum": ["in", "out"] },
                "from": { "type": "string" },
                "to": { "type": "string" },
                "outcome": { "type": "string", "enum": ["delivered", "held", "released", "dropped", "refused"] },
                "reason": { "type": "string" },
                "size": { "type": "integer", "minimum": 0 },
                "text": { "type": "string", "description": "Absent for a refused message." },
                "created_at": timestamp(),
            }),
            &[
                "message_id",
                "direction",
                "from",
                "to",
                "outcome",
                "size",
                "created_at",
            ],
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_sorted_with_strict_inputs() {
        let names = names();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        for tool in &TOOLS {
            let input = (tool.input)();
            assert_eq!(input["additionalProperties"], false, "{}", tool.name);
            for (key, property) in input["properties"].as_object().unwrap() {
                assert!(property["type"].is_string(), "{} {key}", tool.name);
            }
        }
    }
}
