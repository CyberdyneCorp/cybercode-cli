//! The permission engine (`permissions-modes`).

mod engine;
mod filesystem;
mod instructions;
mod removals;
pub mod saved;

pub(crate) use filesystem::literal_edits;

pub use instructions::instruction_read_gate;
pub use removals::{RemovalRisk, RemovalScope, bash_removal, powershell_analysis};

pub use engine::{
    Decision, Effect, Mode, Policy, Request, Rule, WORKFLOW_RUN, defaults, evaluate, evaluate_all,
    evaluate_scoped, is_protected, parse_rules, slash, workflow_resource,
};
