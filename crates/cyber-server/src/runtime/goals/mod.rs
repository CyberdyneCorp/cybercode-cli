//! Session Goals (`goals`): a completion condition the Session keeps working toward. After
//! each Turn that leaves nothing to do, an evaluator judges the active Goal; the Session
//! continues on `not_met`, completes (and moves to the next queued Goal) on `met`, and
//! stops on `impossible`, a blocking error or its Budget.

mod evaluate;
mod fork;
pub mod records;
mod run;
mod service;
mod unattended;

pub use fork::GoalFork;
pub use records::{CheckRun, Evaluated, Goal, Goals};

/// The System Context source key of the active Goal.
pub const CONTEXT_KEY: &str = "core/goal";

/// `core/goal`: the active (or paused) Goal, its last verdict and the queued count;
/// `None` withdraws it.
pub fn context_source(goals: &Goals) -> Option<String> {
    let goal = goals.current()?;
    let mut text = format!(
        "Active goal ({}, {}): {}",
        goal.goal_id,
        goal.status.as_str(),
        goal.condition
    );
    if let Some(check) = &goal.check {
        text.push_str(&format!("\nIt is verified by: `{check}`"));
    }
    if let (Some(verdict), Some(reason)) = (goal.last_verdict, &goal.reason) {
        let verdict = serde_json::to_value(verdict)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        text.push_str(&format!("\nLast verdict: {verdict} — {reason}"));
    }
    text.push_str(&format!("\nQueued goals: {}", goals.queue.len()));
    Some(text)
}
pub use service::{GoalEdit, GoalRequest};
