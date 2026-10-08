//! Durable idle-operation ownership; disposal is not native acknowledgement.
use super::{AdmissionAuthority, Handle, Inner, Runtime, RuntimeError};
use cyber_store::{Expected, NewEvent, Store};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, PoisonError, Weak, atomic::Ordering};
use tokio_util::sync::CancellationToken;

pub(super) const CHANGED: &str = "native.activity.changed.1";
tokio::task_local! { static CANCEL: CancellationToken; }

pub(super) struct Control {
    pub source: String,
    pub admission_sessions: std::collections::HashSet<String>,
    pub stop: CancellationToken,
    pub done: CancellationToken,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Record {
    pub session_id: String,
    pub status: String,
    pub phase: String,
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
        let runtime = Runtime {
            inner: inner.me.upgrade().ok_or(RuntimeError::ShuttingDown)?,
        };
        let source = handle.state.lock().await.info.id.clone();
        let authority = runtime.capture_child_admission(&source)?;
        let owned_handle = runtime.inner.handle(&source).await?;
        let id = cyber_core::ids::new_id("op");
        let record = Record {
            session_id: source.clone(),
            status: "pending".into(),
            phase: "reserved".into(),
            admission_bindings: Some(authority.bindings.clone()),
        };
        inner
            .store
            .append(&id, Expected::Seq(-1), vec![change(&record)])?;
        let control = Arc::new(Control {
            source: source.clone(),
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
}
impl Drop for Scope {
    fn drop(&mut self) {
        self.control.stop.cancel();
        if !self.settled {
            if self.launched {
                self.handle.location_uncertain.store(true, Ordering::SeqCst);
            }
            self.record.status = if self.launched { "unknown" } else { "settled" }.into();
            self.record.admission_bindings = None;
            if let Err(error) =
                self.store
                    .append(&self.id, Expected::Any, vec![change(&self.record)])
            {
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
        let acknowledged = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            for control in controls {
                control.done.cancelled().await;
            }
        })
        .await;
        let handle = self.handle(source).await?;
        if acknowledged.is_err() {
            handle.location_uncertain.store(true, Ordering::SeqCst);
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
