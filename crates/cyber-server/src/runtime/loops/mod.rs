//! Session-scoped loops (`loops-scheduling`): `/loop` re-admits a prompt into its Session
//! on an interval, or at the pace the model sets with `schedule_wakeup`.
//!
//! A loop is a set of `loop.*` events on its Session's stream, folded into
//! [`SessionState::loops`] (the source of truth for every decision) and projected into
//! `session_loop` (the due-time index the sweep reads). Iterations are ordinary queued
//! admissions with `source: "loop"` and the deterministic ID `loopi_<loop>_<n>`, so a
//! retried admission after a crash is idempotent. [`schedule`] runs them: a background
//! sweep admits due iterations, and the Drain's idle exit ends them.

pub mod records;
mod schedule;

use cyber_core::loops::{LoopSettings, LoopSpec, WAKEUP_RANGE};
use serde::Serialize;

pub use records::{
    Inflight, IterationRecord, LoopKind, LoopState, Outcome, StopConditions, StopStatus,
};

use super::events::event;
use super::{Runtime, RuntimeError, SessionState};

/// Ended loops `loops()` lists after the active ones.
const RECENT_ENDED: usize = 20;
/// `schedule_wakeup` reasons longer than this are cut.
const MAX_REASON: usize = 200;

/// What `schedule_wakeup` recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct WakeupAck {
    pub loop_id: String,
    pub kind: LoopKind,
    /// The clamped delay; an interval loop keeps its interval instead.
    pub delay_seconds: u64,
}

impl Runtime {
    /// Create a loop in `session_id` (`/loop`, `POST /sessions/{id}/loops`); its first
    /// iteration is due now.
    pub async fn create_loop(
        &self,
        session_id: &str,
        spec: LoopSpec,
    ) -> Result<LoopState, RuntimeError> {
        spec.validate().map_err(RuntimeError::Invalid)?;
        self.inner.ensure_admission_open(session_id)?;
        let handle = self.inner.handle(session_id).await?;
        let directory = handle.state.lock().await.info.directory.clone();
        let (settings, default_budget) = self
            .inner
            .tools
            .loop_settings(&directory)
            .map_err(RuntimeError::Invalid)?;
        let loop_id = cyber_core::ids::new_id("lop");
        let now = schedule::now_ms();
        let created = records::Created {
            loop_id: loop_id.clone(),
            kind: if spec.interval_seconds.is_some() {
                LoopKind::Interval
            } else {
                LoopKind::SelfPaced
            },
            prompt: spec.prompt.trim().to_owned(),
            interval_seconds: spec.interval_seconds,
            overlap: spec.overlap,
            missed: spec.missed,
            stop: StopConditions {
                max_iterations: spec.max_iterations,
                for_seconds: spec.for_seconds,
                until: spec.until,
            },
            budget: spec.budget.or(default_budget),
            iteration_cap: settings.max_iterations,
            created_ms: now,
            next_due_ms: now,
        };
        let state = {
            let mut state = handle.state.lock().await;
            ensure_room(&state, &settings)?;
            self.inner
                .commit_locked(&mut state, vec![event(records::CREATED, &created)])?;
            state.loops[&loop_id].clone()
        };
        self.nudge_loops();
        Ok(state)
    }

