//! Usage limit waits (`session-runtime` → Usage limit wait, Usage limit wait recovery):
//! the wait events and their fold, what the decision reads (the Session's context and
//! wall deadline), the wait itself, the scheduled resume and the startup recovery.

use std::sync::Arc;
use std::time::Duration;

use cyber_core::usage_limit::{self, NoWait, UsageLimitSettings, WaitFacts, WaitMode, WaitPlan};
use cyber_llm::LlmError;
use cyber_store::{EventRegistry, NewEvent, StoredEvent};
use futures::future::BoxFuture;
use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::bus::LiveEvent;
use super::events::event;
use super::model::SessionState;
use super::{Handle, Inner, Runtime, RuntimeError};

pub const WAITING: &str = "session.usage_limit.waiting.1";
pub const RESUMED: &str = "session.usage_limit.resumed.1";
pub const ABANDONED: &str = "session.usage_limit.abandoned.1";
const PREFIX: &str = "session.usage_limit.";
/// How far up the parent chain a subagent looks for an unattended owner.
const MAX_OWNER_DEPTH: usize = 32;

/// `session.usage_limit.waiting.1`, and the pending wait of the Session view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UsageLimitWait {
    /// The failed step whose Turn runs again.
    pub message_id: String,
    pub resets_at_ms: i64,
    pub resume_at_ms: i64,
    pub started_ms: i64,
    pub by: WaitedBy,
}

/// Who planned a wait: the runtime for an unattended Session, or the user's
/// `/resume-at-reset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitedBy {
    Runtime,
    User,
}

/// `session.usage_limit.resumed.1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Resumed {
    message_id: String,
    resumed_ms: i64,
}

/// `session.usage_limit.abandoned.1`: `interrupted`, `restarted` or `deadline`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Abandoned {
    message_id: String,
    reason: String,
    abandoned_ms: i64,
}

/// The Session's waits, folded from its events.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct UsageWaitState {
    pub(crate) pending: Option<UsageLimitWait>,
    /// When the first wait of the current streak started; `max_wait` counts from it. A
    /// finished step or an abandoned wait ends the streak.
    pub(crate) since_ms: Option<i64>,
    /// The last finished step, when it failed on a usage limit with a known reset.
    pub(crate) last_limit: Option<(String, i64)>,
}

impl UsageWaitState {
    /// `session.step.failed.1`: a usage-limit failure with a known reset can be resumed
    /// and keeps the streak; any other failure ends it.
    pub(crate) fn step_failed(&mut self, data: &serde_json::Value) {
        let quota = data["kind"] == "quota_exceeded";
        self.last_limit = data["resets_at_ms"]
            .as_i64()
            .filter(|_| quota)
            .zip(data["message_id"].as_str())
            .map(|(resets, id)| (id.to_owned(), resets));
        if !quota {
            self.since_ms = None;
        }
    }

    /// `session.step.ended.1`: the Turn got an answer, so the streak ends.
    pub(crate) fn step_ended(&mut self) {
        self.last_limit = None;
        self.since_ms = None;
    }
}

pub(super) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(WAITING, |v| decode::<UsageLimitWait>(v).map(drop))
        .and_then(|r| r.register_with(RESUMED, |v| decode::<Resumed>(v).map(drop)))
        .and_then(|r| r.register_with(ABANDONED, |v| decode::<Abandoned>(v).map(drop)))
        .expect("valid usage limit events");
}

fn decode<T: serde::de::DeserializeOwned>(value: &serde_json::Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|e| e.to_string())
}

/// Fold a `session.usage_limit.*` event into the Session state.
pub(crate) fn fold(state: &mut UsageWaitState, e: &StoredEvent) -> Result<(), String> {
    match e.kind.as_str() {
        WAITING => {
            let wait: UsageLimitWait = decode(&e.data)?;
            state.since_ms.get_or_insert(wait.started_ms);
            state.last_limit = None;
            state.pending = Some(wait);
        }
        RESUMED => state.pending = None,
        ABANDONED => {
            state.pending = None;
            state.since_ms = None;
        }
        _ => {}
    }
    Ok(())
}

