//! Durable goal records (`goals` → Goal events): the `session.goal.*` events of a Session
//! stream, their fold into the Session state that every decision reads, and their
//! projector into `goal`, the index restore and listings use without loading every Session.

use std::collections::{BTreeMap, VecDeque};

use cyber_core::budget::Budget;
use cyber_core::goals::{GoalStatus, Verdict};
use rusqlite::{Transaction, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use cyber_store::{EventRegistry, StoredEvent};

pub const SET: &str = "session.goal.set.1";
pub const QUEUED: &str = "session.goal.queued.1";
pub const ACTIVATED: &str = "session.goal.activated.1";
pub const EVALUATED: &str = "session.goal.evaluated.1";
pub const PAUSED: &str = "session.goal.paused.1";
pub const RESUMED: &str = "session.goal.resumed.1";
pub const COMPLETED: &str = "session.goal.completed.1";
pub const ENDED: &str = "session.goal.ended.1";
pub const EDITED: &str = "session.goal.edited.1";
pub const REORDERED: &str = "session.goal.reordered.1";

/// The projection table a rebuild clears.
pub const TABLES: [&str; 1] = ["goal"];
/// The event type prefix every goal record shares.
pub const PREFIX: &str = "session.goal.";
/// Evaluations kept per Goal in the Session state, newest last.
pub const HISTORY: usize = 50;

/// A new Goal: `session.goal.set.1` (it becomes active) or `session.goal.queued.1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewGoal {
    pub goal_id: String,
    pub condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// The effective Budget (`max_turns` always set).
    pub budget: Budget,
    /// The `goals.templates` entry it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    pub created_ms: i64,
}

/// `session.goal.activated.1`: the Goal starts counting from these Session totals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Activated {
    pub goal_id: String,
    pub cost_before: f64,
    pub tokens_before: u64,
    pub activated_ms: i64,
}

/// The `check` that ran after a `met` verdict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CheckRun {
    pub command: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// The last 50 lines of its output.
    pub tail: String,
}

/// `session.goal.evaluated.1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Evaluated {
    pub goal_id: String,
    pub verdict: Verdict,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckRun>,
    /// The goal Turn this verdict closes (1-based).
    pub turn: u64,
    pub eval_tokens: u64,
    pub eval_cost_usd: f64,
    /// Work tokens and cost since activation, children included, at this verdict.
    pub tokens: u64,
    pub cost_usd: f64,
    pub evaluated_ms: i64,
}

/// `session.goal.paused.1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paused {
    pub goal_id: String,
    pub reason: String,
}

/// `session.goal.resumed.1` and `session.goal.completed.1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalRef {
    pub goal_id: String,
}

/// `session.goal.ended.1`: a terminal status other than reached through `completed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ended {
    pub goal_id: String,
    pub status: GoalStatus,
    pub reason: String,
}

/// `session.goal.edited.1`: the fields that changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edited {
    pub goal_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// `Some(None)` removes the check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<Budget>,
}

/// `session.goal.reordered.1`: the pending Goals in their new order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reordered {
    pub goal_ids: Vec<String>,
}

/// One Goal as the Session state folds it and clients list it.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Goal {
    pub goal_id: String,
    pub condition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    pub budget: Budget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    pub status: GoalStatus,
    /// The last verdict's reason, or why it paused or ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_verdict: Option<Verdict>,
    /// Goal Turns: evaluations since activation.
    pub turns: u64,
    pub tokens: u64,
    pub cost_usd: f64,
    pub eval_count: u64,
    pub eval_tokens: u64,
    pub eval_cost_usd: f64,
    pub created_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activated_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
    #[serde(skip)]
    pub cost_before: f64,
    #[serde(skip)]
    pub tokens_before: u64,
    pub evaluations: VecDeque<Evaluated>,
}

impl Goal {
    fn from_new(new: NewGoal, status: GoalStatus) -> Self {
        Self {
            goal_id: new.goal_id,
            condition: new.condition,
            check: new.check,
            budget: new.budget,
            template: new.template,
            status,
            reason: None,
            last_verdict: None,
            turns: 0,
            tokens: 0,
            cost_usd: 0.0,
            eval_count: 0,
            eval_tokens: 0,
            eval_cost_usd: 0.0,
            created_ms: new.created_ms,
            activated_ms: None,
            ended_ms: None,
            cost_before: 0.0,
            tokens_before: 0,
            evaluations: VecDeque::new(),
        }
    }
}

