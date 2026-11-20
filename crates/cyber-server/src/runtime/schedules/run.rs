//! Running schedules: the sweep starts due runs through the unattended gate, the Drain's
//! idle exit ends them, and startup restores what a stopped service left behind.
//!
//! A run's start and end are each recorded by a decision made inside the writer
//! transaction (the run must still be in flight to end, the schedule free to start), so
//! a sweep, `run now`, a nudge and the idle hook cannot record a transition twice. Work
//! that must not hold the writer (the gate, Session creation, admission) happens between.

use std::sync::PoisonError;
use std::time::Duration;

use cyber_core::loops::{Missed, Notify};
use cyber_store::Expected;
use serde_json::json;

use super::gate::{Draft, plain};
use super::records::{self, Phase, Run, RunOutcome, RunRecord, Schedule, ScheduleStatus};
use super::{append, next_run, now_ms};
use crate::runtime::events::event;
use crate::runtime::{
    CreateSession, InputStatus, Notification, Runtime, RuntimeError, SessionState,
};

/// How often the background sweep looks for due runs.
const SWEEP_EVERY: Duration = Duration::from_secs(5);
/// The longest result a notification shows.
const RESULT_EXCERPT: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// Its time came.
    Due,
    /// `cyber loops run`: now, outside the schedule.
    Manual,
}

fn warn(what: &str, error: &RuntimeError) {
    cyber_core::log::warn(
        "schedules",
        what,
        serde_json::json!({ "error": error.to_string() }),
    );
}

/// The deterministic inbox ID of run `n`.
pub(crate) fn message_id(schedule_id: &str, n: u64) -> String {
    format!("schedi_{}_{n}", schedule_id.trim_start_matches("cron_"))
}

/// Whether `id` is a scheduled run's inbox ID.
pub(crate) fn is_run_message(id: &str) -> bool {
    id.starts_with("schedi_")
}

/// Why `schedule` must not run again: an exhausted budget, as `<limit> <spent>/<max>`.
fn budget_exceeded(schedule: &Schedule, now: i64) -> Option<String> {
    let budget = schedule.budget.as_ref()?;
    if let Some(max) = budget.max_cost_usd
        && schedule.cost_usd >= max
    {
        return Some(format!("max_cost_usd {:.2}/{max:.2}", schedule.cost_usd));
    }
    if let Some(max) = budget.max_tokens
        && schedule.tokens >= max
    {
        return Some(format!("max_tokens {}/{max}", schedule.tokens));
    }
    if let Some(max) = budget.max_turns
        && schedule.turns >= max
    {
        return Some(format!("max_turns {}/{max}", schedule.turns));
    }
    let wall = budget.max_wall_seconds?;
    let elapsed = (now - schedule.created_ms) as f64 / 1000.0;
    (elapsed >= wall).then(|| format!("max_wall_seconds {elapsed:.0}/{wall:.0}"))
}

/// Session totals a run's end is measured against: cost, tokens, model steps.
fn totals(state: &SessionState) -> (f64, u64, u64) {
    let usage = &state.totals.usage;
    let tokens = usage.input + usage.output + usage.reasoning;
    (state.totals.cost, tokens, u64::from(state.totals.steps))
}

fn promoted(state: &SessionState, message_id: &str) -> bool {
    state
        .inbox
        .iter()
        .any(|row| row.message_id == message_id && row.status == InputStatus::Promoted)
}

/// The Session's current run is a scheduled one: its last promoted input is `schedi_`.
pub(crate) fn scheduled_run_current(state: &SessionState) -> bool {
    state
        .inbox
        .iter()
        .filter_map(|row| row.promoted_seq.map(|seq| (seq, row)))
        .max_by_key(|(seq, _)| *seq)
        .is_some_and(|(_, row)| is_run_message(&row.message_id))
}

fn outcome_of(status: Option<&str>) -> RunOutcome {
    match status {
        Some(s) if s.starts_with("failed") => RunOutcome::Failed,
        Some(_) => RunOutcome::Interrupted,
        None => RunOutcome::Completed,
    }
}

