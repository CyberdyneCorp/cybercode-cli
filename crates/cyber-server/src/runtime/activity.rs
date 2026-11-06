//! Durable idle-operation ownership; disposal is not native acknowledgement.
use super::{AdmissionAuthority, Handle, Inner, Runtime, RuntimeError};
use cyber_store::{Expected, NewEvent, Store};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, PoisonError, Weak, atomic::Ordering};
use tokio_util::sync::CancellationToken;

pub(super) const CHANGED: &str = "native.activity.changed.1";
tokio::task_local! { static CANCEL: CancellationToken; }

pub(super) struct Control {
    pub source: String,
    pub binary_document: Option<String>,
    pub snapshot_worker: bool,
    pub admission_sessions: std::collections::HashSet<String>,
    pub stop: CancellationToken,
    pub done: CancellationToken,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Record {
    pub session_id: String,
    pub status: String,
    pub phase: String,
    /// Trusted synchronous filesystem-only workers; no subprocess can escape this scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_document: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub snapshot_worker: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_origin: Option<super::SnapshotOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_bindings: Option<Vec<super::admission_authority::Binding>>,
}
pub(super) struct Scope {
    id: String,
    inner: Weak<Inner>,
    store: Arc<Store>,
    handle: Arc<Handle>,
    authority: AdmissionAuthority,
    record: Record,
    pub control: Arc<Control>,
    launched: bool,
    settled: bool,
}
impl Scope {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn authority(&self) -> AdmissionAuthority {
        self.authority.clone()
    }
    pub async fn reserve(inner: &Inner, handle: &Handle) -> Result<Self, RuntimeError> {
        Self::reserve_kind(inner, handle, None).await
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub async fn reserve_binary(
        inner: &Inner,
        handle: &Handle,
        document: String,
    ) -> Result<Self, RuntimeError> {
        Self::reserve_kind(inner, handle, Some(document)).await
    }
    async fn reserve_kind(
        inner: &Inner,
        handle: &Handle,
        binary_document: Option<String>,
    ) -> Result<Self, RuntimeError> {
        let source = handle.state.lock().await.info.id.clone();
        Self::reserve_owner(inner, source, binary_document, false).await
    }
    pub(super) async fn reserve_snapshot(
        inner: &Inner,
        source: String,
    ) -> Result<Self, RuntimeError> {
        Self::reserve_owner(inner, source, None, true).await
    }
    async fn reserve_owner(
        inner: &Inner,
        source: String,
        binary_document: Option<String>,
        snapshot_worker: bool,
    ) -> Result<Self, RuntimeError> {
        let runtime = Runtime {
            inner: inner.me.upgrade().ok_or(RuntimeError::ShuttingDown)?,
        };
        let authority = runtime.capture_child_admission(&source)?;
        let owned_handle = runtime.inner.handle(&source).await?;
        let id = cyber_core::ids::new_id("op");
        let record = Record {
            session_id: source.clone(),
            status: "pending".into(),
            phase: "reserved".into(),
            binary_document,
            snapshot_worker,
            snapshot_origin: None,
            admission_bindings: Some(authority.bindings.clone()),
        };
        inner
            .store
            .append(&id, Expected::Seq(-1), vec![change(&record)])?;
        let control = Arc::new(Control {
            source: source.clone(),
            binary_document: record.binary_document.clone(),
            snapshot_worker,
            admission_sessions: super::admission_authority::binding_sessions(&authority.bindings),
            stop: inner.closed.child_token(),
            done: CancellationToken::new(),
        });
        inner
            .activities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), control.clone());
        Ok(Self {
            id,
            inner: Arc::downgrade(&runtime.inner),
            store: inner.store.clone(),
            handle: owned_handle,
            authority,
            record,
            control,
            launched: false,
            settled: false,
        })
    }
    pub fn launch(&mut self) -> Result<(), RuntimeError> {
        let runtime = Runtime {
            inner: self.inner.upgrade().ok_or(RuntimeError::ShuttingDown)?,
        };
        self.authority.verify(&runtime, &self.control.source)?;
        if self.control.stop.is_cancelled() {
            return Err(RuntimeError::Invalid(
                "Idle operation cancelled before launch".into(),
            ));
        }
        self.record.phase = "launching".into();
        self.store
            .append(&self.id, Expected::Seq(0), vec![change(&self.record)])?;
        self.launched = true;
        Ok(())
    }
    pub(super) fn launch_snapshot(
        &mut self,
        origin: super::SnapshotOrigin,
    ) -> Result<(), RuntimeError> {
        if !self.record.snapshot_worker
            || self.record.snapshot_origin.is_some()
            || self.launched
            || origin.version != 1
            || origin.binding.directory.to_str().is_none()
            || origin.activity_id != self.id
            || origin.session_id != self.control.source
        {
            return Err(RuntimeError::Conflict(
                "Snapshot origin cannot replace another activity".into(),
            ));
        }
        self.record.snapshot_origin = Some(origin);
        self.launch()
    }
    pub(super) fn snapshot_origin(&self) -> Option<&super::SnapshotOrigin> {
        self.launched
            .then_some(self.record.snapshot_origin.as_ref())
            .flatten()
    }
    pub(super) fn commit_snapshot(
        &mut self,
        state: &mut super::SessionState,
        sequence: i64,
        events: Vec<NewEvent>,
    ) -> Result<(), RuntimeError> {
        let runtime = Runtime {
            inner: self.inner.upgrade().ok_or(RuntimeError::ShuttingDown)?,
        };
        self.authority.verify(&runtime, &self.control.source)?;
        if state.last_seq != sequence
            || !self.record.snapshot_worker
            || state.info.id != self.control.source
            || self.control.stop.is_cancelled()
        {
            return Err(RuntimeError::Conflict(
                "Snapshot commit authority changed".into(),
            ));
        }
        let mut settled = self.record.clone();
        settled.status = "settled".into();
        settled.phase = "snapshot_restore_committed".into();
        settled.admission_bindings = None;
        let bindings = self.authority.bindings.clone();
        let acknowledgement = change(&settled);
        let owners = [
            (state.info.id.clone(), state.last_seq),
            (self.id.clone(), if self.launched { 1 } else { 0 }),
        ];
        let (stored, ()) = self.store.append_pair_checked(owners, move |tx| {
            if !super::admission_authority::bindings_current(tx, &bindings)? {
                return Err(cyber_store::StoreError::InvalidAppend(
                    super::admission_authority::STALE.into(),
                ));
            }
            Ok(([events, vec![acknowledgement]], ()))
        })?;
        self.record = settled;
        self.settled = true;
        for event in &stored[0] {
            state.apply(event).map_err(RuntimeError::Corrupt)?;
        }
        runtime.inner.publish(&stored[0]);
        Ok(())
    }
    pub fn settle(&mut self) -> Result<(), RuntimeError> {
        self.record.status = "settled".into();
        self.record.admission_bindings = None;
        self.store
            .append(&self.id, Expected::Any, vec![change(&self.record)])?;
        self.settled = true;
        Ok(())
    }
    pub async fn run<T>(&self, future: impl std::future::Future<Output = T>) -> T {
        CANCEL
            .scope(
                self.control.stop.clone(),
                self.authority
                    .clone()
                    .with_cancellation(self.control.done.clone())
                    .scope(self.control.stop.clone(), future),
            )
            .await
    }
    fn dispose(&self, record: Record) -> Result<bool, cyber_store::StoreError> {
        let id = self.id.clone();
        let original = self.record.clone();
        let sequence = if self.launched { 1 } else { 0 };
        let (_, uncertain) = self.store.append_checked(&self.id, Expected::Any, move |tx| {
            let latest = tx.query_row(
                "SELECT seq,type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
                [&id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            ).optional()?;
            let Some((seq, kind, data)) = latest else {
                return Err(cyber_store::StoreError::InvalidAppend("Native activity disappeared before disposal".into()));
            };
            let current: Record = serde_json::from_str(&data).map_err(|error|
                cyber_store::StoreError::CorruptEvent { id: id.clone(), reason: error.to_string() })?;
            // An unsuccessful launch may have set the in-memory origin without
            // publishing it. Its durable reservation must still have no origin.
            let origin = if sequence == 1 { original.snapshot_origin.as_ref() } else { None };
            if kind != CHANGED || current.session_id != original.session_id
                || current.binary_document != original.binary_document
                || current.snapshot_worker != original.snapshot_worker
                || current.snapshot_origin.as_ref() != origin
            {
                return Err(cyber_store::StoreError::InvalidAppend("Native activity ownership changed before disposal".into()));
            }
            // Recovery may have committed after the native lock was released but
            // before this scope unwound. Preserve the complete durable receipt.
            if current.status == "settled" {
                return Ok((vec![], false));
            }
            if seq != sequence {
                return Err(cyber_store::StoreError::Concurrency { aggregate: id, expected: sequence, actual: seq });
            }
            let uncertain = record.status == "unknown";
            Ok((vec![change(&record)], uncertain))
        })?;
        Ok(uncertain)
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        self.control.stop.cancel();
        if !self.settled {
            let filesystem_only = self.record.binary_document.is_some();
            // This trusted scope owns only synchronous filesystem calls. Once
            // its actual worker unwinds, it has no escaped task/process to settle;
            // possible file effects remain in the separate document intent.
            let mut disposed = self.record.clone();
            disposed.snapshot_origin = self.snapshot_origin().cloned();
            disposed.status = if self.launched && !filesystem_only {
                "unknown"
            } else {
                "settled"
            }
            .into();
            if filesystem_only {
                disposed.phase = "filesystem_worker_disposed_query_document".into();
            }
            disposed.admission_bindings = None;
            let result = self.dispose(disposed);
            if self.launched && !filesystem_only && result.as_ref().copied().unwrap_or(true) {
                self.handle.location_uncertain.store(true, Ordering::SeqCst);
            }
            if let Err(error) = result {
                cyber_core::log::error(
                    "activity",
                    &error.to_string(),
                    serde_json::json!({"operation_id":self.id,"session_id":self.control.source}),
                );
            }
        }
        if let Some(inner) = self.inner.upgrade() {
            inner
                .activities
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&self.id);
        }
        self.control.done.cancel();
    }
}
fn change(record: &Record) -> NewEvent {
    NewEvent::new(
        CHANGED,
        serde_json::to_value(record).expect("activity serializes"),
    )
}
impl Inner {
    pub(super) fn cancel_idle_activities(&self, source: &str) -> Vec<Arc<Control>> {
        let controls: Vec<_> = self
            .activities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|control| control.source == source)
            .cloned()
            .collect();
        for control in &controls {
            control.stop.cancel();
        }
        controls
    }
    pub(super) async fn wait_idle_activities(
        &self,
        source: &str,
        controls: Vec<Arc<Control>>,
    ) -> Result<(), RuntimeError> {
        if controls.is_empty() {
            return Ok(());
        }
        let filesystem_only = controls
            .iter()
            .all(|control| control.binary_document.is_some() || control.snapshot_worker);
        let snapshot_worker = controls.iter().any(|control| control.snapshot_worker);
        let acknowledged = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            for control in controls {
                control.done.cancelled().await;
            }
        })
        .await;
        let handle = self.handle(source).await?;
        if acknowledged.is_err() {
            if !filesystem_only {
                handle.location_uncertain.store(true, Ordering::SeqCst);
            }
            return Err(RuntimeError::Invalid(
                if filesystem_only && snapshot_worker {
                    "Snapshot filesystem work is still settling; inspect recovery evidence before retrying".into()
                } else if filesystem_only {
                    "Binary filesystem work is still settling; query its operation before retrying"
                        .into()
                } else {
                    "Idle operation did not acknowledge interruption; recovery is required".into()
                },
            ));
        }
        if handle.location_uncertain.load(Ordering::SeqCst) {
            return Err(RuntimeError::Invalid(
                "Idle operation did not acknowledge interruption; recovery is required".into(),
            ));
        }
        Ok(())
    }
}
pub(super) fn cancellation(fallback: &CancellationToken) -> CancellationToken {
    CANCEL
        .try_with(Clone::clone)
        .unwrap_or_else(|_| fallback.child_token())
}
