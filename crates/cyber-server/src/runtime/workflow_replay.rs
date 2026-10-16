//! Event-sourced replay of Workflow host calls (`workflows` → Durable agent results and
//! resume, Replay of workflow host calls).
//!
//! Every host call gets an issue sequence number and a call key
//! (`sha256(kind, prompt or name, canonical options)` plus its ordinal). A relaunch of the
//! same revision re-executes the script and delivers the recorded results in the order the
//! script first received them, so the deterministic script reaches the same state; a call
//! issued in that prefix whose key differs from the record, or a recorded result whose call
//! is never issued, fails the run with `WorkflowReplayMismatch`. The prefix ends at the first
//! recorded failure, so a resumed run retries it. After the prefix (and for a new revision)
//! calls are matched by key alone: completed results and checkpoint values are reused, an
//! unsettled agent resumes its existing child Session, and anything else runs anew.
//!
//! Every call is issued (sequenced) when the engine dispatches it, so all calls of one
//! dispatch batch are issued before any of their futures is polled; a call the host
//! rejects is sequenced and recorded like the others. `ctx.budget` is replayed too: the
//! script sees the snapshot recorded when the revision started and with each delivered
//! result, so a script that branches on its spend repeats the recorded attempt.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::future::poll_fn;
use std::task::{Poll, Waker};

use cyber_store::Store;
use cyber_workflow::{Diagnostic, DiagnosticKind};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::RuntimeError;

/// What a settled host call delivered to the script.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Value(Value),
    Error(Diagnostic),
}

impl Outcome {
    pub(super) fn of(result: &Result<Value, Diagnostic>) -> Self {
        match result {
            Ok(value) => Self::Value(value.clone()),
            Err(error) => Self::Error(error.clone()),
        }
    }

    pub(super) fn into_result(self) -> Result<Value, Diagnostic> {
        match self {
            Self::Value(value) => Ok(value),
            Self::Error(error) => Err(error),
        }
    }
}

/// One recorded call of a revision.
#[derive(Debug, Clone)]
struct Recorded {
    kind: String,
    call_key: String,
    order: Option<i64>,
    outcome: Option<Outcome>,
}

/// How an issued call is served.
pub(super) enum Plan {
    /// Deliver this recorded result when the prefix reaches `index`.
    Replay {
        index: usize,
        outcome: Outcome,
    },
    /// Served after the prefix: by key, by resuming, or anew.
    Free {
        seq: u64,
    },
    Mismatch(Diagnostic),
}

/// The replay state of one execution attempt. It lives on the engine thread.
pub(super) struct Replay {
    pub(super) revision: u32,
    calls: BTreeMap<u64, Recorded>,
    /// Issue sequence numbers of the prefix, in delivery order.
    prefix: Vec<u64>,
    cursor: Cell<usize>,
    issued: Cell<u64>,
    next_order: Cell<i64>,
    /// Reusable results of any revision by call key: completed agents and checkpoints.
    reusable: HashMap<String, Outcome>,
    /// Agents started but never settled, by call key: their `agt_` ID.
    unsettled: HashMap<String, String>,
    /// Logs and phases this revision already recorded, and how many this attempt emitted.
    recorded_notes: u64,
    notes: Cell<u64>,
    /// A result the record did not have reached the script: from here on the script may
    /// differ from the earlier attempt.
    diverged: Cell<bool>,
    wakers: RefCell<Vec<Waker>>,
    /// `ctx.budget` when the revision started.
    initial_budget: Option<Value>,
    /// `ctx.budget` after each recorded result, by issue sequence number.
    budgets: HashMap<u64, Value>,
}

impl Replay {
    /// A first execution: nothing recorded.
    pub(super) fn fresh(revision: u32) -> Self {
        Self::from_parts(revision, BTreeMap::new(), HashMap::new(), HashMap::new())
    }