/// Project pending waits into `session_usage_wait`, which startup recovery reads.
pub(super) fn project(tx: &Transaction<'_>, e: &StoredEvent) -> Result<(), String> {
    if !e.kind.starts_with(PREFIX) {
        return Ok(());
    }
    let result = if e.kind == WAITING {
        let wait: UsageLimitWait = decode(&e.data)?;
        tx.execute(
            "INSERT INTO session_usage_wait (session_id, message_id, resume_at_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT (session_id) DO UPDATE SET message_id = excluded.message_id,
             resume_at_ms = excluded.resume_at_ms",
            params![e.aggregate_id, wait.message_id, wait.resume_at_ms],
        )
    } else {
        tx.execute(
            "DELETE FROM session_usage_wait WHERE session_id = ?1",
            params![e.aggregate_id],
        )
    };
    result.map(drop).map_err(|error| error.to_string())
}

/// The time a wait reads and sleeps on; tests replace it (`Runtime::configure_wait_clock`).
pub trait WaitClock: Send + Sync {
    fn now_ms(&self) -> i64;
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()>;
}

/// The host clock. Debug builds divide every wait by `CYBER_TEST_USAGE_WAIT_SCALE`, so
/// a real-binary test waits a 40-minute reset in milliseconds.
pub struct SystemClock {
    scale: u32,
}

impl SystemClock {
    pub fn from_env() -> Self {
        #[cfg(debug_assertions)]
        let scale = std::env::var("CYBER_TEST_USAGE_WAIT_SCALE")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|scale| *scale > 0)
            .unwrap_or(1);
        #[cfg(not(debug_assertions))]
        let scale = 1;
        Self { scale }
    }
}

impl WaitClock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration / self.scale))
    }
}

/// What `fail_step` does with a `QuotaExceeded` failure before output.
pub(super) enum LimitOutcome {
    /// Not a usage limit.
    None,
    Wait(UsageLimitWait),
    Fail {
        note: Option<String>,
        /// The deadline that ended the run, when it decided.
        deadline: Option<Deadline>,
    },
}

impl LimitOutcome {
    pub(super) fn note(&self) -> Option<&str> {
        match self {
            Self::Fail { note, .. } => note.as_deref(),
            _ => None,
        }
    }

    pub(super) fn waiting_event(&self) -> Option<NewEvent> {
        match self {
            Self::Wait(wait) => Some(event(WAITING, wait)),
            _ => None,
        }
    }
}

/// The earliest wall deadline of a Session and whose it is.
#[derive(Debug, Clone)]
pub(super) struct Deadline {
    at_ms: i64,
    owner: DeadlineOwner,
}

#[derive(Debug, Clone)]
enum DeadlineOwner {
    /// A Session or Workflow Run budget scope.
    Budget(String),
    Goal(String),
    Loop(String),
}

/// How a wait ended.
pub(super) enum WaitEnd {
    Resumed,
    /// The Drain was interrupted; the wait is abandoned.
    Interrupted,
    /// The wait would now pass a deadline, whose outcome ended the run.
    Ended,
    /// The runtime is shutting down; the wait stays pending for the restart.
    Shutdown,
}

impl Inner {
    pub(super) fn wait_clock(&self) -> Arc<dyn WaitClock> {
        Arc::clone(
            self.wait_clock
                .get_or_init(|| Arc::new(SystemClock::from_env())),
        )
    }

    /// Decide a usage-limit failure of the step `message_id`.
    pub(super) async fn usage_limit_outcome(
        &self,
        handle: &Handle,
        message_id: &str,
        e: &LlmError,
    ) -> Result<LimitOutcome, RuntimeError> {
        let (session_id, directory, since_ms) = {
            let state = handle.state.lock().await;
            let info = &state.info;
            (
                info.id.clone(),
                info.directory.clone(),
                state.usage_wait.since_ms,
            )
        };
        let settings = self
            .tools
            .usage_limit_settings(&directory)
            .map_err(RuntimeError::Invalid)?;
        let facts = WaitFacts {
            unattended: self.unattended(handle).await,
            resets_at_ms: e.resets_at.map(|seconds| seconds.saturating_mul(1000)),
            since_ms,
            ..WaitFacts::default()
        };
        self.outcome(
            handle,
            &session_id,
            message_id,
            &settings,
            facts,
            WaitedBy::Runtime,
        )
        .await
    }