fn clip(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let cut: String = line.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Whether an ended run notifies under `notify`; a refused run counts as a failure.
fn run_notifies(notify: Notify, run: &Run) -> bool {
    match run.outcome {
        Some(RunOutcome::Completed) => notify.notifies(false, run.noop),
        Some(RunOutcome::Failed | RunOutcome::Refused) => notify.notifies(true, false),
        _ => notify == Notify::Always,
    }
}

/// The one-line result of an ended run.
fn run_result(run: &Run) -> String {
    let outcome = run.outcome.map_or("ended", RunOutcome::as_str);
    match run.reason.as_deref().map(|r| clip(r, RESULT_EXCERPT)) {
        Some(reason) => format!("run {} {outcome}: {reason}", run.n),
        None => format!("run {} {outcome}", run.n),
    }
}

/// A run record that ends a run which never started (skipped or refused).
fn unstarted(
    schedule: &Schedule,
    n: u64,
    due_ms: i64,
    outcome: RunOutcome,
    reason: String,
    next_due_ms: Option<i64>,
) -> Run {
    let mut run = Run::new(
        &schedule.id,
        &cyber_core::ids::new_id("srun"),
        n,
        Phase::Ended,
        due_ms,
    );
    run.outcome = Some(outcome);
    run.reason = Some(reason);
    run.next_due_ms = next_due_ms;
    run
}

impl Runtime {
    /// Start every due run (and record skipped ones) at `now_ms`; the number of schedules
    /// visited. Tests drive it with a chosen time.
    pub async fn sweep_schedules(&self, now_ms: i64) -> Result<usize, RuntimeError> {
        let due = self
            .inner
            .store
            .read(move |db| Ok(records::due(db, now_ms)?))?;
        for id in &due {
            if self.is_shutting_down() {
                break;
            }
            if let Err(error) = self.run_due_schedule(id, now_ms).await {
                warn("a scheduled run failed to start", &error);
            }
        }
        Ok(due.len())
    }

    /// `cyber loops run`: start a run now, outside the schedule (also when paused); the
    /// run's ID.
    pub async fn run_schedule_now(&self, id: &str) -> Result<String, RuntimeError> {
        let schedule = self.schedule(id, false)?;
        if schedule.inflight.is_some() {
            return Err(RuntimeError::Invalid(format!(
                "Schedule {id} has a run in flight"
            )));
        }
        if !matches!(
            schedule.status,
            ScheduleStatus::Active | ScheduleStatus::Paused
        ) {
            return Err(RuntimeError::Invalid(format!(
                "Schedule {id} is {}",
                schedule.status.as_str()
            )));
        }
        self.start_run(schedule, now_ms(), Trigger::Manual).await
    }

    async fn run_due_schedule(&self, id: &str, now: i64) -> Result<(), RuntimeError> {
        let key = id.to_owned();
        let Some(schedule) = self
            .inner
            .store
            .read(move |db| Ok(records::load(db, &key)?))?
        else {
            return Ok(());
        };
        if schedule.status != ScheduleStatus::Active
            || schedule.next_due_ms.is_none_or(|due| due > now)
        {
            return Ok(());
        }
        if schedule.inflight.is_some() {
            let next = next_run(&schedule.cron, schedule.jitter_ms, now);
            let due = schedule.next_due_ms.unwrap_or(now);
            let skip = unstarted(
                &schedule,
                schedule.runs + 1,
                due,
                RunOutcome::SkippedOverlap,
                "the previous run was still running".into(),
                next,
            );
            return self.end_unstarted(&schedule, skip).await;
        }
        self.start_run(schedule, now, Trigger::Due).await.map(drop)
    }

    /// Gate, prepare the Session, record the start and admit the prompt; the run ID.
    async fn start_run(
        &self,
        schedule: Schedule,
        now: i64,
        trigger: Trigger,
    ) -> Result<String, RuntimeError> {
        let (due, next) = match trigger {
            Trigger::Due => (
                schedule.next_due_ms.unwrap_or(now),
                next_run(&schedule.cron, schedule.jitter_ms, now),
            ),
            Trigger::Manual => (now, None),
        };
        let n = schedule.runs + 1;
        if let Some(reason) = budget_exceeded(&schedule, now) {
            self.stop_for_budget(&schedule, &reason).await;
            return Err(RuntimeError::Invalid(format!(
                "Schedule {} exceeded its budget: {reason}",
                schedule.id
            )));
        }
        let draft = Draft {
            directory: &schedule.directory,
            agent: schedule.agent.as_deref().unwrap_or("build"),
            mode: &schedule.mode,
            session_id: schedule.session_id.as_deref(),
        };
        if let Err(reason) = self.schedule_gate(&draft).await {
            let refused = unstarted(&schedule, n, due, RunOutcome::Refused, reason, next);
            let run_id = refused.run_id.clone();
            self.end_unstarted(&schedule, refused).await?;
            return Ok(run_id);
        }
        let session_id = match self.run_session(&schedule, now).await {
            Ok(id) => id,
            Err((outcome, reason)) => {
                let failed = unstarted(&schedule, n, due, outcome, reason, next);
                let run_id = failed.run_id.clone();
                self.end_unstarted(&schedule, failed).await?;
                return Ok(run_id);
            }
        };
        self.admit_run(&schedule, session_id, n, due, next).await
    }

    /// The Session a run uses: a new unattended one, or the schedule's own Session switched
    /// to the schedule's Mode (a busy one is skipped).
    async fn run_session(
        &self,
        schedule: &Schedule,
        now: i64,
    ) -> Result<String, (RunOutcome, String)> {
        let failed = |error: RuntimeError| (RunOutcome::Failed, plain(&error));
        if let Some(id) = &schedule.session_id {
            if self.is_running(id) {
                return Err((RunOutcome::SkippedOverlap, format!("Session {id} was busy")));
            }
            self.switch_mode(id, &schedule.mode).await.map_err(failed)?;
            return Ok(id.clone());
        }
        let title = format!(
            "{} · {}",
            schedule.name,
            cyber_core::cron::local_time_text(now)
        );
        let info = self
            .create_session(CreateSession {
                directory: schedule.directory.clone(),
                model: schedule.model.clone(),
                model_is_default: true,
                agent: schedule.agent.clone(),
                mode: Some(schedule.mode.clone()),
                title: Some(title),
                unattended: true,
                ..CreateSession::default()
            })
            .await
            .map_err(failed)?;
        Ok(info.id)
    }

    /// Record the start, then admit the prompt queued; a refused admission ends the run.
    async fn admit_run(
        &self,
        schedule: &Schedule,
        session_id: String,
        n: u64,
        due: i64,
        next: Option<i64>,
    ) -> Result<String, RuntimeError> {
        let message = message_id(&schedule.id, n);
        let (cost_before, tokens_before, turns_before) = totals(&self.state(&session_id).await?);
        let mut started = Run::new(
            &schedule.id,
            &cyber_core::ids::new_id("srun"),
            n,
            Phase::Started,
            due,
        );
        started.session_id = Some(session_id.clone());
        started.message_id = Some(message.clone());
        started.cost_before = cost_before;
        started.tokens_before = tokens_before;
        started.turns_before = turns_before;
        started.next_due_ms = next;
        let run_id = started.run_id.clone();
        if !self.record_schedule_start(started)? {
            return Ok(run_id);
        }
        let admitted = match self
            .prompt_admission(&session_id, &schedule.prompt, message)
            .await
        {
            Ok(admission) => self
                .admit_loop_iteration(&session_id, admission, Vec::new())
                .await
                .map(drop),
            Err(error) => Err(error),
        };
        if let Err(error) = admitted {
            self.end_run(
                &schedule.id,
                &run_id,
                RunOutcome::Failed,
                Some(plain(&error)),
            )
            .await;
        }
        Ok(run_id)
    }

    /// Record a run's start if the schedule is still free and at `n - 1` runs.
    fn record_schedule_start(&self, started: Run) -> Result<bool, RuntimeError> {
        let id = started.schedule_id.clone();
        let (_, recorded) = self
            .inner
            .store
            .append_checked(&id, Expected::Any, move |tx| {
                let free = records::load(tx, &started.schedule_id)?.is_some_and(|s| {
                    s.inflight.is_none()
                        && s.runs + 1 == started.n
                        && s.status != ScheduleStatus::Deleted
                });
                Ok(if free {
                    (vec![event(records::RUN, &started)], true)
                } else {
                    (Vec::new(), false)
                })
            })?;
        Ok(recorded)
    }

    /// Record a run that never started, then notify.
    async fn end_unstarted(&self, schedule: &Schedule, run: Run) -> Result<(), RuntimeError> {
        append(self, &schedule.id, vec![event(records::RUN, &run)])?;
        self.run_followups(schedule, &run).await;
        Ok(())
    }

    /// End a started run with `outcome` unless it already ended; measured against its
    /// Session's totals. Then the budget check, notifications and the hook.
    async fn end_run(
        &self,
        schedule_id: &str,
        run_id: &str,
        outcome: RunOutcome,
        reason: Option<String>,
    ) {
        if let Err(error) = self.try_end_run(schedule_id, run_id, outcome, reason).await {
            warn("ending a scheduled run failed", &error);
        }
    }

    async fn try_end_run(
        &self,
        schedule_id: &str,
        run_id: &str,
        outcome: RunOutcome,
        reason: Option<String>,
    ) -> Result<(), RuntimeError> {
        let key = run_id.to_owned();
        let Some(record) = self
            .inner
            .store
            .read(move |db| Ok(records::run(db, &key)?))?
        else {
            return Ok(());
        };
        let totals = match &record.session_id {
            Some(id) => self.state(id).await.map(|s| totals(&s)).ok(),
            None => None,
        };
        let ended = ended_run(schedule_id, &record, outcome, reason, totals);
        let recorded = ended.clone();
        let (_, done) = self
            .inner
            .store
            .append_checked(schedule_id, Expected::Any, move |tx| {
                let running =
                    records::run(tx, &recorded.run_id)?.is_some_and(|r| r.outcome == "running");
                Ok(if running {
                    (vec![event(records::RUN, &recorded)], true)
                } else {
                    (Vec::new(), false)
                })
            })?;
        if !done {
            return Ok(());
        }
        let schedule = self.schedule(schedule_id, false).ok();
        if let Some(schedule) = schedule {
            self.run_followups(&schedule, &ended).await;
            if let Some(reason) = budget_exceeded(&schedule, now_ms()) {
                self.stop_for_budget(&schedule, &reason).await;
            }
        }
        self.nudge_schedules();
        Ok(())
    }

    /// Stop `schedule` with `budget_exceeded`, and notify unless `never`.
    async fn stop_for_budget(&self, schedule: &Schedule, reason: &str) {
        let updated = records::Updated {
            schedule_id: schedule.id.clone(),
            status: ScheduleStatus::BudgetExceeded,
            reason: Some(reason.to_owned()),
            next_due_ms: None,
        };
        if let Err(error) = append(self, &schedule.id, vec![event(records::UPDATED, &updated)]) {
            return warn("stopping a schedule failed", &error);
        }
        if schedule.notify != Notify::Never {
            let body = format!("stopped (budget_exceeded): {reason}");
            self.schedule_notify(schedule, None, &body).await;
        }
    }

    /// Notifications and the `ScheduleRun` hook of an ended run.
    async fn run_followups(&self, schedule: &Schedule, run: &Run) {
        if run_notifies(schedule.notify, run) {
            self.schedule_notify(schedule, run.session_id.as_deref(), &run_result(run))
                .await;
        }
        let Some(session_id) = run.session_id.as_deref() else {
            return;
        };
        let Ok(handle) = self.inner.handle(session_id).await else {
            return;
        };
        let fields = json!({
            "schedule": {
                "id": schedule.id,
                "name": schedule.name,
                "cron": schedule.cron,
                "directory": schedule.directory,
            },
            "run": {
                "run_id": run.run_id,
                "n": run.n,
                "outcome": run.outcome,
                "noop": run.noop,
                "duration_ms": run.duration_ms,
                "cost": run.cost,
                "tokens": run.tokens,
                "turns": run.turns,
                "reason": run.reason,
            },
        });
        let hook = self
            .inner
            .lifecycle_hook(&handle, "ScheduleRun", schedule.id.clone(), fields)
            .await;
        self.inner.observe_lifecycle(hook).await;
    }

    /// A notification in the run's Session (clients and remote devices) and on the desktop.
    async fn schedule_notify(&self, schedule: &Schedule, session_id: Option<&str>, body: &str) {
        let title = format!("Schedule {}", clip(&schedule.name, 60));
        if let Some(session_id) = session_id {
            let notification = Notification {
                call_id: schedule.id.clone(),
                title: title.clone(),
                body: body.to_owned(),
            };
            // Past the per-Session rate limit it is dropped; the run record keeps it.
            let _ = self.record_notification(session_id, notification).await;
        }
        if let Some(notifier) = self.inner.desktop_notifier.get() {
            notifier.notify(&title, body);
        }
    }

    /// The Session's Drain ended (`drain::run`): end every scheduled run it ran.
    pub(crate) async fn schedules_drain_ended(&self, session_id: &str, status: Option<&str>) {
        if self.is_shutting_down() {
            return;
        }
        let id = session_id.to_owned();
        let running = match self
            .inner
            .store
            .read(move |db| Ok(records::running_in(db, &id)?))
        {
            Ok(running) if !running.is_empty() => running,
            Ok(_) => return,
            Err(error) => {
                return warn("reading scheduled runs failed", &RuntimeError::Store(error));
            }
        };
        let Ok(state) = self.state(session_id).await else {
            return;
        };
        let status = status.map(str::to_owned).or_else(|| {
            let (last, _) = crate::runtime::messaging::watch::last_run(&state);
            (last.starts_with("failed") || last == crate::runtime::INTERRUPTED).then_some(last)
        });
        for (schedule_id, run_id) in running {
            let key = run_id.clone();
            let message = self
                .inner
                .store
                .read(move |db| Ok(records::run(db, &key)?))
                .ok()
                .flatten()
                .and_then(|r| r.message_id);
            if message.is_some_and(|m| promoted(&state, &m)) {
                self.end_run(
                    &schedule_id,
                    &run_id,
                    outcome_of(status.as_deref()),
                    status.clone(),
                )
                .await;
            }
        }
    }

    /// `schedule_wakeup` during a scheduled run: record its `noop` and reason. `false`
    /// when the Session's current run is not a scheduled one.
    pub async fn report_schedule_run(
        &self,
        session_id: &str,
        reason: &str,
        noop: bool,
    ) -> Result<bool, RuntimeError> {
        let state = self.state(session_id).await?;
        if !scheduled_run_current(&state) {
            return Ok(false);
        }
        let id = session_id.to_owned();
        let running = self
            .inner
            .store
            .read(move |db| Ok(records::running_in(db, &id)?))?;
        for (schedule_id, run_id) in running {
            let key = run_id.clone();
            let Some(record) = self
                .inner
                .store
                .read(move |db| Ok(records::run(db, &key)?))?
            else {
                continue;
            };
            if !record
                .message_id
                .as_deref()
                .is_some_and(|m| promoted(&state, m))
            {
                continue;
            }
            let mut reported = Run::new(
                &schedule_id,
                &run_id,
                record.n,
                Phase::Reported,
                record.due_ms,
            );
            reported.noop = noop;
            reported.reason = Some(reason.trim().chars().take(200).collect());
            append(self, &schedule_id, vec![event(records::RUN, &reported)])?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Whether a scheduled run is in flight in `session_id` (offers `schedule_wakeup`).
    /// Reads only the index.
    pub fn schedule_run_running(&self, session_id: &str) -> bool {
        let id = session_id.to_owned();
        self.inner
            .store
            .read(move |db| Ok(!records::running_in(db, &id)?.is_empty()))
            .unwrap_or(false)
    }

    /// Whether the Session's current run is a scheduled one (its asks get
    /// [`super::NO_SCHEDULE_APPROVER`]).
    pub async fn scheduled_run_active(&self, session_id: &str) -> bool {
        self.schedule_run_running(session_id)
            && self
                .state(session_id)
                .await
                .is_ok_and(|state| scheduled_run_current(&state))
    }

    /// Run the sweep soon, without waiting for its next period.
    pub(crate) fn nudge_schedules(&self) {
        if !self
            .inner
            .schedules_on
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let weak = self.downgrade();
        tokio::spawn(async move {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            if let Err(error) = runtime.sweep_schedules(now_ms()).await {
                warn("schedule sweep failed", &error);
            }
        });
    }

    /// Restore schedules after a restart: end runs a stopped service cut off and apply the
    /// missed-run policy. Must run before [`Self::spawn_schedule_sweep`]; the number of
    /// records written.
    pub async fn recover_schedules(&self) -> Result<usize, RuntimeError> {
        self.recover_schedules_at(now_ms()).await
    }

    /// [`Self::recover_schedules`] at a chosen time.
    pub async fn recover_schedules_at(&self, now: i64) -> Result<usize, RuntimeError> {
        let ids = self.inner.store.read(|db| Ok(records::restorable(db)?))?;
        let mut changed = 0;
        for id in ids {
            changed += self.restore_schedule(&id, now).await?;
        }
        Ok(changed)
    }

    async fn restore_schedule(&self, id: &str, now: i64) -> Result<usize, RuntimeError> {
        let key = id.to_owned();
        let Some(schedule) = self
            .inner
            .store
            .read(move |db| Ok(records::load(db, &key)?))?
        else {
            return Ok(0);
        };
        if let Some(run_id) = &schedule.inflight {
            return self.restore_inflight(&schedule, run_id).await;
        }
        let missed = schedule.status == ScheduleStatus::Active
            && schedule.missed == Missed::Skip
            && schedule.next_due_ms.is_some_and(|due| due < now);
        if !missed {
            return Ok(0);
        }
        let mut skip = unstarted(
            &schedule,
            schedule.runs + 1,
            schedule.next_due_ms.unwrap_or(now),
            RunOutcome::SkippedMissed,
            "the service was not running".into(),
            next_run(&schedule.cron, schedule.jitter_ms, now),
        );
        skip.reason = Some("the service was not running".into());
        append(self, id, vec![event(records::RUN, &skip)])?;
        Ok(1)
    }

    /// A run in flight when the service stopped: one whose prompt was promoted in a Session
    /// that no longer runs ends `interrupted`; one still queued runs on its next Drain.
    async fn restore_inflight(
        &self,
        schedule: &Schedule,
        run_id: &str,
    ) -> Result<usize, RuntimeError> {
        let key = run_id.to_owned();
        let Some(record) = self
            .inner
            .store
            .read(move |db| Ok(records::run(db, &key)?))?
        else {
            return Ok(0);
        };
        let (Some(session_id), Some(message)) = (&record.session_id, &record.message_id) else {
            return Ok(0);
        };
        if self.is_running(session_id) {
            return Ok(0);
        }
        let cut_off = match self.state(session_id).await {
            Ok(state) => promoted(&state, message),
            // Its Session is gone: nothing will ever end it.
            Err(_) => true,
        };
        if !cut_off {
            return Ok(0);
        }
        self.end_run(
            &schedule.id,
            run_id,
            RunOutcome::Interrupted,
            Some("interrupted by a service restart".into()),
        )
        .await;
        Ok(1)
    }

    /// Run the schedule sweep every [`SWEEP_EVERY`] until shutdown. Only the served App
    /// (the `cyber service` process) runs schedules.
    pub fn spawn_schedule_sweep(&self) {
        self.inner
            .schedules_on
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let weak = self.downgrade();
        let closed = self.inner.closed.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = closed.cancelled() => return,
                    () = tokio::time::sleep(SWEEP_EVERY) => {}
                }
                let Some(runtime) = weak.upgrade() else {
                    return;
                };
                if let Err(error) = runtime.sweep_schedules(now_ms()).await {
                    warn("schedule sweep failed", &error);
                }
            }
        });
        let mut owners = self
            .inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        owners.retain(|owner| !owner.is_finished());
        owners.push(task);
    }
}