    /// Load the records of `run_id` for a relaunch of `revision`.
    pub(super) fn load(store: &Store, run_id: &str, revision: u32) -> Result<Self, RuntimeError> {
        let run = run_id.to_owned();
        let rows = store.read(move |db| {
            let mut statement = db.prepare(
                "SELECT revision, seq, kind, call_key, agent_id, status, settled_order, outcome
                 FROM workflow_call WHERE run_id = ?1 ORDER BY revision, seq",
            )?;
            let rows = statement
                .query_map(params![run], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        let (run, kinds) = (
            run_id.to_owned(),
            [super::workflows::LOG, super::workflows::PHASE],
        );
        let recorded_notes = store.read(move |db| {
            Ok(db.query_row(
                "SELECT COUNT(*) FROM event WHERE aggregate_id = ?1 AND type IN (?2, ?3)
                 AND COALESCE(json_extract(data, '$.revision'), 1) = ?4",
                params![run, kinds[0], kinds[1], i64::from(revision)],
                |row| row.get::<_, i64>(0),
            )?)
        })?;
        let mut calls = BTreeMap::new();
        let mut reusable = HashMap::new();
        let mut unsettled = HashMap::new();
        for (row_revision, seq, kind, call_key, agent_id, status, order, outcome) in rows {
            let outcome = outcome
                .map(|text| serde_json::from_str::<Outcome>(&text))
                .transpose()
                .map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
            index_call(
                &mut reusable,
                &mut unsettled,
                &call_key,
                &outcome,
                agent_id,
                &status,
            );
            if row_revision == i64::from(revision) {
                let seq = u64::try_from(seq).map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
                calls.insert(
                    seq,
                    Recorded {
                        kind,
                        call_key,
                        order: order.filter(|_| outcome.is_some()),
                        outcome,
                    },
                );
            }
        }
        let mut replay = Self::from_parts(revision, calls, reusable, unsettled);
        replay.recorded_notes = u64::try_from(recorded_notes).unwrap_or_default();
        (replay.initial_budget, replay.budgets) = load_budgets(store, run_id, revision)?;
        Ok(replay)
    }

    /// The `ctx.budget` a new revision starts with.
    pub(super) fn starting_with(mut self, budget: Value) -> Self {
        self.initial_budget = Some(budget);
        self
    }

    /// `ctx.budget` when the revision started, if recorded.
    pub(super) fn initial_budget(&self) -> Option<Value> {
        self.initial_budget.clone()
    }

    /// `ctx.budget` recorded with the prefix result at `index`.
    pub(super) fn delivered_budget(&self, index: usize) -> Option<Value> {
        let seq = self.prefix.get(index)?;
        self.budgets.get(seq).cloned()
    }

    fn from_parts(
        revision: u32,
        calls: BTreeMap<u64, Recorded>,
        reusable: HashMap<String, Outcome>,
        unsettled: HashMap<String, String>,
    ) -> Self {
        let mut ordered: Vec<(i64, u64)> = calls
            .iter()
            .filter_map(|(seq, call)| call.order.map(|order| (order, *seq)))
            .collect();
        ordered.sort_unstable();
        // The prefix ends at the first recorded failure, which a relaunch retries.
        let prefix: Vec<u64> = ordered
            .into_iter()
            .map(|(_, seq)| seq)
            .take_while(|seq| !matches!(calls[seq].outcome, Some(Outcome::Error(_))))
            .collect();
        Self {
            revision,
            next_order: Cell::new(prefix.len() as i64),
            calls,
            prefix,
            cursor: Cell::new(0),
            issued: Cell::new(0),
            reusable,
            unsettled,
            recorded_notes: 0,
            notes: Cell::new(0),
            diverged: Cell::new(false),
            wakers: RefCell::new(Vec::new()),
            initial_budget: None,
            budgets: HashMap::new(),
        }
    }

    /// Recorded results after this order are discarded by the relaunch (`resumed.1`).
    pub(super) fn first_open_order(&self) -> i64 {
        self.prefix.len() as i64
    }

    /// Whether a log or phase must be recorded. Until a result the record did not have
    /// reaches the script, the script repeats the earlier attempt, whose notes are stored.
    pub(super) fn record_note(&self) -> bool {
        let index = self.notes.get();
        self.notes.set(index + 1);
        self.diverged.get() || index >= self.recorded_notes
    }

    /// Whether recorded results are still being delivered.
    pub(super) fn replaying(&self) -> bool {
        self.cursor.get() < self.prefix.len()
    }

    /// Assign the next issue sequence number and decide how the call is served.
    pub(super) fn issue(&self, kind: &str, call_key: &str) -> Plan {
        let seq = self.issued.get();
        self.issued.set(seq + 1);
        if !self.replaying() {
            return Plan::Free { seq };
        }
        let Some(call) = self.calls.get(&seq) else {
            return Plan::Free { seq };
        };
        if call.kind != kind || call.call_key != call_key {
            return Plan::Mismatch(mismatch(format!(
                "host call {seq} is {kind} {call_key} but the run recorded {} {}; start a new revision to change the script",
                call.kind, call.call_key
            )));
        }
        match (
            self.prefix.iter().position(|recorded| *recorded == seq),
            &call.outcome,
        ) {
            (Some(index), Some(outcome)) => Plan::Replay {
                index,
                outcome: outcome.clone(),
            },
            _ => Plan::Free { seq },
        }
    }

    /// A completed result or checkpoint value recorded for `call_key` in any revision.
    pub(super) fn reusable(&self, call_key: &str) -> Option<Outcome> {
        self.reusable.get(call_key).cloned()
    }

    /// The `agt_` ID of an agent started for `call_key` that never settled.
    pub(super) fn unsettled(&self, call_key: &str) -> Option<String> {
        self.unsettled.get(call_key).cloned()
    }

    /// The delivery order of a result reaching the script now.
    pub(super) fn take_order(&self) -> i64 {
        self.diverged.set(true);
        let order = self.next_order.get();
        self.next_order.set(order + 1);
        order
    }

    /// Wait until the prefix reaches `index`, then mark that result delivered.
    pub(super) async fn turn(&self, index: usize) -> Result<(), Diagnostic> {
        poll_fn(|cx| {
            if self.cursor.get() == index {
                self.cursor.set(index + 1);
                for waker in self.wakers.borrow_mut().drain(..) {
                    waker.wake();
                }
                return Poll::Ready(Ok(()));
            }
            self.pending(cx.waker())
        })
        .await
    }

    /// Wait until every prefix result has been delivered.
    pub(super) async fn after_prefix(&self) -> Result<(), Diagnostic> {
        poll_fn(|cx| {
            if !self.replaying() {
                return Poll::Ready(Ok(()));
            }
            self.pending(cx.waker())
        })
        .await
    }

    /// Host calls are issued when dispatched, so host futures are only polled while the
    /// script is idle with every queued call issued: a prefix result whose call was never
    /// issued can never be delivered, because the script diverged from the record.
    fn pending(&self, waker: &Waker) -> Poll<Result<(), Diagnostic>> {
        let head = self.prefix[self.cursor.get()];
        if head >= self.issued.get() {
            let call = &self.calls[&head];
            return Poll::Ready(Err(mismatch(format!(
                "the script no longer issues recorded host call {head} ({} {}); start a new revision to change the script",
                call.kind, call.call_key
            ))));
        }
        self.wakers.borrow_mut().push(waker.clone());
        Poll::Pending
    }
}

/// Index one recorded row (rows arrive by revision, then issue sequence): a completed
/// result becomes reusable by key, and any settlement, completed or failed, ends the
/// key's unsettled agent. Only an agent still started and unsettled resumes its child; a
/// failed one is retried with a new agent.
fn index_call(
    reusable: &mut HashMap<String, Outcome>,
    unsettled: &mut HashMap<String, String>,
    call_key: &str,
    outcome: &Option<Outcome>,
    agent_id: Option<String>,
    status: &str,
) {
    match (outcome, agent_id) {
        (Some(outcome), _) => {
            unsettled.remove(call_key);
            if let Outcome::Value(_) = outcome {
                reusable.insert(call_key.to_owned(), outcome.clone());
            }
        }
        (None, Some(agent)) if status == "started" => {
            unsettled.insert(call_key.to_owned(), agent);
        }
        _ => {}
    }
}

/// The recorded `ctx.budget` snapshots of a revision: at its start (`run.started` or
/// `run.revised`) and with each settled call, the last record of a call winning.
type Budgets = (Option<Value>, HashMap<u64, Value>);

fn load_budgets(store: &Store, run_id: &str, revision: u32) -> Result<Budgets, RuntimeError> {
    use super::workflows::{AGENT_SETTLED, CALL_RECORDED, RUN_REVISED, RUN_STARTED};
    let run = run_id.to_owned();
    let rows = store.read(move |db| {
        let mut statement = db.prepare(
            "SELECT json_extract(data, '$.seq'), json_extract(data, '$.budget_snapshot') FROM event
             WHERE aggregate_id = ?1 AND type IN (?2, ?3, ?4, ?5)
             AND COALESCE(json_extract(data, '$.revision'), 1) = ?6
             AND json_extract(data, '$.budget_snapshot') IS NOT NULL ORDER BY seq",
        )?;
        let rows = statement
            .query_map(
                params![
                    run,
                    RUN_STARTED,
                    RUN_REVISED,
                    AGENT_SETTLED,
                    CALL_RECORDED,
                    i64::from(revision)
                ],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })?;
    let mut budgets = (None, HashMap::new());
    for (seq, text) in rows {
        let value: Value =
            serde_json::from_str(&text).map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
        match seq.and_then(|seq| u64::try_from(seq).ok()) {
            Some(seq) => {
                budgets.1.insert(seq, value);
            }
            None => budgets.0 = Some(value),
        }
    }
    Ok(budgets)
}

fn mismatch(message: String) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::WorkflowReplayMismatch, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    use serde_json::json;

    fn recorded(entries: &[(u64, &str, &str, Option<i64>, bool)]) -> Replay {
        let calls = entries
            .iter()
            .map(|(seq, kind, key, order, failed)| {
                let outcome = order.map(|_| match failed {
                    true => Outcome::Error(Diagnostic::new(DiagnosticKind::AgentFailed, "no")),
                    false => Outcome::Value(json!(key)),
                });
                let call = Recorded {
                    kind: (*kind).to_owned(),
                    call_key: (*key).to_owned(),
                    order: *order,
                    outcome,
                };
                (*seq, call)
            })
            .collect();
        Replay::from_parts(2, calls, HashMap::new(), HashMap::new())
    }

    fn three_recorded() -> Replay {
        recorded(&[
            (0, "agent", "a:1", Some(1), false),
            (1, "agent", "b:1", Some(0), false),
            (2, "agent", "c:1", Some(2), true),
            (3, "checkpoint", "d:1", Some(3), false),
            (4, "agent", "e:1", None, false),
        ])
    }

    #[test]
    fn the_prefix_follows_delivery_order_and_stops_at_the_first_failure() {
        let replay = three_recorded();
        assert_eq!(replay.prefix, [1, 0]);
        assert_eq!(replay.first_open_order(), 2);
        assert!(matches!(
            replay.issue("agent", "a:1"),
            Plan::Replay { index: 1, .. }
        ));
        match replay.issue("agent", "b:1") {
            Plan::Replay { index: 0, outcome } => {
                assert_eq!(outcome, Outcome::Value(json!("b:1")));
            }
            _ => panic!("recorded result not replayed"),
        }
        // The failed call is retried after the prefix.
        assert!(matches!(
            replay.issue("agent", "c:1"),
            Plan::Free { seq: 2 }
        ));
    }

    #[test]
    fn recorded_results_are_delivered_in_recorded_order() {
        let replay = three_recorded();
        for key in ["a:1", "b:1", "c:1"] {
            replay.issue("agent", key);
        }
        assert!(replay.turn(1).now_or_never().is_none());
        assert_eq!(replay.turn(0).now_or_never(), Some(Ok(())));
        assert!(replay.after_prefix().now_or_never().is_none());
        assert_eq!(replay.turn(1).now_or_never(), Some(Ok(())));
        assert_eq!(replay.after_prefix().now_or_never(), Some(Ok(())));
        assert!(!replay.replaying());
        assert_eq!(replay.take_order(), 2);
    }

    #[test]
    fn notes_of_the_repeated_part_are_not_recorded_again() {
        let mut replay = recorded(&[(0, "agent", "a:1", Some(0), false)]);
        replay.recorded_notes = 2;
        assert!(!replay.record_note());
        assert!(!replay.record_note());
        assert!(replay.record_note());
        let mut replay = recorded(&[]);
        replay.recorded_notes = 2;
        assert!(!replay.record_note());
        replay.take_order();
        assert!(
            replay.record_note(),
            "after a new result the script may differ"
        );
    }

    #[test]
    fn a_changed_or_reordered_call_is_a_replay_mismatch() {
        let replay = recorded(&[
            (0, "agent", "a:1", Some(0), false),
            (1, "agent", "b:1", Some(1), false),
        ]);
        match replay.issue("agent", "b:1") {
            Plan::Mismatch(error) => {
                assert_eq!(error.kind, DiagnosticKind::WorkflowReplayMismatch);
                assert!(error.message.contains("host call 0"), "{}", error.message);
            }
            _ => panic!("reordered call accepted"),
        }
        let replay = recorded(&[(0, "checkpoint", "a:1", Some(0), false)]);
        assert!(matches!(replay.issue("agent", "a:1"), Plan::Mismatch(_)));
    }

    #[test]
    fn a_batch_issued_together_replays_in_recorded_order_whatever_is_polled_first() {
        // `b` settled before `a` in the first attempt; both were dispatched together.
        let replay = recorded(&[
            (0, "agent", "a:1", Some(1), false),
            (1, "agent", "b:1", Some(0), false),
        ]);
        assert!(matches!(
            replay.issue("agent", "a:1"),
            Plan::Replay { index: 1, .. }
        ));
        assert!(matches!(
            replay.issue("agent", "b:1"),
            Plan::Replay { index: 0, .. }
        ));
        // `a` is polled first and waits for `b`'s turn instead of failing.
        let mut a = Box::pin(replay.turn(1));
        assert!(a.as_mut().now_or_never().is_none());
        assert_eq!(replay.turn(0).now_or_never(), Some(Ok(())));
        assert_eq!(a.now_or_never(), Some(Ok(())));
    }

    #[test]
    fn a_recorded_result_whose_call_is_never_issued_is_a_mismatch() {
        let replay = recorded(&[
            (0, "agent", "a:1", Some(1), false),
            (1, "agent", "b:1", Some(0), false),
        ]);
        assert!(matches!(
            replay.issue("agent", "a:1"),
            Plan::Replay { index: 1, .. }
        ));
        // The script waits on `a` while the record says `b` (never issued) came first.
        let error = replay.turn(1).now_or_never().unwrap().unwrap_err();
        assert_eq!(error.kind, DiagnosticKind::WorkflowReplayMismatch);
        assert!(
            error.message.contains("no longer issues"),
            "{}",
            error.message
        );
    }

    #[test]
    fn outcomes_round_trip_as_tagged_values() {
        let ok = Outcome::of(&Ok(json!({"text": "x"})));
        let text = serde_json::to_string(&ok).unwrap();
        assert_eq!(text, r#"{"value":{"text":"x"}}"#);
        let error = Outcome::of(&Err(Diagnostic::new(DiagnosticKind::AgentFailed, "no")));
        let back: Outcome = serde_json::from_str(&serde_json::to_string(&error).unwrap()).unwrap();
        assert_eq!(
            back.into_result().unwrap_err().kind,
            DiagnosticKind::AgentFailed
        );
    }
}
