//! Recovery of several runs (`workflows` → Recovery of several runs).
//!
//! On startup every run a previous process left active is re-read under its own lock, in
//! `started_ms` order. A `running` or `waiting_input` run is durably paused with
//! `interrupted: true`, which marks its unsettled agents `interrupted`. A `queued` run
//! keeps its place: what it executes is rebuilt through the host's [`WorkflowRelaunch`],
//! and once every queued run is back in the queue they are dispatched in their durable
//! queue order (`queued_ms`, not `started_ms`) as run slots allow. Then ended runs are
//! handed back, and with
//! `workflows.resume_after_restart` the interrupted runs are resumed through the run
//! queue, as a user's resume would.
//!
//! A run is interrupted when its latest lifecycle event is a pause with
//! `interrupted: true` (recovery's, or the pause a server records while shutting down).

use std::path::Path;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::json;

use super::workflow_control::{ResumeWorkflowRun, relaunch_mismatch};
use super::workflow_queue::Pending;
use super::workflow_replay::Replay;
use super::workflows::{Engine, LIFECYCLE, RUN_PAUSED, WorkflowRun, WorkflowRunStatus};
use super::{Runtime, RuntimeError};

/// Rebuilds what the next attempt of a run executes, exactly as a user's resume would: the
/// saved module or inline script read again, the Location's engine limits and tiers, the
/// share, and the owner's `workflow.run` permission. The HTTP layer provides it, because
/// saved Workflows and configuration live there.
pub trait WorkflowRelaunch: Send + Sync {
    /// The relaunch of `run` (no budget override, no new revision), or why it is refused.
    fn relaunch<'a>(
        &'a self,
        runtime: &'a Runtime,
        run: &'a WorkflowRun,
    ) -> BoxFuture<'a, Result<ResumeWorkflowRun, String>>;
}

/// What resuming the interrupted runs did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, schemars::JsonSchema)]
pub struct InterruptedResume {
    /// Resumed runs, in `started_ms` order; each is `running` or `queued`.
    pub resumed: Vec<String>,
    /// Interrupted runs left paused, with the refusal.
    pub refused: Vec<RefusedResume>,
}

/// An interrupted run that could not be resumed.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RefusedResume {
    pub run_id: String,
    /// For example `WorkflowReplayMismatch: the script changed …`.
    pub reason: String,
}

impl Runtime {
    /// Set how runs are rebuilt after a restart, once, before recovery. False when already
    /// set. Without it, recovery pauses queued runs too, for a manual resume.
    pub fn configure_workflow_relaunch(&self, relaunch: Arc<dyn WorkflowRelaunch>) -> bool {
        self.inner.workflow_relaunch.set(relaunch).is_ok()
    }

    /// Startup recovery. `resume_after_restart` (`workflows.resume_after_restart`) then
    /// resumes the interrupted runs; one that is refused stays paused with the refusal as
    /// its reason.
    pub async fn recover_workflow_runs(
        &self,
        resume_after_restart: bool,
    ) -> Result<(), RuntimeError> {
        self.ensure_workflow_progress();
        let mut active: Vec<WorkflowRun> = self
            .workflow_runs(None)?
            .into_iter()
            .filter(|run| run.status.is_active())
            .collect();
        active.sort_by(|a, b| (a.started_ms, &a.id).cmp(&(b.started_ms, &b.id)));
        for listed in active {
            self.recover_workflow_run(&listed.id).await?;
        }
        // Only now: a run dispatched while others were still being re-queued could pass
        // one queued ahead of it. The task holds the runtime weakly until it runs, so an
        // application dropped right after startup still releases its host and database.
        let runtime = self.downgrade();
        tokio::spawn(async move {
            if let Some(runtime) = runtime.upgrade() {
                runtime.dispatch_workflow_runs().await;
            }
        });
        self.recover_workflow_handbacks().await;
        if resume_after_restart {
            let report = self.resume_interrupted_workflow_runs(None).await?;
            for refused in report.refused {
                self.record_refused_resume(&refused).await?;
            }
        }
        Ok(())
    }

