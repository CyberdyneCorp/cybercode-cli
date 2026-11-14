//! Runtime-issued ownership for synchronous snapshot filesystem workers.
//! The blocking worker retains Session, checkout and lifecycle ownership even
//! after its requester disappears. A journal is not authority to settle a process.
use super::{Handle, Inner, RestoreError, Runtime, RuntimeError, SessionState, activity};
use cyber_store::NewEvent;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::OwnedRwLockReadGuard;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotBinding {
    pub directory: PathBuf,
    pub root_identity: String,
    pub namespace_identity: String,
    pub journal_id: String,
    pub journal_identity: String,
    /// Exact private lock inode. Legacy origins do not acquire this proof retroactively.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotOrigin {
    pub version: u32,
    pub session_id: String,
    pub session_created_id: String,
    pub activity_id: String,
    pub worktree_id: Option<String>,
    pub binding: SnapshotBinding,
}

/// Exact native intent and Session batch consumed by an ordinary restore.
/// This is durable acknowledgement evidence, never authority to replay files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRestoreReceipt {
    pub origin: SnapshotOrigin,
    pub intent_digest: String,
    pub session_before: i64,
    pub session_after: i64,
    pub events_digest: String,
    /// Missing on historical receipts; absence never authorizes checkout acknowledgement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<Box<SnapshotRestoreCheckout>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRestoreCheckout {
    pub version: u32,
    pub lifetime: super::SnapshotLifetimeBinding,
    pub resources_digest: String,
}

pub(super) fn restore_events_digest(events: &[NewEvent]) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(events).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub struct SnapshotRestoreRequest {
    pub directory: String,
    pub target: String,
    pub recorded: String,
    pub expected: String,
    pub paths: Option<Vec<String>>,
}

/// Native locks and descriptors held through the consuming runtime commit.
pub trait SnapshotNativeGuard: Send {
    /// Captured immutable intent under the retained native locks. Older adapters
    /// may omit this; their settlement cannot authorize later journal writes.
    fn intent_digest(&self) -> Option<&str> {
        None
    }
    fn verify(&self, origin: Option<&SnapshotOrigin>) -> Result<(), String>;
    /// Called only after observed durable runtime acknowledgement, under retained locks.
    fn acknowledge(&mut self, origin: Option<&SnapshotOrigin>) -> Result<(), String>;
}

pub struct SnapshotNativeResult {
    pub paths: Vec<String>,
    pub recovery: Option<super::SnapshotRecoveryReceipt>,
    guard: Box<dyn SnapshotNativeGuard>,
}

impl SnapshotNativeResult {
    pub fn new(paths: Vec<String>, guard: Box<dyn SnapshotNativeGuard>) -> Self {
        Self {
            paths,
            guard,
            recovery: None,
        }
    }
}

pub struct SnapshotWorker {
    sequence: i64,
    runtime: Runtime,
    directory: PathBuf,
    created_id: String,
    worktree_id: Option<String>,
    _location: Option<Box<dyn super::SnapshotLocationGuard>>,
    _original: Option<OriginalResources>,
    // Release native lifetime ownership after checkout and journal guards.
    _lifetime: Option<Box<dyn super::SnapshotLifetime>>,
    admission: SnapshotAdmission,
    // Completion is observed only after all native guards and admission drop.
    scope: activity::Scope,
    // Shutdown's admission barrier must cover the complete worker disposal.
    _lifecycle: OwnedRwLockReadGuard<()>,
}

impl Drop for SnapshotWorker {
    fn drop(&mut self) {
        self.scope.dispose_before_completion();
    }
}

