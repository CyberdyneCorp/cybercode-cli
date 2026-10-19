//! Durable loop records (`loops-scheduling` → Loop events and hooks): the `loop.*` events
//! of a Session stream, their fold into the Session state that the loop logic reads, and
//! their projector into `session_loop`, the due-time index the sweep queries without
//! loading every Session.

use std::collections::{BTreeMap, VecDeque};

use cyber_core::budget::Budget;
use cyber_core::loops::{Missed, Overlap};
use rusqlite::{Transaction, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use cyber_store::{EventRegistry, StoredEvent};

pub const CREATED: &str = "loop.created.1";
pub const ITERATION_STARTED: &str = "loop.iteration.started.1";
pub const WAKEUP: &str = "loop.wakeup.1";
pub const ITERATION_ENDED: &str = "loop.iteration.ended.1";
pub const STOPPED: &str = "loop.stopped.1";

/// The projection table a rebuild clears.
pub const TABLES: [&str; 1] = ["session_loop"];
/// The event type prefix every loop record shares.
pub const PREFIX: &str = "loop.";
/// Iterations kept per loop in the Session state, newest last.
pub const HISTORY: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopKind {
    Interval,
    SelfPaced,
}

/// Why a loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopStatus {
    /// `/loop stop` or the API.
    Stopped,
    /// A self-paced iteration ended without `schedule_wakeup`.
    Completed,
    MaxIterations,
    Duration,
    UntilMet,
    BudgetExceeded,
    Failed,
}

/// How an iteration ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Completed,
    Interrupted,
    Failed,
    /// A tick that came while the previous iteration still ran (overlap `skip`).
    SkippedOverlap,
    /// Ticks missed while the server was not running (missed-run policy `skip`).
    SkippedMissed,
}

/// What stops a loop besides `/loop stop`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StopConditions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
}

/// `loop.created.1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Created {
    pub loop_id: String,
    pub kind: LoopKind,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<u64>,
    pub overlap: Overlap,
    pub missed: Missed,
    pub stop: StopConditions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    /// `max_iterations` when `stop` sets none (`loops.max_iterations`).
    pub iteration_cap: u64,
    pub created_ms: i64,
    pub next_due_ms: i64,
}

/// `loop.iteration.started.1`, committed with the iteration's admission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IterationStarted {
    pub loop_id: String,
    pub n: u64,
    pub message_id: String,
    pub started_ms: i64,
    /// Session totals when the iteration was admitted, so its end reports a delta.
    pub cost_before: f64,
    pub tokens_before: u64,
    pub turns_before: u64,
    /// The tail of a failed `--until` check that ran before this iteration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_tail: Option<String>,
    /// The next interval tick; `None` for a self-paced loop, which the wakeup paces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due_ms: Option<i64>,
}

/// `loop.wakeup.1`: a clamped `schedule_wakeup` call of iteration `n`; the last one wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wakeup {
    pub loop_id: String,
    pub n: u64,
    pub delay_seconds: u64,
    pub reason: String,
    pub noop: bool,
}

/// `loop.iteration.ended.1`. A skipped tick has `n: 0` and changes no totals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IterationEnded {
    pub loop_id: String,
    pub n: u64,
    pub outcome: Outcome,
    pub noop: bool,
    pub duration_ms: i64,
    pub cost: f64,
    pub tokens: u64,
    pub turns: u64,
    /// When the next iteration is due; `None` when this iteration ends the loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `loop.stopped.1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stopped {
    pub loop_id: String,
    pub status: StopStatus,
    pub reason: String,
}

/// An iteration that was admitted and has not ended.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Inflight {
    pub n: u64,
    pub message_id: String,
    pub started_ms: i64,
    #[serde(skip)]
    pub cost_before: f64,
    #[serde(skip)]
    pub tokens_before: u64,
    #[serde(skip)]
    pub turns_before: u64,
}

