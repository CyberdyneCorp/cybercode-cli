//! `PromptResult` and waiting (`mcp-server` → Prompt results and waiting): the status
//! of an admitted prompt derived from its inbox row, the Session's Drain, its pending
//! requests, the background Jobs the run started and durable history, so the same
//! answer comes back after a server restart; and `cyber_prompt_wait`.
//!
//! A prompt's run is bounded: it ends for good where a later user input (TUI, SDK or
//! another MCP prompt) is promoted after the run's first finished Turn, or waits to be.
//! Nothing after that boundary (Turns, usage, text, Jobs, pending requests) is the
//! prompt's.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::Instant;

use super::super::ApiError;
use super::connection::{Connection, WaitSlot};
use super::progress::Progress;
use super::read::pending;
use super::route::route;
use super::tools::{Outcome, ToolError, ToolOutput};
use crate::runtime::settlement::owned_work_settled;
use crate::runtime::{Delivery, Entry, InputStatus, LiveEvent, SessionState};

/// `final_text` keeps at most this many bytes (its head).
const FINAL_TEXT_CAP: usize = 64 * 1024;
pub(super) const DEFAULT_WAIT_SECONDS: u64 = 50;
/// Re-derive at least this often while waiting, also without events.
const POLL: Duration = Duration::from_millis(500);

/// Where the prompt's inbox row stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Input {
    Queued,
    Promoted,
    /// Refused, or removed from the inbox without promotion.
    Refused,
}

/// How the run's last Turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RunEnd {
    pub stop_reason: &'static str,
    pub finished_at: i64,
    pub message: Option<String>,
}

/// Everything a `PromptResult` is derived from, at one instant.
#[derive(Debug, Clone)]
pub(super) struct Facts {
    pub session_id: String,
    pub message_id: String,
    pub input: Input,
    pub admitted_at: i64,
    /// The Session's Drain is running.
    pub running: bool,
    /// Pending permission requests and questions of the Session (`PendingRequest`s).
    pub pending: Vec<Value>,
    /// Inputs admitted after the promotion still wait to be promoted (a handback).
    pub later_input: bool,
    /// Every background Job started after the promotion has settled.
    pub settled: bool,
    pub end: Option<RunEnd>,
    pub turns: u64,
    pub usage: Option<Value>,
    pub final_text: Option<String>,
}

/// `status` of a `PromptResult`.
pub(super) fn status(facts: &Facts) -> &'static str {
    match facts.input {
        Input::Refused => "refused",
        Input::Queued => "queued",
        Input::Promoted if facts.running && !facts.pending.is_empty() => "waiting_for_approval",
        Input::Promoted if facts.running || facts.later_input || !facts.settled => "running",
        Input::Promoted => match facts.end.as_ref().map(|end| end.stop_reason) {
            Some("completed") => "completed",
            Some("error") => "failed",
            Some("budget_exceeded") => "budget_exceeded",
            // A run that stopped without finishing a Turn (an interrupt, a restart).
            _ => "interrupted",
        },
    }
}

fn terminal(status: &str) -> bool {
    !matches!(status, "queued" | "running" | "waiting_for_approval")
}

