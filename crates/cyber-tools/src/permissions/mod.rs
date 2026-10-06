//! The permission engine (`permissions-modes`).

mod engine;
mod removals;
pub mod saved;

pub use removals::{RemovalRisk, RemovalScope, bash_removal};

pub use engine::{
    Decision, Effect, Mode, Policy, Request, Rule, defaults, evaluate, evaluate_all, is_protected,
    parse_rules, slash,
};
