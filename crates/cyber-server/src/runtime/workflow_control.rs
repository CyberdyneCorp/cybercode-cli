//! Run lifecycle control (`workflows` → Run lifecycle states, Background execution on the
//! server): pause, stop, resume, shutdown and startup recovery.
//!
//! Each executing run has a process-local [`Control`], like a Job's. Durable state is the
//! `run_` aggregate: pause and stop are recorded at once, then the engine winds down; a
//! paused run lets in-flight agents finish and starts no new ones, a stopped run cancels
//! them (and stops their subtrees). Resume relaunches the script through replay and is
//! refused while the previous attempt still owns an engine thread or an agent task, so a
//! child is never driven by two attempts. Shutdown durably pauses running runs before the
//! tool host closes; startup recovery is in `workflow_recovery`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cyber_workflow::{Diagnostic, DiagnosticKind, Limits, WorkflowModule};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::workflow_queue::{Admission, Pending, RunSlot};
use super::workflow_replay::Replay;
use super::workflows::{
    Engine, RUN_DISPATCHED, RUN_FAILED, RUN_PAUSED, RUN_RESUMED, RUN_REVISED, RUN_STOPPED,
    RunShare, WorkflowRun, WorkflowRunStatus, config_digest,
};
use super::{Runtime, RuntimeError};

/// The refusal while an earlier attempt still owns agents.
pub const AGENTS_EXITING: &str = "Run has agents still exiting; retry shortly";
/// How long shutdown waits for engines and agent tasks to wind down.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Signal {
    Running,
    /// Let in-flight agents finish; start none.
    Pausing,
    /// Cancel in-flight agents.
    Stopping,
    /// The server is stopping: cancel and leave unsettled agents for a resume.
    Closing,
}

/// The process-local handle of one executing attempt.
pub(super) struct Control {
    signal: Mutex<Signal>,
    /// Engine threads and agent tasks still alive; an agent task owns its child (permit,
    /// ChildGuard, ownership claim) until it returns.
    leases: AtomicUsize,
    /// Cancels in-flight agents: stop and shutdown.
    pub(super) cancel: CancellationToken,
    /// Signalled when the signal changes or an agent settles.
    pub(super) changed: Notify,
    /// The run slot of the attempt (`workflows` → Run queue), released when the last
    /// lease ends: a paused run whose agents still settle keeps it.
    slot: Mutex<Option<RunSlot>>,
}

/// Keeps a [`Control`] busy until dropped.
pub(super) struct Lease(Arc<Control>);

impl Drop for Lease {
    fn drop(&mut self) {
        if self.0.leases.fetch_sub(1, Ordering::SeqCst) == 1 {
            let slot = self
                .0
                .slot
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            drop(slot);
        }
        self.0.changed.notify_waiters();
    }
}

impl Control {
    fn new(cancel: CancellationToken) -> Self {
        Self {
            signal: Mutex::new(Signal::Running),
            leases: AtomicUsize::new(0),
            cancel,
            changed: Notify::new(),
            slot: Mutex::new(None),
        }
    }

    /// Keep `slot` until the attempt's last lease ends. Called while a lease is held.
    pub(super) fn hold(&self, slot: RunSlot) {
        debug_assert!(self.busy());
        *self.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(slot);
    }

