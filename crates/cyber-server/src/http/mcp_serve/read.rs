//! The reading tools and `cyber_interrupt` (`mcp-server` → Session reading over MCP,
//! Prompt results and waiting). Each maps onto the `server-api` session routes through
//! the connection's in-process client, so route errors reach the client unchanged;
//! durable sequence numbers and times come from the event log.

use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::OptionalExtension;
use serde_json::{Value, json};

use super::super::sessions::list_page;
use super::super::{ApiError, AppState};
use super::caller::Caller;
use super::connection::Connection;
use super::route::route;
use super::tools::{Outcome, ToolError, ToolOutput};
use crate::runtime::{ListFilter, Runtime, SessionRow};

/// Cap of each transcript `text` and tool `output`.
const ITEM_CAP: usize = 16 * 1024;
const PREVIEW_CAP: usize = 2 * 1024;
const SUMMARY_CHARS: usize = 512;
const PENDING_CAP: usize = 20;
const RECENT_CAP: usize = 20;
const DEFAULT_MAX_BYTES: u64 = 65_536;
/// Rows one `cyber_session_list` call examines at most while filtering by state.
const SCAN_CAP: usize = 1000;

fn count(args: &Value, key: &str, default: u64) -> usize {
    usize::try_from(args[key].as_u64().unwrap_or(default)).unwrap_or(usize::MAX)
}

fn session_id(args: &Value) -> &str {
    args["session_id"].as_str().unwrap_or_default()
}

/// `idle`, `running`, or `waiting_for_approval` (running with a pending request).
pub(super) fn session_state(runtime: &Runtime, id: &str) -> &'static str {
    if !runtime.is_running(id) {
        "idle"
    } else if runtime.pending_requests(Some(id)).is_empty() {
        "running"
    } else {
        "waiting_for_approval"
    }
}

/// `SessionSummary` from a `SessionRow` or `Session` JSON object; `mcp_depth` is the
/// depth of its running run (0 when idle).
pub(super) fn summary(
    row: &Value,
    created_at: i64,
    updated_at: Option<i64>,
    (state, mcp_depth): (&str, u32),
) -> Value {
    let mut summary = json!({
        "session_id": row["id"],
        "title": row["title"],
        "directory": row["directory"],
        "agent": row["agent"],
        "model": row["model"],
        "mode": row["mode"],
        "state": state,
        "mcp_depth": mcp_depth,
        "created_at": created_at,
        "updated_at": updated_at.unwrap_or(created_at),
    });
    if row["parent_id"].is_string() {
        summary["parent_id"] = row["parent_id"].clone();
    }
    summary
}

/// The MCP clients that created `ids` through MCP serve, from its records.
fn mcp_clients(state: &AppState, ids: Vec<String>) -> Result<HashMap<String, String>, ApiError> {
    cyber_store::mcp_serve::session_clients(&state.store, ids).map_err(ApiError::unknown)
}

// Truncation and paging (`mcp-server` → Session reading over MCP, MCP serve limits).

fn marker(bytes: usize) -> String {
    format!("[… truncated {bytes} bytes]")
}

/// `text` within `limit` bytes, its head kept and the marker appended when cut.
fn cap(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_owned(), false);
    }
    let mut keep = limit.saturating_sub(marker(text.len()).len());
    while !text.is_char_boundary(keep) {
        keep -= 1;
    }
    (
        format!("{}{}", &text[..keep], marker(text.len() - keep)),
        true,
    )
}

/// A page item whose text fields may be shortened, in order, to fit a byte budget.
struct Cuttable {
    base: Value,
    /// `(parent key, key, full text)`; the parent is `None` for a top-level field.
    fields: Vec<(Option<&'static str>, &'static str, String)>,
}

