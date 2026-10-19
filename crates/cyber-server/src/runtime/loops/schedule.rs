//! Running loops: the sweep admits due iterations, the Drain's idle exit ends them, and
//! startup restores what a stopped server left behind.
//!
//! Every decision is made from the Session state under its lock and committed there, so
//! the periodic sweep, a nudge after a change and the idle hook never record the same
//! transition twice. Work that must not hold the lock (the `--until` check, admission)
//! happens between two locked steps; admission is idempotent by its `loopi_` ID.

use std::sync::PoisonError;
use std::time::Duration;

use cyber_core::loops::Missed;

use super::records::{self, LoopKind, LoopState, Outcome, StopStatus};
use crate::runtime::events::event;
use crate::runtime::{Admission, Delivery, InputStatus, Runtime, RuntimeError, SessionState};
use cyber_store::NewEvent;

/// How often the background sweep looks for due iterations.
const SWEEP_EVERY: Duration = Duration::from_secs(5);
/// Consecutive failed iterations that stop a loop.
const MAX_FAILURES: u64 = 3;
/// Characters of a failed `--until` check kept for display.
const UNTIL_TAIL: usize = 2000;

pub(super) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn warn(what: &str, error: &RuntimeError) {
    cyber_core::log::warn(
        "loops",
        what,
        serde_json::json!({ "error": error.to_string() }),
    );
}

/// The first interval tick after `now`, counted from the loop's creation.
fn next_tick(state: &LoopState, now: i64) -> Option<i64> {
    let interval = i64::try_from(state.interval_seconds?)
        .ok()?
        .saturating_mul(1000);
    let elapsed = now.saturating_sub(state.created_ms).max(0);
    Some(state.created_ms + (elapsed / interval + 1) * interval)
}

/// Why `state` must stop at `now`, before admitting another iteration.
fn stop_reason(state: &LoopState, now: i64) -> Option<(StopStatus, String)> {
    let max = state.max_iterations();
    if state.iterations >= max {
        return Some((StopStatus::MaxIterations, format!("ran {max} iterations")));
    }
    if let Some(seconds) = state.stop.for_seconds
        && now >= state.created_ms + i64::try_from(seconds).unwrap_or(i64::MAX / 2000) * 1000
    {
        return Some((StopStatus::Duration, format!("ran for {seconds}s")));
    }
    budget_exceeded(state, now).map(|reason| (StopStatus::BudgetExceeded, reason))
}

fn budget_exceeded(state: &LoopState, now: i64) -> Option<String> {
    let budget = state.budget.as_ref()?;
    if let Some(max) = budget.max_cost_usd
        && state.cost_usd >= max
    {
        return Some(format!("max_cost_usd {:.2}/{max:.2}", state.cost_usd));
    }
    if let Some(max) = budget.max_tokens
        && state.tokens >= max
    {
        return Some(format!("max_tokens {}/{max}", state.tokens));
    }
    if let Some(max) = budget.max_turns
        && state.turns >= max
    {
        return Some(format!("max_turns {}/{max}", state.turns));
    }
    let wall = budget.max_wall_seconds?;
    let elapsed = (now - state.created_ms) as f64 / 1000.0;
    (elapsed >= wall).then(|| format!("max_wall_seconds {elapsed:.0}/{wall:.0}"))
}

fn stopped(loop_id: &str, status: StopStatus, reason: String) -> NewEvent {
    let stopped = records::Stopped {
        loop_id: loop_id.to_owned(),
        status,
        reason,
    };
    event(records::STOPPED, &stopped)
}

fn skipped(state: &LoopState, outcome: Outcome, next_due_ms: Option<i64>) -> NewEvent {
    let ended = records::IterationEnded {
        loop_id: state.loop_id.clone(),
        n: 0,
        outcome,
        noop: false,
        duration_ms: 0,
        cost: 0.0,
        tokens: 0,
        turns: 0,
        next_due_ms,
        reason: None,
    };
    event(records::ITERATION_ENDED, &ended)
}

/// Session totals an iteration's end is measured against.
fn totals(state: &SessionState) -> (f64, u64, u64) {
    let usage = &state.totals.usage;
    let tokens = usage.input + usage.output + usage.reasoning;
    (state.totals.cost, tokens, u64::from(state.totals.steps))
}

/// What the locked step of a due loop decided.
enum Due {
    Nothing,
    Run(Box<Iteration>),
}

/// An iteration to admit once the lock is released.
struct Iteration {
    loop_id: String,
    n: u64,
    prompt: String,
    until: Option<String>,
    directory: String,
    started: records::IterationStarted,
}