/// `text` within `cap` bytes, its head kept at a character boundary.
pub(super) fn head(text: &str, cap: usize) -> (&str, bool) {
    if text.len() <= cap {
        return (text, false);
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// The `PromptResult` of `facts`.
pub(super) fn result(facts: &Facts) -> Value {
    let status = status(facts);
    let done = terminal(status);
    let mut value = json!({
        "session_id": facts.session_id,
        "message_id": facts.message_id,
        "status": status,
        "done": done,
        "admitted_at": facts.admitted_at,
        "turns": facts.turns,
    });
    if let Some(usage) = &facts.usage {
        value["usage"] = usage.clone();
    }
    if status == "waiting_for_approval" {
        value["pending"] = json!(facts.pending);
    }
    match status {
        "refused" => {
            value["error"] = json!({ "_tag": "ConflictError",
                "message": "the prompt left the inbox without being promoted" });
        }
        _ if done => finished(&mut value, facts),
        _ => {}
    }
    value
}

fn finished(value: &mut Value, facts: &Facts) {
    let end = facts.end.as_ref();
    value["stop_reason"] = json!(end.map_or("interrupted", |end| end.stop_reason));
    if let Some(end) = end {
        value["finished_at"] = json!(end.finished_at);
        if end.stop_reason == "error" {
            let message = end.message.as_deref().unwrap_or("the Turn failed");
            value["error"] = json!({ "_tag": "UnknownError", "message": message });
        }
    }
    if let Some(text) = &facts.final_text {
        let (kept, truncated) = head(text, FINAL_TEXT_CAP);
        value["final_text"] = json!(kept);
        if truncated {
            value["truncated"] = json!(true);
        }
    }
}

// Observation.

/// Durable facts of the run after its promotion, from the event log.
#[derive(Default)]
struct History {
    admitted_at: Option<i64>,
    promoted_at: Option<i64>,
    end: Option<RunEnd>,
    turns: u64,
    usage: Usage,
    /// Request ID → when it was asked.
    asked: HashMap<String, i64>,
    /// The promotion of the later user input that ended the run for good: its seq,
    /// time and message.
    cutoff: Option<(i64, i64, String)>,
}

#[derive(Default)]
struct Usage {
    tokens: [u64; 5],
    cost: f64,
    unpriced: bool,
    steps: u64,
}

impl Usage {
    fn add(&mut self, data: &Value) {
        let usage = &data["usage"];
        for (slot, key) in self.tokens.iter_mut().zip(USAGE_KEYS) {
            *slot += usage[key].as_u64().unwrap_or(0);
        }
        match data["cost"].as_f64() {
            Some(cost) => self.cost += cost,
            None => self.unpriced = true,
        }
        self.steps += 1;
    }

    fn value(&self) -> Option<Value> {
        (self.steps > 0).then(|| {
            let [input, output, reasoning, cache_read, cache_write] = self.tokens;
            json!({
                "input_tokens": input, "output_tokens": output, "reasoning_tokens": reasoning,
                "cache_read_tokens": cache_read, "cache_write_tokens": cache_write,
                "cost_usd": if self.unpriced { Value::Null } else { json!(self.cost) },
            })
        })
    }
}

const USAGE_KEYS: [&str; 5] = ["input", "output", "reasoning", "cache_read", "cache_write"];

const HISTORY_EVENTS: &str = "'session.prompt.admitted.1','session.prompt.promoted.1',\
'session.step.ended.1','session.step.failed.1','permission.asked.1','question.asked.1'";

const PROMOTED: &str = "session.prompt.promoted.1";

impl History {
    fn record(&mut self, seq: i64, time: i64, kind: &str, data: &Value, marks: (i64, i64)) {
        let (admitted, promoted) = marks;
        match kind {
            _ if seq == admitted => self.admitted_at = Some(time),
            _ if seq == promoted => self.promoted_at = Some(time),
            _ if seq < promoted => {}
            PROMOTED => {}
            "session.step.ended.1" => {
                self.usage.add(data);
                if data["finish"] != "tool_calls" {
                    self.ended("completed", time, None);
                }
            }
            "session.step.failed.1" => {
                let reason = match data["kind"].as_str() {
                    Some("interrupted") => "interrupted",
                    Some("budget_exceeded") => "budget_exceeded",
                    _ => "error",
                };
                self.ended(reason, time, data["message"].as_str().map(str::to_owned));
            }
            _ => {
                if let Some(id) = data["id"].as_str() {
                    self.asked.insert(id.to_owned(), time);
                }
            }
        }
    }

    fn ended(&mut self, stop_reason: &'static str, finished_at: i64, message: Option<String>) {
        self.turns += 1;
        self.end = Some(RunEnd {
            stop_reason,
            finished_at,
            message,
        });
    }
}

/// The admission and run events of a prompt admitted at `admitted` and promoted at
/// `promoted` (`i64::MAX` when not promoted), up to the promotion of a later user input
/// (one of `users`) after the run's first finished Turn.
fn history(
    connection: &Connection,
    session_id: &str,
    (admitted, promoted): (i64, i64),
    users: &HashSet<String>,
) -> Result<History, ApiError> {
    let id = session_id.to_owned();
    let rows = connection
        .state
        .store
        .read(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT seq, time, type, data FROM event WHERE aggregate_id = ?1 AND seq >= ?2 \
                 AND type IN ({HISTORY_EVENTS}) ORDER BY seq"
            ))?;
            let rows = stmt
                .query_map(rusqlite::params![id, admitted], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<Result<Vec<(i64, i64, String, String)>, _>>()?;
            Ok(rows)
        })
        .map_err(ApiError::unknown)?;
    let mut history = History::default();
    for (seq, time, kind, data) in rows {
        let data: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
        let message = data["message_id"].as_str().unwrap_or_default();
        if kind == PROMOTED && seq > promoted && history.end.is_some() && users.contains(message) {
            history.cutoff = Some((seq, time, message.to_owned()));
            break;
        }
        history.record(seq, time, &kind, &data, (admitted, promoted));
    }
    Ok(history)
}

