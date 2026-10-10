//! Built-in tools, the permission engine and the tool host (`builtin-tools`,
//! `tool-registry`, `permissions-modes`).

mod auto_permissions;
pub mod bash_analysis;
mod budget;
mod hook_authority;
pub mod hook_commands;
pub mod hook_http;
pub mod hook_mcp;
pub mod hook_prompt;
mod hook_reports;
mod host;
pub mod lsp;
pub mod mcp;
mod memory_context;
mod parent_permissions;
pub mod permissions;
mod reconcile;
mod registered;
mod sandboxing;
mod schema;
mod subagents;
mod tool_hooks;
pub use tool_hooks::HookConfigFn;
mod tools;
mod worktrees;

pub use budget::Budget;
pub use host::{BuiltinHost, ConfigFn, HostOptions};
pub use schema::validate as validate_input;
pub use tools::process::Process as HookCommandProcess;
#[cfg(windows)]
pub use tools::process::Process as AppContainerProcess;

pub use worktrees::{WorktreeListing, WorktreeSession, WorktreeSessionRequest};