    pub(super) fn signal(&self) -> Signal {
        *self.signal.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Move to `signal` unless a stronger one is set; true when it changed.
    fn set(&self, signal: Signal) -> bool {
        let mut current = self.signal.lock().unwrap_or_else(PoisonError::into_inner);
        let rank = |s: Signal| match s {
            Signal::Running => 0,
            Signal::Pausing => 1,
            Signal::Closing => 2,
            Signal::Stopping => 3,
        };
        if rank(signal) <= rank(*current) {
            return false;
        }
        *current = signal;
        drop(current);
        if matches!(signal, Signal::Stopping | Signal::Closing) {
            self.cancel.cancel();
        }
        self.changed.notify_waiters();
        true
    }

    pub(super) fn engine_lease(self: &Arc<Self>) -> Lease {
        self.lease()
    }

    pub(super) fn lease(self: &Arc<Self>) -> Lease {
        self.leases.fetch_add(1, Ordering::SeqCst);
        Lease(Arc::clone(self))
    }

    fn busy(&self) -> bool {
        self.leases.load(Ordering::SeqCst) > 0
    }
}

/// A relaunch of a paused, failed, stopped or budget-halted run.
pub struct ResumeWorkflowRun {
    pub run_id: String,
    /// The script to execute: the stored inline source or the re-read saved module.
    pub module: WorkflowModule,
    pub limits: Limits,
    pub tiers: BTreeMap<String, Option<String>>,
    /// Fields that override the run budget (for example a raised `max_cost_usd`).
    pub budget: Option<Value>,
    /// Accept a changed script or configuration as a new revision.
    pub revision: bool,
    /// The run's share of the agent slots for this attempt, resolved like a start's; not
    /// part of the configuration digest, so a change needs no revision.
    pub share: RunShare,
}

impl Runtime {
    /// Register the control of a new attempt; refused while an earlier one is alive.
    pub(super) fn claim_workflow_control(
        &self,
        run_id: &str,
    ) -> Result<Arc<Control>, RuntimeError> {
        let mut controls = self
            .inner
            .workflow_controls
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.workflow_closing() {
            return Err(RuntimeError::ShuttingDown);
        }
        controls.retain(|_, control| control.busy());
        if controls.contains_key(run_id) {
            return Err(RuntimeError::Conflict(AGENTS_EXITING.into()));
        }
        let control = Arc::new(Control::new(self.inner.closed.child_token()));
        controls.insert(run_id.to_owned(), Arc::clone(&control));
        Ok(control)
    }

    /// Whether shutdown has started pausing runs (or admission closed): nothing launches or
    /// is dispatched any more.
    pub(super) fn workflow_closing(&self) -> bool {
        self.inner.workflow_closing.load(Ordering::SeqCst) || self.is_shutting_down()
    }

    /// The lifecycle lock of one run (`workflows` → Concurrent runs). Pause, stop, resume
    /// and the record of the script's own end take only their run's lock; shutdown and
    /// recovery take one run's lock at a time, so no task ever holds two.
    pub(super) fn workflow_lock(&self, run_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .inner
            .workflow_locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // A lock nobody holds or waits for is recreated on demand.
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        Arc::clone(locks.entry(run_id.to_owned()).or_default())
    }

    pub(super) fn workflow_control(&self, run_id: &str) -> Option<Arc<Control>> {
        self.inner
            .workflow_controls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(run_id)
            .filter(|control| control.busy())
            .cloned()
    }

    /// Whether an earlier attempt or one of its agents' child Sessions is still alive.
    pub fn workflow_agents_exiting(&self, run_id: &str) -> Result<bool, RuntimeError> {
        if self.workflow_control(run_id).is_some() {
            return Ok(true);
        }
        Ok(self
            .workflow_children(run_id, false)?
            .iter()
            .any(|child| self.is_running(child)))
    }

    /// Child Sessions of the run's agents; with `unsettled`, only of agents still running.
    fn workflow_children(
        &self,
        run_id: &str,
        unsettled: bool,
    ) -> Result<Vec<String>, RuntimeError> {
        let run = run_id.to_owned();
        Ok(self.inner.store.read(move |db| {
            let mut statement = db.prepare(
                "SELECT s.id FROM session s WHERE s.workflow_run_id = ?1 AND (?2 = 0 OR EXISTS(
                    SELECT 1 FROM workflow_agent a WHERE a.run_id = ?1 AND a.id = s.subagent_name AND a.status = 'running'))
                 ORDER BY s.id",
            )?;
            let ids = statement
                .query_map(rusqlite::params![run, unsettled], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })?)
    }

    /// Pause a running run: in-flight agents finish and are recorded, no new agent starts.
    /// A queued run leaves the run queue.
    pub async fn pause_workflow_run(&self, run_id: &str) -> Result<WorkflowRun, RuntimeError> {
        let lock = self.workflow_lock(run_id);
        let _guard = lock.lock().await;
        let mut run = self.existing_workflow_run(run_id)?;
        if run.status == WorkflowRunStatus::Paused {
            return Ok(run);
        }
        if !run.status.is_active() {
            return Err(RuntimeError::Conflict(format!(
                "Run {run_id} is {}; only a running, waiting or queued run can be paused",
                status_name(run.status)
            )));
        }
        run.status = WorkflowRunStatus::Paused;
        run.reason = Some("paused by the user".into());
        run.queued_ms = None;
        run.queue_position = None;
        self.run_log().record(&run, RUN_PAUSED, json!({}))?;
        self.workflow_queue().remove(run_id);
        if let Some(control) = self.workflow_control(run_id) {
            control.set(Signal::Pausing);
            self.workflow_scheduler().suspend(run_id);
        }
        Ok(run)
    }