impl Cuttable {
    fn render(&self, limits: &[usize]) -> (Value, bool) {
        let mut value = self.base.clone();
        let mut cut = false;
        for ((parent, key, text), &limit) in self.fields.iter().zip(limits) {
            let (text, shortened) = cap(text, limit);
            cut |= shortened;
            let target = match parent {
                Some(parent) => &mut value[*parent],
                None => &mut value,
            };
            target[*key] = Value::String(text);
        }
        (value, cut)
    }

    /// Render with each field capped at `cap`, shortening the fields in order until the
    /// serialized item fits `budget` (or nothing is left to shorten).
    fn fit(&self, budget: usize, cap: usize) -> (Value, bool) {
        let mut limits = vec![cap; self.fields.len()];
        loop {
            let (value, cut) = self.render(&limits);
            let size = value.to_string().len();
            let next = (0..limits.len()).find(|&i| limits[i] > 0 && !self.fields[i].2.is_empty());
            match next {
                Some(i) if size > budget => {
                    let current = limits[i].min(self.fields[i].2.len());
                    limits[i] = current.saturating_sub(size - budget);
                }
                _ => return (value, cut),
            }
        }
    }
}

/// Items of one page, bounded by the serialized size of the item array.
struct Pager {
    max_bytes: usize,
    used: usize,
    items: Vec<Value>,
    truncated: bool,
}

impl Pager {
    fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            used: 2,
            items: Vec::new(),
            truncated: false,
        }
    }

    /// Add the next item, cut to fit when it is the first; `false` (nothing added) when
    /// the page is full.
    fn push(&mut self, item: &Cuttable, cap: usize) -> bool {
        let separator = usize::from(!self.items.is_empty());
        let room = self.max_bytes.saturating_sub(self.used + separator);
        let budget = if self.items.is_empty() {
            room
        } else {
            usize::MAX
        };
        let (value, cut) = item.fit(budget, cap);
        let size = value.to_string().len();
        if !self.items.is_empty() && size > room {
            return false;
        }
        self.used += separator + size;
        self.truncated |= cut;
        self.items.push(value);
        true
    }
}

// cyber_session_list

pub(super) async fn list(connection: &Connection, args: &Value) -> Outcome {
    let directory = args["directory"]
        .as_str()
        .map(absolute_directory)
        .transpose()?;
    let mut pager = Pager::new(count(args, "max_bytes", DEFAULT_MAX_BYTES));
    let filter = Listing {
        directory,
        state: args["state"].as_str(),
        mcp_only: args["mcp_only"] == true,
        limit: count(args, "limit", 50),
    };
    let cursor = args["cursor"].as_str().map(str::to_owned);
    let next = filter.collect(&connection.state, cursor, &mut pager)?;
    let mut value = json!({ "sessions": pager.items, "truncated": pager.truncated });
    if let Some(next) = next {
        value["next_cursor"] = json!(next);
    }
    Ok(ToolOutput {
        value,
        untrusted: true,
    })
}

fn absolute_directory(directory: &str) -> Result<String, ApiError> {
    let path = std::path::Path::new(directory);
    if !path.is_absolute() {
        return Err(
            ApiError::invalid(format!("{directory} is not an absolute directory"))
                .with_reason("not_a_directory"),
        );
    }
    Ok(std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string())
}

struct Listing<'a> {
    directory: Option<String>,
    state: Option<&'a str>,
    /// Only Sessions created through MCP serve.
    mcp_only: bool,
    limit: usize,
}