/// One ended iteration, as clients list it.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct IterationRecord {
    pub n: u64,
    /// `None` for a skipped tick.
    pub message_id: Option<String>,
    pub outcome: Outcome,
    pub noop: bool,
    pub cost: f64,
    pub ended_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A loop as the Session state folds it.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct LoopState {
    pub loop_id: String,
    pub kind: LoopKind,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<u64>,
    pub overlap: Overlap,
    pub missed: Missed,
    pub stop: StopConditions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
    pub iteration_cap: u64,
    /// `active`, or the [`StopStatus`] that ended it.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    pub created_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
    /// When the next iteration is due; `None` while a self-paced iteration runs or after
    /// the loop ended.
    pub next_due_ms: Option<i64>,
    pub inflight: Option<Inflight>,
    /// The latest `schedule_wakeup` of the in-flight iteration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wakeup: Option<WakeupState>,
    pub iterations: u64,
    pub skipped: u64,
    /// Consecutive `failed` iterations.
    pub failures: u64,
    /// Consecutive noop iterations.
    pub noop_streak: u64,
    pub cost_usd: f64,
    pub tokens: u64,
    pub turns: u64,
    pub history: VecDeque<IterationRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct WakeupState {
    pub n: u64,
    pub delay_seconds: u64,
    pub reason: String,
    pub noop: bool,
}

impl LoopState {
    pub fn active(&self) -> bool {
        self.status == "active"
    }

    /// The iteration cap that applies: `--max`, else `loops.max_iterations`.
    pub fn max_iterations(&self) -> u64 {
        self.stop.max_iterations.unwrap_or(self.iteration_cap)
    }

    pub(crate) fn from_created(c: Created) -> Self {
        Self {
            loop_id: c.loop_id,
            kind: c.kind,
            prompt: c.prompt,
            interval_seconds: c.interval_seconds,
            overlap: c.overlap,
            missed: c.missed,
            stop: c.stop,
            budget: c.budget,
            iteration_cap: c.iteration_cap,
            status: "active".into(),
            stop_reason: None,
            created_ms: c.created_ms,
            ended_ms: None,
            next_due_ms: Some(c.next_due_ms),
            inflight: None,
            wakeup: None,
            iterations: 0,
            skipped: 0,
            failures: 0,
            noop_streak: 0,
            cost_usd: 0.0,
            tokens: 0,
            turns: 0,
            history: VecDeque::new(),
        }
    }

    fn record(&mut self, record: IterationRecord) {
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(record);
    }
}

pub(crate) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(CREATED, validate_created)
        .and_then(|r| {
            r.register_with(ITERATION_STARTED, |v| {
                decode::<IterationStarted>(v).map(drop)
            })
        })
        .and_then(|r| {
            r.register_with(WAKEUP, |v| {
                let w: Wakeup = decode(v)?;
                let (min, max) = cyber_core::loops::WAKEUP_RANGE;
                if !(min..=max).contains(&w.delay_seconds) {
                    return Err("a wakeup delay is clamped to 60..=3600 seconds".into());
                }
                Ok(())
            })
        })
        .and_then(|r| r.register_with(ITERATION_ENDED, |v| decode::<IterationEnded>(v).map(drop)))
        .and_then(|r| r.register_with(STOPPED, |v| decode::<Stopped>(v).map(drop)))
        .expect("valid loop events");
}

fn validate_created(data: &Value) -> Result<(), String> {
    let c: Created = decode(data)?;
    if !cyber_core::loops::is_loop_id(&c.loop_id) {
        return Err(format!("loop ID {:?} is not a lop_ ID", c.loop_id));
    }
    let interval = c.interval_seconds.is_some();
    if interval != (c.kind == LoopKind::Interval) {
        return Err("an interval loop, and only one, has an interval".into());
    }
    if c.prompt.trim().is_empty() || c.iteration_cap == 0 {
        return Err("a loop has a prompt and an iteration cap".into());
    }
    Ok(())
}

fn decode<T: DeserializeOwned>(data: &Value) -> Result<T, String> {
    serde_json::from_value(data.clone()).map_err(|error| error.to_string())
}

/// Fold a loop event into a Session's loops.
pub(crate) fn fold(loops: &mut BTreeMap<String, LoopState>, e: &StoredEvent) -> Result<(), String> {
    match e.kind.as_str() {
        CREATED => {
            let created: Created = decode(&e.data)?;
            loops.insert(created.loop_id.clone(), LoopState::from_created(created));
        }
        ITERATION_STARTED => {
            let s: IterationStarted = decode(&e.data)?;
            let state = known(loops, &s.loop_id)?;
            state.iterations = state.iterations.max(s.n);
            state.wakeup = None;
            state.next_due_ms = s.next_due_ms;
            state.inflight = Some(Inflight {
                n: s.n,
                message_id: s.message_id,
                started_ms: s.started_ms,
                cost_before: s.cost_before,
                tokens_before: s.tokens_before,
                turns_before: s.turns_before,
            });
        }
        WAKEUP => {
            let w: Wakeup = decode(&e.data)?;
            let state = known(loops, &w.loop_id)?;
            state.wakeup = Some(WakeupState {
                n: w.n,
                delay_seconds: w.delay_seconds,
                reason: w.reason,
                noop: w.noop,
            });
        }
        ITERATION_ENDED => {
            let ended: IterationEnded = decode(&e.data)?;
            fold_ended(known(loops, &ended.loop_id)?, ended, e.time_ms);
        }
        STOPPED => {
            let s: Stopped = decode(&e.data)?;
            let state = known(loops, &s.loop_id)?;
            state.status = status_name(s.status).into();
            state.stop_reason = Some(s.reason);
            state.ended_ms = Some(e.time_ms);
            state.next_due_ms = None;
        }
        _ => {}
    }
    Ok(())
}