    /// Stop a run: in-flight agents are cancelled and their subtrees stopped.
    pub async fn stop_workflow_run(&self, run_id: &str) -> Result<WorkflowRun, RuntimeError> {
        // Awaiting the children's stop under this run's lock serializes it with the
        // script's own end, and only with that.
        let lock = self.workflow_lock(run_id);
        let _guard = lock.lock().await;
        let mut run = self.existing_workflow_run(run_id)?;
        if !(run.status.is_active() || run.status == WorkflowRunStatus::Paused) {
            return Err(RuntimeError::Conflict(format!(
                "Run {run_id} is {}; only a running or paused run can be stopped",
                status_name(run.status)
            )));
        }
        let live = self.workflow_control(run_id);
        if let Some(control) = &live {
            control.set(Signal::Stopping);
            self.workflow_scheduler().suspend(run_id);
        }
        let mut stops = Vec::new();
        for child in self.workflow_children(run_id, true)? {
            if live.is_none() && !self.is_running(&child) {
                continue;
            }
            match self.stop_subtree(&child).await {
                Ok(report) => stops.push(serde_json::to_value(report).unwrap_or_default()),
                Err(RuntimeError::SessionNotFound(_)) => {}
                Err(error) => stops.push(json!({"session_id": child, "error": error.to_string()})),
            }
        }
        run.status = WorkflowRunStatus::Stopped;
        run.reason = Some("stopped by the user".into());
        run.ended_ms = Some(chrono::Utc::now().timestamp_millis());
        run.queued_ms = None;
        run.queue_position = None;
        self.run_log()
            .record(&run, RUN_STOPPED, json!({"stops": stops}))?;
        // A queued run leaves the queue without starting an engine.
        self.workflow_queue().remove(run_id);
        self.spawn_run_ended(run.clone());
        Ok(run)
    }

    /// Hooks and the summary of an ended run, off the run's lifecycle lock.
    fn spawn_run_ended(&self, run: WorkflowRun) {
        let runtime = self.clone();
        tokio::spawn(async move { runtime.workflow_run_ended(run).await });
    }

    /// Relaunch a paused, failed, stopped or budget-halted run. Settled results replay
    /// from the store, unsettled agents resume their child Sessions. A changed script or
    /// configuration without `revision` fails closed with `WorkflowReplayMismatch`.
    pub async fn resume_workflow_run(
        &self,
        resume: ResumeWorkflowRun,
    ) -> Result<WorkflowRun, RuntimeError> {
        if self.workflow_closing() {
            return Err(RuntimeError::ShuttingDown);
        }
        self.ensure_workflow_progress();
        let lock = self.workflow_lock(&resume.run_id);
        let _guard = lock.lock().await;
        let mut run = self.existing_workflow_run(&resume.run_id)?;
        if run.status.is_active() || run.status == WorkflowRunStatus::Completed {
            return Err(RuntimeError::Conflict(format!(
                "Run {} is {}; only a paused, failed, stopped or budget-exceeded run can be resumed",
                run.id,
                status_name(run.status)
            )));
        }
        if self.workflow_agents_exiting(&run.id)? {
            return Err(RuntimeError::Conflict(AGENTS_EXITING.into()));
        }
        let configured = config_digest(&resume.limits, &resume.tiers);
        let changed = resume.module.digest != run.digest || configured != run.config_digest;
        if changed && !resume.revision {
            return Err(RuntimeError::Conflict(replay_mismatch(
                &run,
                &resume.module,
                &configured,
            )));
        }
        if let Some(layer) = &resume.budget {
            let budget = super::workflow_host::run_budget(&run)
                .overridden(layer)
                .map_err(RuntimeError::Invalid)?;
            run.budget = Some(budget);
        }
        // A new attempt: its end gets its own result and summary.
        run.share = Some(self.workflow_share(resume.share.max_concurrent));
        run.max_concurrent = resume.share.requested;
        run.reason = None;
        run.error = None;
        run.result = None;
        run.result_file = None;
        run.result_preview = None;
        run.notified = false;
        run.ended_ms = None;
        if changed {
            run.revision += 1;
            run.digest = resume.module.digest.clone();
            run.config_digest = configured;
            let spend = super::budget::run_spend(&self.inner.store, &run.id)?;
            let started = self.workflow_agents(&run.id)?.len() as u64;
            let budget = super::workflow_host::budget_snapshot(
                &super::workflow_host::run_budget(&run),
                &spend,
                started,
            );
            self.run_log().record(
                &run,
                RUN_REVISED,
                json!({"source": resume.module.source, "budget_snapshot": budget}),
            )?;
        }
        let replay = Replay::load(&self.inner.store, &run.id, run.revision)?;
        // Every run slot taken: the run goes back to the end of the run queue.
        let admission = self.admit_workflow_run(&run.id, chrono::Utc::now().timestamp_millis());
        (run.status, run.queued_ms) = match &admission {
            Admission::Slot(_) => (WorkflowRunStatus::Running, None),
            Admission::Queued(place) => (WorkflowRunStatus::Queued, Some(place.queued_ms())),
        };
        self.run_log().record(
            &run,
            RUN_RESUMED,
            json!({"replay_from_order": replay.first_open_order()}),
        )?;
        let engine = Engine {
            module: resume.module,
            limits: resume.limits,
            tiers: resume.tiers,
        };
        let slot = match admission {
            Admission::Slot(slot) => slot,
            Admission::Queued(place) => {
                self.enqueue_workflow_run(place, Pending { engine, replay });
                return Ok(self.workflow_run(&run.id)?.unwrap_or(run));
            }
        };
        if let Err(error) = self.launch(run.clone(), engine, replay, slot).await {
            self.fail_launch(run, &error)?;
            return Err(error);
        }
        Ok(run)
    }