impl Runtime {
    /// Admit every due iteration (and record skipped ticks and stops) at `now_ms`; the
    /// number of loops visited. Tests drive it with a chosen time.
    pub async fn sweep_loops(&self, now_ms: i64) -> Result<usize, RuntimeError> {
        let due = self
            .inner
            .store
            .read(move |db| Ok(records::due(db, now_ms)?))?;
        for item in &due {
            if self.is_shutting_down() {
                break;
            }
            if let Err(error) = self.run_due(&item.session_id, &item.loop_id, now_ms).await {
                warn("loop iteration failed to start", &error);
            }
        }
        Ok(due.len())
    }

    async fn run_due(&self, session_id: &str, loop_id: &str, now: i64) -> Result<(), RuntimeError> {
        let Due::Run(iteration) = self.decide_due(session_id, loop_id, now).await? else {
            return Ok(());
        };
        let mut until_tail = None;
        if let Some(command) = &iteration.until {
            match self
                .until_check(&iteration.directory, session_id, command)
                .await
            {
                Ok(outcome) if outcome.exit_code == Some(0) => {
                    let reason = format!("--until \"{command}\" exited 0");
                    return self
                        .commit_loop(
                            session_id,
                            loop_id,
                            vec![stopped(loop_id, StopStatus::UntilMet, reason)],
                        )
                        .await;
                }
                Ok(outcome) => until_tail = Some(tail(&outcome.tail)),
                Err(error) => until_tail = Some(format!("--until check failed: {error}")),
            }
        }
        self.admit_iteration(session_id, *iteration, until_tail, now)
            .await
    }