/// A Session's Goals: every Goal by ID, the pending queue in order, the active (or
/// paused) one, and whether it must be evaluated before continuing (restore).
#[derive(Debug, Clone, Default, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Goals {
    pub goals: BTreeMap<String, Goal>,
    /// Pending Goal IDs in queue order.
    pub queue: Vec<String>,
    /// The active or paused Goal.
    pub current: Option<String>,
    #[serde(skip)]
    pub pending_eval: bool,
}

impl Goals {
    pub fn current(&self) -> Option<&Goal> {
        self.goals.get(self.current.as_deref()?)
    }

    /// The active (not paused) Goal.
    pub fn active(&self) -> Option<&Goal> {
        self.current().filter(|g| g.status == GoalStatus::Active)
    }

    fn set_status(
        &mut self,
        id: &str,
        status: GoalStatus,
        time_ms: i64,
    ) -> Result<&mut Goal, String> {
        let goal = self
            .goals
            .get_mut(id)
            .ok_or_else(|| format!("goal {id} was never set in this Session"))?;
        if goal.status != status && !goal.status.can_become(status) {
            return Err(format!(
                "goal {id} cannot go from {} to {}",
                goal.status.as_str(),
                status.as_str()
            ));
        }
        goal.status = status;
        if status.is_terminal() {
            goal.ended_ms = Some(time_ms);
        }
        Ok(goal)
    }

    fn leave(&mut self, id: &str) {
        self.queue.retain(|q| q != id);
        if self.current.as_deref() == Some(id) {
            self.current = None;
            self.pending_eval = false;
        }
    }
}

pub(crate) fn register(registry: &mut EventRegistry) {
    let new_goal = |v: &Value| {
        let goal: NewGoal = decode(v)?;
        if !cyber_core::goals::is_goal_id(&goal.goal_id) {
            return Err(format!("goal ID {:?} is not a gol_ ID", goal.goal_id));
        }
        cyber_core::goals::check_condition(&goal.condition)
    };
    registry
        .register_with(SET, new_goal)
        .and_then(|r| r.register_with(QUEUED, new_goal))
        .and_then(|r| r.register_with(ACTIVATED, |v| decode::<Activated>(v).map(drop)))
        .and_then(|r| {
            r.register_with(EVALUATED, |v| {
                let e: Evaluated = decode(v)?;
                if e.reason.chars().count() > cyber_core::goals::MAX_REASON {
                    return Err("a verdict reason is at most 300 characters".into());
                }
                Ok(())
            })
        })
        .and_then(|r| r.register_with(PAUSED, |v| decode::<Paused>(v).map(drop)))
        .and_then(|r| r.register_with(RESUMED, |v| decode::<GoalRef>(v).map(drop)))
        .and_then(|r| r.register_with(COMPLETED, |v| decode::<GoalRef>(v).map(drop)))
        .and_then(|r| {
            r.register_with(ENDED, |v| {
                let e: Ended = decode(v)?;
                (e.status.is_terminal() && e.status != GoalStatus::Met)
                    .then_some(())
                    .ok_or_else(|| "goal.ended names a terminal status other than met".into())
            })
        })
        .and_then(|r| r.register_with(EDITED, |v| decode::<Edited>(v).map(drop)))
        .and_then(|r| r.register_with(REORDERED, |v| decode::<Reordered>(v).map(drop)))
        .expect("valid goal events");
}

fn decode<T: DeserializeOwned>(data: &Value) -> Result<T, String> {
    serde_json::from_value(data.clone()).map_err(|error| error.to_string())
}