    /// Plan with `facts` completed by the clock, the deadline and the jitter.
    async fn outcome(
        &self,
        handle: &Handle,
        session_id: &str,
        message_id: &str,
        settings: &UsageLimitSettings,
        mut facts: WaitFacts,
        by: WaitedBy,
    ) -> Result<LimitOutcome, RuntimeError> {
        facts.now_ms = self.wait_clock().now_ms();
        let deadline = self.wall_deadline(handle).await?;
        facts.deadline_ms = deadline.as_ref().map(|d| d.at_ms);
        facts.jitter_ms = facts
            .resets_at_ms
            .map_or(0, |resets| usage_limit::jitter_ms(session_id, resets));
        Ok(match usage_limit::plan(settings, &facts) {
            WaitPlan::Wait { resume_at_ms } => LimitOutcome::Wait(UsageLimitWait {
                message_id: message_id.to_owned(),
                resets_at_ms: facts.resets_at_ms.unwrap_or(facts.now_ms),
                resume_at_ms,
                started_ms: facts.now_ms,
                by,
            }),
            WaitPlan::Fail(reason) => LimitOutcome::Fail {
                note: reason.note(),
                deadline: deadline.filter(|_| matches!(reason, NoWait::PastDeadline { .. })),
            },
        })
    }

    /// Whether the Session has no interactive owner (design → Who is unattended): a
    /// non-interactive server, or a marker on the Session or a parent.
    pub(super) async fn unattended(&self, handle: &Handle) -> bool {
        if !self.options.interactive {
            return true;
        }
        let mut parent = {
            let state = handle.state.lock().await;
            if unattended_marker(&state) {
                return true;
            }
            state.info.parent_id.clone()
        };
        for _ in 0..MAX_OWNER_DEPTH {
            let Some(id) = parent else {
                return false;
            };
            let Ok(owner) = self.handle(&id).await else {
                return false;
            };
            let state = owner.state.lock().await;
            if unattended_marker(&state) {
                return true;
            }
            parent = state.info.parent_id.clone();
        }
        false
    }

    /// The earliest wall deadline: Session, ancestor and Workflow Run budgets, the active
    /// Goal, and loops with an iteration in flight.
    async fn wall_deadline(&self, handle: &Handle) -> Result<Option<Deadline>, RuntimeError> {
        let (session_id, mut deadlines) = {
            let state = handle.state.lock().await;
            (state.info.id.clone(), state_deadlines(&state))
        };
        let now = chrono::Utc::now().timestamp_millis();
        let budget = super::budget::wall_deadline(&self.store, &session_id, now)?;
        deadlines.extend(budget.map(|(scope, at_ms)| Deadline {
            at_ms,
            owner: DeadlineOwner::Budget(scope),
        }));
        Ok(deadlines.into_iter().min_by_key(|d| d.at_ms))
    }

    /// End the run with the outcome of the deadline the wait would pass: a budget scope
    /// fails the Drain, a Goal or loop ends `budget_exceeded`.
    pub(super) async fn deadline_reached(
        &self,
        handle: &Arc<Handle>,
        deadline: Option<&Deadline>,
    ) -> Result<(), RuntimeError> {
        let Some(deadline) = deadline else {
            return Ok(());
        };
        let reason = "max_wall_seconds: the usage limit resets after the wall deadline";
        match &deadline.owner {
            DeadlineOwner::Budget(scope) => Err(RuntimeError::BudgetExceeded {
                scope: scope.clone(),
                limit: "max_wall_seconds".into(),
            }),
            DeadlineOwner::Goal(goal_id) => {
                let runtime = Runtime {
                    inner: self.me.upgrade().ok_or(RuntimeError::ShuttingDown)?,
                };
                runtime.goal_wall_deadline(handle, goal_id, reason).await
            }
            DeadlineOwner::Loop(loop_id) => {
                let stopped = super::loops::records::Stopped {
                    loop_id: loop_id.clone(),
                    status: super::loops::records::StopStatus::BudgetExceeded,
                    reason: reason.into(),
                };
                let stop = event(super::loops::records::STOPPED, &stopped);
                self.commit(handle, vec![stop]).await.map(drop)
            }
        }
    }

