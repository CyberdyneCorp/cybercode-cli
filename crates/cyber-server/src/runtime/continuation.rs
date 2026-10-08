//! Explicit user continuation starts a fresh child attempt without stealing tool ownership.

use std::sync::{Arc, PoisonError, atomic::Ordering};

use tokio_util::sync::CancellationToken;

use super::events::{ADMITTED, Admitted, RESUMED};
use super::{
    Admission, CallStatus, ChildContinuation, ChildExecution, Delivery, DrainEntry, Handle,
    Receipt, Runtime, RuntimeError, SessionState, digest, drain, event, existing_receipt,
    names::Resumed, receipt_for,
};

impl Runtime {
    /// Public user input may continue an idle child, while active input remains steering.
    pub async fn admit_user(
        &self,
        id: &str,
        mut admission: Admission,
    ) -> Result<Receipt, RuntimeError> {
        self.inner.ensure_admission_open(id)?;
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
        if state.info.parent_id.is_none() {
            drop(lifecycle);
            return self.admit(id, admission).await;
        }
        if self.child_is_settling(id) {
            return Err(RuntimeError::Busy(id.into()));
        }
        if self.is_running(id) || !admission.resume || admission.delivery == Delivery::Hold {
            drop(lifecycle);
            return self.admit(id, admission).await;
        }
        drop(lifecycle);
        let prepared = self.prepare_user_child(&handle, &state).await?;
        let _lifecycle = self.inner.open().await?;
        {
            let current = handle.state.lock().await;
            prepared.check(&current)?;
        }
        self.inner.commit_staged_revert(&handle).await?;
        let mut state = handle.state.lock().await;
        if let Some(receipt) = existing_receipt(&state, &message, &digest)? {
            return Ok(receipt);
        }
        prepared.check(&state)?;
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
                        admission_bindings: prepared.admission_bindings(),
                        name,
                        output_schema: None,
                    },
                ),
                event(
                    ADMITTED,
                    &Admitted {
                        admission_bindings: prepared.admission_bindings(),
                        wake: true,
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
        prepared.launch(&self.inner, &mut drains, id);
        Ok(receipt)
    }

    /// Return false for a root or an active child, preserving its normal safe boundary.
    pub(super) async fn continue_child_input(
        &self,
        id: &str,
        release: Option<(&str, Delivery)>,
        requested_only: bool,
    ) -> Result<bool, RuntimeError> {
        drop(self.inner.open().await?);
        let handle = self.inner.handle(id).await?;
        let state = handle.state.lock().await.clone();
        if state.info.parent_id.is_none() {
            return Ok(false);
        }
        state.ensure_worktree_ready()?;
        if self.child_is_settling(id) {
            return Err(RuntimeError::Busy(id.into()));
        }
        if self.is_running(id) {
            return Ok(false);
        }
        if let Some((message, _)) = release {
            let row = state
                .input(message)
                .ok_or_else(|| RuntimeError::Invalid(format!("no inbox row {message}")))?;
            super::check_inbox_action(row, super::events::InboxAction::Released)?;
        } else if !eligible_child_input(&state, requested_only) {
            return Ok(true);
        }
        let prepared = self.prepare_user_child(&handle, &state).await?;
        let _lifecycle = self.inner.open().await?;
        {
            let current = handle.state.lock().await;
            prepared.check(&current)?;
        }
        self.inner.commit_staged_revert(&handle).await?;
        let mut state = handle.state.lock().await;
        prepared.check(&state)?;
        let mut events = Vec::new();
        if let Some((message, delivery)) = release {
            let row = state
                .input(message)
                .ok_or_else(|| RuntimeError::Invalid(format!("no inbox row {message}")))?;
            super::check_inbox_action(row, super::events::InboxAction::Released)?;
            events.push(event(
                super::events::INBOX_UPDATED,
                &super::events::InboxUpdated {
                    message_id: message.into(),
                    action: super::events::InboxAction::Released,
                    parts: None,
                    delivery: Some(delivery),
                },
            ));
        } else if !eligible_child_input(&state, requested_only) {
            return Ok(true);
        }
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
        events.insert(
            0,
            event(
                RESUMED,
                &Resumed {
                    admission_bindings: prepared.admission_bindings(),
                    name: state.info.subagent_name.clone(),
                    output_schema: None,
                },
            ),
        );
        self.inner.commit_locked(&mut state, events)?;
        prepared.launch(&self.inner, &mut drains, id);
        Ok(true)
    }

    pub(super) fn child_is_settling(&self, id: &str) -> bool {
        self.inner
            .drains
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .is_some_and(|entry| entry.child_settling)
    }

    pub(super) fn ensure_user_child_outcome(
        &self,
        handle: &Handle,
        state: &SessionState,
    ) -> Result<(), RuntimeError> {
        state.ensure_worktree_ready()?;
        if state.child_continuation_unknown
            || handle.location_uncertain.load(Ordering::SeqCst)
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
        Ok(())
    }

    pub(super) async fn ensure_user_child_ready(
        &self,
        handle: &Handle,
        state: &SessionState,
        cancel: CancellationToken,
    ) -> Result<(), RuntimeError> {
        self.ensure_user_child_outcome(handle, state)?;
        let lease = self
            .inner
            .claim_location(&state.info, false, cancel)
            .await?;
        if lease.worktree_id != state.info.worktree_id {
            return Err(RuntimeError::Invalid(
                "Managed checkout identity changed".into(),
            ));
        }
        lease.settle().map_err(RuntimeError::Invalid)
    }
}

fn has_pending_input(state: &SessionState) -> bool {
    state.pending(Delivery::Steer).next().is_some()
        || state.pending(Delivery::Queue).next().is_some()
}

