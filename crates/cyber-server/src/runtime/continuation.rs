//! Explicit user continuation starts a fresh child attempt without stealing tool ownership.

use std::sync::{Arc, PoisonError, atomic::Ordering};

use tokio_util::sync::CancellationToken;

use super::events::{ADMITTED, Admitted, RESUMED};
use super::{
    Admission, CallStatus, ChildExecution, Delivery, DrainEntry, Handle, Receipt, Runtime,
    RuntimeError, SessionState, digest, drain, event, existing_receipt, names::Resumed,
    receipt_for,
};

impl Runtime {
    /// Public user input may continue an idle child, while active input remains steering.
    pub async fn admit_user(
        &self,
        id: &str,
        mut admission: Admission,
    ) -> Result<Receipt, RuntimeError> {
        let lifecycle = self.inner.open().await?;
        let handle = self.inner.handle(id).await?;
        let state = handle.state.lock().await.clone();
        let message = admission
            .message_id
            .get_or_insert_with(|| cyber_core::ids::new_id("msg"))
            .clone();
        let digest = digest(&admission.parts, admission.delivery);
        if let Some(receipt) = existing_receipt(&state, &message, &digest)? {
            return Ok(receipt);
        }
        let Some(parent) = &state.info.parent_id else {
            drop(lifecycle);
            return self.admit(id, admission).await;
        };
        if self.is_running(id) || !admission.resume || admission.delivery == Delivery::Hold {
            drop(lifecycle);
            return self.admit(id, admission).await;
        }
        let owner = self
            .claim_child_execution(parent, id)
            .map_err(|_| RuntimeError::Busy(id.into()))?;
        self.ensure_user_child_ready(&handle, &state).await?;
        self.inner.commit_staged_revert(&handle).await?;
        let mut state = handle.state.lock().await;
        if let Some(receipt) = existing_receipt(&state, &message, &digest)? {
            return Ok(receipt);
        }
        state.ensure_worktree_ready()?;
        if self.inner.closed.is_cancelled() {
            return Err(RuntimeError::ShuttingDown);
        }
        let mut drains = self
            .inner
            .drains
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if drains.contains_key(id) {
            return Err(RuntimeError::Busy(id.into()));
        }
        let name = state.info.subagent_name.clone();
        let stored = self.inner.commit_locked(
            &mut state,
            vec![
                event(
                    RESUMED,
                    &Resumed {
                        name,
                        output_schema: None,
                    },
                ),
                event(
                    ADMITTED,
                    &Admitted {
                        message_id: message.clone(),
                        parts: admission.parts,
                        delivery: admission.delivery,
                        source: admission.source,
                        digest,
                    },
                ),
            ],
        )?;
        let receipt = receipt_for(
            &state,
            &message,
            stored.last().expect("admission event").seq,
        );
        launch_child(&self.inner, &mut drains, id, owner);
        Ok(receipt)
    }

    async fn ensure_user_child_ready(
        &self,
        handle: &Handle,
        state: &SessionState,
    ) -> Result<(), RuntimeError> {
        state.ensure_worktree_ready()?;
        if handle.location_uncertain.load(Ordering::SeqCst)
            || state.calls.values().any(|call| {
                matches!(
                    call.status,
                    CallStatus::Dispatched | CallStatus::OutcomeUnknown
                )
            })
        {
            return Err(RuntimeError::Invalid(
                "Child execution outcome is unknown; recovery is required".into(),
            ));
        }
        if self
            .jobs(state.info.parent_id.as_deref())?
            .iter()
            .any(|job| job.child_id == state.info.id && job.status == super::JobStatus::Running)
        {
            return Err(RuntimeError::Busy(state.info.id.clone()));
        }
        let lease = self
            .inner
            .claim_location(&state.info, false, self.inner.closed.child_token())
            .await?;
        if lease.worktree_id != state.info.worktree_id {
            return Err(RuntimeError::Invalid(
                "Managed checkout identity changed".into(),
            ));
        }
        lease.settle().map_err(RuntimeError::Invalid)
    }
}

fn launch_child(
    inner: &Arc<super::Inner>,
    drains: &mut std::collections::HashMap<String, DrainEntry>,
    id: &str,
    owner: ChildExecution,
) {
    let cancel = CancellationToken::new();
    drains.insert(
        id.into(),
        DrainEntry {
            _child_owner: Some(owner),
            cancel: cancel.clone(),
            follow_up: false,
        },
    );
    tokio::spawn(drain::run(Arc::clone(inner), id.into(), false, cancel));
}