impl Listing<'_> {
    /// Fill the page from store pages; the result is the cursor after the last examined
    /// row when more rows remain.
    fn collect(
        &self,
        state: &AppState,
        mut cursor: Option<String>,
        pager: &mut Pager,
    ) -> Result<Option<String>, ApiError> {
        let mut scanned = 0;
        loop {
            let filter = ListFilter {
                directory: self.directory.clone(),
                limit: Some(200),
                cursor: cursor.clone(),
                ..ListFilter::default()
            };
            let page = list_page(state, &filter)?;
            let clients = mcp_clients(state, page.sessions.iter().map(|r| r.id.clone()).collect())?;
            for row in &page.sessions {
                if pager.items.len() == self.limit || scanned == SCAN_CAP {
                    return Ok(cursor);
                }
                scanned += 1;
                let label = session_state(&state.runtime, &row.id);
                let client = clients.get(&row.id);
                let depth = state.runtime.run_mcp_depth(&row.id);
                if self.keeps(label, client)
                    && !pager.push(&listed(row, (label, depth), client), usize::MAX)
                {
                    return Ok(cursor);
                }
                cursor = Some(row.cursor());
            }
            if page.next.is_none() {
                return Ok(None);
            }
        }
    }
}

impl Listing<'_> {
    fn keeps(&self, state: &str, client: Option<&String>) -> bool {
        self.state.is_none_or(|wanted| wanted == state) && (!self.mcp_only || client.is_some())
    }
}

fn listed(row: &SessionRow, state: (&str, u32), client: Option<&String>) -> Cuttable {
    let json = serde_json::to_value(row).unwrap_or_default();
    let mut base = summary(&json, row.created_at, Some(row.updated_at), state);
    if let Some(client) = client {
        base["mcp_client"] = json!(client);
    }
    let fields = ["title", "directory"].map(|key| {
        let text = base[key].as_str().unwrap_or_default().to_owned();
        (None, key, text)
    });
    if let Some(map) = base.as_object_mut() {
        map.remove("title");
        map.remove("directory");
    }
    Cuttable {
        base,
        fields: fields.into(),
    }
}

// cyber_session_read

/// Durable positions and times of one Session, read from its event log.
#[derive(Default)]
struct Timeline {
    updated_at: Option<i64>,
    /// `message_id` → `(seq, time)` of the event that started it.
    messages: HashMap<String, (i64, i64)>,
    /// `call_id` → `(seq, time)` of its `session.tool.called.1`.
    calls: HashMap<String, (i64, i64)>,
    /// Request ID → time it was asked.
    asked: HashMap<String, i64>,
    last_turn: Option<TurnEnd>,
}

struct TurnEnd {
    message_id: String,
    finished_at: i64,
    stop_reason: &'static str,
}

const TIMELINE_EVENTS: &str = "'session.prompt.promoted.1','session.step.started.1','session.system.added.1',\
'session.context.updated.1','session.tool.called.1','session.step.ended.1','session.step.failed.1',\
'permission.asked.1','question.asked.1'";

type EventRow = (i64, i64, String, String);

fn timeline(state: &AppState, id: &str) -> Result<Timeline, ApiError> {
    let id = id.to_owned();
    let (updated_at, events) = state
        .store
        .read(move |conn| {
            let updated_at: Option<i64> = conn
                .query_row("SELECT updated_at FROM session WHERE id = ?1", [&id], |r| r.get(0))
                .optional()?;
            let mut stmt = conn.prepare(&format!(
                "SELECT seq, time, type, data FROM event WHERE aggregate_id = ?1 AND type IN ({TIMELINE_EVENTS}) ORDER BY seq"
            ))?;
            let events = stmt
                .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<Result<Vec<EventRow>, _>>()?;
            Ok((updated_at, events))
        })
        .map_err(ApiError::unknown)?;
    let mut timeline = Timeline {
        updated_at,
        ..Timeline::default()
    };
    for (seq, time, kind, data) in events {
        if let Ok(data) = serde_json::from_str::<Value>(&data) {
            timeline.record(seq, time, &kind, &data);
        }
    }
    Ok(timeline)
}

