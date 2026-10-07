//! Durable session event payloads, the event registry and SQL projectors.

use std::collections::BTreeMap;

use cyber_llm::{Content, FinishReason, Usage};
use cyber_store::{EventRegistry, NewEvent, StoredEvent};
use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{CallState, CallStatus, Delivery, Entry, RetrySafety, SessionInfo};

pub const CREATED: &str = "session.created.1";
pub const RESUMED: &str = "session.subagent.resumed.1";
pub const ADMITTED: &str = "session.prompt.admitted.1";
pub const INBOX_UPDATED: &str = "session.inbox.updated.1";
pub const PROMOTED: &str = "session.prompt.promoted.1";
pub const EPOCH_STARTED: &str = "session.context.epoch_started.1";
pub const CONTEXT_UPDATED: &str = "session.context.updated.1";
pub const SYSTEM_ADDED: &str = "session.system.added.1";
pub const STEP_STARTED: &str = "session.step.started.1";
pub const TEXT_ENDED: &str = "session.text.ended.1";
pub const REASONING_ENDED: &str = "session.reasoning.ended.1";
pub const TOOL_CALLED: &str = "session.tool.called.1";
pub const TOOL_DISPATCHED: &str = "session.tool.dispatched.1";
pub const TOOL_SETTLED: &str = "session.tool.settled.1";
pub const STEP_ENDED: &str = "session.step.ended.1";
pub const STEP_FAILED: &str = "session.step.failed.1";
pub const COMPACTION_STARTED: &str = "session.compaction.started.1";
pub const COMPACTION_COMPLETED: &str = "session.compaction.completed.1";
pub const COMPACTION_FAILED: &str = "session.compaction.failed.1";
pub const TITLE_GENERATED: &str = "session.title.generated.1";
pub const RENAMED: &str = "session.renamed.1";
pub const ARCHIVED: &str = "session.archived.1";
pub const AGENT_SWITCHED: &str = "session.agent.switched.1";
pub const MODEL_SWITCHED: &str = "session.model.switched.1";
pub const MODE_SWITCHED: &str = "session.mode.switched.1";
pub const AUTO_DECIDED: &str = "permission.auto_decided.1";
pub const PERMISSION_ASKED: &str = "permission.asked.1";
pub const PERMISSION_REPLIED: &str = "permission.replied.1";
pub const QUESTION_ASKED: &str = "question.asked.1";
pub const QUESTION_REPLIED: &str = "question.replied.1";
pub const SNAPSHOT_TAKEN: &str = "snapshot.taken.1";
pub const DIFF_COMPUTED: &str = "session.diff.1";
pub const REVERTED: &str = "session.reverted.1";

