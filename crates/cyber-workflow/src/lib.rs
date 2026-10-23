//! Cyber Code Workflows (`workflows`): the module format, the bundled Workflows, saved-Workflow
//! discovery and the sandboxed QuickJS engine that runs scripts against a [`WorkflowHost`].
//!
//! This crate has no server or store dependency. Durable runs, agent spawning, budgets and
//! the HTTP surface live in the server runtime, which implements [`WorkflowHost`].

pub mod bundled;
pub mod diagnostic;
pub mod discovery;
pub mod engine;
pub mod meta;
pub mod module;
mod scan;
pub mod solve;

pub use diagnostic::{Diagnostic, DiagnosticKind, Position};
pub use discovery::{Entry, Issue, Listing, Root, discover, discover_library, read_module, roots};
pub use engine::{
    AgentCall, HostFuture, LOG_TRUNCATED, Limits, MAX_LOG_BYTES, MAX_PHASE_TITLE_CHARS, MAX_PHASES,
    MODULE_NAME, RunInput, RunOutput, WorkflowHost, run, too_many_phases,
};
pub use meta::{Meta, Phase};
pub use module::{MAX_SOURCE_BYTES, Origin, Scope, WorkflowModule};
