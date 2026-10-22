//! The permission engine (`permissions-modes`).

mod dry_run;
mod engine;
mod filesystem;
mod instructions;
mod removals;
pub mod saved;

pub(crate) use filesystem::literal_edits;

pub use dry_run::{Explained, layer_name};
pub use instructions::instruction_read_gate;
pub use removals::{RemovalRisk, RemovalScope, bash_removal, powershell_analysis};

pub use engine::{
    Decision, Effect, GOAL_SET, MESSAGE_SEND, Mode, Policy, Request, Rule, TEAM_MANAGE, TEAM_TASK,
    WORKFLOW_RUN, boundary_evaluate, defaults, evaluate, evaluate_all, evaluate_scoped,
    is_protected, parse_rules, slash, workflow_resource,
};
