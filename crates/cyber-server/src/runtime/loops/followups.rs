//! What follows a committed loop record (`loops-scheduling` → Completion notifications,
//! Loop events and hooks): a notification under the loop's `notify` policy, and the
//! observational `LoopIteration` hook. Both run after the Session lock is released;
//! neither changes what was recorded.

use cyber_core::loops::Notify;
use cyber_store::NewEvent;
use serde_json::json;

use super::records::{self, IterationEnded, LoopKind, LoopState, Outcome, StopStatus, Stopped};
use crate::runtime::{Notification, Runtime};

/// The longest prompt excerpt a notification shows.
const PROMPT_EXCERPT: usize = 60;
/// The longest reason a notification shows.
const REASON_EXCERPT: usize = 160;

/// One committed record that may notify or fire a hook.
enum Record {
    Ended(IterationEnded),
    Stopped(Stopped),
}

fn records_of(events: &[NewEvent]) -> Vec<Record> {
    events
        .iter()
        .filter_map(|e| match e.kind.as_str() {
            records::ITERATION_ENDED => serde_json::from_value(e.data.clone())
                .ok()
                .map(Record::Ended),
            records::STOPPED => serde_json::from_value(e.data.clone())
                .ok()
                .map(Record::Stopped),
            _ => None,
        })
        .collect()
}

/// Whether an ended iteration notifies under `notify`.
fn iteration_notifies(notify: Notify, ended: &IterationEnded) -> bool {
    match ended.outcome {
        Outcome::Completed => notify.notifies(false, ended.noop),
        Outcome::Failed => notify.notifies(true, false),
        Outcome::Interrupted => notify == Notify::Always,
        Outcome::SkippedOverlap | Outcome::SkippedMissed => false,
    }
}

/// Whether a stop notifies: a failure or an exhausted budget, unless `never`.
fn stop_notifies(notify: Notify, stopped: &Stopped) -> bool {
    notify != Notify::Never
        && matches!(
            stopped.status,
            StopStatus::Failed | StopStatus::BudgetExceeded
        )
}