/// User inputs of the Session other than `message_id` (the inputs that start runs of
/// their own).
fn other_user_inputs(state: &SessionState, message_id: &str) -> HashSet<String> {
    state
        .inbox
        .iter()
        .filter(|row| row.source == "user" && row.message_id != message_id)
        .map(|row| row.message_id.clone())
        .collect()
}

/// A later user input waits to be promoted after the run's Turn finished: the Drain now
/// runs for it, not for the prompt.
fn user_input_waiting(state: &SessionState, message_id: &str, promoted: i64) -> bool {
    state.inbox.iter().any(|row| {
        row.source == "user"
            && row.message_id != message_id
            && row.status == InputStatus::Pending
            && row.delivery != Delivery::Hold
            && row.admitted_seq > promoted
    })
}

/// The last assistant text after the prompt's history entry, before the entry of the
/// input that ended its run (`until`).
fn final_text(state: &SessionState, message_id: &str, until: Option<&str>) -> Option<String> {
    let start = state
        .entries
        .iter()
        .position(|entry| entry.id() == message_id)?;
    let end = until
        .and_then(|id| state.entries.iter().position(|entry| entry.id() == id))
        .filter(|&end| end > start)
        .unwrap_or(state.entries.len());
    state.entries[start..end]
        .iter()
        .rev()
        .find_map(|entry| match entry {
            Entry::Assistant(a) if !a.text.is_empty() => Some(a.text.clone()),
            _ => None,
        })
}

/// Observe one prompt now.
pub(super) async fn observe(
    connection: &Connection,
    session_id: &str,
    message_id: &str,
) -> Result<Facts, ToolError> {
    let runtime = &connection.state.runtime;
    let state = runtime.state(session_id).await.map_err(ApiError::from)?;
    let record = cyber_store::mcp_serve::input(&connection.state.store, message_id)
        .map_err(ApiError::unknown)?
        .filter(|record| record.session_id == session_id);
    let Some(row) = state.input(message_id) else {
        let Some(record) = record else {
            return Err(message_not_found(message_id).into());
        };
        return Ok(removed(session_id, message_id, record.admitted_ms));
    };
    let (input, promoted) = match (row.status, row.promoted_seq) {
        (InputStatus::Refused, _) => (Input::Refused, i64::MAX),
        (InputStatus::Promoted, Some(seq)) => (Input::Promoted, seq),
        _ => (Input::Queued, i64::MAX),
    };
    let users = other_user_inputs(&state, message_id);
    let history = history(connection, session_id, (row.admitted_seq, promoted), &users)?;
    let over = history.cutoff.is_some()
        || (history.end.is_some() && user_input_waiting(&state, message_id, promoted));
    let running = !over && runtime.is_running(session_id);
    let pending = if running {
        pending(connection, session_id, &history.asked).await?
    } else {
        Vec::new()
    };
    let until = history.cutoff.as_ref().map(|(_, time, _)| *time);
    let settled = input != Input::Promoted
        || running
        || jobs_settled(connection, session_id, (history.promoted_at, until)).await?;
    let admitted_at = record
        .map(|r| r.admitted_ms)
        .or(history.admitted_at)
        .unwrap_or_default();
    let cutoff_message = history.cutoff.as_ref().map(|(_, _, id)| id.as_str());
    Ok(Facts {
        session_id: session_id.to_owned(),
        message_id: message_id.to_owned(),
        input,
        admitted_at,
        running,
        pending,
        later_input: !over && later_input(&state, promoted),
        settled,
        final_text: (input == Input::Promoted)
            .then(|| final_text(&state, message_id, cutoff_message))
            .flatten(),
        usage: history.usage.value(),
        turns: history.turns,
        end: history.end,
    })
}

fn message_not_found(message_id: &str) -> ApiError {
    ApiError::not_found(
        "MessageNotFoundError",
        format!("message {message_id} not found"),
    )
}

/// A prompt admitted through MCP serve whose inbox row was removed before promotion.
fn removed(session_id: &str, message_id: &str, admitted_at: i64) -> Facts {
    Facts {
        session_id: session_id.to_owned(),
        message_id: message_id.to_owned(),
        input: Input::Refused,
        admitted_at,
        running: false,
        pending: Vec::new(),
        later_input: false,
        settled: true,
        end: None,
        turns: 0,
        usage: None,
        final_text: None,
    }
}

