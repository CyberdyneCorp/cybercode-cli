use super::{LspError, Pool, ServerStatus, Settlement};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;

/// Discovery runs on an owned blocking task; no native process starts until file admission.
pub type PoolFactory = Arc<dyn Fn(&Path) -> Result<Pool, LspError> + Send + Sync>;

struct Warm {
    checkouts: Vec<cyber_core::worktrees::Managed>,
    file: PathBuf,
    text: String,
}
struct Entry {
    cancel: CancellationToken,
    touched: Mutex<Instant>,
    sender: mpsc::Sender<Warm>,
    pool: OnceLock<Pool>,
    retired: AtomicBool,
    worker: tokio::sync::Mutex<Worker>,
}
#[derive(Default)]
struct Worker {
    task: Option<JoinHandle<Result<Vec<Settlement>, LspError>>>,
    result: Option<Result<Vec<Settlement>, ()>>,
}
#[derive(Default)]
struct Table {
    closed: bool,
    entries: BTreeMap<PathBuf, Arc<Entry>>,
}
struct Inner {
    table: Mutex<Table>,
    factory: PoolFactory,
    idle: Duration,
}
impl Drop for Inner {
    fn drop(&mut self) {
        let table = self
            .table
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in table.entries.values() {
            entry.cancel.cancel();
        }
    }
}

/// One retained service generation per canonical Location, with bounded background warming.
#[derive(Clone)]
pub struct Locations(Arc<Inner>);
impl Locations {
    pub fn new(factory: PoolFactory) -> Self {
        Self::with_idle(factory, Duration::from_secs(60 * 60))
    }
    pub fn with_idle(factory: PoolFactory, idle: Duration) -> Self {
        Self(Arc::new(Inner {
            table: Mutex::default(),
            factory,
            idle,
        }))
    }
    /// Never await discovery, initialization, queue capacity or diagnostic delivery.
    pub fn warm(&self, location: &Path, file: PathBuf, text: String) -> Result<(), LspError> {
        self.warm_observed(location, file, text, checkout_records(location)?)
    }

    pub(crate) fn warm_observed(
        &self,
        location: &Path,
        file: PathBuf,
        text: String,
        checkouts: Vec<cyber_core::worktrees::Managed>,
    ) -> Result<(), LspError> {
        if text.len() > super::documents::MAX_DOCUMENT_BYTES {
            return Err(unavailable());
        }
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| unavailable())?;
        let location = location.canonicalize().map_err(|_| unavailable())?;
        let file = file.canonicalize().map_err(|_| unavailable())?;
        if !file.starts_with(&location) || !file.is_file() {
            return Err(unavailable());
        }
        let mut table = self.0.table.lock().map_err(|_| unavailable())?;
        if table.closed || !location.is_dir() {
            return Err(unavailable());
        }
        if table
            .entries
            .get(&location)
            .is_some_and(|entry| entry.retired.load(Ordering::Acquire))
        {
            table.entries.remove(&location);
        }
        if table.entries.len() >= 128 && !table.entries.contains_key(&location) {
            return Err(unavailable());
        }
        let entry = table.entries.entry(location.clone()).or_insert_with(|| {
            let (sender, receiver) = mpsc::channel(16);
            let entry = Arc::new(Entry {
                cancel: CancellationToken::new(),
                touched: Mutex::new(Instant::now()),
                sender,
                pool: OnceLock::new(),
                retired: AtomicBool::new(false),
                worker: tokio::sync::Mutex::default(),
            });
            let worker = entry.clone();
            let factory = self.0.factory.clone();
            let idle = self.0.idle;
            entry.worker.try_lock().unwrap().task = Some(
                runtime.spawn(async move { run(worker, location, factory, idle, receiver).await }),
            );
            entry
        });
        if entry.cancel.is_cancelled() {
            return Err(unavailable());
        }
        entry
            .sender
            .try_send(Warm {
                file,
                text,
                checkouts,
            })
            .map_err(|_| unavailable())?;
        *entry.touched.lock().map_err(|_| unavailable())? = Instant::now();
        Ok(())
    }
    /// Observation does not create services or start language servers.
    pub fn status(&self, location: &Path) -> Result<Vec<ServerStatus>, LspError> {
        let location = location.canonicalize().map_err(|_| unavailable())?;
        let table = self.0.table.lock().map_err(|_| unavailable())?;
        match table
            .entries
            .get(&location)
            .and_then(|entry| entry.pool.get())
        {
            Some(pool) => pool.status(),
            None => Ok(vec![]),
        }
    }
    /// Join handles remain retained when the caller cancels this wait.
    pub async fn close(&self) -> Result<Vec<Settlement>, LspError> {
        let entries: Vec<_> = {
            let mut table = self.0.table.lock().map_err(|_| unavailable())?;
            table.closed = true;
            table.entries.values().cloned().collect()
        };
        for entry in &entries {
            entry.cancel.cancel();
        }
        let mut result = vec![];
        for entry in entries {
            let mut worker = entry.worker.lock().await;
            if let Some(task) = worker.task.as_mut() {
                let settled = task
                    .await
                    .map_err(|_| ())
                    .and_then(|settled| settled.map_err(|_| ()));
                worker.result = Some(settled);
                worker.task.take();
            }
            result.extend(
                worker
                    .result
                    .clone()
                    .ok_or_else(unavailable)?
                    .map_err(|_| unavailable())?,
            );
        }
        Ok(result)
    }
}

