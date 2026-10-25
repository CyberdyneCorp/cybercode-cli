//! Session deletion (`sessions` → Delete; `hooks` → SessionEnd).
//!
//! Every refusal happens before `SessionEnd`: the Session and its descendants are
//! fenced first, so hooks only ever observe a deletion that then happens, once.

use std::path::{Path, PathBuf};
use std::sync::PoisonError;

use tokio_util::sync::CancellationToken;

use super::bus::LiveEvent;
use super::{DrainEntry, Runtime, RuntimeError, usage_ledger};

impl Runtime {
    /// Delete a Session, its child Sessions, inbox rows and history. Deleting a team lead
    /// stops its team first (`agent-teams` → Shutdown and resume).
    pub async fn delete(&self, session_id: &str) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let ids = {
            let _jobs = self.inner.job_admission.lock().await;
            self.inner.handle(session_id).await?;
            let ids = self.inner.descendants(session_id)?;
            if self.is_running(session_id) {
                return Err(RuntimeError::Busy(session_id.into()));
            }
            self.cancel_session_jobs(&ids).await?;
            self.fence_deletion(session_id, &ids)?;
            ids
        };
        for id in &ids {
            self.computer_stop(id);
        }
        // SessionEnd is observational and never blocks the deletion it reports.
        self.inner.session_end(&handle, "delete").await;
        self.settle_hook_work(&ids).await;
        let artifacts = match self.owned_artifacts(&ids).await {
            Ok(artifacts) => artifacts,
            Err(error) => {
                self.lift_deletion_fence(&ids);
                return Err(error);
            }
        };
        // Held messages are dropped and their senders notified before the records go;
        // the send locks keep new messages out until the deletion ends.
        let _sends = match self.drop_held_messages(&ids).await {
            Ok(locks) => locks,
            Err(error) => {
                self.lift_deletion_fence(&ids);
                return Err(error);
            }
        };
        // `agent-teams`: a lead's team stops (`lead_deleted`) before the lead goes; a
        // stop that cannot be recorded keeps the lead.
        if let Err(error) = self.stop_for_lead_deletion(session_id).await {
            self.lift_deletion_fence(&ids);
            return Err(error);
        }
        let doomed = ids.clone();
        let deleted = self.inner.store.transaction(move |tx| {
            // Ledger rows outlive the Session: index what the backfill has not yet
            // reached while the events and every owner row still exist.
            for id in &doomed {
                usage_ledger::project_unreached(tx, id)?;
            }
            for id in &doomed {
                super::computer::mark_session_deleted(tx, id)?;
                // A lead's `team.*` records stay: the team outlives its lead, and a
                // rebuild re-projects it from them.
                tx.execute(
                    "DELETE FROM event WHERE aggregate_id = ?1 AND substr(type, 1, 5) <> 'team.'",
                    [id],
                )?;
                tx.execute("DELETE FROM event_sequence WHERE aggregate_id = ?1", [id])?;
                tx.execute(
                    "DELETE FROM session_children_charge WHERE parent_id = ?1",
                    [id],
                )?;
                tx.execute("DELETE FROM session WHERE id = ?1", [id])?;
            }
            Ok(())
        });
        if let Err(error) = deleted {
            self.lift_deletion_fence(&ids);
            return Err(error.into());
        }
        {
            let mut sessions = self
                .inner
                .sessions
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for id in &ids {
                sessions.remove(id);
            }
        }
        self.lift_deletion_fence(&ids);
        remove_artifacts(&artifacts);
        self.prune_computer_captures().await;
        // Watches of surviving Sessions on the deleted ones fire with `exited`; the send
        // locks are still held.
        self.targets_deleted(&ids).await;
        for id in ids {
            self.inner
                .bus
                .publish(LiveEvent::Deleted { session_id: id });
        }
        Ok(())
    }

    /// What the Sessions own outside the database: the copies of the files they sent
    /// (`<data>/sent/<file_id>/`), their browser evidence artifacts
    /// (`<data>/browser/artifacts/<session_id>/`), their prompt attachments
    /// (`<data>/attachments/<session_id>/`) and their process Job logs
    /// (`<data>/jobs/<id>.log`).
    async fn owned_artifacts(&self, ids: &[String]) -> Result<Vec<PathBuf>, RuntimeError> {
        let mut artifacts = Vec::new();
        let attachments = self.attachments()?;
        for id in ids {
            artifacts.extend(attachments.session_dir(id).ok());
            let handle = self.inner.handle(id).await?;
            let state = handle.state.lock().await;
            artifacts.extend(state.sent_files.values().filter_map(sent_copy));
            artifacts.extend(
                state
                    .browser_evidence
                    .iter()
                    .flat_map(|evidence| &evidence.artifacts)
                    .filter_map(|artifact| browser_artifacts(id, artifact)),
            );
        }
        artifacts.sort();
        artifacts.dedup();
        artifacts.extend(
            self.jobs(None)?
                .iter()
                .filter(|job| ids.contains(&job.session_id))
                .filter_map(job_log),
        );
        Ok(artifacts)
    }

    /// Fence the Session and its descendants: a deletion entry in the Drain table
    /// keeps any Drain, child continuation or child Job from starting (a prompt only
    /// coalesces into it). Refuses while any of them still runs.
    pub(super) fn fence_deletion(
        &self,
        session_id: &str,
        ids: &[String],
    ) -> Result<(), RuntimeError> {
        let mut drains = self
            .inner
            .drains
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if ids.iter().any(|id| drains.contains_key(id)) {
            return Err(RuntimeError::Busy(session_id.into()));
        }
        for id in ids {
            drains.insert(
                id.clone(),
                DrainEntry {
                    _child_owner: None,
                    child_settlement: None,
                    child_settling: false,
                    cancel: CancellationToken::new(),
                    follow_up: false,
                    deleting: true,
                },
            );
        }
        Ok(())
    }

    /// Remove the deletion fences. A Session that still exists (the deletion
    /// failed) drains the input that coalesced into its fence.
    pub(super) fn lift_deletion_fence(&self, ids: &[String]) {
        let mut resumed = Vec::new();
        {
            let mut drains = self
                .inner
                .drains
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for id in ids {
                if drains.get(id).is_some_and(|entry| entry.deleting)
                    && drains.remove(id).is_some_and(|entry| entry.follow_up)
                {
                    resumed.push(id.clone());
                }
            }
        }
        self.inner.idle.notify_waiters();
        let alive = self
            .inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        for id in resumed.iter().filter(|id| alive.contains(*id)) {
            self.inner.start_drain(id, false);
        }
    }

    /// Cancel the fenced Sessions' idle operations, including hook executions still
    /// running in the background (async SessionEnd handlers among them), and wait
    /// for them, so none writes receipts or context after its Session is gone.
    async fn settle_hook_work(&self, ids: &[String]) {
        for id in ids {
            let activities = self.inner.cancel_idle_activities(id);
            if let Err(error) = self.inner.wait_idle_activities(id, activities).await {
                cyber_core::log::warn(
                    "sessions",
                    "idle operation did not settle before deletion",
                    serde_json::json!({"session_id": id, "error": error.to_string()}),
                );
            }
        }
    }

    /// Whether `session_id` is being deleted; child Job admission refuses it.
    pub(crate) fn is_deleting(&self, session_id: &str) -> bool {
        self.inner
            .drains
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session_id)
            .is_some_and(|entry| entry.deleting)
    }
}

