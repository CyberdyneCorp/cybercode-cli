//! Owned guard for tool-host inference, including cancellation while opening a stream.
use super::{accounting::Attempt, dispatch::GatedAdapter};
use crate::runtime::{Asker, Handle, Inner, ResolvedModel, RuntimeError};
use cyber_llm::{Adapter, EventStream, LlmError, LlmRequest};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex, PoisonError};

/// A host inference owner. Create before entering cancellation/timeout selects,
/// flush afterwards, and omit legacy billing when reservation_owned() is true.
pub struct GuardedModel {
    inner: Arc<Inner>,
    handle: Arc<Handle>,
    resolved: ResolvedModel,
    purpose: &'static str,
    call_id: Option<String>,
    origin: crate::runtime::usage_origin::StepOrigin,
    reserved: Arc<std::sync::atomic::AtomicBool>,
    attempts: Arc<Mutex<Vec<Arc<Attempt>>>>,
    failure: Arc<Mutex<Option<RuntimeError>>>,
}
impl GuardedModel {
    pub fn reservation_owned(&self) -> bool {
        self.reserved.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn request_ids(&self) -> Vec<String> {
        self.attempts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|a| a.request_id.clone())
            .collect()
    }
    pub async fn flush(&self) -> Result<(), RuntimeError> {
        let attempts = self
            .attempts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for attempt in attempts {
            attempt.flush().await?;
        }
        if let Some(error) = self
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            return Err(error);
        }
        Ok(())
    }
}
impl Adapter for GuardedModel {
    fn stream(&self, request: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>> {
        Box::pin(async move {
            let mut gate =
                GatedAdapter::new(&self.inner, &self.handle, &self.resolved, self.purpose)
                    .for_origin(self.origin.clone());
            if let Some(id) = &self.call_id {
                gate = gate.for_call(id.clone());
            }
            gate.attempts = Arc::clone(&self.attempts);
            gate.failure = Arc::clone(&self.failure);
            gate.ownership = Some(Arc::clone(&self.reserved));
            gate.stream(request).await
        })
    }
}
impl Asker {
    pub async fn guard_model(
        &self,
        resolved: &ResolvedModel,
        purpose: &'static str,
        call_id: Option<String>,
    ) -> Result<GuardedModel, RuntimeError> {
        let inner = self.inner.upgrade().ok_or(RuntimeError::ShuttingDown)?;
        let handle = inner.handle(&self.session_id).await?;
        inner.activate_reserved_budget(&handle).await?;
        let (directory, fallback) = {
            let state = handle.state.lock().await;
            let running = inner
                .drains
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&self.session_id);
            (
                state.info.directory.clone(),
                if running {
                    state.step_origin.clone()
                } else {
                    Default::default()
                },
            )
        };
        let origin = captured_origin(&inner.store, &self.session_id, &self.message_id, fallback)?;
        let daily = inner
            .tools
            .daily_budget(&directory)
            .map_err(RuntimeError::Invalid)?;
        let source = self.session_id.clone();
        let reserved = inner
            .store
            .transaction(move |tx| {
                super::scopes::collect(
                    tx,
                    &source,
                    daily.as_ref(),
                    chrono::Utc::now().timestamp_millis(),
                )
                .map(|(scopes, _)| !scopes.is_empty())
            })
            .map_err(super::error)?;
        Ok(GuardedModel {
            inner,
            handle,
            resolved: resolved.clone(),
            purpose,
            call_id,
            origin,
            reserved: Arc::new(std::sync::atomic::AtomicBool::new(reserved)),
            attempts: Arc::default(),
            failure: Arc::default(),
        })
    }
}

/// A tool's Asker retains its owning message even when a later input is promoted.
/// Resolve that immutable step rather than borrowing the Session's current origin.
fn captured_origin(
    store: &cyber_store::Store,
    session: &str,
    message: &str,
    fallback: crate::runtime::usage_origin::StepOrigin,
) -> Result<crate::runtime::usage_origin::StepOrigin, RuntimeError> {
    if message.is_empty() {
        return Ok(fallback);
    }
    let (session, message) = (session.to_owned(), message.to_owned());
    store
        .read(move |db| {
            crate::runtime::usage_origin::recorded_message_origin(db, &session, &message)
                .map(|origin| origin.unwrap_or_default())
        })
        .map_err(RuntimeError::from)
}
