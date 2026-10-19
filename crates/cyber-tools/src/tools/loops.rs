//! `schedule_wakeup` (`loops-scheduling` → Self-paced loop): the model paces a `/loop` by
//! naming the delay before its next iteration and whether this one found anything new.
//! Offered only while an iteration of an active loop runs.

use futures::future::BoxFuture;
use serde_json::json;

use super::{Tool, ToolError, def, failed, number, text};
use crate::host::Ctx;
use cyber_server::runtime::RetrySafety;
use cyber_server::runtime::loops::LoopKind;

pub(crate) const NAME: &str = "schedule_wakeup";

/// Guidance shown to the model while an iteration runs (Context Source `core/loop`).
pub(crate) const GUIDANCE_KEY: &str = "core/loop";
pub(crate) const GUIDANCE: &str = "This prompt is an iteration of a /loop the user started. \
Before you finish, call schedule_wakeup once: set noop to true when nothing changed since \
the last check, and give a one-sentence reason. In a self-paced loop, delay_seconds \
(60–3600) is the wait before the next iteration, and finishing without calling \
schedule_wakeup ends the loop. In an interval loop the interval keeps the pace and \
delay_seconds is ignored.";

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
            let ack = runtime
                .loop_wakeup(&ctx.inv.session_id, delay, text(input, "reason"), noop)
                .await
                .map_err(|error| failed(error.to_string()))?;
            Ok(match ack.kind {
                LoopKind::SelfPaced => format!("Next iteration in {}s", ack.delay_seconds),
                LoopKind::Interval => {
                    "Recorded; the loop's interval sets the next iteration".into()
                }
            })
        })
    }
}
