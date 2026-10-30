//! The permission engine (`permissions-modes`).

mod dry_run;
mod engine;
mod filesystem;
mod instructions;
mod removals;
pub mod saved;
mod taint;

pub(crate) use filesystem::literal_edits;

pub use dry_run::{Explained, layer_name};
pub use instructions::instruction_read_gate;
pub use removals::{RemovalRisk, RemovalScope, bash_removal, powershell_analysis};
pub use taint::{Risk, TAINT_BYPASS_DENIED, TAINT_NOT_PRE_APPROVED, Taint, risk};
pub(crate) use taint::{path_argument, read_only_shell, secret_path, shell_words_or_split};

pub use engine::{
    COLLAB_MESSAGE_SEND, COLLAB_TASK_WRITE, Decision, DecisionSource, Effect, GOAL_SET,
    MESSAGE_SEND, Mode, Policy, Request, Rule, TEAM_MANAGE, TEAM_TASK, WORKFLOW_RUN,
    boundary_evaluate, defaults, evaluate, evaluate_all, evaluate_scoped, is_protected,
    parse_rules, slash, workflow_resource,
};