impl Timeline {
    fn record(&mut self, seq: i64, time: i64, kind: &str, data: &Value) {
        let text = |key: &str| data[key].as_str().map(str::to_owned);
        match kind {
            "session.tool.called.1" => {
                if let Some(call) = text("call_id") {
                    self.calls.entry(call).or_insert((seq, time));
                }
            }
            "permission.asked.1" | "question.asked.1" => {
                if let Some(request) = text("id") {
                    self.asked.insert(request, time);
                }
            }
            "session.step.ended.1" if data["finish"] == "tool_calls" => {}
            "session.step.ended.1" | "session.step.failed.1" => {
                let stop_reason = match (kind, data["kind"].as_str()) {
                    ("session.step.ended.1", _) => "completed",
                    (_, Some("interrupted")) => "interrupted",
                    (_, Some("budget_exceeded")) => "budget_exceeded",
                    _ => "error",
                };
                self.last_turn = text("message_id").map(|message_id| TurnEnd {
                    message_id,
                    finished_at: time,
                    stop_reason,
                });
            }
            _ => {
                if let Some(message) = text("message_id") {
                    self.messages.entry(message).or_insert((seq, time));
                }
            }
        }
    }
}

pub(super) async fn read(connection: &Connection, args: &Value) -> Outcome {
    let id = session_id(args);
    let session = route(connection, "v1.session.get", &[("sessionID", id)], &[]).await?;
    let session = &session["data"];
    let timeline = timeline(&connection.state, id)?;
    let pending = pending(connection, id, &timeline.asked).await?;
    let inbox = route(connection, "v1.session.inbox", &[("sessionID", id)], &[]).await?;
    let usage = route(
        connection,
        "v1.usage.get",
        &[],
        &[("scope", "session"), ("id", id)],
    )
    .await?;
    let state = match session["status"].as_str() {
        Some("running") if pending.is_empty() => "running",
        Some("running") => "waiting_for_approval",
        _ => "idle",
    };
    let inbox_pending = inbox["data"].as_array().map_or(0, |rows| {
        rows.iter()
            .filter(|row| matches!(row["status"].as_str(), Some("pending" | "held")))
            .count()
    });
    let created_at = session["created_ms"].as_i64().unwrap_or_default();
    let depth = connection.state.runtime.run_mcp_depth(id);
    let mut summary = summary(session, created_at, timeline.updated_at, (state, depth));
    if let Some(client) = mcp_clients(&connection.state, vec![id.to_owned()])?.remove(id) {
        summary["mcp_client"] = json!(client);
    }
    let mut value = json!({
        "session": summary,
        "usage": usage_of(&usage["data"]["total"]),
        "pending": pending,
        "inbox_pending": inbox_pending,
    });
    if let Some(turn) = &timeline.last_turn {
        value["last_turn"] = last_turn(connection, id, turn).await?;
    }
    value["recent_prompts"] = recent_prompts(connection, id).await?;
    Ok(ToolOutput {
        value,
        untrusted: true,
    })
}

/// The 20 newest prompts admitted to the Session through MCP serve, with their status.
async fn recent_prompts(connection: &Connection, id: &str) -> Result<Value, ToolError> {
    let inputs = cyber_store::mcp_serve::recent_inputs(&connection.state.store, id, RECENT_CAP)
        .map_err(ApiError::unknown)?;
    let mut prompts = Vec::with_capacity(inputs.len());
    for input in inputs {
        let facts = super::wait::observe(connection, id, &input.message_id).await?;
        prompts.push(json!({
            "message_id": input.message_id,
            "admitted_at": input.admitted_ms,
            "status": super::wait::status(&facts),
        }));
    }
    Ok(Value::Array(prompts))
}

/// `Usage` from a `UsageAmount`; `cost_usd` is null when any counted step is unpriced.
fn usage_of(amount: &Value) -> Value {
    let tokens = &amount["tokens"];
    let priced = amount["unpriced_steps"].as_u64() == Some(0) && amount["usage_complete"] != false;
    json!({
        "input_tokens": tokens["input"],
        "output_tokens": tokens["output"],
        "reasoning_tokens": tokens["reasoning"],
        "cache_read_tokens": tokens["cache_read"],
        "cache_write_tokens": tokens["cache_write"],
        "cost_usd": if priced { amount["cost"].clone() } else { Value::Null },
    })
}

