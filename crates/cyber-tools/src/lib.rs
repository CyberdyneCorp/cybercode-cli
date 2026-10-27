//! Built-in tools, the permission engine and the tool host (`builtin-tools`,
//! `tool-registry`, `permissions-modes`).

mod auto_permissions;
pub mod bash_analysis;
mod browser;
mod budget;
mod hook_async;
mod hook_authority;
mod hook_client;
pub mod hook_commands;
pub mod hook_http;
pub mod hook_mcp;
pub mod hook_prompt;
mod hook_reports;
mod host;
pub use host::autofix::allowed_tool as autofix_allowed_tool;
mod lifecycle_hooks;
pub mod lsp;
pub mod mcp;
mod memory_context;
mod net_client;
mod parent_permissions;
pub mod permissions;
pub mod plugin_host;
mod reconcile;
mod registered;
pub mod remote_skills;
pub mod sandbox_cli;
mod sandboxing;
mod schema;
mod skill_permissions;
mod subagents;
mod tool_hooks;
pub use tool_hooks::HookConfigFn;
mod tools;
mod worktrees;

pub use browser::health::{BrowserHealth, HealthCheck as BrowserHealthCheck, browser_health};
pub use browser::profiles::{Cleanup as BrowserCleanup, cleanup as cleanup_browser_profiles};
pub use budget::Budget;
pub use host::{
    BuiltinHost, ConfigFn, HostOptions, IsolationOptions, PermissionTest, PermissionTestReport,
    PrepareError,
};
pub use remote_skills::{FetchRequest, PolicyFetcher, RemoteSkillFetcher};
pub use schema::validate as validate_input;
pub use tools::process::Process as HookCommandProcess;
#[cfg(windows)]
pub use tools::process::Process as AppContainerProcess;

pub use worktrees::{SessionCleanup, WorktreeListing, WorktreeSession, WorktreeSessionRequest};