    /// The Session's active loops, then its most recent ended ones.
    pub async fn loops(&self, session_id: &str) -> Result<Vec<LoopState>, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let state = handle.state.lock().await;
        let (active, mut ended): (Vec<_>, Vec<_>) =
            state.loops.values().cloned().partition(LoopState::active);
        ended.sort_by_key(|l| std::cmp::Reverse(l.ended_ms));
        ended.truncate(RECENT_ENDED);
        Ok(active.into_iter().chain(ended).collect())
    }

    /// Stop one active loop, or every active loop with `None`; the IDs stopped. An
    /// iteration already admitted still runs; no later one is admitted.
    pub async fn stop_loops(
        &self,
        session_id: &str,
        loop_id: Option<&str>,
    ) -> Result<Vec<String>, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let targets: Vec<String> = match loop_id {
            Some(id) => {
                let found = state.loops.get(id).filter(|l| l.active());
                vec![
                    found
                        .ok_or_else(|| RuntimeError::LoopNotFound(id.to_owned()))?
                        .loop_id
                        .clone(),
                ]
            }
            None => state
                .loops
                .values()
                .filter(|l| l.active())
                .map(|l| l.loop_id.clone())
                .collect(),
        };
        let events = targets
            .iter()
            .map(|id| {
                let stopped = records::Stopped {
                    loop_id: id.clone(),
                    status: StopStatus::Stopped,
                    reason: "stopped by the user".into(),
                };
                event(records::STOPPED, &stopped)
            })
            .collect::<Vec<_>>();
        if !events.is_empty() {
            self.inner.commit_locked(&mut state, events)?;
        }
        Ok(targets)
    }

    /// Record the model's `schedule_wakeup` for the running iteration (the most recently
    /// promoted one when several loops run). The last call of an iteration wins.
    pub async fn loop_wakeup(
        &self,
        session_id: &str,
        delay_seconds: u64,
        reason: &str,
        noop: bool,
    ) -> Result<WakeupAck, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let (loop_id, n, kind) = running_iteration(&state).ok_or_else(|| {
            RuntimeError::Invalid("schedule_wakeup runs only inside a /loop iteration".into())
        })?;
        let delay = delay_seconds.clamp(WAKEUP_RANGE.0, WAKEUP_RANGE.1);
        let wakeup = records::Wakeup {
            loop_id: loop_id.clone(),
            n,
            delay_seconds: delay,
            reason: reason.trim().chars().take(MAX_REASON).collect(),
            noop,
        };
        self.inner
            .commit_locked(&mut state, vec![event(records::WAKEUP, &wakeup)])?;
        Ok(WakeupAck {
            loop_id,
            kind,
            delay_seconds: delay,
        })
    }

    /// Whether an iteration of an active loop of `session_id` runs, so `schedule_wakeup`
    /// and the loop guidance are offered. Reads only the index.
    pub fn loop_iteration_running(&self, session_id: &str) -> bool {
        let id = session_id.to_owned();
        self.inner
            .store
            .read(move |db| {
                Ok(db
                    .prepare(
                        "SELECT 1 FROM session_loop
                         WHERE session_id = ?1 AND status = 'active' AND inflight = 1",
                    )?
                    .exists([id])?)
            })
            .unwrap_or(false)
    }

    /// Rebuild `session_loop` from the `loop.*` events; the number of events re-projected.
    pub fn rebuild_loops(&self) -> Result<usize, RuntimeError> {
        Ok(self.inner.store.reproject(
            &records::TABLES,
            records::PREFIX,
            std::sync::Arc::new(records::project),
        )?)
    }
}

fn ensure_room(state: &SessionState, settings: &LoopSettings) -> Result<(), RuntimeError> {
    let active = state.loops.values().filter(|l| l.active()).count() as u64;
    if active >= settings.max_per_session {
        return Err(RuntimeError::LoopLimit(settings.max_per_session));
    }
    Ok(())
}

/// The active loop whose in-flight iteration was promoted last: `(loop, n, kind)`.
fn running_iteration(state: &SessionState) -> Option<(String, u64, LoopKind)> {
    state
        .loops
        .values()
        .filter(|l| l.active())
        .filter_map(|l| {
            let inflight = l.inflight.as_ref()?;
            let promoted = state
                .inbox
                .iter()
                .find(|row| row.message_id == inflight.message_id)?
                .promoted_seq?;
            Some((promoted, l.loop_id.clone(), inflight.n, l.kind))
        })
        .max_by_key(|(promoted, ..)| *promoted)
        .map(|(_, id, n, kind)| (id, n, kind))
}
