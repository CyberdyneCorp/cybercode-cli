//! The progress event's rate bound and change rule, over a fixed clock and a fake source.

use std::cell::RefCell;

use serde_json::json;

use super::*;
use crate::runtime::WorkflowRunStatus;
use crate::runtime::workflow_summary::AgentCounts;

/// Runs' current progress, as the list would return it, and what was published.
#[derive(Default)]
struct Fake {
    runs: RefCell<HashMap<String, WorkflowRunProgress>>,
    published: RefCell<Vec<WorkflowRunProgress>>,
}

impl Fake {
    fn set(&self, run: &str, running: u64, queued: u64) {
        self.runs.borrow_mut().insert(
            run.into(),
            WorkflowRunProgress {
                run_id: run.into(),
                status: WorkflowRunStatus::Running,
                phase: None,
                agents: AgentCounts {
                    total: running + queued,
                    running,
                    queued,
                    ..AgentCounts::default()
                },
                tokens: 0,
                cost_usd: 0.0,
            },
        );
    }

    fn end(&self, run: &str) {
        if let Some(progress) = self.runs.borrow_mut().get_mut(run) {
            progress.status = WorkflowRunStatus::Completed;
            progress.agents.running = 0;
            progress.agents.completed = progress.agents.total;
        }
    }

    /// `(run, running, queued)` of each event published since the last call.
    fn take(&self) -> Vec<(String, u64, u64)> {
        self.published
            .borrow_mut()
            .drain(..)
            .map(|p| (p.run_id, p.agents.running, p.agents.queued))
            .collect()
    }
}

impl ProgressSource for Fake {
    fn unsettled(&self) -> Vec<String> {
        self.runs
            .borrow()
            .values()
            .filter(|p| p.status.is_active())
            .map(|p| p.run_id.clone())
            .collect()
    }

    fn progress(&self, run_id: &str) -> Option<(String, WorkflowRunProgress)> {
        let progress = self.runs.borrow().get(run_id).cloned()?;
        Some(("/repo".into(), progress))
    }

    fn publish(&self, directory: String, progress: WorkflowRunProgress) {
        assert_eq!(directory, "/repo");
        self.published.borrow_mut().push(progress);
    }
}

fn durable(run: &str, kind: &str) -> Result<LiveEvent, RecvError> {
    Ok(LiveEvent::Durable {
        session_id: run.into(),
        seq: 1,
        kind: kind.into(),
        data: json!({}),
    })
}

/// "Queued agents counted": a change is published at once, later changes at most every
/// 250 ms and each within 250 ms, and the latest values win.
#[test]
fn changes_are_published_at_most_every_interval_and_within_it() {
    let (fake, mut driver) = (Fake::default(), Driver::default());
    fake.set("run_a", 16, 84);
    driver.mark("run_a");
    driver.flush(1_000, &fake);
    assert_eq!(fake.take(), [("run_a".into(), 16, 84)]);
    assert_eq!(driver.next_due(), None, "nothing changed since");
    // Three changes within the interval: one event, at its end, with the latest counts.
    for (at, queued) in [(1_010, 83), (1_100, 82), (1_200, 81)] {
        fake.set("run_a", 16, queued);
        driver.mark("run_a");
        driver.flush(at, &fake);
        assert!(fake.take().is_empty(), "rate bound at {at}");
    }
    assert_eq!(driver.next_due(), Some(1_250));
    driver.flush(1_249, &fake);
    assert!(fake.take().is_empty());
    driver.flush(1_250, &fake);
    assert_eq!(fake.take(), [("run_a".into(), 16, 81)]);
}

/// Runs are bounded separately: one run's event does not delay another's.
#[test]
fn each_run_has_its_own_interval() {
    let (fake, mut driver) = (Fake::default(), Driver::default());
    fake.set("run_a", 1, 0);
    fake.set("run_b", 1, 0);
    driver.mark("run_a");
    driver.flush(0, &fake);
    driver.mark("run_b");
    driver.flush(100, &fake);
    assert_eq!(
        fake.take(),
        [("run_a".into(), 1, 0), ("run_b".into(), 1, 0)]
    );
}

/// A durable event or scheduler change that leaves every value as it was publishes
/// nothing, and an unchanged run is not read again until marked.
#[test]
fn no_event_without_a_change() {
    let (fake, mut driver) = (Fake::default(), Driver::default());
    fake.set("run_a", 2, 0);
    assert!(driver.observe(durable("run_a", "workflow.log.1"), &fake));
    driver.flush(0, &fake);
    assert_eq!(fake.take().len(), 1);
    assert!(driver.observe(durable("run_a", "workflow.log.1"), &fake));
    driver.flush(500, &fake);
    assert!(fake.take().is_empty(), "same values");
    // Session events and other live events do not mark a run.
    assert!(driver.observe(durable("ses_x", "session.step.ended.1"), &fake));
    fake.set("run_a", 1, 0);
    driver.flush(1_000, &fake);
    assert!(fake.take().is_empty(), "not marked");
    assert!(driver.observe(durable("run_a", "workflow.agent.settled.1"), &fake));
    driver.flush(1_000, &fake);
    assert_eq!(fake.take(), [("run_a".into(), 1, 0)]);
}

/// After a `Lagged` gap, every run that may have moved is read again, including one that
/// ended during the gap, so the stream agrees with the list.
#[test]
fn a_lagged_gap_resyncs_every_unsettled_and_tracked_run() {
    let (fake, mut driver) = (Fake::default(), Driver::default());
    fake.set("run_a", 4, 6);
    fake.set("run_b", 2, 0);
    driver.mark("run_a");
    driver.flush(0, &fake);
    assert_eq!(fake.take().len(), 1);
    // Missed: A's agents moved and A ended; C started.
    fake.set("run_a", 4, 2);
    fake.set("run_c", 1, 0);
    fake.end("run_a");
    let (sender, mut receiver) = broadcast::channel(1);
    for _ in 0..3 {
        sender
            .send(durable("run_a", "workflow.log.1").unwrap())
            .unwrap();
    }
    let lagged = receiver.try_recv().map_err(|error| match error {
        broadcast::error::TryRecvError::Lagged(n) => RecvError::Lagged(n),
        other => panic!("{other:?}"),
    });
    assert!(matches!(lagged, Err(RecvError::Lagged(_))));
    assert!(driver.observe(lagged, &fake));
    driver.flush(1_000, &fake);
    let mut published = fake.take();
    published.sort();
    assert_eq!(
        published,
        [
            ("run_a".into(), 0, 2),
            ("run_b".into(), 2, 0),
            ("run_c".into(), 1, 0)
        ]
    );
    let listed = fake.runs.borrow();
    assert_eq!(listed["run_a"].status, WorkflowRunStatus::Completed);
}

#[test]
fn a_closed_bus_ends_the_driver_and_a_vanished_run_is_forgotten() {
    let (fake, mut driver) = (Fake::default(), Driver::default());
    driver.mark("run_gone");
    driver.flush(0, &fake);
    assert!(fake.take().is_empty());
    assert_eq!(driver.next_due(), None);
    assert!(!driver.observe(Err(RecvError::Closed), &fake));
}