const ALL: &[&str] = &[
    CREATED,
    ADMITTED,
    RESUMED,
    INBOX_UPDATED,
    PROMOTED,
    EPOCH_STARTED,
    CONTEXT_UPDATED,
    SYSTEM_ADDED,
    STEP_STARTED,
    TEXT_ENDED,
    REASONING_ENDED,
    TOOL_CALLED,
    TOOL_DISPATCHED,
    TOOL_SETTLED,
    STEP_ENDED,
    STEP_FAILED,
    COMPACTION_STARTED,
    COMPACTION_COMPLETED,
    COMPACTION_FAILED,
    TITLE_GENERATED,
    RENAMED,
    ARCHIVED,
    AGENT_SWITCHED,
    MODEL_SWITCHED,
    MODE_SWITCHED,
    AUTO_DECIDED,
    PERMISSION_ASKED,
    PERMISSION_REPLIED,
    QUESTION_ASKED,
    QUESTION_REPLIED,
    SNAPSHOT_TAKEN,
    DIFF_COMPUTED,
    REVERTED,
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Created {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_worktree: Option<cyber_core::worktrees::Managed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_context: Option<super::fork::ForkContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mode_default_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<super::selection::ModelSelection>,
    pub info: SessionInfo,
    /// History copied from another Session by fork, with fresh IDs.
    #[serde(default)]
    pub history: Vec<Entry>,
    #[serde(default)]
    pub calls: Vec<CallState>,
    #[serde(default)]
    pub forked_from: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Admitted {
    pub message_id: String,
    pub parts: Vec<Content>,
    pub delivery: Delivery,
    pub source: String,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxAction {
    Edited,
    Removed,
    Released,
    Refused,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxUpdated {
    pub message_id: String,
    pub action: InboxAction,
    pub parts: Option<Vec<Content>>,
    pub delivery: Option<Delivery>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Promoted {
    pub message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochStarted {
    pub epoch: u32,
    pub baseline: String,
    pub snapshot: BTreeMap<String, String>,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextUpdated {
    pub message_id: String,
    pub text: String,
    pub snapshot: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemAdded {
    pub message_id: String,
    pub text: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepStarted {
    /// Agent identity pinned for inference and every tool group of this Turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Permission mode pinned for inference and all tools of this Turn.
    #[serde(default)]
    pub mode: Option<String>,
    pub message_id: String,
    pub provider: String,
    pub model: String,
    pub tools: bool,
    /// The working-tree snapshot taken before the Turn.
    #[serde(default)]
    pub snapshot: Option<String>,
}

/// The working tree after a Turn's tool settlements (`snapshots-checkpoints` → Snapshot events).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotTaken {
    pub session_id: String,
    pub step_id: String,
    pub tree: String,
    /// Paths changed since the step's starting snapshot.
    pub changed: Vec<String>,
    #[serde(default)]
    pub skipped: Vec<String>,
}

/// File diffs for one user message, from its first pre-Turn to its last post-Turn snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffComputed {
    pub message_id: String,
    pub diffs: Vec<super::host::FileDiff>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevertPhase {
    Stage,
    Clear,
    Commit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reverted {
    pub session_id: String,
    pub message_id: String,
    pub target: super::model::RevertTarget,
    pub phase: RevertPhase,
    #[serde(default)]
    pub baseline: Option<String>,
    #[serde(default)]
    pub applied: Option<String>,
    #[serde(default)]
    pub diff: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentEnded {
    pub message_id: String,
    pub text: String,
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCalled {
    pub message_id: String,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    pub input: Option<Value>,
    pub retry_safety: RetrySafety,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDispatched {
    pub call_id: String,
    pub attempt: u32,
    pub input_digest: String,
    /// Stable key passed to idempotent external APIs across retries.
    pub operation_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSettled {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_value"
    )]
    pub structured_output: Option<Value>,
    pub call_id: String,
    pub status: CallStatus,
    /// Model-visible result.
    pub output: String,
    /// Operator-only detail (crash reports, reconciliation evidence).
    #[serde(default)]
    pub detail: Option<String>,
}

pub(crate) fn present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepEnded {
    pub message_id: String,
    pub finish: FinishReason,
    pub usage: Usage,
    /// `None` when the model is unpriced.
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepFailed {
    pub message_id: String,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    Auto,
    Overflow,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionStarted {
    pub trigger: CompactionTrigger,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionCompleted {
    pub message_id: String,
    pub summary: String,
    pub tail_start_id: String,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub trigger: CompactionTrigger,
    /// Usage of the hidden summary call, counted in the Session totals.
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionFailed {
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Titled {
    pub title: String,
    /// Usage of the hidden title call, when generated.
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Archived {
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Switched {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub automatic: bool,
    pub from: String,
    pub to: String,
}

pub fn event<T: Serialize>(kind: &str, payload: &T) -> NewEvent {
    NewEvent::new(
        kind,
        serde_json::to_value(payload).expect("payloads serialize"),
    )
}

/// Registry with every session event type and the projectors for the SQL tables.
pub fn registry() -> EventRegistry {
    let mut registry = EventRegistry::default();
    for kind in ALL {
        registry.register(kind).expect("valid event types");
    }
    super::jobs::register(&mut registry);
    registry.projector(project);
    crate::worktrees::register(&mut registry);
    registry
}

fn project(tx: &Transaction<'_>, e: &StoredEvent) -> Result<(), String> {
    project_event(tx, e).map_err(|err| err.to_string())
}

fn project_event(tx: &Transaction<'_>, e: &StoredEvent) -> rusqlite::Result<()> {
    let id = &e.aggregate_id;
    let d = &e.data;
    super::jobs::project(tx, e)?;
    match e.kind.as_str() {
        CREATED => insert_session(tx, e)?,
        RESUMED => {
            tx.execute(
                "UPDATE session SET subagent_name=?2 WHERE id=?1",
                params![id, d["name"].as_str()],
            )?;
        }
        ADMITTED => {
            let status = if d["delivery"] == "hold" {
                "held"
            } else {
                "pending"
            };
            tx.execute(
                "INSERT INTO session_input (message_id, session_id, delivery, status, source, digest, admitted_seq)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![s(d, "message_id"), id, s(d, "delivery"), status, s(d, "source"), s(d, "digest"), e.seq],
            )?;
        }
        INBOX_UPDATED => project_inbox(tx, d)?,
        PROMOTED => {
            tx.execute(
                "UPDATE session_input SET status = 'promoted', promoted_seq = ?2 WHERE message_id = ?1",
                params![s(d, "message_id"), e.seq],
            )?;
        }
        _ => project_runtime(tx, e)?,
    }
    tx.execute(
        "UPDATE session SET updated_at = ?2, last_seq = ?3 WHERE id = ?1",
        params![id, e.time_ms, e.seq],
    )?;
    Ok(())
}

fn project_runtime(tx: &Transaction<'_>, e: &StoredEvent) -> rusqlite::Result<()> {
    let (id, d) = (&e.aggregate_id, &e.data);
    match e.kind.as_str() {
        TOOL_CALLED => {
            tx.execute(
                "INSERT INTO tool_call (session_id, call_id, message_id, name, status, retry_safety)
                 VALUES (?1, ?2, ?3, ?4, 'called', ?5)",
                params![id, s(d, "call_id"), s(d, "message_id"), s(d, "name"), s(d, "retry_safety")],
            )?;
        }
        TOOL_DISPATCHED => {
            tx.execute(
                "UPDATE tool_call SET status = 'dispatched', attempt = ?3, input_digest = ?4 WHERE session_id = ?1 AND call_id = ?2",
                params![id, s(d, "call_id"), d["attempt"].as_i64(), s(d, "input_digest")],
            )?;
        }
        TOOL_SETTLED => {
            tx.execute(
                "UPDATE tool_call SET status = ?3 WHERE session_id = ?1 AND call_id = ?2",
                params![id, s(d, "call_id"), s(d, "status")],
            )?;
        }
        STEP_ENDED => project_usage(tx, id, d)?,
        AUTO_DECIDED if d["usage"].is_object() => project_usage(tx, id, d)?,
        _ => project_meta(tx, e)?,
    }
    Ok(())
}

fn project_meta(tx: &Transaction<'_>, e: &StoredEvent) -> rusqlite::Result<()> {
    let (id, d) = (&e.aggregate_id, &e.data);
    let column = match e.kind.as_str() {
        TITLE_GENERATED | RENAMED => Some(("title", d["title"].clone())),
        ARCHIVED => Some((
            "archived",
            Value::from(i64::from(d["archived"].as_bool().unwrap_or(false))),
        )),
        AGENT_SWITCHED => Some(("agent", d["to"].clone())),
        MODEL_SWITCHED => Some(("model", d["to"].clone())),
        MODE_SWITCHED => Some(("mode", d["to"].clone())),
        _ => None,
    };
    if let Some((column, value)) = column {
        let value = match value {
            Value::String(text) => rusqlite::types::Value::Text(text),
            Value::Number(n) => rusqlite::types::Value::Integer(n.as_i64().unwrap_or(0)),
            _ => rusqlite::types::Value::Null,
        };
        tx.execute(
            &format!("UPDATE session SET {column} = ?2 WHERE id = ?1"),
            params![id, value],
        )?;
    }
    Ok(())
}

fn insert_session(tx: &Transaction<'_>, e: &StoredEvent) -> rusqlite::Result<()> {
    let info = &e.data["info"];
    tx.execute(
        "INSERT INTO session (id, title, directory, parent_id, agent, model, mode, created_at, updated_at, last_seq, subagent_name)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10)",
        params![
            e.aggregate_id,
            s(info, "title"),
            s(info, "directory"),
            info["parent_id"].as_str(),
            s(info, "agent"),
            s(info, "model"),
            s(info, "mode"),
            e.time_ms,
            e.seq,
            info["subagent_name"].as_str()
        ],
    )?;
    Ok(())
}

fn project_inbox(tx: &Transaction<'_>, d: &Value) -> rusqlite::Result<()> {
    let id = s(d, "message_id");
    match s(d, "action") {
        "removed" => tx.execute("DELETE FROM session_input WHERE message_id = ?1", [id])?,
        "refused" => tx.execute(
            "UPDATE session_input SET status = 'refused' WHERE message_id = ?1",
            [id],
        )?,
        _ => match d["delivery"].as_str() {
            Some(delivery) => {
                let status = if delivery == "hold" {
                    "held"
                } else {
                    "pending"
                };
                tx.execute(
                    "UPDATE session_input SET delivery = ?2, status = ?3 WHERE message_id = ?1",
                    params![id, delivery, status],
                )?
            }
            None => 0,
        },
    };
    Ok(())
}

fn project_usage(tx: &Transaction<'_>, id: &str, d: &Value) -> rusqlite::Result<()> {
    let u = &d["usage"];
    let n = |k: &str| u[k].as_i64().unwrap_or(0);
    let cost = d["cost"].as_f64();
    tx.execute(
        "UPDATE session SET cost = cost + ?2, unpriced_steps = unpriced_steps + ?3,
            input_tokens = input_tokens + ?4, output_tokens = output_tokens + ?5,
            reasoning_tokens = reasoning_tokens + ?6, cache_read_tokens = cache_read_tokens + ?7,
            cache_write_tokens = cache_write_tokens + ?8
         WHERE id = ?1",
        params![
            id,
            cost.unwrap_or(0.0),
            i64::from(cost.is_none()),
            n("input"),
            n("output"),
            n("reasoning"),
            n("cache_read"),
            n("cache_write")
        ],
    )?;
    Ok(())
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_default()
}
