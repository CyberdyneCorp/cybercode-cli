//! Built-in tools, the permission engine and the tool host (`builtin-tools`,
//! `tool-registry`, `permissions-modes`).

mod auto_permissions;
pub mod bash_analysis;
mod budget;
pub mod hook_commands;
mod host;
mod parent_permissions;
pub mod permissions;
mod reconcile;
mod sandboxing;
mod schema;
mod subagents;
mod tools;
mod worktrees;

pub use budget::Budget;
pub use host::{BuiltinHost, ConfigFn, HostOptions};
pub use schema::validate as validate_input;
pub use tools::process::Process as HookCommandProcess;
#[cfg(windows)]
pub use tools::process::Process as AppContainerProcess;

pub use worktrees::{WorktreeListing, WorktreeSession, WorktreeSessionRequest};
