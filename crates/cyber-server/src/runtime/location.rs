//! Location ownership shared by Drains and operations performed while idle.

use super::{CallStatus, Handle, Inner, LocationLease, RuntimeError, SessionInfo};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

impl Inner {
    pub(super) async fn claim_location(
        &self,
        info: &SessionInfo,
        creating: bool,
        cancel: CancellationToken,
    ) -> Result<LocationLease, RuntimeError> {
        let claim = self
            .tools
            .claim_location(info, creating, cancel.child_token());
        tokio::pin!(claim);
        let result = tokio::select! {
            result = &mut claim => result,
            _ = cancel.cancelled() => match tokio::time::timeout(Duration::from_secs(2), &mut claim).await {
                Ok(result) => result,
                Err(_) => return Err(if self.closed.is_cancelled() { RuntimeError::ShuttingDown } else { RuntimeError::Invalid("Location admission did not acknowledge cancellation".into()) }),
            },
        };
        result.map_err(RuntimeError::Invalid)
    }

    pub(crate) async fn with_location<T>(
        &self,
        handle: &Handle,
        cancel: CancellationToken,
        operation: impl Future<Output = Result<T, RuntimeError>>,
    ) -> Result<T, RuntimeError> {
        self.with_setup_location(handle, cancel, operation, false)
            .await
    }

    pub(crate) async fn with_setup_location<T>(
        &self,
        handle: &Handle,
        cancel: CancellationToken,
        operation: impl Future<Output = Result<T, RuntimeError>>,
        setup: bool,
    ) -> Result<T, RuntimeError> {
        let state = handle.state.lock().await;
        if !setup {
            state.ensure_worktree_ready()?;
        }
        let info = state.info.clone();
        self.ensure_admission_open(&info.id)?;
        drop(state);
        let lease = self.claim_location(&info, false, cancel).await?;
        if lease.worktree_id != info.worktree_id {
            return Err(RuntimeError::Invalid(
                "Managed checkout identity changed".into(),
            ));
        }
        let result = match self.ensure_admission_open(&info.id) {
            Ok(()) => operation.await,
            Err(error) => Err(error),
        };
        let unsettled = handle.location_uncertain.load(Ordering::SeqCst)
            || handle.state.lock().await.calls.values().any(|call| {
                matches!(
                    call.status,
                    CallStatus::Dispatched | CallStatus::OutcomeUnknown
                )
            });
        if !unsettled && let Err(error) = lease.settle() {
            handle.location_uncertain.store(true, Ordering::SeqCst);
            return Err(RuntimeError::Invalid(error));
        }
        result
    }

    pub(crate) async fn with_idle_location<T>(
        &self,
        handle: &Handle,
        operation: impl Future<Output = Result<T, RuntimeError>>,
    ) -> Result<T, RuntimeError> {
        self.with_idle_settlement(handle, operation).await
    }

    pub(crate) async fn with_shell_location<T>(
        &self,
        handle: &Handle,
        operation: impl Future<Output = Result<T, RuntimeError>>,
    ) -> Result<T, RuntimeError> {
        self.with_idle_settlement(handle, operation).await
    }

    async fn with_idle_settlement<T>(
        &self,
        handle: &Handle,
        operation: impl Future<Output = Result<T, RuntimeError>>,
    ) -> Result<T, RuntimeError> {
        let admission = self.open().await?;
        let mut scope = super::activity::Scope::reserve(self, handle).await?;
        drop(admission);
        let cancel = scope.control.stop.clone();
        let mut operation = Box::pin(operation);
        let result = self.with_location(handle, cancel.child_token(), async {
            scope.launch()?;
            scope.run(async {
                tokio::select! {
                    biased;
                    result = &mut operation => result,
                    _ = cancel.cancelled() => {
                        // Retain polling native work through its acknowledgement window.
                        if let Ok(result) = tokio::time::timeout(Duration::from_secs(2), &mut operation).await {
                            return result;
                        }
                        handle.location_uncertain.store(true, Ordering::SeqCst);
                        Err(if self.closed.is_cancelled() { RuntimeError::ShuttingDown }
                            else { RuntimeError::Invalid("Idle operation did not acknowledge cancellation; recovery is required".into()) })
                    },
                }
            }).await
        }).await;
        if !handle.location_uncertain.load(Ordering::SeqCst) {
            scope.settle()?;
        }
        result
    }
}
