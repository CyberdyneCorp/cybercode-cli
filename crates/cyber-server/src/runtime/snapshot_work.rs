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

pub struct SnapshotRestoreRequest {
    pub directory: String,
    pub target: String,
    pub recorded: String,
    pub expected: String,
    pub paths: Option<Vec<String>>,
}

/// Native locks and descriptors held through the consuming runtime commit.
pub trait SnapshotNativeGuard: Send {
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
    // Scope disposal records unknown evidence before releasing Session ownership.
    scope: activity::Scope,
    admission: SnapshotAdmission,
    sequence: i64,
    runtime: Runtime,
    directory: PathBuf,
    created_id: String,
    worktree_id: Option<String>,
    _location: Box<dyn Send>,
    _lifecycle: OwnedRwLockReadGuard<()>,
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
    worker: SnapshotWorker,
    native: SnapshotNativeResult,
}

impl SnapshotWork<'_> {
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
        if self.owner.scope.control.stop.is_cancelled() {
            return Err("Snapshot worker cancelled; retain its journal before retrying".into());
        }
        self.owner
            .scope
            .authority()
            .verify(&self.owner.runtime, &self.owner.scope.control.source)
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

    /// Keep Session changes outside the exact synchronous namespace mutation.
    /// The longer-running worker keeps a separate admission owner so Stop and
    /// read-only inspection need not wait on a held Session projection mutex.
    pub fn effect<T>(&self, effect: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        if self.origin().is_none() {
            return Err("Snapshot source effects require a durable worker origin".into());
        }
        let state = self.owner.admission.0.state.blocking_lock();
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
        self.worker
            .scope
            .commit_snapshot(state, self.worker.sequence, events)?;
        self.native
            .guard
            .acknowledge(self.worker.scope.snapshot_origin())
            .map_err(|error| RuntimeError::Conflict(format!(
                "Snapshot runtime acknowledgement committed; native journal acknowledgement requires reconciliation: {error}"
            )))
    }
}

impl Runtime {
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
        let scope = activity::Scope::reserve_snapshot(&self.inner, state.info.id.clone()).await?;
        let lease = self
            .inner
            .claim_location(&state.info, false, scope.control.stop.clone())
            .await?;
        if lease.worktree_id != state.info.worktree_id {
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
        let worktree_id = state.info.worktree_id.clone();
        Ok(SnapshotWorker {
            scope,
            runtime: self.clone(),
            directory,
            created_id,
            worktree_id,
            _location: lease.settle_retained().map_err(RuntimeError::Invalid)?,
            _lifecycle: lifecycle,
            admission,
            sequence: state.last_seq,
        })
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