    /// The locked step: stop, skip a tick, mark a coalesced tick, or hand back an
    /// iteration to admit.
    async fn decide_due(
        &self,
        session_id: &str,
        loop_id: &str,
        now: i64,
    ) -> Result<Due, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let Some(lp) = state.loops.get(loop_id).filter(|l| l.active()).cloned() else {
            return Ok(Due::Nothing);
        };
        if lp.next_due_ms.is_none_or(|due| due > now) {
            return Ok(Due::Nothing);
        }
        if let Some((status, reason)) = stop_reason(&lp, now) {
            self.inner
                .commit_locked(&mut state, vec![stopped(loop_id, status, reason)])?;
            return Ok(Due::Nothing);
        }
        if lp.inflight.is_some() {
            if lp.overlap == cyber_core::loops::Overlap::Coalesce {
                state.loop_coalesced.insert(loop_id.to_owned());
            }
            let skip = skipped(&lp, Outcome::SkippedOverlap, next_tick(&lp, now));
            self.inner.commit_locked(&mut state, vec![skip])?;
            return Ok(Due::Nothing);
        }
        let (cost_before, tokens_before, turns_before) = totals(&state);
        let n = lp.iterations + 1;
        Ok(Due::Run(Box::new(Iteration {
            loop_id: loop_id.to_owned(),
            n,
            prompt: lp.prompt.clone(),
            until: lp.stop.until.clone(),
            directory: state.info.directory.clone(),
            started: records::IterationStarted {
                loop_id: loop_id.to_owned(),
                n,
                message_id: iteration_id(loop_id, n),
                started_ms: now,
                cost_before,
                tokens_before,
                turns_before,
                until_tail: None,
                next_due_ms: next_tick(&lp, now),
            },
        })))
    }

    async fn until_check(
        &self,
        directory: &str,
        session_id: &str,
        command: &str,
    ) -> Result<crate::runtime::CheckOutcome, RuntimeError> {
        let (settings, _) = self
            .inner
            .tools
            .loop_settings(directory)
            .map_err(RuntimeError::Invalid)?;
        let timeout = settings.until_timeout_seconds.saturating_mul(1000);
        let cancel = self.inner.closed.child_token();
        self.inner
            .tools
            .check(directory, session_id, command, timeout, cancel)
            .await
            .map_err(RuntimeError::Invalid)
    }

    async fn admit_iteration(
        &self,
        session_id: &str,
        mut iteration: Iteration,
        until_tail: Option<String>,
        now: i64,
    ) -> Result<(), RuntimeError> {
        iteration.started.until_tail = until_tail;
        let started = event(records::ITERATION_STARTED, &iteration.started);
        let admission = match self.iteration_admission(session_id, &iteration).await {
            Ok(admission) => admission,
            Err(error) => {
                return self
                    .iteration_refused(session_id, &iteration, &error, now)
                    .await;
            }
        };
        match self
            .admit_loop_iteration(session_id, admission, vec![started])
            .await
        {
            Ok(_) => Ok(()),
            Err(error) => {
                self.iteration_refused(session_id, &iteration, &error, now)
                    .await
            }
        }
    }

    /// A queued admission of the iteration's prompt; a `/command` prompt is planned as the
    /// command route plans it.
    async fn iteration_admission(
        &self,
        session_id: &str,
        iteration: &Iteration,
    ) -> Result<Admission, RuntimeError> {
        let message_id = Some(iteration.started.message_id.clone());
        let plain = || Admission {
            message_id: message_id.clone(),
            source: "loop".into(),
            resume: true,
            ..Admission::text(iteration.prompt.clone(), Delivery::Queue)
        };
        let Some(command) = iteration.prompt.strip_prefix('/') else {
            return Ok(plain());
        };
        let (name, arguments) = command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""));
        let handle = self.inner.handle(session_id).await?;
        let turn = crate::runtime::drain::turn_context_for(&*handle.state.lock().await, false);
        let plan = self
            .inner
            .tools
            .loop_command_plan(&turn, name, arguments.trim())
            .await
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?
            .ok_or_else(|| RuntimeError::Invalid(format!("no command or skill named {name}")))?;
        let mut admission = plan.admission(message_id, Delivery::Queue);
        admission.source = "loop".into();
        Ok(admission)
    }

    /// An iteration that could not be admitted counts as a failed one.
    async fn iteration_refused(
        &self,
        session_id: &str,
        iteration: &Iteration,
        error: &RuntimeError,
        now: i64,
    ) -> Result<(), RuntimeError> {
        if matches!(error, RuntimeError::ShuttingDown) {
            return Ok(());
        }
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let Some(lp) = state
            .loops
            .get(&iteration.loop_id)
            .filter(|l| l.active())
            .cloned()
        else {
            return Ok(());
        };
        let next = match lp.kind {
            LoopKind::Interval => next_tick(&lp, now),
            LoopKind::SelfPaced => None,
        };
        let ended = records::IterationEnded {
            loop_id: lp.loop_id.clone(),
            n: iteration.n,
            outcome: Outcome::Failed,
            noop: false,
            duration_ms: 0,
            cost: 0.0,
            tokens: 0,
            turns: 0,
            next_due_ms: next,
            reason: Some(error.to_string()),
        };
        let mut events = vec![event(records::ITERATION_ENDED, &ended)];
        if lp.failures + 1 >= MAX_FAILURES || next.is_none() {
            events.push(stopped(&lp.loop_id, StopStatus::Failed, error.to_string()));
        }
        self.inner.commit_locked(&mut state, events).map(drop)
    }

    async fn commit_loop(
        &self,
        session_id: &str,
        loop_id: &str,
        events: Vec<NewEvent>,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        if state.loops.get(loop_id).is_some_and(LoopState::active) {
            self.inner.commit_locked(&mut state, events)?;
        }
        Ok(())
    }

    /// The Session's Drain ended (`drain::run`): end every iteration it ran, then admit
    /// what is due now (a coalesced tick, a self-paced wakeup of zero remaining delay).
    pub(crate) async fn loops_drain_ended(&self, session_id: &str, status: Option<&str>) {
        if self.is_shutting_down() {
            return;
        }
        let id = session_id.to_owned();
        match self
            .inner
            .store
            .read(move |db| Ok(records::has_active(db, &id)?))
        {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => return warn("reading loops failed", &RuntimeError::Store(error)),
        }
        if let Err(error) = self.end_iterations(session_id, status, now_ms()).await {
            warn("ending a loop iteration failed", &error);
        }
        self.nudge_loops();
    }

    async fn end_iterations(
        &self,
        session_id: &str,
        status: Option<&str>,
        now: i64,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        // A Turn that failed or was interrupted ends the Drain normally; its last run says so.
        let status = status.map(str::to_owned).or_else(|| {
            let (last, _) = crate::runtime::messaging::watch::last_run(&state);
            (last.starts_with("failed") || last == crate::runtime::INTERRUPTED).then_some(last)
        });
        let status = status.as_deref();
        let mut events = Vec::new();
        let ended: Vec<LoopState> = state
            .loops
            .values()
            .filter(|l| l.active())
            .filter(|l| {
                l.inflight
                    .as_ref()
                    .is_some_and(|i| promoted(&state, &i.message_id))
            })
            .cloned()
            .collect();
        for lp in &ended {
            let coalesced = state.loop_coalesced.remove(&lp.loop_id);
            events.extend(iteration_end(&state, lp, status, coalesced, now));
        }
        if !events.is_empty() {
            self.inner.commit_locked(&mut state, events)?;
        }
        Ok(())
    }

    /// Run the sweep soon, without waiting for its next period.
    pub(crate) fn nudge_loops(&self) {
        let weak = self.downgrade();
        tokio::spawn(async move {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            if let Err(error) = runtime.sweep_loops(now_ms()).await {
                warn("loop sweep failed", &error);
            }
        });
    }

    /// Restore loops after a restart (`loops-scheduling` → Restore on resume); must run
    /// during application construction, before [`Self::spawn_loop_sweep`]. Returns the
    /// number of loops it changed.
    pub async fn recover_loops(&self) -> Result<usize, RuntimeError> {
        self.recover_loops_at(now_ms()).await
    }

    /// [`Self::recover_loops`] at a chosen time.
    pub async fn recover_loops_at(&self, now: i64) -> Result<usize, RuntimeError> {
        let sessions = self
            .inner
            .store
            .read(|db| Ok(records::active_sessions(db)?))?;
        let mut changed = 0;
        for session_id in sessions {
            let handle = self.inner.handle(&session_id).await?;
            let mut state = handle.state.lock().await;
            let running = self.is_running(&session_id);
            let mut events = Vec::new();
            for lp in state.loops.values().filter(|l| l.active()) {
                events.extend(restored(&state, lp, running, now));
            }
            if !events.is_empty() {
                changed += events.len();
                self.inner.commit_locked(&mut state, events)?;
            }
        }
        Ok(changed)
    }

    /// Run the sweep every [`SWEEP_EVERY`] until shutdown.
    pub fn spawn_loop_sweep(&self) {
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
                if let Err(error) = runtime.sweep_loops(now_ms()).await {
                    warn("loop sweep failed", &error);
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

    /// Admit an iteration like a typed prompt (templates, `@` mentions, UserPromptSubmit
    /// with `source: "loop"`) with its `loop.iteration.started` record in the same commit.
    async fn admit_loop_iteration(
        &self,
        session_id: &str,
        admission: Admission,
        extra: Vec<NewEvent>,
    ) -> Result<crate::runtime::Receipt, RuntimeError> {
        self.admit_attempt_with(
            session_id,
            admission,
            None,
            crate::runtime::PromptCheck::Run,
            extra,
        )
        .await
    }
}

/// The deterministic inbox ID of iteration `n`.
pub(crate) fn iteration_id(loop_id: &str, n: u64) -> String {
    format!("loopi_{}_{n}", loop_id.trim_start_matches("lop_"))
}

fn promoted(state: &SessionState, message_id: &str) -> bool {
    state
        .inbox
        .iter()
        .any(|row| row.message_id == message_id && row.status == InputStatus::Promoted)
}

fn tail(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(UNTIL_TAIL)..]
        .iter()
        .collect()
}

