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
    receipt: Acknowledgement,
    origin: SnapshotOrigin,
    control: Arc<activity::Control>,
    // Actual resources are released before ordinary admission or Stop waiters.
    _location: Option<Box<dyn Send>>,
    checkouts: Option<RestoreResources>,
    recovery: Option<RecoveryResources>,
    _admission: Admission,
    _completion: Completion,
    // Shutdown's admission barrier must cover the complete worker disposal.
    _lifecycle: OwnedRwLockReadGuard<()>,
}
struct RecoveryResources {
    claims: super::snapshot_recovery_checkout::RecoveryClaims,
    original: Option<Box<dyn super::SnapshotLifetime>>,
    resolution: Box<dyn super::SnapshotLifetime>,
}
impl RecoveryResources {
    fn verify(&self, receipt: &SnapshotRecoveryReceipt) -> Result<(), String> {
        use super::SnapshotRecoveryCheckoutRole as Role;
        for role in [Role::Resolution, Role::Original] {
            let lifetime = match role {
                Role::Resolution => Some(self.resolution.as_ref()),
                Role::Original => self.original.as_deref(),
            };
            match (lifetime, receipt.checkout_evidence(role)) {
                (Some(lifetime), Some(evidence)) => {
                    lifetime.verify()?;
                    let resources = lifetime
                        .checkout_bindings()?
                        .ok_or("Recovery checkout publication is missing")?;
                    if lifetime.binding() != &evidence.lifetime
                        || evidence.lifetime.checkout_resources_digest(&resources)?
                            != evidence.resources_digest
                    {
                        return Err("Recovery acknowledgement resource history changed".into());
                    }
                }
                (None, None) => {}
                _ => return Err("Recovery acknowledgement resource role is incomplete".into()),
            }
        }
        self.claims.verify(receipt)
    }
}
struct RestoreResources {
    claims: Box<dyn super::SnapshotLocationGuard>,
    // Last: retain the original worker's lock until every native claim is released.
    lifetime: Box<dyn super::SnapshotLifetime>,
}
impl RestoreResources {
    fn verify(&self, receipt: &super::SnapshotRestoreReceipt) -> Result<(), String> {
        let checkout = receipt
            .checkout
            .as_ref()
            .ok_or("Missing restore checkout evidence")?;
        if self.lifetime.binding() != &checkout.lifetime {
            return Err("Restore acknowledgement lifetime changed".into());
        }
        self.lifetime.verify()?;
        let resources = self
            .lifetime
            .checkout_bindings()?
            .ok_or("Missing published restore resources")?;
        self.claims.verify()?;
        if self.claims.bindings()? != resources
            || checkout.lifetime.checkout_resources_digest(&resources)? != checkout.resources_digest
        {
            return Err("Original restore checkout resource set differs".into());
        }
        Ok(())
    }
}
#[derive(Clone)]
pub(super) enum Acknowledgement {
    Recovery(SnapshotRecoveryReceipt),
    Restore(super::SnapshotRestoreReceipt),
}
impl Acknowledgement {
    fn verify(
        &self,
        db: &rusqlite::Connection,
        origin: &SnapshotOrigin,
    ) -> Result<(), cyber_store::StoreError> {
        match self {
            Self::Recovery(receipt) => verify_pair(db, receipt, origin),
            Self::Restore(receipt) if receipt.origin == *origin => {
                super::snapshot_restore_receipt::verify(db, receipt)
            }
            Self::Restore(_) => Err(cyber_store::StoreError::InvalidAppend(
                "Foreign restore origin".into(),
            )),
        }
    }
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
    pub fn recovery_receipt(&self) -> Option<&SnapshotRecoveryReceipt> {
        match &self.0.receipt {
            Acknowledgement::Recovery(receipt) => Some(receipt),
            _ => None,
        }
    }
    pub fn restore_receipt(&self) -> Option<&super::SnapshotRestoreReceipt> {
        match &self.0.receipt {
            Acknowledgement::Restore(receipt) => Some(receipt),
            _ => None,
        }
    }
    pub fn origin(&self) -> &SnapshotOrigin {
        &self.0.origin
    }
    pub fn authorize(&self) -> Result<(), String> {
        if self.0.control.stop.is_cancelled() {
            return Err("Snapshot acknowledgement was cancelled".into());
        }
        if let (Some(resources), Acknowledgement::Restore(receipt)) =
            (&self.0.checkouts, &self.0.receipt)
        {
            resources.verify(receipt)?;
        }
        if let (Some(resources), Acknowledgement::Recovery(receipt)) =
            (&self.0.recovery, &self.0.receipt)
        {
            resources.verify(receipt)?;
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
                receipt.verify(&tx, &origin)?;
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
        mut self,
        work: impl FnOnce(&SnapshotAcknowledgementWork<'_>) -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        let cancelled = self.control.stop.clone().drop_guard();
        let joined = tokio::task::spawn_blocking(move || {
            work(&SnapshotAcknowledgementWork(&self))?;
            if self.checkouts.is_some() || self.recovery.is_some() {
                SnapshotAcknowledgementWork(&self).authorize()?;
            }
            if let (Some(resources), Acknowledgement::Restore(receipt)) =
                (&mut self.checkouts, &self.receipt)
            {
                resources.verify(receipt)?;
                resources.claims.acknowledge_restore(receipt)?;
            }
            if let (Some(resources), Acknowledgement::Recovery(receipt)) =
                (&mut self.recovery, &self.receipt)
            {
                resources.verify(receipt)?;
                resources.claims.acknowledge(receipt)?;
            }
            Ok(())
        });
        let result = joined.await.map_err(|_| {
            "Snapshot acknowledgement worker failed; lookup and reconcile its committed receipt"
                .to_owned()
        })?;
        cancelled.disarm();
        result
    }
}

impl Runtime {
    async fn lock_checkout_resources(
        &self,
        info: &super::SessionInfo,
        evidence: &super::SnapshotRestoreCheckout,
    ) -> Result<RestoreResources, RuntimeError> {
        let lifetime = self
            .inner
            .options
            .snapshots
            .lock_worker(evidence.lifetime.clone())
            .await
            .map_err(RuntimeError::Conflict)?;
        let published = lifetime
            .checkout_bindings()
            .map_err(RuntimeError::Conflict)?
            .ok_or_else(|| {
                RuntimeError::Conflict("Original checkout publication is missing".into())
            })?;
        let claims = self
            .inner
            .tools
            .reacquire_snapshot_location(info, &evidence.lifetime, &published)
            .map_err(RuntimeError::Conflict)?;
        let resources = RestoreResources { claims, lifetime };
        if resources.lifetime.binding() != &evidence.lifetime {
            return Err(RuntimeError::Conflict(
                "Original checkout lifetime differs".into(),
            ));
        }
        Ok(resources)
    }
    async fn lock_recovery_resources(
        &self,
        info: &super::SessionInfo,
        receipt: &SnapshotRecoveryReceipt,
    ) -> Result<RecoveryResources, RuntimeError> {
        let checkout = receipt
            .checkout
            .as_ref()
            .ok_or_else(|| RuntimeError::Conflict("Missing recovery checkout evidence".into()))?;
        let original = match &checkout.original {
            Some(original) => Some(self.lock_checkout_resources(info, original).await?),
            None => None,
        };
        let resolution = self
            .lock_checkout_resources(info, &checkout.resolution)
            .await?;
        let (original_claims, original_lifetime) = match original {
            Some(original) => (Some(original.claims), Some(original.lifetime)),
            None => (None, None),
        };
        let mut resources = RecoveryResources {
            claims: super::snapshot_recovery_checkout::RecoveryClaims {
                resolution: resolution.claims,
                original: original_claims,
            },
            original: original_lifetime,
            resolution: resolution.lifetime,
        };
        resources.verify(receipt).map_err(RuntimeError::Conflict)?;
        resources
            .claims
            .prepare(receipt)
            .map_err(RuntimeError::Conflict)?;
        Ok(resources)
    }
    pub(super) async fn snapshot_acknowledgement_worker(
        &self,
        handle: &Arc<Handle>,
        state: &SessionState,
        receipt: SnapshotRecoveryReceipt,
        origin: SnapshotOrigin,
    ) -> Result<SnapshotAcknowledgementWorker, RuntimeError> {
        self.snapshot_acknowledgement_worker_for(
            handle,
            state,
            Acknowledgement::Recovery(receipt),
            origin,
        )
        .await
    }
    pub(super) async fn snapshot_acknowledgement_worker_for(
        &self,
        handle: &Arc<Handle>,
        state: &SessionState,
        receipt: Acknowledgement,
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
            bound_receipt.verify(&tx, &bound_origin)?;
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
        let mut recovery = None;
        let (location, checkouts) = if let Acknowledgement::Recovery(receipt) = &receipt
            && receipt.checkout.is_some()
        {
            recovery = Some(self.lock_recovery_resources(&state.info, receipt).await?);
            (None, None)
        } else if let Acknowledgement::Restore(restore) = &receipt
            && let Some(checkout) = &restore.checkout
        {
            let lifetime = self
                .inner
                .options
                .snapshots
                .lock_worker(checkout.lifetime.clone())
                .await
                .map_err(RuntimeError::Conflict)?;
            let published = lifetime
                .checkout_bindings()
                .map_err(RuntimeError::Conflict)?
                .ok_or_else(|| {
                    RuntimeError::Conflict("Original restore resources are unpublished".into())
                })?;
            let claims = self
                .inner
                .tools
                .reacquire_snapshot_location(&state.info, &checkout.lifetime, &published)
                .map_err(RuntimeError::Conflict)?;
            let mut resources = RestoreResources { claims, lifetime };
            resources.verify(restore).map_err(RuntimeError::Conflict)?;
            resources
                .claims
                .prepare_restore_acknowledgement(restore)
                .map_err(RuntimeError::Conflict)?;
            (None, Some(resources))
        } else {
            let lease = self
                .inner
                .claim_location(&state.info, false, control.stop.clone())
                .await?;
            if lease.worktree_id != state.info.worktree_id {
                return Err(RuntimeError::Conflict(
                    "Acknowledgement checkout changed".into(),
                ));
            }
            (
                Some(lease.settle_retained().map_err(RuntimeError::Invalid)?),
                None,
            )
        };
        Ok(SnapshotAcknowledgementWorker {
            runtime: self.clone(),
            authority,
            sequence: state.last_seq,
            receipt,
            origin,
            control,
            _location: location,
            checkouts,
            recovery,
            _lifecycle: lifecycle,
            _admission: admission,
            _completion: completion,
        })
    }
}

/// Observe both durable owner records in one read snapshot, including after a
/// committed-but-lost writer reply. Filesystem completion alone never passes.
pub(super) fn verify_pair(
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
    if let Some(checkout) = &receipt.checkout {
        receipt
            .validate_checkout(origin)
            .map_err(cyber_store::StoreError::InvalidAppend)?;
        if resolution.snapshot_lifetime.as_ref() != Some(&checkout.resolution.lifetime)
            || original
                .snapshot_lifetime
                .as_ref()
                .filter(|life| life.version == 2)
                != checkout
                    .original
                    .as_ref()
                    .map(|evidence| &evidence.lifetime)
        {
            return Err(cyber_store::StoreError::InvalidAppend(
                "Recovery checkout lifetime history differs".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn verify_committed(
    db: &rusqlite::Connection,
    receipt: &SnapshotRecoveryReceipt,
) -> Result<(), cyber_store::StoreError> {
    let owner = super::snapshot_recovery::request_owner(
        &receipt.original.session_created_id,
        &receipt.request_id,
    );
    let data: String = db.query_row(
        "SELECT data FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq DESC LIMIT 1",
        [&owner, activity::CHANGED],
        |row| row.get(0),
    )?;
    let record: activity::Record = serde_json::from_str(&data)
        .map_err(|error| cyber_store::StoreError::InvalidAppend(error.to_string()))?;
    verify_pair(
        db,
        receipt,
        record.snapshot_origin.as_ref().ok_or_else(|| {
            cyber_store::StoreError::InvalidAppend("Recovery resolution origin is missing".into())
        })?,
    )
}
