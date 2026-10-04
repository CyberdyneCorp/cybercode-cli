//! Session tools: todo, question, history_search, plan_enter, plan_exit.

use cyber_server::runtime::{Question, QuestionReply, RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Tool, ToolError, def, failed, number, text};
use crate::host::Ctx;
use crate::permissions::Request;

pub(crate) struct Todo;
pub(crate) struct QuestionTool;
pub(crate) struct HistorySearch;
pub(crate) struct PlanEnter;
pub(crate) struct PlanExit;

async fn allow(ctx: &Ctx<'_>, action: &str, resource: &str) -> Result<(), ToolError> {
    let req = Request {
        action: action.into(),
        resources: vec![resource.into()],
        read_only: true,
        ..Request::default()
    };
    ctx.authorize(req, vec![resource.into()], Value::Null).await
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Item {
    id: String,
    subject: String,
    #[serde(default)]
    description: String,
    status: String,
    #[serde(default)]
    blocked_by: Vec<String>,
}

impl Tool for Todo {
    fn def(&self) -> ToolDef {
        def(
            "todo",
            "Manage the session task list: op create {subject, description?, blocked_by?}, update {id, status?: pending|in_progress|completed|deleted, subject?, description?}, list, get {id}.",
            json!({"type": "object", "required": ["op"], "properties": {
                "op": {"type": "string", "enum": ["create", "update", "list", "get"]},
                "id": {"type": "string"}, "subject": {"type": "string"}, "description": {"type": "string"},
                "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "deleted"]},
                "blocked_by": {"type": "array", "items": {"type": "string"}}}}),
            RetrySafety::Idempotent,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            allow(ctx, "todo", text(&ctx.inv.input, "op")).await?;
            let mut items = load(ctx)?;
            let input = &ctx.inv.input;
            match text(input, "op") {
                "create" => create(&mut items, input)?,
                "update" => update(&mut items, input)?,
                "get" => {
                    let item = items
                        .iter()
                        .find(|i| i.id == text(input, "id"))
                        .ok_or_else(|| failed("No such task"))?;
                    return Ok(serde_json::to_string_pretty(item).unwrap_or_default());
                }
                _ => return Ok(render(&items)),
            }
            save(ctx, &items)?;
            Ok(render(&items))
        })
    }
}

fn create(items: &mut Vec<Item>, input: &Value) -> Result<(), ToolError> {
    let subject = text(input, "subject");
    if subject.is_empty() {
        return Err(failed("subject is required"));
    }
    let id = format!("t{}", items.len() + 1);
    let blocked_by = input
        .get("blocked_by")
        .and_then(|b| serde_json::from_value(b.clone()).ok())
        .unwrap_or_default();
    items.push(Item {
        id,
        subject: subject.into(),
        description: text(input, "description").into(),
        status: "pending".into(),
        blocked_by,
    });
    Ok(())
}

fn update(items: &mut [Item], input: &Value) -> Result<(), ToolError> {
    let id = text(input, "id");
    let status = text(input, "status");
    if status == "in_progress"
        && items
            .iter()
            .any(|i| i.status == "in_progress" && i.id != id)
    {
        return Err(failed(
            "Only one task may be in_progress at a time; complete or pause the current one first",
        ));
    }
    let item = items
        .iter_mut()
        .find(|i| i.id == id)
        .ok_or_else(|| failed(format!("No such task {id}")))?;
    if !status.is_empty() {
        item.status = status.into();
    }
    for (key, slot) in [
        ("subject", &mut item.subject),
        ("description", &mut item.description),
    ] {
        if let Some(value) = input.get(key).and_then(Value::as_str) {
            *slot = value.into();
        }
    }
    Ok(())
}

fn render(items: &[Item]) -> String {
    let open = |id: &str| {
        items
            .iter()
            .any(|i| i.id == id && i.status != "completed" && i.status != "deleted")
    };
    let rows: Vec<String> = items
        .iter()
        .filter(|i| i.status != "deleted")
        .map(|i| {
            let blocked = i.blocked_by.iter().any(|b| open(b));
            let mark = match (i.status.as_str(), blocked) {
                ("completed", _) => "[x]",
                ("in_progress", _) => "[>]",
                (_, true) => "[blocked]",
                _ => "[ ]",
            };
            format!("{mark} {} {}", i.id, i.subject)
        })
        .collect();
    if rows.is_empty() {
        "No tasks".into()
    } else {
        rows.join("\n")
    }
}

fn load(ctx: &Ctx<'_>) -> Result<Vec<Item>, ToolError> {
    let session = ctx.inv.session_id.clone();
    let raw: Option<String> = ctx
        .host
        .opts
        .store
        .read(move |conn| {
            use rusqlite::OptionalExtension;
            Ok(conn
                .query_row(
                    "SELECT items FROM session_todo WHERE session_id = ?1",
                    [session],
                    |r| r.get(0),
                )
                .optional()?)
        })
        .map_err(|e| failed(e.to_string()))?;
    Ok(raw
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_default())
}