/// Promotable inputs admitted after the promotion (a Job's handback) keep the run going;
/// a later user input starts a run of its own.
fn later_input(state: &SessionState, promoted: i64) -> bool {
    state.inbox.iter().any(|row| {
        row.status == InputStatus::Pending
            && row.delivery != Delivery::Hold
            && row.admitted_seq > promoted
            && row.source != "user"
    })
}

/// The owned-work rule over the Jobs the Session started since the promotion and before
/// the run ended for good (`until`).
async fn jobs_settled(
    connection: &Connection,
    session_id: &str,
    (since, until): (Option<i64>, Option<i64>),
) -> Result<bool, ToolError> {
    let Some(since) = since else {
        return Ok(true);
    };
    let mut jobs = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut query = vec![("session_id", session_id), ("limit", "200")];
        if let Some(cursor) = &cursor {
            query.push(("cursor", cursor.as_str()));
        }
        let mut page = route(connection, "v1.job.list", &[], &query).await?;
        if let Value::Array(rows) = page["data"].take() {
            jobs.extend(rows);
        }
        match page["cursor"]["next"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    if let Some(until) = until {
        jobs.retain(|job| job["started_ms"].as_i64().is_some_and(|t| t < until));
    }
    let now = chrono::Utc::now().timestamp_millis();
    Ok(owned_work_settled(&jobs, since, now))
}

// Waiting.

/// Wait on a prompt until its run is done, `seconds` pass, or the run raises a
/// permission request or question that was not pending when the wait began (`before`;
/// the Session's pending requests now when `None`).
pub(super) async fn wait(
    connection: &Connection,
    session_id: &str,
    message_id: &str,
    seconds: u64,
    progress: &Progress,
    before: Option<HashSet<String>>,
) -> Result<Value, ToolError> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut events = connection.state.runtime.subscribe();
    let before = before.unwrap_or_else(|| request_ids(connection, session_id));
    loop {
        let facts = observe(connection, session_id, message_id).await?;
        let status = status(&facts);
        let raised = status == "waiting_for_approval"
            && facts.pending.iter().any(|p| {
                p["request_id"]
                    .as_str()
                    .is_some_and(|id| !before.contains(id))
            });
        if terminal(status) || raised || Instant::now() >= deadline {
            return Ok(result(&facts));
        }
        next_change(&mut events, session_id, deadline, progress).await;
    }
}

/// The Session's pending permission requests and questions.
pub(super) fn request_ids(connection: &Connection, session_id: &str) -> HashSet<String> {
    connection
        .state
        .runtime
        .pending_requests(Some(session_id))
        .into_iter()
        .map(|request| request.id)
        .collect()
}

