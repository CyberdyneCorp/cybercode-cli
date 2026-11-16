//! Confirmation, resumption and maintenance of construction discards.
//!
//! Order under the Session lock: runtime fences (idle, no Drain, no background
//! work, no admitted snapshot worker, no unresolved preparation), the unchanged
//! review fingerprint, the owner activity's lifetime lock and the native stage
//! fence, then the durable intent. Only after the intent commits is the stage
//! moved into its quarantine slot and unlinked; the completion is recorded last.
use super::{
    ConstructionDiscardConfirmation, ConstructionDiscardReceipt, ConstructionDiscardStatus,
    ConstructionFence, ConstructionFenceError, ConstructionLocation, ConstructionStage,
    ConstructionStageKind, DISCARDED, LOCAL_OWNER, NativeConstructionStage, conflict,
    quarantine_key,
};
use crate::runtime::{Handle, Runtime, RuntimeError, SessionState, activity, events::event};
use cyber_core::construction_discard::Place;
use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, atomic::Ordering};

/// Retries a pending unlink about a minute after start, then hourly, forever.
pub fn spawn_retry(runtime: Runtime) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(first_retry_delay()).await;
        loop {
            if let Err(error) = runtime.retry_construction_discards().await {
                cyber_core::log::warn(
                    "snapshots",
                    "construction discard retry failed",
                    serde_json::json!({"error": error.to_string()}),
                );
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    })
}

/// A minute; debug-build process tests may shorten it with
/// `CYBER_TEST_DISCARD_RETRY_DELAY_MS`. Release builds ignore the variable.
fn first_retry_delay() -> std::time::Duration {
    #[cfg(debug_assertions)]
    if let Some(milliseconds) = std::env::var("CYBER_TEST_DISCARD_RETRY_DELAY_MS")
        .ok()
        .and_then(|value| value.parse().ok())
    {
        return std::time::Duration::from_millis(milliseconds);
    }
    std::time::Duration::from_secs(60)
}

/// Marks the Session as running snapshot work for the whole confirmation.
struct Admission(Arc<Handle>);
impl Admission {
    fn take(handle: &Arc<Handle>, session: &str) -> Result<Self, RuntimeError> {
        handle
            .snapshot_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| RuntimeError::Busy(session.into()))?;
        Ok(Self(handle.clone()))
    }
}
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.snapshot_active.store(false, Ordering::SeqCst);
    }
}

/// Native ownership retained from fencing through the move.
struct Fenced {
    _lifetime: Option<Box<dyn crate::runtime::SnapshotLifetime>>,
    stage: Box<dyn ConstructionFence>,
}

fn fence_error(session: &str, error: ConstructionFenceError) -> RuntimeError {
    match error {
        ConstructionFenceError::Busy(message) => RuntimeError::Conflict(format!(
            "busy: {message} (Session {session}); the stage stays fenced"
        )),
        ConstructionFenceError::Refused(message) => RuntimeError::Conflict(message),
    }
}

