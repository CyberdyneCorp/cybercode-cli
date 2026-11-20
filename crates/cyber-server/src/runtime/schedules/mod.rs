//! Persistent local schedules (`loops-scheduling` → Persistent local schedules, Unattended
//! Mode for schedules, Jitter, Completion notifications, Budgets for loops): prompts the
//! service runs on a cron schedule even when no client is open.
//!
//! A schedule is its own event stream (`schedule.*`, keyed by its `cron_` ID) projected
//! into `schedule` and `schedule_run`. Every transition is decided inside the writer
//! transaction that records it, so the sweep, `run now` and the Drain's idle exit never
//! record the same run twice. [`run`] runs them: each due run passes the unattended gate,
//! then is admitted as queued input `schedi_<id>_<n>` into a new or resumed Session.

mod gate;
pub mod records;
mod run;

use std::sync::Arc;

use cyber_core::budget::Budget;
use cyber_core::cron::Cron;
use cyber_core::loops::{MAX_TEXT, Missed, Notify};
use cyber_store::{Expected, NewEvent};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub use records::{RunOutcome, RunRecord, Schedule, ScheduleStatus};
pub(crate) use run::scheduled_run_current;

use super::events::event;
use super::{Runtime, RuntimeError};

/// The model asks no approver here: the run continues without the action.
pub const NO_SCHEDULE_APPROVER: &str = "Denied: no approver in unattended schedule";
/// The Mode a schedule runs in unless it names one.
pub const DEFAULT_MODE: &str = "dont-ask";
/// The longest schedule name.
const MAX_NAME: usize = 80;

/// A desktop notification outside any client (`loops-scheduling` → Completion
/// notifications): scheduled runs usually happen with no client attached.
pub trait DesktopNotifier: Send + Sync {
    /// Show `title` and `body`; best effort, never blocking for long.
    fn notify(&self, title: &str, body: &str);
}

/// `POST /api/v1/schedules`, `cyber loops add`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleRequest {
    /// A 5-field cron expression in the service's local time zone.
    pub cron: String,
    pub prompt: String,
    /// An absolute directory; the request's Location when absent.
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    /// `dont-ask` when absent; `bypass` needs `loops.allow_bypass` in user or managed
    /// config.
    #[serde(default)]
    pub mode: Option<String>,
    /// The model of new run Sessions; the Location's default when absent.
    #[serde(default)]
    pub model: Option<String>,
    /// Resume this root Session for every run instead of creating one per run.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub notify: Notify,
    #[serde(default)]
    pub missed: Missed,
    #[serde(default)]
    pub budget: Option<Budget>,
}