    /// Record an attempt whose engine could not start as failed, and hand it back. One
    /// refused because shutdown began is paused for a resume instead, as the attempts
    /// shutdown paused are. Called under the run's lock.
    pub(super) fn fail_launch(
        &self,
        mut run: WorkflowRun,
        error: &RuntimeError,
    ) -> Result<(), RuntimeError> {
        if matches!(error, RuntimeError::ShuttingDown) {
            self.record_shutdown_pause(&run.id);
            return Ok(());
        }
        run.status = WorkflowRunStatus::Failed;
        run.error = Some(Diagnostic::execution(error.to_string()));
        run.ended_ms = Some(chrono::Utc::now().timestamp_millis());
        self.run_log().record(&run, RUN_FAILED, json!({}))?;
        self.spawn_run_ended(run);
        Ok(())
    }

    /// Dispatch queued runs while run slots are free, oldest first; none once shutdown
    /// began, so they stay `queued` for recovery.
    pub(super) async fn dispatch_workflow_runs(&self) {
        while !self.workflow_closing() {
            let Some((run_id, pending, slot)) = self.workflow_queue().next() else {
                return;
            };
            let slot = self.dispatching_on_release(slot);
            if let Err(error) = self.dispatch_workflow_run(&run_id, pending, slot).await {
                cyber_core::log::log(
                    cyber_core::log::Level::Error,
                    "workflow",
                    "run_dispatch_failed",
                    json!({"run_id": run_id, "error": error.to_string()}),
                );
            }
        }
    }

    /// Launch one queued run. The run is re-read under its lock, so a pause or stop that
    /// came first leaves it unlaunched and frees the slot for the next one, and a shutdown
    /// that began leaves it `queued`.
    async fn dispatch_workflow_run(
        &self,
        run_id: &str,
        pending: Pending,
        slot: RunSlot,
    ) -> Result<(), RuntimeError> {
        let lock = self.workflow_lock(run_id);
        let _guard = lock.lock().await;
        let Some(mut run) = self.workflow_run(run_id)? else {
            return Ok(());
        };
        if run.status != WorkflowRunStatus::Queued || self.workflow_closing() {
            return Ok(());
        }
        run.status = WorkflowRunStatus::Running;
        run.queued_ms = None;
        run.queue_position = None;
        self.run_log().record(&run, RUN_DISPATCHED, json!({}))?;
        if let Err(error) = self
            .launch(run.clone(), pending.engine, pending.replay, slot)
            .await
        {
            self.fail_launch(run, &error)?;
        }
        Ok(())
    }