/// The events that end `lp`'s in-flight iteration after a Drain with `status`.
fn iteration_end(
    state: &SessionState,
    lp: &LoopState,
    status: Option<&str>,
    coalesced: bool,
    now: i64,
) -> Vec<NewEvent> {
    let Some(inflight) = &lp.inflight else {
        return Vec::new();
    };
    let outcome = match status {
        Some(s) if s.starts_with("failed") => Outcome::Failed,
        Some(_) => Outcome::Interrupted,
        None => Outcome::Completed,
    };
    let wakeup = lp.wakeup.as_ref().filter(|w| w.n == inflight.n);
    let next = match lp.kind {
        LoopKind::Interval if coalesced => Some(now),
        LoopKind::Interval => lp.next_due_ms.or_else(|| next_tick(lp, now)),
        LoopKind::SelfPaced => {
            wakeup.map(|w| now + i64::try_from(w.delay_seconds).unwrap_or(3600) * 1000)
        }
    };
    let (cost, tokens, turns) = totals(state);
    let ended = records::IterationEnded {
        loop_id: lp.loop_id.clone(),
        n: inflight.n,
        outcome,
        noop: wakeup.is_some_and(|w| w.noop),
        duration_ms: now - inflight.started_ms,
        cost: (cost - inflight.cost_before).max(0.0),
        tokens: tokens.saturating_sub(inflight.tokens_before),
        turns: turns.saturating_sub(inflight.turns_before),
        next_due_ms: next,
        reason: status
            .map(str::to_owned)
            .or_else(|| wakeup.map(|w| w.reason.clone())),
    };
    let mut after = lp.clone();
    after.iterations = inflight.n;
    after.cost_usd += ended.cost;
    after.tokens += ended.tokens;
    after.turns += ended.turns;
    let failures = if outcome == Outcome::Failed {
        lp.failures + 1
    } else {
        0
    };
    let mut events = vec![event(records::ITERATION_ENDED, &ended)];
    let stop = if failures >= MAX_FAILURES {
        Some((StopStatus::Failed, status.unwrap_or("failed").to_owned()))
    } else if lp.kind == LoopKind::SelfPaced && wakeup.is_none() {
        Some((
            StopStatus::Completed,
            "the iteration ended without schedule_wakeup".to_owned(),
        ))
    } else {
        stop_reason(&after, now)
    };
    if let Some((status, reason)) = stop {
        events.push(stopped(&lp.loop_id, status, reason));
    }
    events
}