    /// Wait for the Session's pending usage limit reset; `None` when none is pending. A
    /// wait now past a deadline is abandoned and ends with that deadline's outcome.
    pub(super) async fn await_usage_reset(
        &self,
        handle: &Arc<Handle>,
        cancel: &CancellationToken,
    ) -> Result<Option<WaitEnd>, RuntimeError> {
        let Some(wait) = handle.state.lock().await.usage_wait.pending.clone() else {
            return Ok(None);
        };
        if let Some(deadline) = self
            .wall_deadline(handle)
            .await?
            .filter(|d| wait.resume_at_ms >= d.at_ms)
        {
            self.abandon(handle, &wait, "deadline").await?;
            self.deadline_reached(handle, Some(&deadline)).await?;
            let message = NoWait::PastDeadline {
                resume_at_ms: wait.resume_at_ms,
                deadline_ms: deadline.at_ms,
            }
            .note()
            .unwrap_or_default();
            self.bus.publish(LiveEvent::Error {
                session_id: handle.state.lock().await.info.id.clone(),
                kind: "quota_exceeded".into(),
                message: format!("usage limit reached ({message})"),
            });
            return Ok(Some(WaitEnd::Ended));
        }
        let clock = self.wait_clock();
        let left = u64::try_from(wait.resume_at_ms - clock.now_ms()).unwrap_or(0);
        tokio::select! {
            biased;
            () = cancel.cancelled() => {}
            () = clock.sleep(Duration::from_millis(left)) => {
                let resumed = Resumed { message_id: wait.message_id, resumed_ms: clock.now_ms() };
                self.commit(handle, vec![event(RESUMED, &resumed)]).await?;
                return Ok(Some(WaitEnd::Resumed));
            }
        }
        if self.closed.is_cancelled() {
            return Ok(Some(WaitEnd::Shutdown));
        }
        self.abandon(handle, &wait, "interrupted").await?;
        Ok(Some(WaitEnd::Interrupted))
    }

    async fn abandon(
        &self,
        handle: &Handle,
        wait: &UsageLimitWait,
        reason: &str,
    ) -> Result<(), RuntimeError> {
        let abandoned = Abandoned {
            message_id: wait.message_id.clone(),
            reason: reason.into(),
            abandoned_ms: self.wait_clock().now_ms(),
        };
        self.commit(handle, vec![event(ABANDONED, &abandoned)])
            .await
            .map(drop)
    }
}

/// A Session that runs with nobody attached: created by `cyber exec`, a Workflow agent,
/// an active Goal, a loop iteration in flight, or a teammate.
fn unattended_marker(state: &SessionState) -> bool {
    let info = &state.info;
    info.unattended
        || info.workflow_run_id.is_some()
        || state.goals.active().is_some()
        || state
            .loops
            .values()
            .any(|l| l.active() && l.inflight.is_some())
        || info
            .team
            .as_ref()
            .is_some_and(|team| team.role != super::teams::Role::Lead)
}

/// The Goal's and in-flight loops' wall deadlines.
fn state_deadlines(state: &SessionState) -> Vec<Deadline> {
    let at = |start: i64, seconds: f64| start.saturating_add((seconds * 1000.0) as i64);
    let goal = state.goals.active().and_then(|goal| {
        let wall = goal.budget.max_wall_seconds?;
        Some(Deadline {
            at_ms: at(goal.activated_ms.unwrap_or(goal.created_ms), wall),
            owner: DeadlineOwner::Goal(goal.goal_id.clone()),
        })
    });
    let loops = state
        .loops
        .values()
        .filter(|l| l.active() && l.inflight.is_some())
        .filter_map(|l| {
            let wall = l.budget.as_ref()?.max_wall_seconds?;
            Some(Deadline {
                at_ms: at(l.created_ms, wall),
                owner: DeadlineOwner::Loop(l.loop_id.clone()),
            })
        });
    goal.into_iter().chain(loops).collect()
}

