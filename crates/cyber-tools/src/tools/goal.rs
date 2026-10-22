//! `goal` (`goals` → Set the active goal): the model sets or queues a Goal for its own
//! Session. It checks `goal.set`, which asks by default; the user's own routes (`/goal`,
//! the HTTP API) never ask.

use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{Tool, ToolError, def, failed, text};
use crate::host::Ctx;
use crate::permissions::{GOAL_SET, Request};
use cyber_server::runtime::RetrySafety;
use cyber_server::runtime::goals::GoalRequest;

pub(crate) const NAME: &str = "goal";

pub(crate) struct GoalTool;

impl Tool for GoalTool {
    fn def(&self) -> cyber_server::runtime::ToolDef {
        def(
            NAME,
            "Set a goal for this Session: a completion condition you keep working toward across turns. After each turn an evaluator judges it; you continue until it is met, impossible or out of budget. mode set (default) replaces the active goal; add queues it. check is a shell command that must also pass.",
            json!({"type": "object", "required": ["condition"], "additionalProperties": false, "properties": {
                "condition": {"type": "string", "maxLength": cyber_core::goals::MAX_CONDITION},
                "check": {"type": "string"},
                "mode": {"type": "string", "enum": ["set", "add"]}}}),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input = &ctx.inv.input;
            let request = GoalRequest {
                condition: text(input, "condition").to_owned(),
                check: input
                    .get("check")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                ..GoalRequest::default()
            };
            let mode = text(input, "mode");
            if !matches!(mode, "" | "set" | "add") {
                return Err(failed(format!("mode must be set or add, not {mode:?}")));
            }
            let runtime = ctx
                .host
                .runtime()
                .ok_or_else(|| failed("goal requires the runtime"))?;
            ctx.authorize(
                Request {
                    action: GOAL_SET.into(),
                    resources: vec![request.condition.clone()],
                    ..Request::default()
                },
                vec!["*".into()],
                json!({"condition": request.condition, "check": request.check,
                    "mode": if mode == "add" { "add" } else { "set" }}),
            )
            .await?;
            let session = &ctx.inv.session_id;
            let goal = if mode == "add" {
                runtime.add_goal(session, request).await
            } else {
                runtime.set_goal(session, request).await
            }
            .map_err(|error| failed(error.to_string()))?;
            Ok(json!({"goal_id": goal.goal_id, "status": goal.status, "condition": goal.condition})
                .to_string())
        })
    }
}