fn save(ctx: &Ctx<'_>, items: &[Item]) -> Result<(), ToolError> {
    let (session, json) = (
        ctx.inv.session_id.clone(),
        serde_json::to_string(items).unwrap_or_default(),
    );
    ctx.host
        .opts
        .store
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO session_todo (session_id, items) VALUES (?1, ?2) ON CONFLICT(session_id) DO UPDATE SET items = excluded.items",
                rusqlite::params![session, json],
            )?;
            Ok(())
        })
        .map_err(|e| failed(e.to_string()))
}

impl Tool for QuestionTool {
    fn def(&self) -> ToolDef {
        def(
            "question",
            "Ask the user 1-4 multiple-choice questions and wait for the answers.",
            json!({"type": "object", "required": ["questions"], "properties": {"questions": {"type": "array", "items": {
                "type": "object", "required": ["question", "header", "options"], "properties": {
                    "question": {"type": "string"}, "header": {"type": "string"},
                    "options": {"type": "array", "items": {"type": "object", "required": ["label"], "properties": {
                        "label": {"type": "string"}, "description": {"type": "string"}}}},
                    "multi_select": {"type": "boolean"}, "allow_custom": {"type": "boolean"}}}}}}),
            RetrySafety::ReadOnly,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            allow(ctx, "question", "*").await?;
            let questions: Vec<Question> =
                serde_json::from_value(ctx.inv.input["questions"].clone())
                    .map_err(|e| failed(format!("Invalid questions: {e}")))?;
            if !(1..=4).contains(&questions.len())
                || questions
                    .iter()
                    .any(|q| !(2..=4).contains(&q.options.len()) || q.header.chars().count() > 12)
            {
                return Err(failed(
                    "Ask 1-4 questions, each with a header of at most 12 characters and 2-4 options",
                ));
            }
            ask(ctx, questions)
                .await
                .map(|answers| serde_json::to_string(&answers).unwrap_or_default())
        })
    }
}

async fn ask(ctx: &Ctx<'_>, questions: Vec<Question>) -> Result<Vec<Vec<String>>, ToolError> {
    let reply = tokio::select! {
        _ = ctx.cancel.cancelled() => return Err(ToolError::Aborted),
        r = ctx.inv.asker.question(questions) => r,
    };
    match reply {
        QuestionReply::Answers { answers } => Ok(answers),
        QuestionReply::Dismissed => Err(failed("The user dismissed the question")),
        QuestionReply::Unattended => Err(failed(
            "No interactive user is attached to answer questions",
        )),
    }
}

impl Tool for HistorySearch {
    fn def(&self) -> ToolDef {
        def(
            "history_search",
            "Search earlier messages of this session (including compacted ones) by text, or fetch one by message_id.",
            json!({"type": "object", "properties": {"query": {"type": "string"}, "message_id": {"type": "string"},
                "before_seq": {"type": "integer"}, "limit": {"type": "integer"}}}),
            RetrySafety::ReadOnly,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            allow(ctx, "history_search", "*").await?;
            let entries = history(ctx)?;
            let input = &ctx.inv.input;
            if let Some(id) = input.get("message_id").and_then(Value::as_str) {
                let hit = entries
                    .iter()
                    .find(|e| e.id == id)
                    .ok_or_else(|| failed(format!("No message {id} in this session")))?;
                return Ok(format!(
                    "[seq {}] [{}] {}\n{}",
                    hit.seq, hit.role, hit.id, hit.text
                ));
            }
            let words: Vec<String> = text(input, "query")
                .to_lowercase()
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let before = input
                .get("before_seq")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MAX);
            let limit = number(input, "limit").unwrap_or(10).clamp(1, 50) as usize;
            let hits: Vec<String> = entries
                .iter()
                .rev()
                .filter(|e| {
                    e.seq < before && words.iter().all(|w| e.text.to_lowercase().contains(w))
                })
                .take(limit)
                .map(|e| {
                    format!(
                        "[seq {}] [{}] {}: {}",
                        e.seq,
                        e.role,
                        e.id,
                        excerpt(&e.text, words.first().map(String::as_str))
                    )
                })
                .collect();
            Ok(if hits.is_empty() {
                "No matching messages".into()
            } else {
                hits.join("\n")
            })
        })
    }
}

struct HistoryEntry {
    seq: i64,
    role: &'static str,
    id: String,
    text: String,
}

/// Durable history of this Session only; other Sessions are never searched.
fn history(ctx: &Ctx<'_>) -> Result<Vec<HistoryEntry>, ToolError> {
    let store = &ctx.host.opts.store;
    let mut out = Vec::new();
    let mut after = -1;
    loop {
        let page = store
            .read_events(&ctx.inv.session_id, after, 500)
            .map_err(|e| failed(e.to_string()))?;
        for e in &page.events {
            out.extend(entry(e));
        }
        after = page.events.last().map_or(after, |e| e.seq);
        if !page.has_more {
            return Ok(out);
        }
    }
}

