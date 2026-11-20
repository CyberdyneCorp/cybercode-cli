//! `schedule_wakeup` (`loops-scheduling` → Self-paced loop): the model paces a `/loop` by
//! naming the delay before its next iteration and whether this one found anything new.
//! Offered only while an iteration of an active loop runs.
//!
//! `cron_create`, `cron_list` and `cron_delete` (`loops-scheduling` → In-session cron
//! tools): Session-scoped scheduled tasks, offered only to root Sessions outside Workflow
//! Runs. They never create a persistent schedule.

use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{Tool, ToolError, def, failed, number, text};
use crate::host::Ctx;
use cyber_server::runtime::RetrySafety;
use cyber_server::runtime::loops::{CronRequest, LoopKind, LoopState};

pub(crate) const NAME: &str = "schedule_wakeup";
/// The scheduled task tools.
pub(crate) const CRON_TOOLS: [&str; 3] = ["cron_create", "cron_list", "cron_delete"];

/// Guidance shown to the model while an iteration runs (Context Source `core/loop`).
pub(crate) const GUIDANCE_KEY: &str = "core/loop";
pub(crate) const GUIDANCE: &str = "This prompt is an iteration of a /loop or a scheduled \
run the user set up. Before you finish, call schedule_wakeup once: set noop to true when \
nothing changed since the last check, and give a one-sentence reason. In a self-paced loop, \
delay_seconds (60–3600) is the wait before the next iteration, and finishing without calling \
schedule_wakeup ends the loop. In an interval loop, a scheduled task or a scheduled run the \
schedule keeps the pace and delay_seconds is ignored.";

pub(crate) struct ScheduleWakeup;

impl Tool for ScheduleWakeup {
    fn def(&self) -> cyber_server::runtime::ToolDef {
        def(
            NAME,
            "Schedule the next iteration of the current /loop. delay_seconds is clamped to 60–3600; reason is one sentence shown to the user; noop is true when this iteration found nothing new.",
            json!({"type": "object", "required": ["delay_seconds", "reason", "noop"], "properties": {
                "delay_seconds": {"type": "integer", "minimum": 1},
                "reason": {"type": "string"},
                "noop": {"type": "boolean"}}}),
            RetrySafety::ReadOnly,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input = &ctx.inv.input;
            let delay = number(input, "delay_seconds")
                .ok_or_else(|| failed("delay_seconds must be a positive whole number"))?;
            let noop = input
                .get("noop")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| failed("noop must be true or false"))?;
            let runtime = ctx
                .host
                .runtime()
                .ok_or_else(|| failed("schedule_wakeup requires the runtime"))?;
            // A scheduled run reports its `noop` and reason; its cron sets the pace.
            if runtime
                .report_schedule_run(&ctx.inv.session_id, text(input, "reason"), noop)
                .await
                .map_err(|error| failed(error.to_string()))?
            {
                return Ok("Recorded for this scheduled run".into());
            }
            let ack = runtime
                .loop_wakeup(&ctx.inv.session_id, delay, text(input, "reason"), noop)
                .await
                .map_err(|error| failed(error.to_string()))?;
            Ok(match ack.kind {
                LoopKind::SelfPaced => format!("Next iteration in {}s", ack.delay_seconds),
                LoopKind::Interval => {
                    "Recorded; the loop's interval sets the next iteration".into()
                }
                LoopKind::Cron => "Recorded; the task's schedule sets the next iteration".into(),
            })
        })
    }
}

fn runtime(ctx: &Ctx<'_>) -> Result<cyber_server::runtime::Runtime, ToolError> {
    ctx.host
        .runtime()
        .ok_or_else(|| failed("Scheduled tasks require the runtime"))
}

/// A task as the tools report it.
fn task(state: &LoopState) -> Value {
    let mut out = json!({
        "id": state.loop_id,
        "prompt": state.prompt,
        "iterations": state.iterations,
        "next_run": state.next_due_ms.map(cyber_core::cron::local_time_text),
    });
    if let Some(cron) = &state.cron {
        out["cron"] = json!(cron);
    }
    if let Some(at) = state.at_ms {
        out["at"] = json!(cyber_core::cron::local_time_text(at));
    }
    out
}

pub(crate) struct CronCreate;

impl Tool for CronCreate {
    fn def(&self) -> cyber_server::runtime::ToolDef {
        def(
            CRON_TOOLS[0],
            "Schedule a prompt to run later in this Session. Give either cron (a 5-field expression: minute hour day-of-month month day-of-week, in the user's local time) for a recurring task, or at (a local time such as 2026-10-03T09:00, or an RFC 3339 timestamp) for a one-shot task. The prompt is admitted when no Turn runs. missed (skip or run-once) decides what happens to runs missed while the server was down; notify (always, on_change or never) when the user is notified. Returns the task ID and its next run.",
            json!({"type": "object", "required": ["prompt"], "properties": {
                "cron": {"type": "string"},
                "at": {"type": "string"},
                "prompt": {"type": "string"},
                "missed": {"type": "string", "enum": ["skip", "run-once"]},
                "notify": {"type": "string", "enum": ["always", "on_change", "never"]}}}),
            // A retry would create a second task.
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let request: CronRequest = serde_json::from_value(ctx.inv.input.clone())
                .map_err(|error| failed(format!("Invalid cron_create input: {error}")))?;
            let created = runtime(ctx)?
                .create_cron(&ctx.inv.session_id, request)
                .await
                .map_err(|error| failed(refusal(error)))?;
            Ok(task(&created).to_string())
        })
    }
}

pub(crate) struct CronList;

impl Tool for CronList {
    fn def(&self) -> cyber_server::runtime::ToolDef {
        def(
            CRON_TOOLS[1],
            "List this Session's active scheduled tasks with their schedule, prompt and next run.",
            json!({"type": "object", "properties": {}}),
            RetrySafety::ReadOnly,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let tasks = runtime(ctx)?
                .cron_tasks(&ctx.inv.session_id)
                .await
                .map_err(|error| failed(refusal(error)))?;
            Ok(json!({"tasks": tasks.iter().map(task).collect::<Vec<_>>()}).to_string())
        })
    }
}

pub(crate) struct CronDelete;

impl Tool for CronDelete {
    fn def(&self) -> cyber_server::runtime::ToolDef {
        def(
            CRON_TOOLS[2],
            "Delete one of this Session's scheduled tasks by its cron_ ID.",
            json!({"type": "object", "required": ["id"], "properties": {
                "id": {"type": "string"}}}),
            RetrySafety::Idempotent,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let id = text(&ctx.inv.input, "id").trim();
            runtime(ctx)?
                .delete_cron(&ctx.inv.session_id, id)
                .await
                .map_err(|error| failed(refusal(error)))?;
            Ok(json!({"deleted": id}).to_string())
        })
    }
}

/// A runtime refusal without its error-type prefix.
fn refusal(error: cyber_server::runtime::RuntimeError) -> String {
    let text = error.to_string();
    match text.split_once(": ") {
        Some((tag, rest)) if tag.ends_with("Error") => rest.to_owned(),
        _ => text,
    }
}