    /// Recover one run a previous process left active, under its own lock.
    async fn recover_workflow_run(&self, run_id: &str) -> Result<(), RuntimeError> {
        let lock = self.workflow_lock(run_id);
        let _guard = lock.lock().await;
        let Some(run) = self.workflow_run(run_id)? else {
            return Ok(());
        };
        if !run.status.is_active() || self.workflow_control(run_id).is_some() {
            return Ok(());
        }
        if run.status != WorkflowRunStatus::Queued {
            return self.record_interrupted(run, "the server restarted; resume to continue");
        }
        let relaunch = match self.inner.workflow_relaunch.get() {
            Some(relaunch) => relaunch.relaunch(self, &run).await,
            None => Err("the server restarted; resume to continue".to_owned()),
        };
        match relaunch.map(|resume| (relaunch_mismatch(&run, &resume), resume)) {
            Ok((None, resume)) => {
                let replay = Replay::load(&self.inner.store, &run.id, run.revision)?;
                let engine = Engine {
                    module: resume.module,
                    limits: resume.limits,
                    tiers: resume.tiers,
                };
                let queued_ms = run.queued_ms.unwrap_or(run.started_ms);
                self.workflow_queue()
                    .push(&run.id, queued_ms, Pending { engine, replay });
                Ok(())
            }
            // The queued run cannot execute what it was queued with: it fails closed.
            Ok((Some(reason), _)) | Err(reason) => self.record_interrupted(run, &reason),
        }
    }

    /// The durable pause of a run a restart interrupted.
    fn record_interrupted(&self, mut run: WorkflowRun, reason: &str) -> Result<(), RuntimeError> {
        run.status = WorkflowRunStatus::Paused;
        run.reason = Some(reason.to_owned());
        run.queued_ms = None;
        run.queue_position = None;
        self.run_log()
            .record(&run, RUN_PAUSED, json!({"interrupted": true}))
    }

    /// An automatic resume that was refused: the run stays interrupted, with the refusal.
    async fn record_refused_resume(&self, refused: &RefusedResume) -> Result<(), RuntimeError> {
        let lock = self.workflow_lock(&refused.run_id);
        let _guard = lock.lock().await;
        match self.workflow_run(&refused.run_id)? {
            Some(run)
                if run.status == WorkflowRunStatus::Paused
                    && self.workflow_run_interrupted(&run.id)? =>
            {
                self.record_interrupted(run, &refused.reason)
            }
            _ => Ok(()),
        }
    }

    /// Resume every run a restart interrupted, of the Location `directory` or of every
    /// Location, in `started_ms` order and through the run queue. A run paused by a user
    /// is left alone.
    pub async fn resume_interrupted_workflow_runs(
        &self,
        directory: Option<&Path>,
    ) -> Result<InterruptedResume, RuntimeError> {
        let location = directory.map(|path| location_key(&path.to_string_lossy()));
        let mut runs: Vec<WorkflowRun> = self
            .workflow_runs(None)?
            .into_iter()
            .filter(|run| run.status == WorkflowRunStatus::Paused)
            .filter(|run| {
                location
                    .as_ref()
                    .is_none_or(|key| *key == location_key(&run.directory))
            })
            .collect();
        runs.sort_by(|a, b| (a.started_ms, &a.id).cmp(&(b.started_ms, &b.id)));
        let mut report = InterruptedResume::default();
        for run in runs {
            if !self.workflow_run_interrupted(&run.id)? {
                continue;
            }
            match self.resume_interrupted(&run).await {
                Ok(()) => report.resumed.push(run.id),
                Err(reason) => report.refused.push(RefusedResume {
                    run_id: run.id,
                    reason,
                }),
            }
        }
        Ok(report)
    }

    async fn resume_interrupted(&self, run: &WorkflowRun) -> Result<(), String> {
        let relaunch = self
            .inner
            .workflow_relaunch
            .get()
            .ok_or("Workflow Runs cannot be relaunched in this host")?;
        let resume = relaunch.relaunch(self, run).await?;
        self.resume_workflow_run(resume)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                RuntimeError::Conflict(message) => message,
                error => error.to_string(),
            })
    }

    /// Whether the run's latest lifecycle event is a pause with `interrupted: true`.
    pub fn workflow_run_interrupted(&self, run_id: &str) -> Result<bool, RuntimeError> {
        let id = run_id.to_owned();
        let kinds = LIFECYCLE.map(|kind| format!("'{kind}'")).join(", ");
        let sql = format!(
            "SELECT type, json_extract(data, '$.interrupted') FROM event
             WHERE aggregate_id = ?1 AND type IN ({kinds}) ORDER BY seq DESC LIMIT 1"
        );
        Ok(self.inner.store.read(move |db| {
            use rusqlite::OptionalExtension;
            let latest: Option<(String, Option<bool>)> = db
                .query_row(&sql, [id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            Ok(matches!(latest, Some((kind, Some(true))) if kind == RUN_PAUSED))
        })?)
    }
}

/// A Location as runs are grouped by it: the canonical directory, else the given spelling.
pub(super) fn location_key(directory: &str) -> String {
    std::fs::canonicalize(directory)
        .map_or_else(|_| directory.to_owned(), |path| path.display().to_string())
}