/// Fold a goal event into a Session's Goals.
pub(crate) fn fold(goals: &mut Goals, e: &StoredEvent) -> Result<(), String> {
    let time = e.time_ms;
    match e.kind.as_str() {
        SET => {
            let new: NewGoal = decode(&e.data)?;
            let id = new.goal_id.clone();
            goals
                .goals
                .insert(id.clone(), Goal::from_new(new, GoalStatus::Pending));
            goals.queue.insert(0, id);
        }
        QUEUED => {
            let new: NewGoal = decode(&e.data)?;
            let id = new.goal_id.clone();
            goals
                .goals
                .insert(id.clone(), Goal::from_new(new, GoalStatus::Pending));
            goals.queue.push(id);
        }
        ACTIVATED => {
            let a: Activated = decode(&e.data)?;
            let goal = goals.set_status(&a.goal_id, GoalStatus::Active, time)?;
            goal.activated_ms = Some(a.activated_ms);
            goal.cost_before = a.cost_before;
            goal.tokens_before = a.tokens_before;
            goals.queue.retain(|q| q != &a.goal_id);
            goals.current = Some(a.goal_id);
            goals.pending_eval = false;
        }
        EVALUATED => {
            let ev: Evaluated = decode(&e.data)?;
            let goal = goals
                .goals
                .get_mut(&ev.goal_id)
                .ok_or_else(|| format!("goal {} was never set", ev.goal_id))?;
            goal.turns = ev.turn;
            goal.tokens = ev.tokens;
            goal.cost_usd = ev.cost_usd;
            goal.eval_count += 1;
            goal.eval_tokens += ev.eval_tokens;
            goal.eval_cost_usd += ev.eval_cost_usd;
            goal.last_verdict = Some(ev.verdict);
            goal.reason = Some(ev.reason.clone());
            if goal.evaluations.len() == HISTORY {
                goal.evaluations.pop_front();
            }
            goal.evaluations.push_back(ev);
            goals.pending_eval = false;
        }
        PAUSED => {
            let p: Paused = decode(&e.data)?;
            goals
                .set_status(&p.goal_id, GoalStatus::Paused, time)?
                .reason = Some(p.reason);
        }
        RESUMED => {
            let r: GoalRef = decode(&e.data)?;
            goals.set_status(&r.goal_id, GoalStatus::Active, time)?;
        }
        COMPLETED => {
            let c: GoalRef = decode(&e.data)?;
            goals.set_status(&c.goal_id, GoalStatus::Met, time)?;
            goals.leave(&c.goal_id);
        }
        ENDED => {
            let end: Ended = decode(&e.data)?;
            goals.set_status(&end.goal_id, end.status, time)?.reason = Some(end.reason);
            goals.leave(&end.goal_id);
        }
        EDITED => {
            let ed: Edited = decode(&e.data)?;
            let goal = goals
                .goals
                .get_mut(&ed.goal_id)
                .ok_or_else(|| format!("goal {} was never set", ed.goal_id))?;
            if let Some(condition) = ed.condition {
                goal.condition = condition;
            }
            if let Some(check) = ed.check {
                goal.check = check;
            }
            if let Some(budget) = ed.budget {
                goal.budget = budget;
            }
        }
        REORDERED => {
            let r: Reordered = decode(&e.data)?;
            goals.queue = r.goal_ids;
        }
        _ => {}
    }
    Ok(())
}