/// The directory holding a sent file's copy, when it has the managed shape
/// `sent/<file_id>/<name>`: nothing else is ever removed.
fn sent_copy(file: &super::SentFile) -> Option<PathBuf> {
    let dir = Path::new(&file.stored_path).parent()?;
    let managed = dir.file_name()? == file.id.as_str() && dir.parent()?.file_name()? == "sent";
    managed.then(|| dir.to_path_buf())
}

/// A process Job's log, when it has the managed shape `jobs/<job_id>.log`.
fn job_log(job: &super::Job) -> Option<PathBuf> {
    let path = Path::new(job.output_path.as_deref()?);
    let managed = path.file_name()? == format!("{}.log", job.id).as_str()
        && path.parent()?.file_name()? == "jobs";
    managed.then(|| path.to_path_buf())
}

/// The directory of a browser artifact, when it has the managed shape
/// `browser/artifacts/<session_id>/<file>`: nothing else is ever removed.
fn browser_artifacts(session_id: &str, artifact: &super::BrowserArtifact) -> Option<PathBuf> {
    let dir = Path::new(&artifact.path).parent()?;
    let artifacts = dir.parent()?;
    let managed = dir.file_name()? == session_id
        && artifacts.file_name()? == "artifacts"
        && artifacts.parent()?.file_name()? == "browser";
    managed.then(|| dir.to_path_buf())
}

fn remove_artifacts(artifacts: &[PathBuf]) {
    for path in artifacts {
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        if let Err(error) = removed
            && error.kind() != std::io::ErrorKind::NotFound
        {
            cyber_core::log::warn(
                "sessions",
                "artifact_removal_failed",
                serde_json::json!({"path": path.display().to_string(), "error": error.to_string()}),
            );
        }
    }
}