/// What restoring `lp` after a restart records.
fn restored(state: &SessionState, lp: &LoopState, running: bool, now: i64) -> Vec<NewEvent> {
    if let Some(inflight) = &lp.inflight {
        if running || !promoted(state, &inflight.message_id) {
            // It still runs, or waits in the inbox and runs on the next Drain.
            return Vec::new();
        }
        let reason = "interrupted by a server restart";
        let woken = lp.wakeup.as_ref().is_some_and(|w| w.n == inflight.n);
        if lp.kind == LoopKind::SelfPaced && !woken {
            // Cut off before it could pace itself: it runs once more instead of completing.
            return vec![interrupted_retry(lp, inflight, reason, now)];
        }
        return iteration_end(state, lp, Some(reason), false, now);
    }
    match lp.next_due_ms {
        Some(due) if lp.kind == LoopKind::Interval && lp.missed == Missed::Skip && due < now => {
            vec![skipped(lp, Outcome::SkippedMissed, next_tick(lp, now))]
        }
        // `run-once`, and a self-paced wakeup that passed: the sweep runs it once.
        _ => Vec::new(),
    }
}

fn interrupted_retry(
    lp: &LoopState,
    inflight: &records::Inflight,
    reason: &str,
    now: i64,
) -> NewEvent {
    let ended = records::IterationEnded {
        loop_id: lp.loop_id.clone(),
        n: inflight.n,
        outcome: Outcome::Interrupted,
        noop: false,
        duration_ms: now - inflight.started_ms,
        cost: 0.0,
        tokens: 0,
        turns: 0,
        next_due_ms: Some(now),
        reason: Some(reason.to_owned()),
    };
    event(records::ITERATION_ENDED, &ended)
}

#[cfg(test)]
mod tests {
    use cyber_core::budget::Budget;

    use super::*;

    fn interval(created_ms: i64, seconds: u64) -> LoopState {
        let mut state = records::tests::state(LoopKind::Interval);
        state.created_ms = created_ms;
        state.interval_seconds = Some(seconds);
        state
    }

    #[test]
    fn ticks_align_to_creation_and_skip_late_ones() {
        let lp = interval(1_000, 300);
        assert_eq!(next_tick(&lp, 1_000), Some(301_000));
        assert_eq!(next_tick(&lp, 300_999), Some(301_000));
        assert_eq!(next_tick(&lp, 301_000), Some(601_000));
        // An iteration that ran past two ticks resumes at the next one, not the missed ones.
        assert_eq!(next_tick(&lp, 950_000), Some(1_201_000));
        let paced = records::tests::state(LoopKind::SelfPaced);
        assert_eq!(next_tick(&paced, 5_000), None);
    }

    #[test]
    fn stop_reasons_in_order() {
        let mut lp = interval(0, 60);
        lp.iteration_cap = 500;
        assert_eq!(stop_reason(&lp, 1_000), None);
        lp.stop.max_iterations = Some(3);
        lp.iterations = 3;
        assert_eq!(
            stop_reason(&lp, 1_000).unwrap().0,
            StopStatus::MaxIterations
        );
        lp.iterations = 1;
        lp.stop.for_seconds = Some(120);
        assert_eq!(stop_reason(&lp, 119_999), None);
        assert_eq!(stop_reason(&lp, 120_000).unwrap().0, StopStatus::Duration);
        lp.stop.for_seconds = None;
        lp.budget = Some(Budget {
            max_cost_usd: Some(2.0),
            ..Budget::default()
        });
        lp.cost_usd = 2.01;
        let (status, reason) = stop_reason(&lp, 1_000).unwrap();
        assert_eq!(status, StopStatus::BudgetExceeded);
        assert_eq!(reason, "max_cost_usd 2.01/2.00");
    }

    #[test]
    fn iteration_ids_are_deterministic() {
        assert_eq!(iteration_id("lop_01ABC", 3), "loopi_01ABC_3");
    }
}