/// Construction has the same disposal boundary as an admitted worker. In
/// particular, cancellation while awaiting checkout ownership must not publish
/// activity completion before the lifetime guard has actually been released.
struct SnapshotConstruction {
    lease: Option<super::LocationLease>,
    location: Option<Box<dyn super::SnapshotLocationGuard>>,
    original: Option<OriginalResources>,
    lifetime: Option<Box<dyn super::SnapshotLifetime>>,
    admission: Option<SnapshotAdmission>,
    scope: Option<activity::Scope>,
    lifecycle: Option<OwnedRwLockReadGuard<()>>,
}

impl Drop for SnapshotConstruction {
    fn drop(&mut self) {
        if let Some(scope) = &mut self.scope {
            scope.dispose_before_completion();
        }
    }
}

impl SnapshotConstruction {
    fn new(
        scope: activity::Scope,
        original: Option<OriginalResources>,
        lifetime: Option<Box<dyn super::SnapshotLifetime>>,
        admission: SnapshotAdmission,
        lifecycle: OwnedRwLockReadGuard<()>,
    ) -> Self {
        Self {
            lease: None,
            location: None,
            original,
            lifetime,
            admission: Some(admission),
            scope: Some(scope),
            lifecycle: Some(lifecycle),
        }
    }

    fn scope(&self) -> &activity::Scope {
        self.scope.as_ref().expect("construction owns its scope")
    }

    fn retain_checkout(&mut self, worktree: Option<&str>) -> Result<(), RuntimeError> {
        let bindings = SnapshotWorker::checkout_resources(
            self.scope(),
            self.lease.as_ref().expect("checkout acquisition completed"),
            worktree,
        )?;
        if let Some(lifetime) = &self.lifetime {
            lifetime
                .bind_checkouts(&bindings)
                .map_err(RuntimeError::Conflict)?;
        }
        self.location = Some(
            self.lease
                .take()
                .expect("checkout acquisition completed")
                .settle_snapshot_retained()
                .map_err(RuntimeError::Invalid)?,
        );
        SnapshotWorker::verify_checkout(
            self.location.as_deref().expect("checkout retained"),
            &bindings,
        )
    }

    fn finish(
        mut self,
        runtime: &Runtime,
        state: &SessionState,
        directory: PathBuf,
        created_id: String,
    ) -> SnapshotWorker {
        SnapshotWorker {
            sequence: state.last_seq,
            runtime: runtime.clone(),
            directory,
            created_id,
            worktree_id: state.info.worktree_id.clone(),
            _location: self.location.take(),
            _original: self.original.take(),
            _lifetime: self.lifetime.take(),
            admission: self.admission.take().expect("construction owns admission"),
            scope: self.scope.take().expect("construction owns its scope"),
            _lifecycle: self.lifecycle.take().expect("construction owns lifecycle"),
        }
    }
}

struct SnapshotAdmission(Arc<Handle>);

impl Drop for SnapshotAdmission {
    fn drop(&mut self) {
        self.0.snapshot_active.store(false, Ordering::SeqCst);
    }
}

/// Borrowed only by the actual synchronous filesystem callback; cannot be cloned.
pub struct SnapshotWork<'a> {
    owner: &'a mut SnapshotWorker,
}

pub struct SnapshotCompletion {
    // Native guard cleanup remains covered by the worker's lifetime locks.
    native: SnapshotNativeResult,
    worker: SnapshotWorker,
}

struct OriginalResources {
    origin: SnapshotOrigin,
    claims: Option<Box<dyn super::SnapshotLocationGuard>>,
    bindings: Vec<cyber_core::worktrees::CheckoutBinding>,
    // Last: keep the exited original's lock through all native claim disposal.
    lifetime: Box<dyn super::SnapshotLifetime>,
}
impl OriginalResources {
    fn verify(&self) -> Result<(), String> {
        self.lifetime.verify()?;
        if self.lifetime.checkout_bindings()?.as_ref() != Some(&self.bindings) {
            return Err("Original recovery checkout publication changed".into());
        }
        let claims = self
            .claims
            .as_ref()
            .ok_or("Original recovery claims are not retained")?;
        claims.verify()?;
        if claims.bindings()? != self.bindings {
            return Err("Original recovery checkout ownership changed".into());
        }
        super::SnapshotRestoreCheckout {
            version: 1,
            lifetime: self.lifetime.binding().clone(),
            resources_digest: self
                .lifetime
                .binding()
                .checkout_resources_digest(&self.bindings)?,
        }
        .validate(&self.origin)
    }
}