/// What the unattended gate needs to know about a directory (`ToolHost::schedule_policy`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SchedulePolicy {
    /// The checkout's configuration is trusted (`workspace-trust`).
    pub trusted: bool,
    /// `loops.allow_bypass` is true in user or managed config.
    pub allow_bypass: bool,
    /// `loops.jitter_seconds` (default 60; 0 turns jitter off).
    pub jitter_seconds: u64,
    /// `budgets.loop`, the default budget across runs.
    pub budget: Option<Budget>,
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// The fixed offset of a schedule: SHA-256 of its ID modulo the jitter window.
pub fn jitter_ms(schedule_id: &str, jitter_seconds: u64) -> i64 {
    let window = jitter_seconds.saturating_mul(1000);
    if window == 0 {
        return 0;
    }
    let digest = Sha256::digest(schedule_id.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    i64::try_from(u64::from_be_bytes(bytes) % window).unwrap_or(0)
}

/// The next run after `now_ms`: the next occurrence after `now - jitter`, plus jitter,
/// which is always in the future.
pub fn next_run(cron: &str, jitter_ms: i64, now_ms: i64) -> Option<i64> {
    let cron = Cron::parse(cron).ok()?;
    let after = chrono::DateTime::from_timestamp_millis(now_ms.saturating_sub(jitter_ms))?;
    let base = cron.next_after(after, &chrono::Local)?;
    Some(base.timestamp_millis() + jitter_ms)
}

fn not_found(id: &str) -> RuntimeError {
    RuntimeError::ScheduleNotFound(id.to_owned())
}

fn validate(request: &ScheduleRequest) -> Result<(), String> {
    Cron::parse(&request.cron)?;
    let prompt = request.prompt.trim();
    if prompt.is_empty() || prompt.chars().count() > MAX_TEXT {
        return Err(format!(
            "A schedule's prompt has 1 to {MAX_TEXT} characters"
        ));
    }
    if request
        .name
        .as_ref()
        .is_some_and(|n| n.trim().is_empty() || n.chars().count() > MAX_NAME)
    {
        return Err(format!("A schedule's name has 1 to {MAX_NAME} characters"));
    }
    if let Some(budget) = &request.budget {
        budget.validate()?;
    }
    Ok(())
}

impl Runtime {
    /// Create a persistent schedule after the unattended gate admitted its directory,
    /// Mode and Session; its first run is the next occurrence.
    pub async fn create_schedule(
        &self,
        request: ScheduleRequest,
    ) -> Result<Schedule, RuntimeError> {
        validate(&request).map_err(RuntimeError::Invalid)?;
        let directory = request
            .directory
            .clone()
            .ok_or_else(|| RuntimeError::Invalid("A schedule needs a directory".into()))?;
        let mode = request.mode.clone().unwrap_or_else(|| DEFAULT_MODE.into());
        super::selection::validate_mode(&mode)?;
        let model = request
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .ok_or_else(|| RuntimeError::Invalid("A schedule needs a model".into()))?;
        let schedule_id = cyber_core::ids::new_id("cron");
        let draft = gate::Draft {
            directory: &directory,
            agent: request.agent.as_deref().unwrap_or("build"),
            mode: &mode,
            session_id: request.session_id.as_deref(),
        };
        let policy = self
            .schedule_gate(&draft)
            .await
            .map_err(RuntimeError::Invalid)?;
        let jitter = jitter_ms(&schedule_id, policy.jitter_seconds);
        let now = now_ms();
        let next_due_ms = next_run(&request.cron, jitter, now)
            .ok_or_else(|| RuntimeError::Invalid("The expression never matches".into()))?;
        let created = records::Created {
            name: request
                .name
                .map(|n| n.trim().to_owned())
                .unwrap_or_else(|| schedule_id.clone()),
            schedule_id: schedule_id.clone(),
            cron: request.cron.trim().to_owned(),
            directory,
            prompt: request.prompt.trim().to_owned(),
            agent: request.agent,
            mode,
            model,
            session_id: request.session_id,
            notify: request.notify,
            missed: request.missed,
            budget: request.budget.or(policy.budget),
            jitter_ms: jitter,
            created_ms: now,
            next_due_ms,
        };
        self.inner.store.append(
            &schedule_id,
            Expected::Seq(-1),
            vec![event(records::CREATED, &created)],
        )?;
        self.nudge_schedules();
        self.schedule(&schedule_id, false)
    }

    /// Every schedule not removed, oldest first.
    pub fn schedules(&self) -> Result<Vec<Schedule>, RuntimeError> {
        Ok(self.inner.store.read(|db| Ok(records::list(db)?))?)
    }

    /// One schedule; `with_runs` adds its most recent runs.
    pub fn schedule(&self, id: &str, with_runs: bool) -> Result<Schedule, RuntimeError> {
        let key = id.to_owned();
        let found = self.inner.store.read(move |db| {
            let Some(mut schedule) = records::load(db, &key)? else {
                return Ok(None);
            };
            if with_runs {
                schedule.recent = records::runs(db, &key, records::RECENT_RUNS)?;
            }
            Ok(Some(schedule))
        })?;
        found
            .filter(|s| s.status != ScheduleStatus::Deleted)
            .ok_or_else(|| not_found(id))
    }

    /// Pause an active schedule; a run in flight finishes.
    pub fn pause_schedule(&self, id: &str) -> Result<Schedule, RuntimeError> {
        self.update_schedule(id, |schedule, _| match schedule.status {
            ScheduleStatus::Active | ScheduleStatus::Paused => Ok(records::Updated {
                schedule_id: schedule.id.clone(),
                status: ScheduleStatus::Paused,
                reason: Some("paused by the user".into()),
                next_due_ms: None,
            }),
            _ => Err(RuntimeError::Invalid(format!(
                "Schedule {} is {} and cannot be paused",
                schedule.id,
                schedule.status.as_str()
            ))),
        })?;
        self.schedule(id, false)
    }

    /// Resume a paused schedule at its next occurrence. One that exhausted its budget
    /// stays stopped: its budget is fixed, so it is removed and added again.
    pub fn resume_schedule(&self, id: &str) -> Result<Schedule, RuntimeError> {
        self.update_schedule(id, |schedule, now| match schedule.status {
            ScheduleStatus::Active | ScheduleStatus::Paused => Ok(records::Updated {
                schedule_id: schedule.id.clone(),
                status: ScheduleStatus::Active,
                reason: None,
                next_due_ms: next_run(&schedule.cron, schedule.jitter_ms, now),
            }),
            _ => Err(RuntimeError::Invalid(format!(
                "Schedule {} stopped ({}); remove it and add a new one",
                schedule.id,
                schedule.reason.as_deref().unwrap_or("budget exceeded")
            ))),
        })?;
        self.nudge_schedules();
        self.schedule(id, false)
    }

    /// Remove a schedule; its history stays in the store and a run in flight finishes.
    pub fn remove_schedule(&self, id: &str) -> Result<(), RuntimeError> {
        self.update_schedule(id, |schedule, _| {
            Ok(records::Updated {
                schedule_id: schedule.id.clone(),
                status: ScheduleStatus::Deleted,
                reason: Some("removed by the user".into()),
                next_due_ms: None,
            })
        })
    }

    /// Decide and record one `schedule.updated` in the writer transaction.
    fn update_schedule(
        &self,
        id: &str,
        decide: impl FnOnce(&Schedule, i64) -> Result<records::Updated, RuntimeError> + Send + 'static,
    ) -> Result<(), RuntimeError> {
        let key = id.to_owned();
        let missing = not_found(id);
        let (_, outcome) = self
            .inner
            .store
            .append_checked(id, Expected::Any, move |tx| {
                let schedule =
                    records::load(tx, &key)?.filter(|s| s.status != ScheduleStatus::Deleted);
                let decided = match schedule {
                    Some(schedule) => decide(&schedule, now_ms()),
                    None => Err(missing),
                };
                Ok(match decided {
                    Ok(updated) => (vec![event(records::UPDATED, &updated)], Ok(())),
                    Err(error) => (Vec::new(), Err(error)),
                })
            })?;
        outcome
    }

    /// Set the desktop notifier scheduled runs use (the App wires the OS one).
    pub fn set_desktop_notifier(&self, notifier: Arc<dyn DesktopNotifier>) {
        let _ = self.inner.desktop_notifier.set(notifier);
    }

    /// Rebuild `schedule` and `schedule_run` from the `schedule.*` events.
    pub fn rebuild_schedules(&self) -> Result<usize, RuntimeError> {
        Ok(self.inner.store.reproject(
            &records::TABLES,
            records::PREFIX,
            std::sync::Arc::new(records::project),
        )?)
    }
}

/// Append prepared schedule events to one schedule's stream.
pub(crate) fn append(
    runtime: &Runtime,
    schedule_id: &str,
    events: Vec<NewEvent>,
) -> Result<(), RuntimeError> {
    if events.is_empty() {
        return Ok(());
    }
    runtime
        .inner
        .store
        .append(schedule_id, Expected::Any, events)
        .map(drop)
        .map_err(RuntimeError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_is_deterministic_and_bounded() {
        let a = jitter_ms("cron_01ABC", 60);
        assert_eq!(a, jitter_ms("cron_01ABC", 60));
        assert!((0..60_000).contains(&a));
        assert_ne!(a, jitter_ms("cron_01ABD", 60), "IDs spread");
        assert_eq!(jitter_ms("cron_01ABC", 0), 0);
    }

    #[test]
    fn the_next_run_includes_jitter_and_lies_ahead() {
        let jitter = 30_000;
        let now = chrono::Utc::now().timestamp_millis();
        let next = next_run("* * * * *", jitter, now).unwrap();
        assert!(next > now, "{next} > {now}");
        assert_eq!((next - jitter) % 60_000, 0, "a whole minute plus jitter");
        assert!(next - now <= 60_000 + jitter);
    }

    #[test]
    fn requests_are_validated() {
        let ok = ScheduleRequest {
            cron: "0 2 * * *".into(),
            prompt: "review".into(),
            ..ScheduleRequest::default()
        };
        assert!(validate(&ok).is_ok());
        let bad_cron = ScheduleRequest {
            cron: "0 25 * * *".into(),
            ..ok.clone()
        };
        assert!(
            validate(&bad_cron)
                .unwrap_err()
                .contains("Invalid cron expression")
        );
        let empty = ScheduleRequest {
            prompt: " ".into(),
            ..ok
        };
        assert!(validate(&empty).is_err());
    }
}