/// Write `goal` in the event transaction.
pub(crate) fn project(tx: &Transaction<'_>, e: &StoredEvent) -> Result<(), String> {
    if !e.kind.starts_with(PREFIX) {
        return Ok(());
    }
    let sql = |error: rusqlite::Error| error.to_string();
    match e.kind.as_str() {
        SET | QUEUED => {
            let new: NewGoal = decode(&e.data)?;
            tx.execute(
                "INSERT INTO goal (goal_id, session_id, status, condition, created_ms)
                 VALUES (?1, ?2, 'pending', ?3, ?4)",
                params![new.goal_id, e.aggregate_id, new.condition, new.created_ms],
            )
            .map_err(sql)?;
        }
        ACTIVATED | RESUMED | PAUSED | COMPLETED | ENDED => {
            let id = e.data["goal_id"].as_str().unwrap_or_default();
            let status = match e.kind.as_str() {
                ACTIVATED | RESUMED => "active",
                PAUSED => "paused",
                COMPLETED => "met",
                _ => e.data["status"].as_str().unwrap_or("cleared"),
            };
            let ended = matches!(e.kind.as_str(), COMPLETED | ENDED).then_some(e.time_ms);
            tx.execute(
                "UPDATE goal SET status = ?2, ended_ms = COALESCE(?3, ended_ms) WHERE goal_id = ?1",
                params![id, status, ended],
            )
            .map_err(sql)?;
        }
        EDITED => {
            let ed: Edited = decode(&e.data)?;
            if let Some(condition) = ed.condition {
                tx.execute(
                    "UPDATE goal SET condition = ?2 WHERE goal_id = ?1",
                    params![ed.goal_id, condition],
                )
                .map_err(sql)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Sessions with an active or paused Goal: the ones restore visits.
pub(crate) fn sessions_with_goals(db: &rusqlite::Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = db.prepare(
        "SELECT DISTINCT session_id FROM goal WHERE status IN ('active', 'pending') ORDER BY session_id",
    )?;
    stmt.query_map([], |r| r.get(0))?.collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn stored(kind: &str, data: Value, seq: i64) -> StoredEvent {
        StoredEvent {
            id: format!("evt_{seq}"),
            aggregate_id: "ses_1".into(),
            seq,
            kind: kind.into(),
            data,
            time_ms: 1_000 + seq,
            causation_id: None,
        }
    }

    fn new(id: &str) -> Value {
        json!({"goal_id": id, "condition": "tests pass", "budget": {"max_turns": 50}, "created_ms": 1})
    }

    fn activated(id: &str) -> Value {
        json!({"goal_id": id, "cost_before": 0.0, "tokens_before": 0, "activated_ms": 2})
    }

    #[test]
    fn set_queue_activate_complete_and_advance() {
        let mut g = Goals::default();
        fold(&mut g, &stored(SET, new("gol_A"), 1)).unwrap();
        fold(&mut g, &stored(ACTIVATED, activated("gol_A"), 2)).unwrap();
        fold(&mut g, &stored(QUEUED, new("gol_B"), 3)).unwrap();
        fold(&mut g, &stored(QUEUED, new("gol_C"), 4)).unwrap();
        assert_eq!(g.current.as_deref(), Some("gol_A"));
        assert_eq!(g.queue, ["gol_B", "gol_C"]);
        fold(
            &mut g,
            &stored(
                EVALUATED,
                json!({"goal_id": "gol_A", "verdict": "met", "reason": "done",
            "turn": 3, "eval_tokens": 40, "eval_cost_usd": 0.01, "tokens": 900, "cost_usd": 0.2,
            "evaluated_ms": 5}),
                5,
            ),
        )
        .unwrap();
        fold(&mut g, &stored(COMPLETED, json!({"goal_id": "gol_A"}), 6)).unwrap();
        assert_eq!(g.current, None);
        let a = &g.goals["gol_A"];
        assert_eq!(
            (a.status, a.turns, a.eval_count, a.tokens),
            (GoalStatus::Met, 3, 1, 900)
        );
        fold(
            &mut g,
            &stored(REORDERED, json!({"goal_ids": ["gol_C", "gol_B"]}), 7),
        )
        .unwrap();
        fold(&mut g, &stored(ACTIVATED, activated("gol_C"), 8)).unwrap();
        assert_eq!(
            (g.current.as_deref(), g.queue.as_slice()),
            (Some("gol_C"), &["gol_B".to_string()][..])
        );
    }

    #[test]
    fn illegal_transitions_are_refused() {
        let mut g = Goals::default();
        fold(&mut g, &stored(SET, new("gol_A"), 1)).unwrap();
        let error = fold(&mut g, &stored(COMPLETED, json!({"goal_id": "gol_A"}), 2)).unwrap_err();
        assert!(error.contains("cannot go from pending to met"), "{error}");
        fold(&mut g, &stored(ACTIVATED, activated("gol_A"), 3)).unwrap();
        fold(
            &mut g,
            &stored(PAUSED, json!({"goal_id": "gol_A", "reason": "auth"}), 4),
        )
        .unwrap();
        assert_eq!(g.current().unwrap().status, GoalStatus::Paused);
        assert!(g.active().is_none());
        fold(
            &mut g,
            &stored(
                ENDED,
                json!({"goal_id": "gol_A", "status": "cleared", "reason": "user"}),
                5,
            ),
        )
        .unwrap();
        assert!(fold(&mut g, &stored(RESUMED, json!({"goal_id": "gol_A"}), 6)).is_err());
    }
}