/// Sleep until a durable event or the end of a Drain of the Session (reported as
/// progress), the poll interval, or the deadline.
async fn next_change(
    events: &mut tokio::sync::broadcast::Receiver<LiveEvent>,
    session_id: &str,
    deadline: Instant,
    progress: &Progress,
) {
    let poll = tokio::time::sleep_until(deadline.min(Instant::now() + POLL));
    tokio::pin!(poll);
    loop {
        tokio::select! {
            () = &mut poll => return,
            event = events.recv() => match event {
                Ok(event) if concerns(&event, session_id) => {
                    progress.event(&event);
                    return;
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

fn concerns(event: &LiveEvent, session_id: &str) -> bool {
    match event {
        LiveEvent::Durable { session_id: id, .. } | LiveEvent::Idle { session_id: id } => {
            id == session_id
        }
        _ => false,
    }
}

/// `wait_seconds` (default 50) clamped to `max_wait_seconds`.
pub(super) fn wait_seconds(args: &Value, limit: u32) -> u64 {
    args["wait_seconds"]
        .as_u64()
        .unwrap_or(DEFAULT_WAIT_SECONDS)
        .min(u64::from(limit))
}

/// A `max_waits` slot for a call that waits.
pub(super) fn reserve_wait(connection: &std::sync::Arc<Connection>) -> Result<WaitSlot, ApiError> {
    let limit = connection.state.services.mcp_serve()?.limits.max_waits;
    connection.reserve_wait(limit).ok_or_else(|| {
        ApiError::rate_limited(
            "concurrency",
            5000,
            format!("this MCP connection already has {limit} waiting calls"),
        )
    })
}

/// `cyber_prompt_wait`: re-attach to an admitted prompt from durable events.
pub(super) async fn prompt_wait(
    connection: &std::sync::Arc<Connection>,
    args: &Value,
    progress: &Progress,
) -> Outcome {
    let session_id = args["session_id"].as_str().unwrap_or_default();
    let message_id = args["message_id"].as_str().unwrap_or_default();
    let limit = connection
        .state
        .services
        .mcp_serve()?
        .limits
        .max_wait_seconds;
    let seconds = wait_seconds(args, limit);
    let value = if seconds == 0 {
        result(&observe(connection, session_id, message_id).await?)
    } else {
        let _slot = reserve_wait(connection)?;
        // Unknown Sessions and messages fail before the wait begins.
        observe(connection, session_id, message_id).await?;
        progress.attach(session_id, message_id);
        wait(connection, session_id, message_id, seconds, progress, None).await?
    };
    Ok(ToolOutput {
        value,
        untrusted: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(input: Input) -> Facts {
        Facts {
            session_id: "ses_1".into(),
            message_id: "msg_1".into(),
            input,
            admitted_at: 1,
            running: false,
            pending: Vec::new(),
            later_input: false,
            settled: true,
            end: None,
            turns: 0,
            usage: None,
            final_text: None,
        }
    }

    fn ended(reason: &'static str) -> Facts {
        let mut facts = facts(Input::Promoted);
        facts.end = Some(RunEnd {
            stop_reason: reason,
            finished_at: 9,
            message: Some("boom".into()),
        });
        facts.turns = 1;
        facts.final_text = Some("answer".into());
        facts
    }

    #[test]
    fn every_status_is_derived() {
        assert_eq!(status(&facts(Input::Queued)), "queued");
        let mut running = facts(Input::Promoted);
        running.running = true;
        assert_eq!(status(&running), "running");
        running.pending = vec![json!({"request_id": "per_1"})];
        assert_eq!(status(&running), "waiting_for_approval");
        let mut jobs = ended("completed");
        jobs.settled = false;
        assert_eq!(status(&jobs), "running", "an owned Job is unsettled");
        let mut handback = ended("completed");
        handback.later_input = true;
        assert_eq!(status(&handback), "running", "a handback is still to come");
        assert_eq!(status(&ended("completed")), "completed");
        assert_eq!(status(&ended("error")), "failed");
        assert_eq!(status(&ended("interrupted")), "interrupted");
        assert_eq!(status(&ended("budget_exceeded")), "budget_exceeded");
        assert_eq!(status(&facts(Input::Promoted)), "interrupted");
        assert_eq!(status(&facts(Input::Refused)), "refused");
    }

    #[test]
    fn results_carry_what_each_status_needs() {
        let refused = result(&facts(Input::Refused));
        assert_eq!(refused["done"], true);
        assert_eq!(refused["error"]["_tag"], "ConflictError");
        assert!(refused.get("stop_reason").is_none());

        let queued = result(&facts(Input::Queued));
        assert_eq!(queued["done"], false);
        assert!(queued.get("final_text").is_none());

        let failed = result(&ended("error"));
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["stop_reason"], "error");
        assert_eq!(failed["error"]["message"], "boom");
        assert_eq!(failed["finished_at"], 9);

        let done = result(&ended("completed"));
        assert_eq!(done["final_text"], "answer");
        assert!(done.get("error").is_none() && done.get("truncated").is_none());

        let mut waiting = facts(Input::Promoted);
        waiting.running = true;
        waiting.pending = vec![json!({"request_id": "per_1"})];
        let waiting = result(&waiting);
        assert_eq!(waiting["pending"][0]["request_id"], "per_1");
    }

    #[test]
    fn final_text_keeps_its_head_and_stays_valid_json() {
        let mut large = ended("completed");
        let text = format!("{}{}", "\"é\\\n".repeat(60_000), "tail");
        large.final_text = Some(text.clone());
        let value = result(&large);
        assert_eq!(value["truncated"], true);
        let kept = value["final_text"].as_str().unwrap();
        assert!(kept.len() <= FINAL_TEXT_CAP);
        assert!(text.starts_with(kept));
        let round: Value = serde_json::from_str(&value.to_string()).unwrap();
        assert_eq!(round["final_text"], kept);
        assert_eq!(head("short", 10), ("short", false));
        assert_eq!(head("éé", 3), ("é", true));
    }

    #[test]
    fn wait_seconds_default_and_clamp() {
        assert_eq!(wait_seconds(&json!({}), 600), 50);
        assert_eq!(wait_seconds(&json!({"wait_seconds": 3600}), 600), 600);
        assert_eq!(wait_seconds(&json!({"wait_seconds": 0}), 600), 0);
    }
}