fn entry(e: &cyber_store::StoredEvent) -> Option<HistoryEntry> {
    let d = &e.data;
    let (role, id, text) = match e.kind.as_str() {
        "session.prompt.admitted.1" => ("user", d["message_id"].as_str()?, parts_text(&d["parts"])),
        "session.text.ended.1" => (
            "assistant",
            d["message_id"].as_str()?,
            d["text"].as_str()?.to_string(),
        ),
        "session.tool.settled.1" => (
            "tool",
            d["call_id"].as_str()?,
            d["output"].as_str()?.to_string(),
        ),
        _ => return None,
    };
    Some(HistoryEntry {
        seq: e.seq,
        role,
        id: id.into(),
        text,
    })
}

fn parts_text(parts: &Value) -> String {
    parts
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn excerpt(text: &str, word: Option<&str>) -> String {
    let lower = text.to_lowercase();
    let at = word.and_then(|w| lower.find(w)).unwrap_or(0);
    let start = text
        .char_indices()
        .map(|(i, _)| i)
        .rfind(|&i| i <= at.saturating_sub(120))
        .unwrap_or(0);
    let snippet: String = text[start..].chars().take(300).collect();
    snippet.replace('\n', " ")
}

impl Tool for PlanEnter {
    fn def(&self) -> ToolDef {
        def(
            "plan_enter",
            "Switch this session to plan mode: read-only exploration, then a written plan.",
            json!({"type": "object", "properties": {}}),
            RetrySafety::Idempotent,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            allow(ctx, "plan_enter", "*").await?;
            require_user(ctx)?;
            let runtime = ctx
                .host
                .runtime()
                .ok_or_else(|| failed("plan mode is unavailable in this host"))?;
            runtime
                .switch_mode(&ctx.inv.session_id, "plan")
                .await
                .map_err(|e| failed(e.to_string()))?;
            Ok(format!(
                "Plan mode applies from your next step. Explore read-only, write the plan to {}, then call plan_exit.",
                ctx.policy.plan_file.display()
            ))
        })
    }
}

/// `permissions-modes` → plan mode: both plan tools are denied in non-interactive Sessions.
fn require_user(ctx: &Ctx<'_>) -> Result<(), ToolError> {
    if ctx.inv.asker.attended() {
        Ok(())
    } else {
        Err(failed(
            "Plan mode needs an interactive user; it is unavailable in non-interactive sessions",
        ))
    }
}

const APPROVE: &str = "Approve and build";
const APPROVE_EDITS: &str = "Approve with accept-edits";
const APPROVE_AUTO: &str = "Approve with auto";
const CHANGES: &str = "Request changes";

impl Tool for PlanExit {
    fn def(&self) -> ToolDef {
        def(
            "plan_exit",
            "Present the plan file for approval. On approval the session leaves plan mode.",
            json!({"type": "object", "properties": {"summary": {"type": "string"}}}),
            RetrySafety::Idempotent,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            allow(ctx, "plan_exit", "*").await?;
            require_user(ctx)?;
            let plan = tokio::fs::read_to_string(&ctx.policy.plan_file)
                .await
                .unwrap_or_default();
            let summary = text(&ctx.inv.input, "summary");
            let option = |label: &str| cyber_server::runtime::QuestionOption {
                label: label.into(),
                description: String::new(),
            };
            let question = Question {
                question: format!(
                    "Approve this plan?\n\n{}",
                    if plan.is_empty() { summary } else { &plan }
                ),
                header: "Plan".into(),
                options: vec![
                    option(APPROVE),
                    option(APPROVE_EDITS),
                    option(APPROVE_AUTO),
                    option(CHANGES),
                ],
                multi_select: false,
                allow_custom: true,
            };
            let answer = ask(ctx, vec![question])
                .await?
                .into_iter()
                .next()
                .unwrap_or_default()
                .join(" ");
            let mode = match answer.as_str() {
                APPROVE => "default",
                APPROVE_EDITS => "accept-edits",
                APPROVE_AUTO => "auto",
                _ => {
                    return Ok(format!(
                        "The user requested changes: {}",
                        if answer == CHANGES {
                            "(no details)"
                        } else {
                            &answer
                        }
                    ));
                }
            };
            let runtime = ctx
                .host
                .runtime()
                .ok_or_else(|| failed("plan mode is unavailable in this host"))?;
            runtime
                .switch_mode(&ctx.inv.session_id, mode)
                .await
                .map_err(|e| failed(e.to_string()))?;
            Ok(format!(
                "Plan approved. The session leaves plan mode and continues in {mode} mode from the next step."
            ))
        })
    }
}