impl Runtime {
    /// Replace the clock usage limit waits read and sleep on (tests).
    pub fn configure_wait_clock(&self, clock: Arc<dyn WaitClock>) {
        let _ = self.inner.wait_clock.set(clock);
    }

    /// `/resume-at-reset`: wait for the usage limit that failed the idle Session's last
    /// Turn, then run that Turn again. The mode does not apply (the user asked);
    /// `max_wait` and the wall deadlines do.
    pub async fn resume_at_reset(&self, session_id: &str) -> Result<UsageLimitWait, RuntimeError> {
        self.inner.ensure_admission_open(session_id)?;
        if self.is_running(session_id) {
            return Err(RuntimeError::Busy(session_id.into()));
        }
        let handle = self.inner.handle(session_id).await?;
        let (last, directory) = {
            let state = handle.state.lock().await;
            (
                state.usage_wait.last_limit.clone(),
                state.info.directory.clone(),
            )
        };
        let Some((message_id, resets_at_ms)) = last else {
            return Err(RuntimeError::Invalid(
                "nothing to resume: the last Turn did not fail on a usage limit with a known reset time"
                    .into(),
            ));
        };
        let settings = UsageLimitSettings {
            mode: WaitMode::Always,
            ..self
                .inner
                .tools
                .usage_limit_settings(&directory)
                .map_err(RuntimeError::Invalid)?
        };
        let facts = WaitFacts {
            unattended: true,
            resets_at_ms: Some(resets_at_ms),
            ..WaitFacts::default()
        };
        let outcome = self
            .inner
            .outcome(
                &handle,
                session_id,
                &message_id,
                &settings,
                facts,
                WaitedBy::User,
            )
            .await?;
        let LimitOutcome::Wait(wait) = outcome else {
            let note = outcome.note().unwrap_or("not waiting").to_owned();
            return Err(RuntimeError::Invalid(note));
        };
        self.inner
            .commit(&handle, vec![event(WAITING, &wait)])
            .await?;
        self.resume(session_id).await?;
        Ok(wait)
    }

    /// Startup: resume the pending waits of top-level Sessions, and abandon those of
    /// subagents, Workflow agents and teammates, whose owners recover them. The number
    /// of Sessions resumed.
    pub async fn recover_usage_limit_waits(&self) -> Result<usize, RuntimeError> {
        let sessions = self.inner.store.read(|db| {
            let mut statement =
                db.prepare("SELECT session_id FROM session_usage_wait ORDER BY session_id")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })?;
        let mut resumed = 0;
        for session_id in sessions {
            if self.is_running(&session_id) {
                continue;
            }
            match self.recover_wait(&session_id).await {
                Ok(true) => resumed += 1,
                Ok(false) => {}
                Err(error) => cyber_core::log::warn(
                    "usage_limit",
                    "usage limit wait recovery failed",
                    serde_json::json!({"session_id": session_id, "error": error.to_string()}),
                ),
            }
        }
        Ok(resumed)
    }

    async fn recover_wait(&self, session_id: &str) -> Result<bool, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let (wait, owned) = {
            let state = handle.state.lock().await;
            let info = &state.info;
            let owned =
                info.parent_id.is_some() || info.workflow_run_id.is_some() || info.team.is_some();
            (state.usage_wait.pending.clone(), owned)
        };
        let Some(wait) = wait else {
            return Ok(false);
        };
        if !owned {
            match self.resume(session_id).await {
                Ok(()) => return Ok(true),
                Err(error) => cyber_core::log::warn(
                    "usage_limit",
                    "could not resume a usage limit wait; abandoning it",
                    serde_json::json!({"session_id": session_id, "error": error.to_string()}),
                ),
            }
        }
        self.inner.abandon(&handle, &wait, "restarted").await?;
        self.inner.bus.publish(LiveEvent::Error {
            session_id: session_id.into(),
            kind: "quota_exceeded".into(),
            message: "usage limit reached; the wait for its reset ended with a server restart"
                .into(),
        });
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(kind: &str, data: serde_json::Value) -> StoredEvent {
        StoredEvent {
            id: "evt_1".into(),
            aggregate_id: "ses_1".into(),
            seq: 1,
            kind: kind.into(),
            data,
            time_ms: 1,
            causation_id: None,
        }
    }

