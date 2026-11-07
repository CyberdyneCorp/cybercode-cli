//! Reconcile metadata for an already committed owner pair. No source-file capability.
use super::snapshot_recovery_authority::RecoveryAuthority;
use super::{
    Handle, Runtime, RuntimeError, SessionState, SnapshotOrigin, SnapshotRecoveryReceipt, activity,
};
use std::sync::{Arc, PoisonError, atomic::Ordering};
use tokio::sync::OwnedRwLockReadGuard;

pub struct SnapshotAcknowledgementWorker {
    runtime: Runtime,
    authority: RecoveryAuthority,
    sequence: i64,
    receipt: SnapshotRecoveryReceipt,
    origin: SnapshotOrigin,
    control: Arc<activity::Control>,
    // Actual resources are released before ordinary admission or Stop waiters.
    _location: Box<dyn Send>,
    _lifecycle: OwnedRwLockReadGuard<()>,
    _admission: Admission,
    _completion: Completion,
}
struct Admission(Arc<Handle>);
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.snapshot_active.store(false, Ordering::SeqCst);
    }
}
struct Completion {
    runtime: Runtime,
    id: String,
    control: Arc<activity::Control>,
}
impl Drop for Completion {
    fn drop(&mut self) {
        self.runtime
            .inner
            .activities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
        self.control.done.cancel();
    }
}

/// Only acknowledgement metadata may be changed. This does not expose source
/// installation, intent creation, generic launch authority or worker-origin adoption.
pub struct SnapshotAcknowledgementWork<'a>(&'a SnapshotAcknowledgementWorker);
impl SnapshotAcknowledgementWork<'_> {
    pub fn receipt(&self) -> &SnapshotRecoveryReceipt {
        &self.0.receipt
    }
    pub fn origin(&self) -> &SnapshotOrigin {
        &self.0.origin
    }
    pub fn authorize(&self) -> Result<(), String> {
        if self.0.control.stop.is_cancelled() {
            return Err("Snapshot acknowledgement was cancelled".into());
        }
        let authority = self.0.authority.clone();
        let receipt = self.0.receipt.clone();
        let origin = self.0.origin.clone();
        let sequence = self.0.sequence;
        self.0
            .runtime
            .inner
            .store
            .read(move |db| {
                let tx = db.unchecked_transaction()?;
                verify_pair(&tx, &receipt, &origin)?;
                let current: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(seq),-1) FROM event WHERE aggregate_id=?1",
                    [&origin.session_id],
                    |row| row.get(0),
                )?;
                if current != sequence || !authority.current(&tx)? {
                    return Err(cyber_store::StoreError::InvalidAppend(
                        "Snapshot acknowledgement authority changed".into(),
                    ));
                }
                tx.commit()?;
                Ok(())
            })
            .map_err(|error| error.to_string())
    }
}