/// The first 20 pending permission requests and questions, as `PendingRequest`s, with
/// the times they were asked (by request ID).
pub(super) async fn pending(
    connection: &Connection,
    id: &str,
    asked: &HashMap<String, i64>,
) -> Result<Vec<Value>, ToolError> {
    let mut pending = Vec::new();
    for operation in ["v1.permission.list", "v1.question.list"] {
        let listed = route(connection, operation, &[], &[("session_id", id)]).await?;
        for request in listed["data"].as_array().into_iter().flatten() {
            pending.push(pending_request(request, asked));
        }
    }
    pending.truncate(PENDING_CAP);
    Ok(pending)
}

fn pending_request(request: &Value, asked: &HashMap<String, i64>) -> Value {
    let summary = if request["kind"] == "permission" {
        let resources: Vec<&str> = request["resources"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        format!(
            "{} {}",
            request["action"].as_str().unwrap_or_default(),
            resources.join(", ")
        )
    } else {
        let questions: Vec<&str> = request["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|q| q["question"].as_str())
            .collect();
        questions.join(" / ")
    };
    let mut value = json!({
        "request_id": request["id"],
        "kind": request["kind"],
        "session_id": request["session_id"],
        "summary": summary.trim().chars().take(SUMMARY_CHARS).collect::<String>(),
    });
    if let Some(time) = request["id"].as_str().and_then(|id| asked.get(id)) {
        value["created_at"] = json!(time);
    }
    value
}

async fn last_turn(connection: &Connection, id: &str, turn: &TurnEnd) -> Result<Value, ToolError> {
    let path = [("sessionID", id), ("messageID", turn.message_id.as_str())];
    let text = match route(connection, "v1.message.get", &path, &[]).await {
        Ok(message) => message["data"]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        // A message rewound away leaves the outcome without its text.
        Err(ToolError(body)) if body["_tag"] == "MessageNotFoundError" => String::new(),
        Err(error) => return Err(error),
    };
    Ok(json!({
        "finished_at": turn.finished_at,
        "stop_reason": turn.stop_reason,
        "final_text_preview": cap(&text, PREVIEW_CAP).0,
    }))
}

// cyber_session_transcript

/// The whole durable history, oldest first, through the message routes.
async fn messages(connection: &Connection, id: &str) -> Result<Vec<Value>, ToolError> {
    let mut messages = Vec::new();
    let mut cursor = String::from("0");
    loop {
        let query = [
            ("order", "asc"),
            ("limit", "200"),
            ("cursor", cursor.as_str()),
        ];
        let mut page = route(connection, "v1.message.list", &[("sessionID", id)], &query).await?;
        if let Value::Array(data) = page["data"].take() {
            messages.extend(data);
        }
        match page["cursor"]["next"].as_str() {
            Some(next) => cursor = next.to_owned(),
            None => return Ok(messages),
        }
    }
}

struct Positions<'a> {
    timeline: &'a Timeline,
    include_output: bool,
    seq: i64,
}

impl Positions<'_> {
    /// Items of one history entry: user, notice or assistant text, then its tool calls.
    fn items(&mut self, message: &Value, out: &mut Vec<Cuttable>) {
        let id = message["id"].as_str().unwrap_or_default();
        let at = self.timeline.messages.get(id).copied();
        let (role, text) = match message["kind"].as_str() {
            Some("user") => ("user", user_text(&message["parts"])),
            Some("system") => (
                "notice",
                message["text"].as_str().unwrap_or_default().to_owned(),
            ),
            _ => (
                "assistant",
                message["text"].as_str().unwrap_or_default().to_owned(),
            ),
        };
        let calls = message["tools"].as_array().map_or(&[][..], Vec::as_slice);
        if role != "assistant" || !text.is_empty() || calls.is_empty() {
            let mut base = self.base(id, role, at);
            if role == "user" {
                base["origin"] = message["source"].clone();
            }
            out.push(Cuttable {
                base,
                fields: vec![(None, "text", text)],
            });
        }
        for call in calls {
            out.push(self.call(id, call, at));
        }
    }

    fn base(&mut self, id: &str, role: &str, at: Option<(i64, i64)>) -> Value {
        // Entries without a recorded start (imported history) keep the previous position.
        self.seq = at.map_or(self.seq, |(seq, _)| seq.max(self.seq));
        let mut base = json!({ "seq": self.seq, "message_id": id, "role": role });
        if let Some((_, time)) = at {
            base["created_at"] = json!(time);
        }
        base
    }

    fn call(&mut self, id: &str, call: &Value, entry: Option<(i64, i64)>) -> Cuttable {
        let call_id = call["call_id"].as_str().unwrap_or_default();
        let at = self.timeline.calls.get(call_id).copied().or(entry);
        let mut base = self.base(id, "tool", at);
        let arguments = call["arguments"].as_str().unwrap_or_default();
        base["tool"] = json!({
            "name": call["name"],
            "call_id": call_id,
            "status": call["status"],
            "input_summary": arguments.chars().take(SUMMARY_CHARS).collect::<String>(),
        });
        let mut fields = Vec::new();
        if self.include_output
            && let Some(output) = call["output"].as_str()
        {
            fields.push((Some("tool"), "output", output.to_owned()));
        }
        Cuttable { base, fields }
    }
}

