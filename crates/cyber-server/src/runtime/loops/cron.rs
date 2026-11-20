//! Scheduled tasks the model creates (`loops-scheduling` → In-session cron tools): Loops
//! of kind `cron` with `cron_` IDs that share every Loop mechanism and the per-Session
//! cap. Only root Sessions outside Workflow Runs have them, and the model can stop only
//! these, never a `/loop` the user started.

use chrono::{DateTime, Local, Utc};
use cyber_core::cron::{Cron, parse_at};
use cyber_core::loops::{MAX_TEXT, Missed, Notify, Overlap};
use serde::Deserialize;

use super::records::{self, LoopKind, LoopState, StopConditions, StopStatus};
use super::schedule::now_ms;
use crate::runtime::events::event;
use crate::runtime::{Runtime, RuntimeError};

/// `cron_create`: exactly one of `cron` and `at`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CronRequest {
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default)]
    pub at: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub missed: Missed,
    #[serde(default)]
    pub notify: Notify,
}

/// When a task first runs: its expression's next occurrence, or its one-shot time.
enum Timing {
    Cron(Cron),
    At(DateTime<Utc>),
}

impl CronRequest {
    fn timing(&self, now: DateTime<Utc>) -> Result<Timing, String> {
        match (&self.cron, &self.at) {
            (Some(expression), None) => Cron::parse(expression).map(Timing::Cron),
            (None, Some(at)) => parse_at(at, now, &Local).map(Timing::At),
            _ => Err("Give exactly one of cron (a 5-field expression) and at (a time)".into()),
        }
    }

    fn prompt(&self) -> Result<String, String> {
        let prompt = self.prompt.trim();
        if prompt.is_empty() {
            return Err("The scheduled task's prompt is empty".into());
        }
        if prompt.chars().count() > MAX_TEXT {
            return Err(format!("The prompt is longer than {MAX_TEXT} characters"));
        }
        Ok(prompt.to_owned())
    }
}

/// The next occurrence of a recurring task strictly after `now_ms`, in local time.
pub(super) fn next_occurrence(expression: &str, now_ms: i64) -> Option<i64> {
    let cron = Cron::parse(expression).ok()?;
    let now = DateTime::<Utc>::from_timestamp_millis(now_ms)?;
    cron.next_after(now, &Local).map(|t| t.timestamp_millis())
}

impl Runtime {
    /// Create a scheduled task in `session_id` (`cron_create`).
    pub async fn create_cron(
        &self,
        session_id: &str,
        request: CronRequest,
    ) -> Result<LoopState, RuntimeError> {
        let prompt = request.prompt().map_err(RuntimeError::Invalid)?;
        let now = now_ms();
        let now_utc = DateTime::<Utc>::from_timestamp_millis(now).unwrap_or_default();
        let timing = request.timing(now_utc).map_err(RuntimeError::Invalid)?;
        self.inner.ensure_admission_open(session_id)?;
        let handle = self.inner.handle(session_id).await?;
        let directory = handle.state.lock().await.info.directory.clone();
        let (settings, budget) = self
            .inner
            .tools
            .loop_settings(&directory)
            .map_err(RuntimeError::Invalid)?;
        let (cron, at_ms, next_due_ms) = match &timing {
            Timing::Cron(cron) => {
                let next = cron
                    .next_after(now_utc, &Local)
                    .ok_or_else(|| RuntimeError::Invalid("the expression never matches".into()))?;
                (
                    Some(cron.as_str().to_owned()),
                    None,
                    next.timestamp_millis(),
                )
            }
            Timing::At(at) => (None, Some(at.timestamp_millis()), at.timestamp_millis()),
        };
        let loop_id = cyber_core::ids::new_id("cron");
        let created = records::Created {
            reservation_scope_version: u32::from(
                budget
                    .as_ref()
                    .is_some_and(|b| b.enforcement == cyber_core::budget::Enforcement::Reserved),
            ),
            loop_id: loop_id.clone(),
            kind: LoopKind::Cron,
            prompt,
            interval_seconds: None,
            cron,
            at_ms,
            overlap: Overlap::Skip,
            missed: request.missed,
            notify: request.notify,
            stop: StopConditions::default(),
            budget,
            iteration_cap: settings.max_iterations,
            created_ms: now,
            next_due_ms,
        };
        let state = {
            let mut state = handle.state.lock().await;
            if state.info.parent_id.is_some() || state.info.workflow_run_id.is_some() {
                return Err(RuntimeError::Invalid(
                    "Scheduled tasks belong to root Sessions outside Workflow Runs".into(),
                ));
            }
            super::ensure_room(&state, &settings)?;
            self.inner
                .commit_locked(&mut state, vec![event(records::CREATED, &created)])?;
            state.loops[&loop_id].clone()
        };
        self.nudge_loops();
        Ok(state)
    }

    /// The Session's active scheduled tasks (`cron_list`).
    pub async fn cron_tasks(&self, session_id: &str) -> Result<Vec<LoopState>, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let state = handle.state.lock().await;
        Ok(state
            .loops
            .values()
            .filter(|l| l.kind == LoopKind::Cron && l.active())
            .cloned()
            .collect())
    }

    /// Stop one active scheduled task (`cron_delete`); a `/loop` is not one, so it is
    /// reported unknown and keeps running.
    pub async fn delete_cron(&self, session_id: &str, task_id: &str) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let known = state
            .loops
            .get(task_id)
            .is_some_and(|l| l.kind == LoopKind::Cron && l.active());
        if !known {
            return Err(RuntimeError::Invalid(format!(
                "Unknown scheduled task {task_id}"
            )));
        }
        let stopped = records::Stopped {
            loop_id: task_id.to_owned(),
            status: StopStatus::Stopped,
            reason: "deleted with cron_delete".into(),
        };
        self.inner
            .commit_locked(&mut state, vec![event(records::STOPPED, &stopped)])?;
        Ok(())
    }
}