    /// Shutdown, before admission closes: durably pause every executing run and tell its
    /// engine to leave unsettled agents for a resume, taking each run's lock in turn. No
    /// attempt launches after the snapshot, so a slot freed meanwhile dispatches nothing
    /// and queued runs stay `queued`.
    pub(super) async fn pause_workflow_runs_for_shutdown(&self) {
        let controls: Vec<(String, Arc<Control>)> = {
            let controls = self
                .inner
                .workflow_controls
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            self.inner.workflow_closing.store(true, Ordering::SeqCst);
            // Not only busy ones: an attempt claimed just now takes its first lease later.
            controls
                .iter()
                .map(|(id, control)| (id.clone(), Arc::clone(control)))
                .collect()
        };
        for (id, control) in controls {
            let lock = self.workflow_lock(&id);
            let _guard = lock.lock().await;
            control.set(Signal::Closing);
            self.workflow_scheduler().suspend(&id);
            self.record_shutdown_pause(&id);
        }
    }

    /// The durable pause of a run that was running when the server stopped.
    fn record_shutdown_pause(&self, id: &str) {
        let Ok(Some(mut run)) = self.workflow_run(id) else {
            return;
        };
        if !matches!(
            run.status,
            WorkflowRunStatus::Running | WorkflowRunStatus::WaitingInput
        ) {
            return;
        }
        run.status = WorkflowRunStatus::Paused;
        run.reason = Some("the server stopped; resume to continue".into());
        if let Err(error) = self
            .run_log()
            .record(&run, RUN_PAUSED, json!({"interrupted": true}))
        {
            cyber_core::log::log(
                cyber_core::log::Level::Error,
                "workflow",
                "shutdown_pause_unrecorded",
                json!({"run_id": id, "error": error.to_string()}),
            );
        }
    }

    /// Shutdown, after Drains settled: wait (bounded) until engines and agent tasks end, so
    /// no agent outlives the tool host.
    pub(super) async fn await_workflow_exits(&self) {
        let wait = async {
            loop {
                let busy: Vec<Arc<Control>> = self
                    .inner
                    .workflow_controls
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .values()
                    .filter(|control| control.busy())
                    .cloned()
                    .collect();
                let Some(control) = busy.first() else {
                    return;
                };
                let changed = control.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if control.busy() {
                    let _ = tokio::time::timeout(Duration::from_millis(50), changed).await;
                }
            }
        };
        let _ = tokio::time::timeout(SHUTDOWN_GRACE, wait).await;
    }

    /// Reopen a stopped child's admission before a resume drives it again.
    pub(super) async fn reopen_stopped_child(&self, child: &str) -> Result<(), RuntimeError> {
        let id = child.to_owned();
        let found = self.inner.store.read(move |db| {
            let Some(scope) = super::subtree_reopen::active_scope(db, &id)? else {
                return Ok(None);
            };
            use rusqlite::OptionalExtension;
            let receipt: Option<String> = db
                .query_row(
                    "SELECT id FROM event WHERE aggregate_id=?1 AND type='session.subtree.stopped.1'
                     AND json_extract(data,'$.scope_id')=?2 ORDER BY seq DESC LIMIT 1",
                    rusqlite::params![id, scope],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(Some((scope, receipt)))
        })?;
        match found {
            None => Ok(()),
            Some((scope, Some(receipt))) => self
                .reopen_subtree(child, &scope, &receipt)
                .await
                .map(|_| ()),
            Some((_, None)) => Err(RuntimeError::Conflict(format!(
                "Child {child} admission is closed without a stop receipt; recovery is required"
            ))),
        }
    }
}

/// The refusal of relaunching `run` with `resume` without a new revision, if its script
/// or configuration changed.
pub(super) fn relaunch_mismatch(run: &WorkflowRun, resume: &ResumeWorkflowRun) -> Option<String> {
    let configured = config_digest(&resume.limits, &resume.tiers);
    let changed = resume.module.digest != run.digest || configured != run.config_digest;
    changed.then(|| replay_mismatch(run, &resume.module, &configured))
}

fn replay_mismatch(run: &WorkflowRun, module: &WorkflowModule, configured: &str) -> String {
    let what = if module.digest != run.digest {
        format!("the script changed ({} → {})", run.digest, module.digest)
    } else {
        format!(
            "the configuration changed ({} → {configured})",
            run.config_digest
        )
    };
    let error = Diagnostic::new(
        DiagnosticKind::WorkflowReplayMismatch,
        format!(
            "{what} since revision {}; resume with revision: true to start a new revision",
            run.revision
        ),
    );
    error.to_string()
}

pub(super) fn status_name(status: WorkflowRunStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}