    fn waiting(started_ms: i64) -> serde_json::Value {
        serde_json::json!({"message_id": "msg_1", "resets_at_ms": 10, "resume_at_ms": 20,
            "started_ms": started_ms, "by": "runtime"})
    }

    /// The streak start survives a resume and ends with a finished step or an abandon.
    #[test]
    fn fold_tracks_the_pending_wait_and_the_streak() {
        let failed = |id: &str, kind: &str, resets: Option<i64>| serde_json::json!({"message_id": id, "kind": kind, "resets_at_ms": resets});
        let mut state = UsageWaitState::default();
        state.step_failed(&failed("msg_1", "quota_exceeded", Some(10)));
        assert_eq!(state.last_limit, Some(("msg_1".into(), 10)));
        fold(&mut state, &stored(WAITING, waiting(5))).unwrap();
        assert_eq!(state.pending.as_ref().map(|w| w.resume_at_ms), Some(20));
        assert_eq!((state.since_ms, state.last_limit.clone()), (Some(5), None));
        let resumed = serde_json::json!({"message_id": "msg_1", "resumed_ms": 21});
        fold(&mut state, &stored(RESUMED, resumed)).unwrap();
        assert!(state.pending.is_none());
        fold(&mut state, &stored(WAITING, waiting(30))).unwrap();
        assert_eq!(
            state.since_ms,
            Some(5),
            "a second wait keeps the streak start"
        );
        let abandoned =
            serde_json::json!({"message_id": "msg_1", "reason": "interrupted", "abandoned_ms": 31});
        fold(&mut state, &stored(ABANDONED, abandoned)).unwrap();
        assert_eq!((state.pending.is_none(), state.since_ms), (true, None));
        fold(&mut state, &stored(WAITING, waiting(40))).unwrap();
        state.step_failed(&failed("msg_2", "quota_exceeded", Some(50)));
        assert_eq!(state.since_ms, Some(40), "a second limit keeps the streak");
        state.step_ended();
        assert_eq!(state.since_ms, None, "a finished step ends the streak");
        state.step_failed(&failed("msg_3", "quota_exceeded", None));
        assert_eq!(state.last_limit, None, "an unknown reset cannot be resumed");
        fold(&mut state, &stored(WAITING, waiting(60))).unwrap();
        state.step_failed(&failed("msg_4", "authentication", None));
        assert_eq!(state.since_ms, None, "another failure ends the streak");
    }

    fn state(extra: serde_json::Value) -> SessionState {
        let mut info = serde_json::json!({
            "id": "ses_1", "title": "t", "directory": "/repo", "parent_id": null,
            "agent": "build", "model": "test/main", "mode": "default", "created_ms": 0,
        });
        info.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        SessionState::new(serde_json::from_value(info).unwrap())
    }

    /// Design → Who is unattended: the Session's own markers (a parent's are walked by
    /// `Inner::unattended`).
    #[test]
    fn unattended_markers() {
        let team = |role: &str| {
            serde_json::json!({"team": {"team_id": "team_a", "role": role,
                "name": if role == "lead" { "lead" } else { "ana" }}})
        };
        assert!(!unattended_marker(&state(serde_json::json!({}))));
        assert!(unattended_marker(&state(
            serde_json::json!({"unattended": true})
        )));
        assert!(unattended_marker(&state(
            serde_json::json!({"workflow_run_id": "run_1"})
        )));
        assert!(unattended_marker(&state(team("teammate"))));
        assert!(!unattended_marker(&state(team("lead"))));
    }

    #[test]
    fn a_malformed_wait_is_refused() {
        let mut state = UsageWaitState::default();
        let missing = serde_json::json!({"message_id": "msg_1"});
        assert!(fold(&mut state, &stored(WAITING, missing)).is_err());
    }
}