fn fold_ended(state: &mut LoopState, ended: IterationEnded, time_ms: i64) {
    state.next_due_ms = ended.next_due_ms;
    let skip = ended.outcome;
    if !matches!(skip, Outcome::SkippedOverlap | Outcome::SkippedMissed) {
        let message_id = state.inflight.take().map(|i| i.message_id);
        state.wakeup = None;
        state.cost_usd += ended.cost;
        state.tokens += ended.tokens;
        state.turns += ended.turns;
        state.failures = if ended.outcome == Outcome::Failed {
            state.failures + 1
        } else {
            0
        };
        state.noop_streak = if ended.noop { state.noop_streak + 1 } else { 0 };
        state.record(IterationRecord {
            n: ended.n,
            message_id,
            outcome: ended.outcome,
            noop: ended.noop,
            cost: ended.cost,
            ended_ms: time_ms,
            reason: ended.reason,
        });
        return;
    }
    state.skipped += 1;
    state.record(IterationRecord {
        n: 0,
        message_id: None,
        outcome: skip,
        noop: false,
        cost: 0.0,
        ended_ms: time_ms,
        reason: None,
    });
}

fn known<'a>(
    loops: &'a mut BTreeMap<String, LoopState>,
    id: &str,
) -> Result<&'a mut LoopState, String> {
    loops
        .get_mut(id)
        .ok_or_else(|| format!("loop {id} was never created in this Session"))
}

pub fn status_name(status: StopStatus) -> &'static str {
    match status {
        StopStatus::Stopped => "stopped",
        StopStatus::Completed => "completed",
        StopStatus::MaxIterations => "max_iterations",
        StopStatus::Duration => "duration",
        StopStatus::UntilMet => "until_met",
        StopStatus::BudgetExceeded => "budget_exceeded",
        StopStatus::Failed => "failed",
    }
}

/// Write `session_loop` in the event transaction.
pub(crate) fn project(tx: &Transaction<'_>, e: &StoredEvent) -> Result<(), String> {
    if !e.kind.starts_with(PREFIX) {
        return Ok(());
    }
    let result = match e.kind.as_str() {
        CREATED => {
            let c: Created = decode(&e.data)?;
            tx.execute(
                "INSERT INTO session_loop (loop_id, session_id, status, next_due_ms, inflight, created_ms)
                 VALUES (?1, ?2, 'active', ?3, 0, ?4)",
                params![c.loop_id, e.aggregate_id, c.next_due_ms, c.created_ms],
            )
        }
        ITERATION_STARTED => {
            let s: IterationStarted = decode(&e.data)?;
            tx.execute(
                "UPDATE session_loop SET inflight = 1, next_due_ms = ?2 WHERE loop_id = ?1",
                params![s.loop_id, s.next_due_ms],
            )
        }
        ITERATION_ENDED => {
            let ended: IterationEnded = decode(&e.data)?;
            let inflight = i64::from(matches!(
                ended.outcome,
                Outcome::SkippedOverlap | Outcome::SkippedMissed
            ));
            tx.execute(
                "UPDATE session_loop SET next_due_ms = ?2,
                   inflight = CASE WHEN ?3 = 1 THEN inflight ELSE 0 END
                 WHERE loop_id = ?1",
                params![ended.loop_id, ended.next_due_ms, inflight],
            )
        }
        STOPPED => {
            let s: Stopped = decode(&e.data)?;
            tx.execute(
                "UPDATE session_loop SET status = ?2, next_due_ms = NULL, inflight = 0, ended_ms = ?3
                 WHERE loop_id = ?1",
                params![s.loop_id, status_name(s.status), e.time_ms],
            )
        }
        _ => return Ok(()),
    };
    result.map(drop).map_err(|error| error.to_string())
}

/// A due loop the sweep visits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueLoop {
    pub loop_id: String,
    pub session_id: String,
}

/// Active loops due at `now_ms`, oldest due first.
pub(crate) fn due(db: &rusqlite::Connection, now_ms: i64) -> rusqlite::Result<Vec<DueLoop>> {
    let mut stmt = db.prepare(
        "SELECT loop_id, session_id FROM session_loop
         WHERE status = 'active' AND next_due_ms IS NOT NULL AND next_due_ms <= ?1
         ORDER BY next_due_ms, loop_id",
    )?;
    stmt.query_map(params![now_ms], |r| {
        Ok(DueLoop {
            loop_id: r.get(0)?,
            session_id: r.get(1)?,
        })
    })?
    .collect()
}

