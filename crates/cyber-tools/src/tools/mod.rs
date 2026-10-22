//! Built-in tool implementations (`builtin-tools`).

pub(crate) mod agent;
mod background;
pub(crate) mod bash;
mod deliver;
mod dev_server;
mod formatters;
pub(crate) mod fs;
pub(crate) mod goal;
pub(crate) mod intelligence;
pub(crate) mod loops;
mod lsp;
pub(crate) mod mcp;
pub(crate) mod memory;
pub(crate) mod messaging;
mod monitor;
mod notebook;
pub(crate) mod patch;
pub(crate) mod powershell;
mod pr_links;
pub(crate) mod process;
#[cfg(unix)]
mod pty;
pub(crate) mod search;
mod session;
#[cfg(any(windows, test))]
pub(crate) mod shell_resolve;
mod skill;
mod task;
pub(crate) mod team;
#[cfg(unix)]
mod terminal;
mod tool_search;
mod web;
pub(crate) mod websearch;
mod workflow;
mod worktree;

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
    let tools: Vec<Box<dyn Tool>> = vec![
        Box::new(fs::Read),
        Box::new(lsp::Lsp),
        Box::new(fs::Write),
        Box::new(fs::Edit),
        Box::new(patch::ApplyPatch),
        Box::new(notebook::NotebookEdit),
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
        Box::new(agent::Agent),
        Box::new(workflow::Workflow),
        Box::new(messaging::ListSessions),
        Box::new(messaging::SendMessage),
        Box::new(messaging::WatchSession),
        Box::new(loops::ScheduleWakeup),
        Box::new(goal::GoalTool),
        Box::new(team::TeamSpawn),
        Box::new(team::TeamMerge),
        Box::new(team::TeamTaskCreate),
        Box::new(team::TeamTaskList),
        Box::new(team::TeamTaskGet),
        Box::new(team::TeamTaskUpdate),
        Box::new(task::TaskStop),
        Box::new(monitor::Monitor),
        Box::new(deliver::Notify),
        Box::new(deliver::SendFile),
        Box::new(mcp::WaitForMcp),
        Box::new(mcp::ResourceTool::List),
        Box::new(mcp::ResourceTool::Templates),
        Box::new(mcp::ResourceTool::Read),
        Box::new(memory::Memory),
        Box::new(tool_search::ToolSearch),
        Box::new(worktree::EnterWorktree),
        Box::new(worktree::ExitWorktree),
    ];
    #[cfg(windows)]
    let tools = {
        let mut tools = tools;
        tools.push(Box::new(powershell::PowerShell));
        tools
    };
    // PTYs need a pseudo-terminal; Windows ConPTY is not wired to the sandboxed launch.
    #[cfg(unix)]
    let tools = {
        let mut tools = tools;
        tools.push(Box::new(pty::PtyStart));
        tools.push(Box::new(pty::PtyWrite));
        tools.push(Box::new(pty::PtyRead));
        tools
    };
    tools
}

/// The permission action a tool checks.
pub(crate) fn action_of(name: &str) -> &str {
    match name {
        "write" | "edit" | "apply_patch" | "notebook_edit" => "edit",
        "powershell" | "monitor" | "pty_start" | "pty_write" | "pty_read" => "bash",
        "send_file" => "read",
        "enter_worktree" | "exit_worktree" => "worktree",
        "workflow" => crate::permissions::WORKFLOW_RUN,
        "send_message" => crate::permissions::MESSAGE_SEND,
        goal::NAME => crate::permissions::GOAL_SET,
        team::SPAWN | team::MERGE => crate::permissions::TEAM_MANAGE,
        team::TASK_CREATE | team::TASK_LIST | team::TASK_GET | team::TASK_UPDATE => {
            crate::permissions::TEAM_TASK
        }
        "mcp_list_resources" | "mcp_list_resource_templates" | "mcp_read_resource" => "read",
        other => other,
    }
}

/// Whether a worktree tool fits the Session's checkout: `enter_worktree` outside a
/// managed checkout, `exit_worktree` only after entering one, neither when the
/// Session was started isolated.
pub(crate) fn worktree_offered(name: &str, worktree: cyber_server::runtime::TurnWorktree) -> bool {
    use cyber_server::runtime::TurnWorktree;
    match name {
        "enter_worktree" => worktree == TurnWorktree::Unmanaged,
        "exit_worktree" => worktree == TurnWorktree::Entered,
        _ => true,
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
        scope: cyber_server::runtime::ToolScope::Builtin,
        deferred: false,
        registration: None,
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