impl SnapshotAcknowledgementWorker {
    pub async fn run(
        self,
        work: impl FnOnce(&SnapshotAcknowledgementWork<'_>) -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        let cancelled = self.control.stop.clone().drop_guard();
        let joined = tokio::task::spawn_blocking(move || work(&SnapshotAcknowledgementWork(&self)));
        let result = joined.await.map_err(|_| {
            "Snapshot acknowledgement worker failed; lookup and reconcile its committed receipt"
                .to_owned()
        })?;
        cancelled.disarm();
        result
    }
}

impl Runtime {
    pub(super) async fn snapshot_acknowledgement_worker(
        &self,
        handle: &Arc<Handle>,
        state: &SessionState,
        receipt: SnapshotRecoveryReceipt,
        origin: SnapshotOrigin,
    ) -> Result<SnapshotAcknowledgementWorker, RuntimeError> {
        let lifecycle = self.memory_mutation_lease().await?;
        self.inner.ensure_idle(&state.info.id)?;
        self.ensure_no_background(&state.info.id)?;
        state.ensure_worktree_ready()?;
        if self
            .inner
            .activities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .any(|control| control.source == state.info.id)
        {
            return Err(RuntimeError::Busy(state.info.id.clone()));
        }
        let directory = std::fs::canonicalize(&state.info.directory)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        if origin.session_id != state.info.id
            || origin.worktree_id != state.info.worktree_id
            || origin.binding.directory != directory
        {
            return Err(RuntimeError::Conflict(
                "Committed acknowledgement belongs to another Location".into(),
            ));
        }
        let authority = RecoveryAuthority::capture(self, &state.info.id)?;
        let bound_receipt = receipt.clone();
        let bound_origin = origin.clone();
        self.inner.store.read(move |db| {
            let tx = db.unchecked_transaction()?;
            verify_pair(&tx, &bound_receipt, &bound_origin)?;
            tx.commit()?;
            Ok(())
        })?;
        handle
            .snapshot_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| RuntimeError::Busy(state.info.id.clone()))?;
        let admission = Admission(handle.clone());
        let control = Arc::new(activity::Control {
            source: state.info.id.clone(),
            binary_document: None,
            snapshot_worker: true,
            admission_sessions: super::admission_authority::binding_sessions(authority.bindings()),
            stop: self.inner.closed.child_token(),
            done: tokio_util::sync::CancellationToken::new(),
        });
        let id = cyber_core::ids::new_id("op_ack");
        // This control is intentionally volatile: source effects already have
        // their durable paired owners. A crash can leave only idempotent journal
        // acknowledgement work, never a new source operation or orphan intent.
        self.inner
            .activities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), control.clone());
        let completion = Completion {
            runtime: self.clone(),
            id,
            control: control.clone(),
        };
        let lease = self
            .inner
            .claim_location(&state.info, false, control.stop.clone())
            .await?;
        if lease.worktree_id != state.info.worktree_id {
            return Err(RuntimeError::Conflict(
                "Acknowledgement checkout changed".into(),
            ));
        }
        Ok(SnapshotAcknowledgementWorker {
            runtime: self.clone(),
            authority,
            sequence: state.last_seq,
            receipt,
            origin,
            control,
            _location: lease.settle_retained().map_err(RuntimeError::Invalid)?,
            _lifecycle: lifecycle,
            _admission: admission,
            _completion: completion,
        })
    }
}

/// Observe both durable owner records in one read snapshot, including after a
/// committed-but-lost writer reply. Filesystem completion alone never passes.
fn verify_pair(
    db: &rusqlite::Connection,
    receipt: &SnapshotRecoveryReceipt,
    origin: &SnapshotOrigin,
) -> Result<(), cyber_store::StoreError> {
    let (_, original) = super::snapshot_recovery_record::attest(db, &receipt.original)?;
    let (_, resolution) = super::snapshot_recovery_record::attest(db, origin)?;
    let created: String = db.query_row(
        "SELECT id FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq LIMIT 1",
        [&origin.session_id, super::events::CREATED],
        |row| row.get(0),
    )?;
    let request = resolution.snapshot_request.as_ref().ok_or_else(|| {
        cyber_store::StoreError::InvalidAppend("Missing committed request".into())
    })?;
    if original.status != "settled"
        || resolution.status != "settled"
        || original.phase != "snapshot_recovery_committed"
        || resolution.phase != "snapshot_recovery_committed"
        || original.snapshot_receipt.as_ref() != Some(receipt)
        || resolution.snapshot_receipt.as_ref() != Some(receipt)
        || created != origin.session_created_id
        || created != receipt.original.session_created_id
        || origin.activity_id
            != super::snapshot_recovery::request_owner(&created, &receipt.request_id)
        || origin.binding.journal_id != receipt.resolution_id
        || receipt.journal_id != receipt.original.binding.journal_id
        || request.original != receipt.original
        || request.confirmation.request_id != receipt.request_id
        || request.confirmation.journal_id != receipt.journal_id
        || request.confirmation.action != receipt.action
        || request.confirmation.confirmation != receipt.confirmation
        || !receipt.filesystem_complete
    {
        return Err(cyber_store::StoreError::InvalidAppend(
            "Journal acknowledgement requires the exact committed native pair".into(),
        ));
    }
    Ok(())
}