/// Sessions with an active loop: the ones restore visits.
pub(crate) fn active_sessions(db: &rusqlite::Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = db.prepare(
        "SELECT DISTINCT session_id FROM session_loop WHERE status = 'active' ORDER BY session_id",
    )?;
    stmt.query_map([], |r| r.get(0))?.collect()
}

/// Whether `session_id` has an active loop (a cheap check before taking its state).
pub(crate) fn has_active(db: &rusqlite::Connection, session_id: &str) -> rusqlite::Result<bool> {
    db.prepare("SELECT 1 FROM session_loop WHERE session_id = ?1 AND status = 'active'")?
        .exists([session_id])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn state(kind: LoopKind) -> LoopState {
        LoopState::from_created(Created {
            loop_id: "lop_01TEST".into(),
            kind,
            prompt: "check".into(),
            interval_seconds: (kind == LoopKind::Interval).then_some(60),
            overlap: Overlap::Skip,
            missed: Missed::Skip,
            stop: StopConditions::default(),
            budget: None,
            iteration_cap: 500,
            created_ms: 0,
            next_due_ms: 0,
        })
    }

    fn stored(kind: &str, data: Value, seq: i64) -> StoredEvent {
        StoredEvent {
            id: format!("evt_{seq}"),
            aggregate_id: "ses_1".into(),
            seq,
            kind: kind.into(),
            data,
            time_ms: 10_000 + seq,
            causation_id: None,
        }
    }

    #[test]
    fn fold_tracks_iterations_wakeups_skips_and_stop() {
        let mut loops = BTreeMap::new();
        let created = state(LoopKind::SelfPaced);
        let data = serde_json::json!({"loop_id": created.loop_id, "kind": "self_paced", "prompt": "check",
            "overlap": "skip", "missed": "skip", "stop": {}, "iteration_cap": 500, "created_ms": 0, "next_due_ms": 0});
        fold(&mut loops, &stored(CREATED, data, 1)).unwrap();
        fold(&mut loops, &stored(ITERATION_STARTED, serde_json::json!({"loop_id": "lop_01TEST", "n": 1,
            "message_id": "loopi_01TEST_1", "started_ms": 5, "cost_before": 0.5, "tokens_before": 10, "turns_before": 1}), 2)).unwrap();
        let lp = &loops["lop_01TEST"];
        assert_eq!(lp.iterations, 1);
        assert_eq!(lp.next_due_ms, None);
        assert!(lp.inflight.is_some());
        fold(
            &mut loops,
            &stored(
                WAKEUP,
                serde_json::json!({"loop_id": "lop_01TEST", "n": 1,
            "delay_seconds": 600, "reason": "CI", "noop": true}),
                3,
            ),
        )
        .unwrap();
        assert_eq!(
            loops["lop_01TEST"].wakeup.as_ref().unwrap().delay_seconds,
            600
        );
        fold(&mut loops, &stored(ITERATION_ENDED, serde_json::json!({"loop_id": "lop_01TEST", "n": 1,
            "outcome": "completed", "noop": true, "duration_ms": 9, "cost": 0.25, "tokens": 40, "turns": 2,
            "next_due_ms": 700}), 4)).unwrap();
        let lp = &loops["lop_01TEST"];
        assert!(lp.inflight.is_none() && lp.wakeup.is_none());
        assert_eq!(
            (lp.noop_streak, lp.cost_usd, lp.tokens, lp.turns),
            (1, 0.25, 40, 2)
        );
        assert_eq!(lp.next_due_ms, Some(700));
        fold(&mut loops, &stored(ITERATION_ENDED, serde_json::json!({"loop_id": "lop_01TEST", "n": 0,
            "outcome": "skipped_missed", "noop": false, "duration_ms": 0, "cost": 0.0, "tokens": 0, "turns": 0,
            "next_due_ms": 900}), 5)).unwrap();
        assert_eq!(
            (loops["lop_01TEST"].skipped, loops["lop_01TEST"].noop_streak),
            (1, 1)
        );
        fold(
            &mut loops,
            &stored(
                STOPPED,
                serde_json::json!({"loop_id": "lop_01TEST", "status": "stopped",
            "reason": "user"}),
                6,
            ),
        )
        .unwrap();
        let lp = &loops["lop_01TEST"];
        assert_eq!(lp.status, "stopped");
        assert_eq!(lp.next_due_ms, None);
        assert_eq!(lp.history.len(), 2);
    }

    #[test]
    fn an_unknown_loop_is_refused() {
        let mut loops = BTreeMap::new();
        let error = fold(
            &mut loops,
            &stored(
                STOPPED,
                serde_json::json!({"loop_id": "lop_X",
            "status": "stopped", "reason": "x"}),
                1,
            ),
        )
        .unwrap_err();
        assert!(error.contains("never created"));
    }
}