fn user_text(parts: &Value) -> String {
    let texts: Vec<&str> = parts
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "text")
        .filter_map(|part| part["text"].as_str())
        .collect();
    texts.join("\n")
}

/// A transcript cursor: the next item index going forward, or the exclusive end index
/// going backward from the end.
enum Position {
    Forward(usize),
    Backward(usize),
}

fn position(args: &Value, len: usize) -> Result<Position, ApiError> {
    let Some(cursor) = args["cursor"].as_str() else {
        return Ok(match args["from"].as_str() {
            Some("end") => Position::Backward(len),
            _ => Position::Forward(0),
        });
    };
    let invalid = || {
        ApiError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "InvalidCursorError",
            "malformed or expired cursor",
        )
    };
    let (direction, index) = cursor.split_once(':').ok_or_else(invalid)?;
    let index: usize = index.parse().map_err(|_| invalid())?;
    match direction {
        "f" if index <= len => Ok(Position::Forward(index)),
        "b" if index <= len => Ok(Position::Backward(index)),
        _ => Err(invalid()),
    }
}

pub(super) async fn transcript(connection: &Connection, args: &Value) -> Outcome {
    let id = session_id(args);
    let history = messages(connection, id).await?;
    let timeline = timeline(&connection.state, id)?;
    let mut positions = Positions {
        timeline: &timeline,
        include_output: args["include_tool_output"] == true,
        seq: 0,
    };
    let mut items = Vec::new();
    for message in &history {
        positions.items(message, &mut items);
    }
    let limit = count(args, "limit", 20);
    let mut pager = Pager::new(count(args, "max_bytes", DEFAULT_MAX_BYTES));
    let next = match position(args, items.len())? {
        Position::Forward(start) => {
            let mut index = start;
            while index < items.len()
                && pager.items.len() < limit
                && pager.push(&items[index], ITEM_CAP)
            {
                index += 1;
            }
            (index < items.len()).then(|| format!("f:{index}"))
        }
        Position::Backward(end) => {
            let mut index = end;
            while index > 0 && pager.items.len() < limit && pager.push(&items[index - 1], ITEM_CAP)
            {
                index -= 1;
            }
            pager.items.reverse();
            (index > 0).then(|| format!("b:{index}"))
        }
    };
    let mut value = json!({ "items": pager.items, "truncated": pager.truncated });
    if let Some(next) = next {
        value["next_cursor"] = json!(next);
    }
    Ok(ToolOutput {
        value,
        untrusted: true,
    })
}