impl SnapshotWork<'_> {
    pub fn preparation_record_budget(&self, origin: &SnapshotOrigin) -> Result<u64, String> {
        let Some(lifetime) = self.owner.scope.snapshot_lifetime() else {
            return Ok(0);
        };
        let receipt = super::SnapshotPreparationReceipt {
            activity_id: lifetime.owner.activity_id.clone(),
            request_id: "\u{1}".repeat(256),
            confirmation: "0".repeat(64),
            lifetime: lifetime.clone(),
            native_sequence: 2,
            session_sequence: i64::MAX,
            origin: Some(origin.clone()),
            source_admitted: Some(false),
            checkout_resources_digest: Some("0".repeat(64)),
        };
        let bytes = serde_json::to_vec(&receipt)
            .map_err(|error| error.to_string())?
            .len() as u64;
        if bytes > super::SnapshotPreparationReceipt::MAX_ENCODED_BYTES {
            return Err("Preparation cancellation record exceeds limit".into());
        }
        Ok(bytes)
    }
    pub fn origin(&self) -> Option<&SnapshotOrigin> {
        self.owner.scope.snapshot_origin()
    }
    /// Metadata for pre-effect storage budgeting only; this does not publish or
    /// authorize the origin. Source effects still require a successful `begin`.
    pub fn preview_origin(&self, binding: SnapshotBinding) -> SnapshotOrigin {
        SnapshotOrigin {
            version: 1,
            session_id: self.owner.scope.control.source.clone(),
            session_created_id: self.owner.created_id.clone(),
            activity_id: self.owner.scope.id().to_owned(),
            worktree_id: self.owner.worktree_id.clone(),
            binding,
        }
    }
    pub fn authorize(&self) -> Result<(), String> {
        self.owner.location()?.verify()?;
        if let Some(lifetime) = &self.owner._lifetime {
            lifetime.verify()?;
        }
        if let Some(original) = &self.owner._original {
            original.verify()?;
        }
        if self.owner.scope.control.stop.is_cancelled() {
            return Err("Snapshot worker cancelled; retain its journal before retrying".into());
        }
        self.owner
            .scope
            .verify_authority()
            .map_err(|error| error.to_string())?;
        let source = self.owner.scope.control.source.clone();
        let sequence = self.owner.sequence;
        let current = self
            .owner
            .runtime
            .inner
            .store
            .aggregate_seq(&source)
            .map_err(|error| error.to_string())?;
        if current != Some(sequence) {
            return Err("Snapshot Session changed before its filesystem effect".into());
        }
        Ok(())
    }

    /// Source admission for a registered checkout within private application
    /// data. Identification alone cannot grant this: verify the actual worker's
    /// complete retained native claims on both sides of the registry read.
    pub fn authorize_managed_location(&self, directory: &std::path::Path) -> Result<(), String> {
        self.authorize()?;
        if directory != self.owner.directory {
            return Err("Managed source differs from snapshot worker Location".into());
        }
        let managed = cyber_core::worktrees::Repository::managed_locations_at(directory)
            .map_err(|error| error.to_string())?;
        let bindings = self.owner.location()?.bindings()?;
        if managed.is_empty() || managed.len() != bindings.len() {
            return Err("Managed source requires the complete native checkout set".into());
        }
        for (_, managed) in managed {
            if !managed.ready
                || !bindings.iter().any(|binding| {
                    binding.version == 2
                        && binding.worktree_id == managed.id
                        && binding.name == managed.name
                        && binding.common_dir == managed.common_dir
                        && binding.owner_id == self.owner.scope.id()
                        && binding.lifetime.as_ref().is_some_and(|owner| {
                            owner.directory == directory
                                && owner.activity_id == self.owner.scope.id()
                                && owner.session_id == self.owner.scope.control.source
                                && owner.session_created_id == self.owner.created_id
                        })
                })
            {
                return Err("Managed source differs from retained native checkout proof".into());
            }
        }
        self.authorize()
    }

    /// Keep Session changes outside the exact synchronous namespace mutation.
    /// The longer-running worker keeps a separate admission owner so Stop and
    /// read-only inspection need not wait on a held Session projection mutex.
    pub fn effect<T>(&self, effect: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        if self.origin().is_none() {
            return Err("Snapshot source effects require a durable worker origin".into());
        }
        let state = self.owner.admission.0.state.blocking_lock();
        self.authorize()?;
        self.owner
            .scope
            .admit_snapshot_source()
            .map_err(|error| error.to_string())?;
        self.authorize()?;
        if let Some(lifetime) = &self.owner._lifetime {
            lifetime.admit_source(self.origin().ok_or("Source admission lacks origin")?)?;
        }
        self.authorize()?;
        if state.last_seq != self.owner.sequence {
            return Err("Snapshot Session projection changed before its filesystem effect".into());
        }
        effect()
    }

    /// Persist immutable runtime origin before the first intent or source effect.
    pub fn begin(&mut self, binding: SnapshotBinding) -> Result<SnapshotOrigin, String> {
        self.authorize()?;
        if binding.directory.to_str().is_none() {
            return Err("Snapshot Location path cannot be represented as UTF-8".into());
        }
        if binding.directory != self.owner.directory
            || binding
                .lock_identity
                .as_ref()
                .is_some_and(|id| id.is_empty() || id.len() > 256)
            || [
                &binding.root_identity,
                &binding.namespace_identity,
                &binding.journal_id,
                &binding.journal_identity,
            ]
            .iter()
            .any(|id| id.is_empty() || id.len() > 256)
            || !binding.journal_id.starts_with("restore_")
            || !binding
                .journal_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err("Snapshot native binding differs from the admitted Location".into());
        }
        let origin = self.preview_origin(binding);
        self.owner
            .scope
            .launch_snapshot(origin.clone())
            .map_err(|error| error.to_string())?;
        Ok(origin)
    }
}