fn clip(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let cut: String = line.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

fn title(lp: &LoopState) -> String {
    let id = lp.loop_id.rsplit('_').next().unwrap_or_default();
    let short = id
        .get(id.len().saturating_sub(4)..)
        .unwrap_or(id)
        .to_lowercase();
    match lp.kind {
        LoopKind::Cron => format!("Scheduled task {short}"),
        LoopKind::Interval | LoopKind::SelfPaced => format!("Loop {short}"),
    }
}

fn iteration_result(ended: &IterationEnded) -> String {
    let reason = ended.reason.as_deref().map(|r| clip(r, REASON_EXCERPT));
    match (ended.outcome, reason) {
        (Outcome::Failed, Some(reason)) => format!("iteration {} failed: {reason}", ended.n),
        (Outcome::Failed, None) => format!("iteration {} failed", ended.n),
        (Outcome::Interrupted, _) => format!("iteration {} was interrupted", ended.n),
        (_, Some(reason)) => format!("iteration {}: {reason}", ended.n),
        (_, None) => format!("iteration {} finished", ended.n),
    }
}

impl Runtime {
    /// Notify and run hooks for loop records `session_id` just committed.
    pub(crate) async fn loop_followups(&self, session_id: &str, events: &[NewEvent]) {
        let records = records_of(events);
        if records.is_empty() || self.is_shutting_down() {
            return;
        }
        let Ok(handle) = self.inner.handle(session_id).await else {
            return;
        };
        let loops = handle.state.lock().await.loops.clone();
        // A loop that stopped with a notice says so once, in place of its iteration's.
        let stopping: Vec<&str> = records
            .iter()
            .filter_map(|r| match r {
                Record::Stopped(s) => loops
                    .get(&s.loop_id)
                    .filter(|lp| stop_notifies(lp.notify, s))
                    .map(|_| s.loop_id.as_str()),
                Record::Ended(_) => None,
            })
            .collect();
        for record in &records {
            match record {
                Record::Ended(ended) => {
                    let Some(lp) = loops.get(&ended.loop_id) else {
                        continue;
                    };
                    if iteration_notifies(lp.notify, ended)
                        && !stopping.contains(&ended.loop_id.as_str())
                    {
                        self.loop_notify(session_id, lp, &iteration_result(ended))
                            .await;
                    }
                    self.loop_iteration_hook(session_id, lp, ended).await;
                }
                Record::Stopped(stopped) => {
                    if let Some(lp) = loops.get(&stopped.loop_id)
                        && stop_notifies(lp.notify, stopped)
                    {
                        let status = records::status_name(stopped.status);
                        let result = format!(
                            "stopped ({status}): {}",
                            clip(&stopped.reason, REASON_EXCERPT)
                        );
                        self.loop_notify(session_id, lp, &result).await;
                    }
                }
            }
        }
    }

    async fn loop_notify(&self, session_id: &str, lp: &LoopState, result: &str) {
        let notification = Notification {
            call_id: lp.loop_id.clone(),
            title: title(lp),
            body: format!("{}: {result}", clip(&lp.prompt, PROMPT_EXCERPT)),
        };
        // A notice past the per-Session rate limit is dropped; the loop events keep it.
        let _ = self.record_notification(session_id, notification).await;
    }

    /// `LoopIteration` (observational) for an iteration that ran; skipped ticks have none.
    async fn loop_iteration_hook(&self, session_id: &str, lp: &LoopState, ended: &IterationEnded) {
        if ended.n == 0 {
            return;
        }
        let Ok(handle) = self.inner.handle(session_id).await else {
            return;
        };
        let fields = json!({
            "loop": lp,
            "iteration": {
                "n": ended.n,
                "outcome": ended.outcome,
                "noop": ended.noop,
                "duration_ms": ended.duration_ms,
                "cost": ended.cost,
                "tokens": ended.tokens,
                "turns": ended.turns,
                "reason": ended.reason,
            },
        });
        let hook = self
            .inner
            .lifecycle_hook(&handle, "LoopIteration", lp.loop_id.clone(), fields)
            .await;
        self.inner.observe_lifecycle(hook).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ended(outcome: Outcome, noop: bool) -> IterationEnded {
        IterationEnded {
            loop_id: "lop_01TEST".into(),
            n: 1,
            outcome,
            noop,
            duration_ms: 1,
            cost: 0.0,
            tokens: 0,
            turns: 0,
            next_due_ms: None,
            reason: Some("CI is green".into()),
        }
    }

    #[test]
    fn notify_policy_by_outcome() {
        use Outcome::*;
        assert!(iteration_notifies(
            Notify::OnChange,
            &ended(Completed, false)
        ));
        assert!(!iteration_notifies(
            Notify::OnChange,
            &ended(Completed, true)
        ));
        assert!(iteration_notifies(Notify::OnChange, &ended(Failed, false)));
        assert!(!iteration_notifies(
            Notify::OnChange,
            &ended(Interrupted, false)
        ));
        assert!(iteration_notifies(Notify::Always, &ended(Completed, true)));
        assert!(!iteration_notifies(
            Notify::Always,
            &ended(SkippedOverlap, false)
        ));
        assert!(!iteration_notifies(Notify::Never, &ended(Failed, false)));
        let stop = |status| Stopped {
            loop_id: "lop_01TEST".into(),
            status,
            reason: "max_cost_usd 2.01/2.00".into(),
        };
        assert!(stop_notifies(
            Notify::OnChange,
            &stop(StopStatus::BudgetExceeded)
        ));
        assert!(stop_notifies(Notify::OnChange, &stop(StopStatus::Failed)));
        assert!(!stop_notifies(Notify::OnChange, &stop(StopStatus::Stopped)));
        assert!(!stop_notifies(
            Notify::Never,
            &stop(StopStatus::BudgetExceeded)
        ));
    }

    #[test]
    fn results_are_one_line() {
        assert_eq!(
            iteration_result(&ended(Outcome::Completed, false)),
            "iteration 1: CI is green"
        );
        assert_eq!(
            iteration_result(&ended(Outcome::Failed, false)),
            "iteration 1 failed: CI is green"
        );
        assert_eq!(clip("a\nb   c", 10), "a b c");
        assert_eq!(clip("abcdefghijk", 5), "abcd…");
    }
}
