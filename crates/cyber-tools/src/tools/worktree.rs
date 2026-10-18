//! `enter_worktree` and `exit_worktree` (`worktrees` → Enter and exit tools).

use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde_json::json;

use super::{Tool, ToolError, def};
use crate::host::Ctx;

pub(crate) struct EnterWorktree;
pub(crate) struct ExitWorktree;

impl Tool for EnterWorktree {
    fn def(&self) -> ToolDef {
        def(
            "enter_worktree",
            "Move this Session into a managed Git worktree: an existing one by name or path, or a new one on branch cyber/<name> (a name is generated when omitted). Later tool calls resolve paths there; exit_worktree returns.",
            json!({"type": "object", "properties": {
                "name": {"type": "string", "description": "Worktree name, ^[a-z0-9][a-z0-9._-]{0,62}$"},
                "path": {"type": "string", "description": "Root of an existing managed worktree"}}}),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(ctx.host.enter_worktree_tool(ctx))
    }
}

impl Tool for ExitWorktree {
    fn def(&self) -> ToolDef {
        def(
            "exit_worktree",
            "Return this Session to the Location it entered the worktree from. Unless keep is true, a worktree this Session created and left without changes is then removed with its branch; one with changes, or an existing worktree it entered, is always kept.",
            json!({"type": "object", "properties": {
                "keep": {"type": "boolean", "description": "Keep the worktree even when it has no changes"}}}),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(ctx.host.exit_worktree_tool(ctx))
    }
}
