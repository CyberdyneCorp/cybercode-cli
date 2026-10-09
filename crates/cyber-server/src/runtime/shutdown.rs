//! Stop admission before cancelling and settling every owned runtime task.

use super::{Inner, Runtime, RuntimeError};
use std::sync::PoisonError;
use tokio::sync::RwLockReadGuard;

impl Inner {
    pub(super) async fn open(&self) -> Result<RwLockReadGuard<'_, ()>, RuntimeError> {
        let guard = self.lifecycle.read().await;
        if self.closed.is_cancelled() {
            return Err(RuntimeError::ShuttingDown);
        }
        Ok(guard)
    }
}

impl Runtime {
    /// Close admission permanently, cancel every Drain and join its durable settlement.
    /// Pending inbox rows survive; a new runtime can resume them after restart.
    pub async fn shutdown(&self) {
        let _shutdown = self.inner.shutdown_lock.lock().await;
        self.inner.closed.cancel();
        {
            let _admission = self.inner.lifecycle.write().await;
            let mut drains = self
                .inner
                .drains
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for drain in drains.values_mut() {
                drain.follow_up = false;
                drain.cancel.cancel();
            }
        }
        loop {
            let idle = self.inner.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self
                .inner
                .drains
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty()
            {
                break;
            }
            idle.await;
        }
        self.inner
            .waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        let tasks = std::mem::take(
            &mut *self
                .inner
                .background
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for task in tasks {
            let _ = task.await;
        }
        self.inner.tools.shutdown().await;
    }

    /// Whether this runtime has permanently closed admission.
    pub fn is_shutting_down(&self) -> bool {
        self.inner.closed.is_cancelled()
    }

    /// Resolve when this runtime stops accepting new work.
    pub async fn shutting_down(&self) {
        self.inner.closed.cancelled().await;
    }
}