fn has_requested_input(state: &SessionState) -> bool {
    state.inbox.iter().any(|row| {
        row.status == super::InputStatus::Pending
            && row.delivery != Delivery::Hold
            && state.child_requested_inputs.contains(&row.message_id)
    })
}

fn eligible_child_input(state: &SessionState, requested_only: bool) -> bool {
    if requested_only {
        has_requested_input(state)
    } else {
        has_pending_input(state)
    }
}

pub(super) fn launch_child(
    inner: &Arc<super::Inner>,
    drains: &mut std::collections::HashMap<String, DrainEntry>,
    id: &str,
    owner: ChildExecution,
    settlement: Option<Box<dyn ChildContinuation>>,
) {
    let cancel = CancellationToken::new();
    drains.insert(
        id.into(),
        DrainEntry {
            _child_owner: Some(owner),
            child_settlement: settlement,
            child_settling: false,
            cancel: cancel.clone(),
            follow_up: false,
        },
    );
    tokio::spawn(drain::run(Arc::clone(inner), id.into(), false, cancel));
}

impl Runtime {
    /// Dispatch requested child input only after the previous result owner has settled.
    /// Registration makes shutdown wait for preparation as well as any resulting Drain.
    /// The returned token signals preparation-task completion, not cancellation of its work.
    pub fn dispatch_queued_child(&self, id: &str) -> CancellationToken {
        let done = CancellationToken::new();
        let mut owners = self
            .inner
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.inner.closed.is_cancelled() {
            done.cancel();
            return done;
        }
        let weak = self.downgrade();
        let id = id.to_owned();
        let on_done = done.clone().drop_guard();
        let work: futures::future::BoxFuture<'static, ()> = Box::pin(async move {
            let _on_done = on_done;
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            let result = runtime.dispatch_requested_input(&id).await;
            if let Err(error) = result
                && !matches!(error, RuntimeError::Busy(_) | RuntimeError::ShuttingDown)
            {
                runtime.inner.bus.publish(super::bus::LiveEvent::Error {
                    session_id: id,
                    kind: "child_queue".into(),
                    message: error.to_string(),
                });
            }
        });
        owners.retain(|owner| !owner.is_finished());
        owners.push(tokio::spawn(work));
        done
    }

    async fn dispatch_requested_input(&self, id: &str) -> Result<(), RuntimeError> {
        let state = self.state(id).await?;
        if state.info.parent_id.is_none() || !has_requested_input(&state) {
            return Ok(());
        }
        self.continue_child_input(id, None, true).await?;
        Ok(())
    }
}

impl super::Inner {
    pub(super) async fn settle_user_child(
        self: &Arc<Self>,
        id: &str,
        cancel: &CancellationToken,
        succeeded: bool,
    ) {
        use futures::FutureExt;
        let settlement = {
            let mut drains = self.drains.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(entry) = drains.get_mut(id) else {
                return;
            };
            if entry.follow_up && !cancel.is_cancelled() {
                return;
            }
            let Some(settlement) = entry.child_settlement.take() else {
                return;
            };
            entry.child_settling = true;
            settlement
        };
        let Ok(handle) = self.handle(id).await else {
            return;
        };
        let state = handle.state.lock().await.clone();
        let completed = succeeded && !cancel.is_cancelled() && completed_answer(&state);
        let token = cancel.clone();
        let work =
            std::panic::AssertUnwindSafe(async move { settlement.settle(completed, token).await })
                .catch_unwind();
        tokio::pin!(work);
        let outcome = tokio::select! {
            biased;
            result = &mut work => Some(result),
            _ = cancel.cancelled() => tokio::time::timeout(std::time::Duration::from_secs(2), &mut work).await.ok(),
        };
        let (result, unknown) = match outcome {
            Some(Ok(result)) => (result, false),
            Some(Err(_)) => (
                Err("Child settlement stopped on an internal error".into()),
                true,
            ),
            None => (
                Err("Child settlement did not acknowledge cancellation".into()),
                true,
            ),
        };
        if unknown {
            handle.location_uncertain.store(true, Ordering::SeqCst);
        }
        let (worktree, error) = match result {
            Ok(worktree) => (worktree, None),
            Err(error) => (None, Some(error)),
        };
        let event = event(
            super::events::CHILD_CONTINUATION_SETTLED,
            &serde_json::json!({
                "completed":completed, "worktree":worktree, "error":error, "unknown":unknown,
            }),
        );
        if let Err(error) = self.commit(&handle, vec![event]).await {
            handle.location_uncertain.store(true, Ordering::SeqCst);
            self.bus.publish(super::LiveEvent::Error {
                session_id: id.into(),
                kind: "storage".into(),
                message: error.to_string(),
            });
        }
        if let Some(message) = error {
            self.bus.publish(super::LiveEvent::Error {
                session_id: id.into(),
                kind: "child_settlement".into(),
                message,
            });
        }
    }
}

fn completed_answer(state: &SessionState) -> bool {
    if state.pending(Delivery::Steer).next().is_some()
        || state.pending(Delivery::Queue).next().is_some()
        || !state.unresolved().is_empty()
    {
        return false;
    }
    if state.output_schema().is_some() {
        return state.structured_result().is_some();
    }
    let Some(input) = state
        .inbox
        .iter()
        .rev()
        .find(|row| row.status == super::InputStatus::Promoted)
    else {
        return false;
    };
    let Some(start) = state
        .entries
        .iter()
        .position(|entry| matches!(entry, super::Entry::User {id,..} if id == &input.message_id))
    else {
        return false;
    };
    state.entries[start + 1..]
        .iter()
        .rev()
        .find_map(|entry| match entry {
            super::Entry::Assistant(answer) => {
                Some(answer.finished && answer.error.is_none() && answer.calls.is_empty())
            }
            _ => None,
        })
        .unwrap_or(false)
}
