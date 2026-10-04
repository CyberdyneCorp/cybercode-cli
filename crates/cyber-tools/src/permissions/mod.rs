//! The permission engine (`permissions-modes`).

mod engine;
pub mod saved;

pub use engine::{
    Decision, Effect, Mode, Policy, Request, Rule, defaults, evaluate, evaluate_all, is_protected,
    parse_rules, slash,
};