impl SnapshotWorker {
    fn checkout_resources(
        scope: &activity::Scope,
        lease: &super::LocationLease,
        worktree: Option<&str>,
    ) -> Result<Vec<cyber_core::worktrees::CheckoutBinding>, RuntimeError> {
        let bindings = lease.snapshot_bindings().map_err(RuntimeError::Conflict)?;
        let expected = scope
            .snapshot_lifetime()
            .map(super::SnapshotLifetimeBinding::checkout_owner)
            .transpose()
            .map_err(RuntimeError::Conflict)?;
        if bindings.len() > 4096
            || worktree.is_some_and(|id| !bindings.iter().any(|binding| binding.worktree_id == id))
        {
            return Err(RuntimeError::Conflict(
                "Snapshot checkout resource proof is incomplete or exceeds limits".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for binding in &bindings {
            if binding.version != if expected.is_some() { 2 } else { 1 }
                || binding.owner_id != scope.id()
                || binding.lifetime != expected
                || !seen.insert((&binding.common_dir, &binding.worktree_id))
            {
                return Err(RuntimeError::Conflict(
                    "Snapshot checkout proof has foreign lifetime or ownership".into(),
                ));
            }
        }
        Ok(bindings)
    }
    #[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
    fn checkout_proof(
        scope: &activity::Scope,
        lease: super::LocationLease,
        worktree: Option<&str>,
    ) -> Result<Box<dyn super::SnapshotLocationGuard>, RuntimeError> {
        let bindings = Self::checkout_resources(scope, &lease, worktree)?;
        let proof = lease
            .settle_snapshot_retained()
            .map_err(RuntimeError::Invalid)?;
        Self::verify_checkout(proof.as_ref(), &bindings)?;
        Ok(proof)
    }
    fn verify_checkout(
        proof: &dyn super::SnapshotLocationGuard,
        bindings: &[cyber_core::worktrees::CheckoutBinding],
    ) -> Result<(), RuntimeError> {
        proof.verify().map_err(RuntimeError::Conflict)?;
        if proof.bindings().map_err(RuntimeError::Conflict)? != bindings {
            return Err(RuntimeError::Conflict(
                "Snapshot checkout binding changed during retained settlement".into(),
            ));
        }
        Ok(())
    }
    fn location(&self) -> Result<&dyn super::SnapshotLocationGuard, String> {
        self._location
            .as_deref()
            .ok_or_else(|| "Snapshot checkout proof is unavailable".into())
    }
    /// Trusted adapters must perform only synchronous filesystem work here;
    /// spawning a process or detached task cannot use this acknowledgement proof.
    pub async fn run(
        mut self,
        work: impl FnOnce(&mut SnapshotWork<'_>) -> Result<SnapshotNativeResult, RestoreError>
        + Send
        + 'static,
    ) -> Result<SnapshotCompletion, RestoreError> {
        let _cancel_on_disposal = self.scope.control.stop.clone().drop_guard();
        let joined = tokio::task::spawn_blocking(move || {
            let native = work(&mut SnapshotWork { owner: &mut self })?;
            native
                .guard
                .verify(self.scope.snapshot_origin())
                .map_err(RestoreError::Failed)?;
            self.location()
                .map_err(RestoreError::Failed)?
                .verify()
                .map_err(RestoreError::Failed)?;
            Ok(SnapshotCompletion {
                worker: self,
                native,
            })
        });
        let completed = joined.await.map_err(|_| RestoreError::Failed(
            "Snapshot worker did not acknowledge completion; inspect retained recovery evidence".into()
        ))??;
        _cancel_on_disposal.disarm();
        Ok(completed)
    }
}

impl SnapshotCompletion {
    pub fn recovery_receipt(&self) -> Option<&super::SnapshotRecoveryReceipt> {
        self.native.recovery.as_ref()
    }
    pub(crate) fn commit_recovery(
        mut self,
        state: &SessionState,
    ) -> Result<super::SnapshotRecoveryReceipt, RuntimeError> {
        let mut receipt = self
            .native
            .recovery
            .as_ref()
            .ok_or_else(|| RuntimeError::Conflict("Missing recovery filesystem receipt".into()))?
            .clone();
        self.native
            .guard
            .verify(self.worker.scope.snapshot_origin())
            .map_err(RuntimeError::Conflict)?;
        if let Some(original) = &self.worker._original {
            original.verify().map_err(RuntimeError::Conflict)?;
        }
        if let Some(lifetime) = &self.worker._lifetime {
            lifetime.verify().map_err(RuntimeError::Conflict)?;
            let published = lifetime
                .checkout_bindings()
                .map_err(RuntimeError::Conflict)?
                .ok_or_else(|| {
                    RuntimeError::Conflict("Resolution resources are unpublished".into())
                })?;
            let claims = self.worker.location().map_err(RuntimeError::Conflict)?;
            if claims.bindings().map_err(RuntimeError::Conflict)? != published {
                return Err(RuntimeError::Conflict(
                    "Resolution checkout publication differs".into(),
                ));
            }
            let resolution = super::SnapshotRestoreCheckout {
                version: 1,
                lifetime: lifetime.binding().clone(),
                resources_digest: lifetime
                    .binding()
                    .checkout_resources_digest(&published)
                    .map_err(RuntimeError::Conflict)?,
            };
            let original = self
                .worker
                ._original
                .as_ref()
                .map(|original| {
                    Ok::<_, RuntimeError>(Box::new(super::SnapshotRestoreCheckout {
                        version: 1,
                        lifetime: original.lifetime.binding().clone(),
                        resources_digest: original
                            .lifetime
                            .binding()
                            .checkout_resources_digest(&original.bindings)
                            .map_err(RuntimeError::Conflict)?,
                    }))
                })
                .transpose()?;
            let resolution_origin = self
                .worker
                .scope
                .snapshot_origin()
                .cloned()
                .ok_or_else(|| RuntimeError::Conflict("Resolution has no native origin".into()))?;
            receipt.checkout = Some(Box::new(super::SnapshotRecoveryCheckout {
                version: 1,
                resolution_origin: resolution_origin.clone(),
                original,
                resolution: Box::new(resolution),
            }));
            receipt
                .validate_checkout(&resolution_origin)
                .map_err(RuntimeError::Conflict)?;
        }
        let location = self
            .worker
            ._location
            .take()
            .ok_or_else(|| RuntimeError::Conflict("Missing snapshot checkout proof".into()))?;
        location.verify().map_err(RuntimeError::Conflict)?;
        let claims = super::snapshot_recovery_checkout::RecoveryClaims {
            resolution: location,
            original: self
                .worker
                ._original
                .as_mut()
                .and_then(|original| original.claims.take()),
        };
        let claims =
            self.worker
                .scope
                .commit_recovery(state, self.worker.sequence, &receipt, claims)?;
        self.worker._location = Some(claims.resolution);
        if let Some(original) = &mut self.worker._original {
            original.claims = claims.original;
        }
        self.native.guard.acknowledge(self.worker.scope.snapshot_origin()).map_err(|error| RuntimeError::Conflict(format!(
            "Recovery runtime acknowledgement committed; native journal acknowledgement requires reconciliation: {error}"
        )))?;
        Ok(receipt)
    }
    pub(crate) fn commit(
        mut self,
        state: &mut SessionState,
        events: Vec<NewEvent>,
    ) -> Result<(), RuntimeError> {
        if self.native.recovery.is_some() {
            return Err(RuntimeError::Conflict(
                "Recovery requires its exact native consuming commit".into(),
            ));
        }
        self.native
            .guard
            .verify(self.worker.scope.snapshot_origin())
            .map_err(RuntimeError::Conflict)?;
        if let Some(lifetime) = &self.worker._lifetime {
            lifetime.verify().map_err(RuntimeError::Conflict)?;
            let published = lifetime
                .checkout_bindings()
                .map_err(RuntimeError::Conflict)?
                .ok_or_else(|| {
                    RuntimeError::Conflict("Restore checkout publication is missing".into())
                })?;
            if self
                .worker
                .location()
                .map_err(RuntimeError::Conflict)?
                .bindings()
                .map_err(RuntimeError::Conflict)?
                != published
            {
                return Err(RuntimeError::Conflict(
                    "Restore checkout publication differs".into(),
                ));
            }
        }
        let location = self
            .worker
            ._location
            .take()
            .ok_or_else(|| RuntimeError::Conflict("Missing snapshot checkout proof".into()))?;
        location.verify().map_err(RuntimeError::Conflict)?;
        self.worker._location = Some(self.worker.scope.commit_snapshot(
            state,
            self.worker.sequence,
            events,
            self.native.guard.intent_digest(),
            location,
        )?);
        // Fixture point: the Store writer committed; the native journal is unacknowledged.
        super::process_cut::pause("ordinary-restore-writer-commit");
        self.native
            .guard
            .acknowledge(self.worker.scope.snapshot_origin())
            .map_err(|error| RuntimeError::Conflict(format!(
                "Snapshot runtime acknowledgement committed; native journal acknowledgement requires reconciliation: {error}"
            )))
    }
}

impl Runtime {
    pub(super) async fn snapshot_recovery_worker(
        &self,
        handle: &Arc<Handle>,
        state: &SessionState,
        mut request: super::snapshot_recovery_types::RecoveryRequest,
    ) -> Result<SnapshotWorker, RuntimeError> {
        let lifecycle = self.memory_mutation_lease().await?;
        self.inner.ensure_idle(&state.info.id)?;
        self.ensure_no_background(&state.info.id)?;
        if self
            .inner
            .activities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .any(|control| control.source == state.info.id)
        {
            return Err(RuntimeError::Busy(state.info.id.clone()));
        }
        state.ensure_worktree_ready()?;
        if request.original.session_id != state.info.id
            || request.original.worktree_id != state.info.worktree_id
        {
            return Err(RuntimeError::Conflict(
                "Recovery Session/checkout changed".into(),
            ));
        }
        handle
            .snapshot_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| RuntimeError::Busy(state.info.id.clone()))?;
        let admission = SnapshotAdmission(handle.clone());
        let authority =
            super::snapshot_recovery_authority::RecoveryAuthority::capture(self, &state.info.id)?;
        if self.inner.store.aggregate_seq(&state.info.id)? != Some(state.last_seq) {
            return Err(RuntimeError::Conflict(
                "Recovery Session changed before worker admission".into(),
            ));
        }
        request.authority = Some(authority.clone());
        let id = super::snapshot_recovery::request_owner(
            &request.original.session_created_id,
            &request.confirmation.request_id,
        );
        let original_origin = request.original.clone();
        let original_lifetime = self.inner.store.read(move |db| {
            let (_, original) = super::snapshot_recovery_record::attest(db, &original_origin)?;
            match original.snapshot_lifetime {
                Some(binding) if binding.version == 2 => Ok(Some(binding)),
                None => Ok(None),
                Some(binding) if binding.version == 1 => Ok(None),
                Some(_) => Err(cyber_store::StoreError::InvalidAppend(
                    "Unsupported original recovery lifetime version".into(),
                )),
            }
        })?;
        let original = if let Some(binding) = original_lifetime {
            let lifetime = self
                .inner
                .options
                .snapshots
                .lock_worker(binding.clone())
                .await
                .map_err(RuntimeError::Conflict)?;
            if lifetime.binding() != &binding {
                return Err(RuntimeError::Conflict(
                    "Original recovery lifetime differs".into(),
                ));
            }
            let bindings = lifetime
                .checkout_bindings()
                .map_err(RuntimeError::Conflict)?
                .ok_or_else(|| {
                    RuntimeError::Conflict(
                        "Original recovery resource publication is missing".into(),
                    )
                })?;
            let claims = self
                .inner
                .tools
                .reacquire_snapshot_location(&state.info, &binding, &bindings)
                .map_err(RuntimeError::Conflict)?;
            let original = OriginalResources {
                origin: request.original.clone(),
                claims: Some(claims),
                bindings,
                lifetime,
            };
            original.verify().map_err(RuntimeError::Conflict)?;
            Some(original)
        } else {
            None
        };
        let lifetime = self.snapshot_lifetime(state, &id).await?;
        if original.is_some() && lifetime.is_none() {
            return Err(RuntimeError::Conflict(
                "Resolution lifetime proof is unavailable".into(),
            ));
        }
        let scope = activity::Scope::reserve_recovery(
            self,
            handle.clone(),
            id,
            request.clone(),
            authority,
            lifetime.as_ref().map(|owner| owner.binding().clone()),
        )?;
        let mut construction =
            SnapshotConstruction::new(scope, original, lifetime, admission, lifecycle);
        construction.lease = Some(
            self.inner
                .claim_snapshot_location(
                    &state.info,
                    construction.scope().id(),
                    construction.scope().snapshot_lifetime(),
                    construction.scope().control.stop.clone(),
                )
                .await?,
        );
        let directory = std::fs::canonicalize(&state.info.directory)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        let created_id = self
            .inner
            .store
            .read_events(&state.info.id, -1, 1)?
            .events
            .first()
            .ok_or_else(|| RuntimeError::Conflict("Recovery Session origin missing".into()))?
            .id
            .clone();
        if directory != request.original.binding.directory
            || created_id != request.original.session_created_id
            || construction
                .lease
                .as_ref()
                .expect("checkout acquired")
                .worktree_id
                != request.original.worktree_id
        {
            return Err(RuntimeError::Conflict(
                "Recovery native Location/creation binding changed".into(),
            ));
        }
        construction.retain_checkout(state.info.worktree_id.as_deref())?;
        Ok(construction.finish(self, state, directory, created_id))
    }
    pub(crate) async fn snapshot_worker(
        &self,
        handle: &Arc<Handle>,
        state: &SessionState,
    ) -> Result<SnapshotWorker, RuntimeError> {
        let lifecycle = self.memory_mutation_lease().await?;
        self.inner.ensure_idle(&state.info.id)?;
        self.ensure_no_background(&state.info.id)?;
        self.ensure_user_child_outcome(handle, state)?;
        self.inner.ensure_admission_open(&state.info.id)?;
        handle
            .snapshot_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| RuntimeError::Busy(state.info.id.clone()))?;
        let admission = SnapshotAdmission(handle.clone());
        if self.inner.store.aggregate_seq(&state.info.id)? != Some(state.last_seq) {
            return Err(RuntimeError::Conflict(
                "Snapshot Session changed before worker admission".into(),
            ));
        }
        let id = cyber_core::ids::new_id("op");
        let lifetime = self.snapshot_lifetime(state, &id).await?;
        let scope = activity::Scope::reserve_snapshot_owned(
            &self.inner,
            state.info.id.clone(),
            id,
            lifetime.as_ref().map(|owner| owner.binding().clone()),
        )
        .await?;
        let mut construction =
            SnapshotConstruction::new(scope, None, lifetime, admission, lifecycle);
        construction.lease = Some(
            self.inner
                .claim_snapshot_location(
                    &state.info,
                    construction.scope().id(),
                    construction.scope().snapshot_lifetime(),
                    construction.scope().control.stop.clone(),
                )
                .await?,
        );
        if construction
            .lease
            .as_ref()
            .expect("checkout acquired")
            .worktree_id
            != state.info.worktree_id
        {
            return Err(RuntimeError::Conflict(
                "Snapshot checkout binding changed".into(),
            ));
        }
        let directory = std::fs::canonicalize(&state.info.directory)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        let created_id = self
            .inner
            .store
            .read_events(&state.info.id, -1, 1)?
            .events
            .first()
            .ok_or_else(|| RuntimeError::Conflict("Snapshot Session origin is missing".into()))?
            .id
            .clone();
        construction.retain_checkout(state.info.worktree_id.as_deref())?;
        Ok(construction.finish(self, state, directory, created_id))
    }
}

impl Inner {
    pub(super) fn ensure_snapshot_admission(
        &self,
        handle: &Handle,
        session: &str,
    ) -> Result<(), RuntimeError> {
        if handle.snapshot_active.load(Ordering::SeqCst) {
            return Err(RuntimeError::Busy(session.into()));
        }
        let source = session.to_owned();
        let unresolved = self.store.read(move |db| {
            Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM event e WHERE e.type='native.activity.changed.1' AND json_extract(e.data,'$.session_id')=?1 AND json_extract(e.data,'$.snapshot_worker')=1 AND json_extract(e.data,'$.status')<>'settled' AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq))", [source], |row| row.get::<_, bool>(0))?)
        })?;
        if unresolved {
            return Err(RuntimeError::Conflict(
                "Snapshot worker outcome requires owned recovery before new input".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
#[path = "snapshot_work_tests.rs"]
mod tests;
