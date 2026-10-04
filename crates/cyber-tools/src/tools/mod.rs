//! Built-in tool implementations (`builtin-tools`).

pub(crate) mod bash;
mod fs;
pub(crate) mod patch;
mod search;
mod session;
mod skill;
mod web;
pub(crate) mod websearch;

use cyber_llm::ToolSpec;
use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde_json::Value;

use crate::host::Ctx;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolError {
    /// An expected failure; the message is shown to the model.
    Failed(String),
    /// Stopped by the abort signal.
    Aborted,
}

pub(crate) trait Tool: Send + Sync {
    fn def(&self) -> ToolDef;
    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>>;
}

pub(crate) fn all() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(fs::Read),
        Box::new(fs::Write),
        Box::new(fs::Edit),
        Box::new(patch::ApplyPatch),
        Box::new(fs::List),
        Box::new(search::Glob),
        Box::new(search::Grep),
        Box::new(bash::Bash),
        Box::new(web::WebFetch),
        Box::new(websearch::WebSearch),
        Box::new(skill::SkillTool),
        Box::new(session::Todo),
        Box::new(session::QuestionTool),
        Box::new(session::HistorySearch),
        Box::new(session::PlanEnter),
        Box::new(session::PlanExit),
    ]
}

/// The permission action a tool checks.
pub(crate) fn action_of(name: &str) -> &str {
    match name {
        "write" | "edit" | "apply_patch" => "edit",
        other => other,
    }
}

pub(crate) fn def(
    name: &str,
    description: &str,
    schema: Value,
    safety: RetrySafety,
    parallel: bool,
) -> ToolDef {
    ToolDef {
        spec: ToolSpec {
            name: name.into(),
            description: description.into(),
            input_schema: schema,
        },
        retry_safety: safety,
        concurrency_safe: parallel,
    }
}

pub(crate) fn text<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or_default()
}

pub(crate) fn number(input: &Value, key: &str) -> Option<u64> {
    input.get(key).and_then(Value::as_u64)
}

pub(crate) fn failed(message: impl Into<String>) -> ToolError {
    ToolError::Failed(message.into())
}