// cyber_interrupt

pub(super) async fn interrupt(
    connection: &Arc<Connection>,
    args: &Value,
    caller: &Caller,
) -> Outcome {
    let id = session_id(args);
    caller.check_target(connection, id)?;
    let was_running = connection.state.runtime.is_running(id);
    // Detached: cancellation ends only the answer, never a partial stop.
    let (task, target, background) = (
        Arc::clone(connection),
        id.to_owned(),
        args["stop_background"] == true,
    );
    tokio::spawn(async move {
        let path = [("sessionID", target.as_str())];
        route(&task, "v1.session.interrupt", &path, &[]).await?;
        if background {
            stop_background(&task, &target).await?;
        }
        Ok::<_, ToolError>(())
    })
    .await
    .map_err(ApiError::unknown)??;
    Ok(ToolOutput {
        value: json!({ "session_id": id, "was_running": was_running }),
        untrusted: false,
    })
}

/// Stop the Session's running background tasks through the job routes.
async fn stop_background(connection: &Connection, id: &str) -> Result<(), ToolError> {
    let mut running = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut query = vec![("session_id", id), ("limit", "200")];
        if let Some(cursor) = &cursor {
            query.push(("cursor", cursor.as_str()));
        }
        let page = route(connection, "v1.job.list", &[], &query).await?;
        let jobs = page["data"].as_array().into_iter().flatten();
        running.extend(
            jobs.filter(|job| job["status"] == "running")
                .filter_map(|job| job["id"].as_str().map(str::to_owned)),
        );
        match page["cursor"]["next"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    for job in running {
        route(connection, "v1.job.stop", &[("id", job.as_str())], &[]).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_keep_the_head_and_count_cut_bytes() {
        assert_eq!(cap("short", 10), ("short".into(), false));
        let text = "é".repeat(100);
        let (cut, shortened) = cap(&text, 60);
        assert!(shortened);
        assert!(cut.len() <= 60, "{cut}");
        let kept = cut.split("[…").next().unwrap();
        assert_eq!(cut, format!("{kept}{}", marker(text.len() - kept.len())));
    }

    #[test]
    fn pages_hold_at_least_one_item_cut_to_fit() {
        let item = |text: &str| Cuttable {
            base: json!({ "seq": 1 }),
            fields: vec![(None, "text", text.to_owned())],
        };
        let mut pager = Pager::new(1024);
        assert!(pager.push(&item(&"x".repeat(16_000)), ITEM_CAP));
        assert!(pager.truncated);
        assert!(serde_json::to_string(&pager.items).unwrap().len() <= 1024);
        assert!(!pager.push(&item("small"), ITEM_CAP));
        assert_eq!(pager.items.len(), 1);

        let mut pager = Pager::new(1024);
        assert!(pager.push(&item("a"), ITEM_CAP));
        assert!(!pager.push(&item(&"y".repeat(2000)), ITEM_CAP));
        assert!(!pager.truncated);

        let mut pager = Pager::new(196_608);
        assert!(pager.push(&item(&"z".repeat(40_000)), ITEM_CAP));
        let text = pager.items[0]["text"].as_str().unwrap();
        assert!(text.len() <= ITEM_CAP && text.ends_with("bytes]"));
        assert!(pager.truncated);
    }

    #[test]
    fn list_items_shorten_the_title_then_the_directory() {
        let item = Cuttable {
            base: json!({ "session_id": "ses_1" }),
            fields: vec![
                (None, "title", "t".repeat(600)),
                (None, "directory", "/d".repeat(600)),
            ],
        };
        let (value, cut) = item.fit(1024, usize::MAX);
        assert!(cut);
        assert!(value.to_string().len() <= 1024, "{value}");
        assert!(value["title"].as_str().unwrap().starts_with("[…"));
        assert!(value["directory"].as_str().unwrap().starts_with("/d/d"));
    }
}