/// The `ended` record of a started run, measured against its Session's current totals.
fn ended_run(
    schedule_id: &str,
    record: &RunRecord,
    outcome: RunOutcome,
    reason: Option<String>,
    totals: Option<(f64, u64, u64)>,
) -> Run {
    let mut ended = Run::new(
        schedule_id,
        &record.run_id,
        record.n,
        Phase::Ended,
        record.due_ms,
    );
    ended.session_id = record.session_id.clone();
    ended.message_id = record.message_id.clone();
    ended.outcome = Some(outcome);
    ended.noop = record.noop && outcome == RunOutcome::Completed;
    ended.reason = reason.or_else(|| record.reason.clone());
    ended.duration_ms = record
        .started_ms
        .map_or(0, |started| (now_ms() - started).max(0));
    if let Some((cost, tokens, turns)) = totals {
        ended.cost = (cost - record.cost_before).max(0.0);
        ended.tokens = tokens.saturating_sub(record.tokens_before);
        ended.turns = turns.saturating_sub(record.turns_before);
    }
    ended
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_and_notify_policy() {
        assert_eq!(message_id("cron_01ABC", 3), "schedi_01ABC_3");
        assert!(is_run_message("schedi_01ABC_3"));
        let mut run = Run::new("cron_01ABC", "srun_1", 1, Phase::Ended, 0);
        run.outcome = Some(RunOutcome::Completed);
        assert!(run_notifies(Notify::OnChange, &run));
        run.noop = true;
        assert!(!run_notifies(Notify::OnChange, &run));
        assert!(run_notifies(Notify::Always, &run));
        run.outcome = Some(RunOutcome::Refused);
        assert!(run_notifies(Notify::OnChange, &run));
        assert!(!run_notifies(Notify::Never, &run));
        run.outcome = Some(RunOutcome::SkippedOverlap);
        assert!(!run_notifies(Notify::OnChange, &run));
        run.reason = Some("the previous run was still running".into());
        assert_eq!(
            run_result(&run),
            "run 1 skipped_overlap: the previous run was still running"
        );
    }
}
