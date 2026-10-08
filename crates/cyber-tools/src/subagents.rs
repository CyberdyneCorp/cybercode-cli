//! Child ownership: permits remain held until a canceled child has settled.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use cyber_server::runtime::Runtime;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

type Pool = (usize, Weak<Semaphore>);

#[derive(Default)]
pub(crate) struct Slots(Mutex<HashMap<String, Pool>>);

impl Slots {
    pub fn pool(&self, root: &str, limit: usize) -> Result<Arc<Semaphore>, String> {
        let mut pools = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        pools.retain(|_, (_, pool)| pool.strong_count() > 0);
        if let Some((previous, weak)) = pools.get(root)
            && let Some(pool) = weak.upgrade()
        {
            if *previous != limit {
                return Err("Subagent concurrency settings changed while tasks are active; retry after they settle".into());
            }
            return Ok(pool);
        }
        let pool = Arc::new(Semaphore::new(limit));
        pools.insert(root.into(), (limit, Arc::downgrade(&pool)));
        Ok(pool)
    }
}

/// A dropped tool future cancels its child without releasing the permit early.
pub(crate) struct ChildGuard {
    stop: CancellationToken,
    finished: CancellationToken,
    owner: Option<JoinHandle<()>>,
    execution: Arc<cyber_server::runtime::ChildExecution>,
}

impl ChildGuard {
    pub fn new(
        runtime: Runtime,
        id: String,
        permit: OwnedSemaphorePermit,
        execution: cyber_server::runtime::ChildExecution,
    ) -> Self {
        let execution = Arc::new(execution);
        let held = execution.clone();
        let stop = CancellationToken::new();
        let finished = CancellationToken::new();
        let (cancel, done) = (stop.clone(), finished.clone());
        let weak = runtime.downgrade();
        let owner = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => { if let Some(runtime) = weak.upgrade() { let _ = runtime.interrupt(&id).await; } }
                _ = done.cancelled() => {}
            }
            drop(held);
            drop(permit);
        });
        Self {
            stop,
            finished,
            owner: Some(owner),
            execution,
        }
    }

    pub fn execution(&self) -> &cyber_server::runtime::ChildExecution {
        &self.execution
    }

    pub async fn settle(mut self, canceled: bool) {
        if canceled {
            self.stop.cancel();
        } else {
            self.finished.cancel();
        }
        if let Some(owner) = self.owner.take() {
            let _ = owner.await;
        }
    }

    pub async fn settle_foreground(self, runtime: &Runtime, id: &str, canceled: bool) {
        self.settle(canceled).await;
        if !canceled {
            runtime.dispatch_queued_child(id);
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