async fn run(
    entry: Arc<Entry>,
    location: PathBuf,
    factory: PoolFactory,
    idle: Duration,
    mut receiver: mpsc::Receiver<Warm>,
) -> Result<Vec<Settlement>, LspError> {
    let observed_location = location.clone();
    let pool = tokio::task::spawn_blocking(move || factory(&location))
        .await
        .map_err(|_| unavailable())??;
    entry.pool.set(pool.clone()).map_err(|_| unavailable())?;
    while let Some(warm) = next_warm(&entry, idle, &mut receiver).await? {
        if !current_warm(&observed_location, &warm, &pool, &entry.cancel).await {
            continue;
        }
        tokio::select! { biased; _ = entry.cancel.cancelled() => break, _ = pool.warm(&warm.file, warm.text) => {} }
    }
    close_pool(&entry, &pool).await
}

async fn next_warm(
    entry: &Entry,
    idle: Duration,
    receiver: &mut mpsc::Receiver<Warm>,
) -> Result<Option<Warm>, LspError> {
    loop {
        let deadline = *entry.touched.lock().map_err(|_| unavailable())? + idle;
        let warm = tokio::select! {
            biased;
            _ = entry.cancel.cancelled() => return Ok(None),
            _ = tokio::time::sleep_until(deadline) => {
                if Instant::now() >= *entry.touched.lock().map_err(|_| unavailable())? + idle { return Ok(None); }
                continue;
            },
            warm = receiver.recv() => warm
        };
        return Ok(warm);
    }
}

async fn close_pool(entry: &Entry, pool: &Pool) -> Result<Vec<Settlement>, LspError> {
    entry.cancel.cancel();
    let result = pool.close().await;
    if result
        .as_ref()
        .is_ok_and(|settled| settled.iter().all(|s| s.acknowledged))
    {
        entry.retired.store(true, Ordering::Release);
    }
    result
}

async fn current_warm(
    location: &Path,
    warm: &Warm,
    pool: &Pool,
    cancel: &CancellationToken,
) -> bool {
    let location = location.to_path_buf();
    let expected = warm.checkouts.clone();
    let mut observation =
        tokio::task::spawn_blocking(move || checkout_records(&location).ok() == Some(expected));
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            let _ = pool.close().await;
            let _ = observation.await;
            false
        },
        result = &mut observation => result.unwrap_or(false)
    }
}

fn unavailable() -> LspError {
    LspError::Protocol("Location language services unavailable")
}

pub(crate) fn checkout_records(
    location: &Path,
) -> Result<Vec<cyber_core::worktrees::Managed>, LspError> {
    cyber_core::worktrees::Repository::managed_locations_at(location)
        .map(|locations| locations.into_iter().map(|(_, managed)| managed).collect())
        .map_err(|_| unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn bounded_warming_deduplicates_discovery_and_retains_cancelled_close() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file.rs");
        std::fs::write(&file, "").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let (permit, receiver) = std::sync::mpsc::channel();
        let receiver = Mutex::new(receiver);
        let observed = calls.clone();
        let locations = Locations::new(Arc::new(move |directory| {
            observed.fetch_add(1, Ordering::SeqCst);
            receiver
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
            Pool::new(
                directory,
                vec![],
                Arc::new(|_, _| Box::pin(async { unreachable!() })),
            )
        }));
        assert!(locations.status(root.path()).unwrap().is_empty());
        for _ in 0..16 {
            locations
                .warm(root.path(), file.clone(), "bounded".into())
                .unwrap();
        }
        assert!(
            locations
                .warm(root.path(), file.clone(), "overflow".into())
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), locations.close())
                .await
                .is_err()
        );
        assert!(locations.warm(root.path(), file, "late".into()).is_err());
        permit.send(()).unwrap();
        assert!(locations.close().await.unwrap().is_empty());
        assert!(locations.close().await.unwrap().is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_discovery_cannot_become_successful_on_repeated_close() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file.rs");
        std::fs::write(&file, "").unwrap();
        let locations = Locations::new(Arc::new(|_| Err(unavailable())));
        locations
            .warm(root.path(), file, "snapshot".into())
            .unwrap();
        assert!(locations.close().await.is_err());
        assert!(locations.close().await.is_err());
    }

    #[tokio::test]
    async fn external_and_oversized_snapshots_refuse_before_discovery() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let file = root.path().join("file.rs");
        std::fs::write(&file, "").unwrap();
        let locations = Locations::new(Arc::new(|_| panic!("unexpected discovery")));
        assert!(
            locations
                .warm(root.path(), outside.path().into(), "private".into())
                .is_err()
        );
        assert!(
            locations
                .warm(
                    root.path(),
                    file,
                    "x".repeat(super::super::documents::MAX_DOCUMENT_BYTES + 1)
                )
                .is_err()
        );
        assert!(locations.close().await.unwrap().is_empty());
    }
}