/// Only a held lifetime lock is a live holder; anything else refuses as it is.
fn lifetime_error(error: String) -> ConstructionFenceError {
    if error == "Snapshot worker is still active" {
        ConstructionFenceError::Busy(error)
    } else {
        ConstructionFenceError::Refused(error)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn unresolved_preparation(
    db: &rusqlite::Connection,
    session: &str,
) -> Result<bool, cyber_store::StoreError> {
    use rusqlite::OptionalExtension;
    Ok(db
        .query_row(
            "SELECT 1 FROM event e WHERE type=?2 AND json_extract(data,'$.session_id')=?1 \
             AND json_extract(data,'$.snapshot_worker')=1 AND json_extract(data,'$.status')<>'settled' \
             AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq) LIMIT 1",
            [session, activity::CHANGED],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

impl Runtime {
    /// Confirm a reviewed discard, or resume the request with this ID.
    pub async fn construction_discard_confirm(
        &self,
        session: &str,
        confirmation: &ConstructionDiscardConfirmation,
    ) -> Result<ConstructionDiscardReceipt, RuntimeError> {
        crate::runtime::snapshot_recovery::validate_request_id(&confirmation.request_id)?;
        if !cyber_core::construction_discard::valid_key(&confirmation.confirmation) {
            return Err(RuntimeError::Invalid("Invalid discard fingerprint".into()));
        }
        if let Some(receipt) = self
            .construction_discard_lookup(session, &confirmation.request_id)
            .await?
        {
            if receipt.confirmation != confirmation.confirmation
                || receipt.stage.name != confirmation.stage
            {
                return Err(conflict("Discard request content changed"));
            }
            return self.resume_construction_discard(receipt).await;
        }
        let _lifecycle = self.memory_mutation_lease().await?;
        let handle = self.inner.handle(session).await?;
        let mut state = handle.state.lock().await;
        let _admission = self.discard_fences(&handle, &state).await?;
        let (stage, activities) = self
            .reviewed_stage(&state.info, &confirmation.stage)
            .await?;
        if Self::discard_fingerprint(session, state.last_seq, &stage)? != confirmation.confirmation
        {
            return Err(conflict(
                "stale: the construction stage or Session changed since review; review again",
            ));
        }
        let fenced = self.fence_stage(&state, &activities, &stage).await?;
        let receipt = ConstructionDiscardReceipt {
            request_id: confirmation.request_id.clone(),
            session_id: session.into(),
            confirmation: confirmation.confirmation.clone(),
            stage,
            actor: LOCAL_OWNER.into(),
            recorded_ms: now_ms(),
            status: ConstructionDiscardStatus::Intent,
            detail: None,
        };
        self.inner
            .commit_locked(&mut state, vec![event(DISCARDED, &receipt)])?;
        self.execute_discard(&mut state, fenced, receipt).await
    }

    /// Runtime fences, checked with the Session lock held.
    async fn discard_fences(
        &self,
        handle: &Arc<Handle>,
        state: &SessionState,
    ) -> Result<Admission, RuntimeError> {
        let session = state.info.id.as_str();
        self.inner.ensure_idle(session)?;
        self.ensure_no_background(session)?;
        state.ensure_worktree_ready()?;
        if self
            .inner
            .activities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .any(|control| control.source == session)
        {
            return Err(RuntimeError::Busy(session.into()));
        }
        let admission = Admission::take(handle, session)?;
        let source = session.to_owned();
        if self
            .inner
            .store
            .read(move |db| unresolved_preparation(db, &source))?
        {
            return Err(conflict(
                "The Session has an unresolved snapshot preparation; cancel or recover it before discarding construction evidence",
            ));
        }
        Ok(admission)
    }

    /// Acquire the owner activity's lifetime lock and the native stage fence.
    async fn fence_stage(
        &self,
        state: &SessionState,
        activities: &BTreeMap<String, activity::Record>,
        stage: &ConstructionStage,
    ) -> Result<Fenced, RuntimeError> {
        let session = state.info.id.as_str();
        let record = activities
            .get(&stage.activity_id)
            .ok_or_else(|| conflict("Construction stage owner changed"))?;
        let native = NativeConstructionStage {
            name: stage.name.clone(),
            kind: stage.kind,
            activity_id: Some(stage.activity_id.clone()),
            worktree_id: stage.worktree_id.clone(),
            journal_id: stage.journal_id.clone(),
            evidence: stage.evidence.clone(),
            source: stage.source.clone(),
            namespace: stage.namespace.clone(),
        };
        if stage.kind == ConstructionStageKind::CancellationRecord {
            let origin = super::cancellation_origin(record)
                .ok_or_else(|| conflict("Cancellation record owner changed"))?
                .clone();
            let fence = self
                .inner
                .options
                .snapshots
                .fence_cancellation_stage(origin, native)
                .await
                .map_err(|error| fence_error(session, error))?;
            return Ok(Fenced {
                _lifetime: None,
                stage: fence,
            });
        }
        let lifetime = match (&record.snapshot_lifetime, stage.location) {
            (Some(binding), ConstructionLocation::Present) => Some(
                self.inner
                    .options
                    .snapshots
                    .lock_worker(binding.clone())
                    .await
                    .map_err(|error| fence_error(session, lifetime_error(error)))?,
            ),
            _ => None,
        };
        let fence = self
            .inner
            .tools
            .fence_snapshot_construction(&state.info, &native)
            .map_err(|error| fence_error(session, error))?;
        Ok(Fenced {
            _lifetime: lifetime,
            stage: fence,
        })
    }

    /// Move, unlink and record the completion of an intent already committed.
    async fn execute_discard(
        &self,
        state: &mut SessionState,
        fenced: Fenced,
        receipt: ConstructionDiscardReceipt,
    ) -> Result<ConstructionDiscardReceipt, RuntimeError> {
        let key = quarantine_key(&receipt.session_id, &receipt.request_id);
        let stage = receipt.stage.clone();
        let moved = tokio::task::spawn_blocking(move || {
            let mut fenced = fenced;
            crate::runtime::process_cut::pause("construction-discard-intent-committed");
            fenced.stage.quarantine(&key)?;
            drop(fenced);
            crate::runtime::process_cut::pause("construction-discard-quarantined");
            let place = super::unlink(&stage, &key);
            crate::runtime::process_cut::pause("construction-discard-unlinked");
            Ok::<_, String>(place)
        })
        .await
        .map_err(|_| conflict("Construction discard interrupted; repeat the confirmation with the same request ID"))?
        .map_err(|error| {
            conflict(format!(
                "Construction discard intent recorded but the stage was not moved: {error}; \
                 repeat the confirmation with the same request ID"
            ))
        })?;
        self.complete_discard(state, receipt, moved)
    }

    fn complete_discard(
        &self,
        state: &mut SessionState,
        mut receipt: ConstructionDiscardReceipt,
        place: Result<Place, String>,
    ) -> Result<ConstructionDiscardReceipt, RuntimeError> {
        let (status, detail) = match place {
            Ok(Place::Gone) => (ConstructionDiscardStatus::Completed, None),
            Ok(Place::Changed(detail)) => (ConstructionDiscardStatus::Changed, Some(detail)),
            Ok(Place::Quarantined | Place::Source) => (
                ConstructionDiscardStatus::Pending,
                Some("The stage is still present".into()),
            ),
            Err(error) => (ConstructionDiscardStatus::Pending, Some(error)),
        };
        if receipt.status == status && receipt.detail == detail {
            return Ok(receipt);
        }
        receipt.status = status;
        receipt.detail = detail;
        receipt.recorded_ms = now_ms();
        self.inner
            .commit_locked(state, vec![event(DISCARDED, &receipt)])?;
        Ok(receipt)
    }

    /// Continue a recorded request from wherever its stage is now.
    async fn resume_construction_discard(
        &self,
        receipt: ConstructionDiscardReceipt,
    ) -> Result<ConstructionDiscardReceipt, RuntimeError> {
        if !receipt.pending() {
            return Ok(receipt);
        }
        let handle = self.inner.handle(&receipt.session_id).await?;
        let mut state = handle.state.lock().await;
        let latest = self.discard_receipt(&receipt.session_id, &receipt.request_id)?;
        let Some(current) = latest.clone().filter(ConstructionDiscardReceipt::pending) else {
            return latest.ok_or_else(|| conflict("Discard receipt disappeared"));
        };
        let key = quarantine_key(&current.session_id, &current.request_id);
        let stage = current.stage.clone();
        let place = tokio::task::spawn_blocking(move || super::locate(&stage, &key))
            .await
            .map_err(|_| conflict("Construction discard resumption interrupted"))?;
        match place {
            Ok(Place::Source) => self.refence_and_execute(&handle, &mut state, current).await,
            Ok(Place::Quarantined) => {
                let key = quarantine_key(&current.session_id, &current.request_id);
                let stage = current.stage.clone();
                let unlinked = tokio::task::spawn_blocking(move || super::unlink(&stage, &key))
                    .await
                    .map_err(|_| conflict("Construction discard unlink interrupted"))?;
                self.complete_discard(&mut state, current, unlinked)
            }
            other => self.complete_discard(&mut state, current, other),
        }
    }

    /// The stage never moved: fence it again exactly as reviewed before moving it.
    async fn refence_and_execute(
        &self,
        handle: &Arc<Handle>,
        state: &mut SessionState,
        receipt: ConstructionDiscardReceipt,
    ) -> Result<ConstructionDiscardReceipt, RuntimeError> {
        let _lifecycle = self.memory_mutation_lease().await?;
        let _admission = self.discard_fences(handle, state).await?;
        let activities = self.construction_activities(&receipt.session_id).await?;
        let fenced = self.fence_stage(state, &activities, &receipt.stage).await?;
        self.execute_discard(state, fenced, receipt).await
    }

    /// Maintenance: finish every request whose stage left its namespace but was not
    /// unlinked. A stage that never moved waits for the owner's repeated confirmation.
    pub async fn retry_construction_discards(&self) -> Result<usize, RuntimeError> {
        let mut finished = 0;
        for receipt in self.construction_receipts(None).await? {
            if !receipt.pending() {
                continue;
            }
            match self.retry_one(receipt).await {
                Ok(done) => finished += usize::from(done),
                Err(error) => cyber_core::log::warn(
                    "snapshots",
                    "construction discard stays pending",
                    serde_json::json!({"error": error.to_string()}),
                ),
            }
        }
        Ok(finished)
    }

    /// Finish one moved stage; a stage that never moved is left for the owner.
    async fn retry_one(&self, receipt: ConstructionDiscardReceipt) -> Result<bool, RuntimeError> {
        let key = quarantine_key(&receipt.session_id, &receipt.request_id);
        let stage = receipt.stage.clone();
        let place = tokio::task::spawn_blocking(move || super::locate(&stage, &key))
            .await
            .map_err(|_| conflict("Construction discard retry interrupted"))?;
        if matches!(place, Ok(Place::Source)) {
            return Ok(false);
        }
        Ok(!self.resume_construction_discard(receipt).await?.pending())
    }
}
