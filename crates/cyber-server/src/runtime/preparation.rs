//! Child preparation stays owned until its native work and admission acknowledge cancellation.

use std::sync::{Arc, PoisonError, Weak, atomic::Ordering};
use std::time::Duration;

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use super::{
    ChildContinuation, ChildExecution, DrainEntry, Handle, Inner, Runtime, RuntimeError,
    SessionState,
};

pub(super) struct Control {
    pub cancel: CancellationToken,
    pub done: CancellationToken,
}

struct Scope {
    authority: super::AdmissionAuthority,
    owner: Option<ChildExecution>,
    inner: Weak<Inner>,
    id: String,
    pause_seq: i64,
    control: Arc<Control>,
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.control.cancel.cancel();
        drop(self.owner.take());
        if let Some(inner) = self.inner.upgrade() {
            let mut active = inner
                .child_preparations
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if active
                .get(&self.id)
                .is_some_and(|control| Arc::ptr_eq(control, &self.control))
            {
                active.remove(&self.id);
            }
        }
        self.control.done.cancel();
    }
}

pub(super) struct PreparedChild {
    settlement: Option<Box<dyn ChildContinuation>>,
    scope: Scope,
}

impl PreparedChild {
    pub fn check(&self, state: &SessionState) -> Result<(), RuntimeError> {
        if self.scope.control.cancel.is_cancelled() || state.child_pause_seq != self.scope.pause_seq
        {
            return Err(interrupted());
        }
        let runtime = Runtime {
            inner: self
                .scope
                .inner
                .upgrade()
                .ok_or(RuntimeError::ShuttingDown)?,
        };
        self.scope.authority.verify(&runtime, &self.scope.id)?;
        state.ensure_worktree_ready()
    }

    pub fn admission_bindings(&self) -> Option<Vec<super::admission_authority::Binding>> {
        Some(self.scope.authority.bindings.clone())
    }

    pub fn launch(
        self,
        inner: &Arc<Inner>,
        drains: &mut std::collections::HashMap<String, DrainEntry>,
        id: &str,
    ) {
        let Self {
            settlement,
            mut scope,
        } = self;
        let owner = scope.owner.take().expect("owned preparation");
        super::continuation::launch_child(inner, drains, id, owner, settlement);
        drop(scope);
    }
}

fn interrupted() -> RuntimeError {
    RuntimeError::Invalid("Child continuation preparation was interrupted".into())
}

impl Runtime {
    pub(super) async fn pause_requested_child_input(
        &self,
        id: &str,
    ) -> (Result<(), RuntimeError>, Option<Arc<Control>>) {
        let handle = match self.inner.handle(id).await {
            Ok(handle) => handle,
            Err(RuntimeError::SessionNotFound(_)) => return (Ok(()), None),
            Err(error) => return (Err(error), None),
        };
        let mut state = handle.state.lock().await;
        let preparation = self
            .inner
            .child_preparations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let paused = {
            self.inner
                .commit_locked(
                    &mut state,
                    vec![super::event(
                        super::events::CHILD_INPUT_PAUSED,
                        &serde_json::json!({"reason":"interrupt"}),
                    )],
                )
                .map(|_| ())
        };
        if let Some(control) = &preparation {
            control.cancel.cancel();
        }
        (paused, preparation)
    }

    pub(super) async fn prepare_user_child(
        &self,
        handle: &Handle,
        state: &SessionState,
    ) -> Result<PreparedChild, RuntimeError> {
        let admission = self.inner.open().await?;
        let parent = state
            .info
            .parent_id
            .as_deref()
            .ok_or_else(|| RuntimeError::Invalid("Not a child Session".into()))?;
        let owner = self
            .claim_child_execution(parent, &state.info.id)
            .map_err(|_| RuntimeError::Busy(state.info.id.clone()))?;
        let current = handle.state.lock().await;
        if current.child_pause_seq != state.child_pause_seq {
            return Err(interrupted());
        }
        self.ensure_user_child_outcome(handle, &current)?;
        let control = Arc::new(Control {
            cancel: self.inner.closed.child_token(),
            done: CancellationToken::new(),
        });
        let scope = Scope {
            authority: self.capture_child_admission(&state.info.id)?,
            owner: Some(owner),
            inner: Arc::downgrade(&self.inner),
            id: state.info.id.clone(),
            pause_seq: state.child_pause_seq,
            control: control.clone(),
        };
        self.inner
            .child_preparations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(state.info.id.clone(), control.clone());
        let (send, receive) = tokio::sync::oneshot::channel();
        let weak = self.downgrade();
        let state = state.clone();
        let task = tokio::spawn(async move {
            let result = if let Some(runtime) = weak.upgrade() {
                runtime.prepare_owned_child(state, scope).await
            } else {
                Err(RuntimeError::ShuttingDown)
            };
            let _ = send.send(result);
        });
        {
            let mut tasks = self
                .inner
                .background
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            tasks.retain(|task| !task.is_finished());
            tasks.push(task);
        }
        drop(current);
        drop(admission);
        let on_disposal = control.cancel.clone().drop_guard();
        let result = receive
            .await
            .map_err(|_| RuntimeError::Invalid("Child preparation owner was lost".into()))?;
        if result.is_ok() {
            on_disposal.disarm();
        }
        result
    }

    async fn prepare_owned_child(
        &self,
        state: SessionState,
        scope: Scope,
    ) -> Result<PreparedChild, RuntimeError> {
        let handle = self.inner.handle(&state.info.id).await?;
        let cancel = scope.control.cancel.clone();
        if cancel.is_cancelled() {
            return Err(interrupted());
        }
        let outcome = {
            let work =
                std::panic::AssertUnwindSafe(scope.authority.clone().run(cancel.clone(), async {
                    let parent = self
                        .state(state.info.parent_id.as_deref().expect("owned child"))
                        .await?
                        .info;
                    let settlement = self
                        .inner
                        .tools
                        .prepare_child_continuation(
                            &parent,
                            &state,
                            scope.owner.as_ref().expect("owned preparation"),
                            cancel.child_token(),
                        )
                        .await
                        .map_err(RuntimeError::Invalid)?;
                    let current = handle.state.lock().await.clone();
                    self.ensure_user_child_ready(&handle, &current, cancel.child_token())
                        .await?;
                    Ok::<_, RuntimeError>(settlement)
                }))
                .catch_unwind();
            tokio::pin!(work);
            tokio::select! {
                result = &mut work => Some(result),
                _ = cancel.cancelled() => tokio::time::timeout(Duration::from_secs(2), &mut work).await.ok(),
            }
        };
        let settlement = match outcome {
            Some(Ok(result)) => result?,
            None | Some(Err(_)) => {
                let message =
                    "Child preparation did not acknowledge completion; recovery is required";
                self.mark_child_preparation_unknown(&handle, message)
                    .await?;
                return Err(RuntimeError::Invalid(message.into()));
            }
        };
        if cancel.is_cancelled() {
            return Err(interrupted());
        }
        let prepared = PreparedChild { settlement, scope };
        {
            let current = handle.state.lock().await;
            prepared.check(&current)?;
        }
        Ok(prepared)
    }

    pub(super) async fn mark_child_preparation_unknown(
        &self,
        handle: &Handle,
        message: &str,
    ) -> Result<(), RuntimeError> {
        handle.location_uncertain.store(true, Ordering::SeqCst);
        self.inner.commit(handle, vec![super::event(super::events::CHILD_CONTINUATION_SETTLED, &serde_json::json!({
            "completed":false, "worktree":null, "error":message, "unknown":true, "phase":"preparation",
        }))]).await?;
        Ok(())
    }
}
