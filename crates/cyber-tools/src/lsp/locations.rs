use super::{LspError, Pool, ServerStatus, Settlement};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Notify, mpsc, oneshot},
    task::JoinHandle,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

/// Discovery runs on an owned blocking task; no native process starts until file admission.
pub type PoolFactory = Arc<dyn Fn(&Path) -> Result<Pool, LspError> + Send + Sync>;

struct Use {
    users: usize,
    touched: Instant,
}
struct ActivityState {
    state: Mutex<Use>,
    changed: Notify,
}
impl Default for ActivityState {
    fn default() -> Self {
        Self {
            state: Mutex::new(Use {
                users: 0,
                touched: Instant::now(),
            }),
            changed: Notify::new(),
        }
    }
}
impl ActivityState {
    fn deadline(&self, idle: Duration) -> Result<Option<Instant>, LspError> {
        let state = self.state.lock().map_err(|_| unavailable())?;
        Ok((state.users == 0).then_some(state.touched + idle))
    }
    fn expire(&self, cancel: &CancellationToken, idle: Duration) -> Result<bool, LspError> {
        let state = self.state.lock().map_err(|_| unavailable())?;
        if state.users != 0 || Instant::now() < state.touched + idle {
            return Ok(false);
        }
        cancel.cancel();
        Ok(true)
    }
}

/// Activity does not discover or launch a service. Disposal starts a fresh idle window.
pub struct Activity {
    owner: Weak<Inner>,
    location: PathBuf,
    state: Arc<ActivityState>,
}
impl Drop for Activity {
    fn drop(&mut self) {
        {
            let mut state = self
                .state
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.users -= 1;
            state.touched = Instant::now();
        }
        self.state.changed.notify_one();
        if let Some(owner) = self.owner.upgrade()
            && let Ok(mut table) = owner.table.lock()
            && !table.entries.contains_key(&self.location)
            && !table.fences.contains_key(&self.location)
            && table
                .activities
                .get(&self.location)
                .is_some_and(|state| Arc::ptr_eq(state, &self.state))
            && self.state.state.lock().is_ok_and(|state| state.users == 0)
        {
            table.activities.remove(&self.location);
        }
    }
}

struct Warm {
    origin: ReadOrigin,
    file: PathBuf,
    text: String,
    removed: bool,
    save: Option<Save>,
}

type Saved = Vec<(super::ServerHandle, Arc<super::pool::SaveReceipt>)>;

struct Save {
    reply: oneshot::Sender<Saved>,
    _activity: Activity,
}

pub(crate) struct ReadOrigin {
    location: Vec<cyber_core::worktrees::Managed>,
    pub(crate) document: Vec<cyber_core::worktrees::Managed>,
    created: bool,
}

pub(crate) fn read_origin(location: &Path, file: &Path) -> Result<ReadOrigin, LspError> {
    Ok(ReadOrigin {
        location: checkout_records(location)?,
        document: checkout_records(file)?,
        created: false,
    })
}

pub(crate) fn edit_origin(location: &Path, file: &Path) -> Result<ReadOrigin, LspError> {
    let mut existing = file;
    while !existing.exists() {
        existing = existing.parent().ok_or_else(unavailable)?;
    }
    let mut origin = read_origin(location, existing)?;
    origin.created = !file.exists();
    Ok(origin)
}
struct Entry {
    cancel: CancellationToken,
    activity: Arc<ActivityState>,
    sender: mpsc::Sender<Warm>,
    pool: OnceLock<Pool>,
    retired: AtomicBool,
    discovery_failed: AtomicBool,
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
    fences: BTreeMap<PathBuf, Arc<()>>,
    activities: BTreeMap<PathBuf, Arc<ActivityState>>,
}
struct Fence {
    location: PathBuf,
    token: Arc<()>,
    entry: Option<Arc<Entry>>,
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
    pub fn acquire(&self, location: &Path) -> Result<Activity, LspError> {
        let location = location.canonicalize().map_err(|_| unavailable())?;
        if !location.is_dir() {
            return Err(unavailable());
        }
        let mut table = self.0.table.lock().map_err(|_| unavailable())?;
        if table.closed {
            return Err(unavailable());
        }
        let state = table
            .activities
            .entry(location.clone())
            .or_default()
            .clone();
        {
            let mut current = state.state.lock().map_err(|_| unavailable())?;
            current.users = current.users.checked_add(1).ok_or_else(unavailable)?;
            current.touched = Instant::now();
        }
        state.changed.notify_one();
        Ok(Activity {
            owner: Arc::downgrade(&self.0),
            location,
            state,
        })
    }
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
        let origin = read_origin(location, &file)?;
        self.warm_observed(location, file, text, origin)
    }

    pub(crate) fn warm_observed(
        &self,
        location: &Path,
        file: PathBuf,
        text: String,
        origin: ReadOrigin,
    ) -> Result<(), LspError> {
        self.enqueue(
            location,
            Warm {
                file,
                text,
                origin,
                removed: false,
                save: None,
            },
        )
    }

    pub(crate) async fn removed(
        &self,
        location: &Path,
        file: PathBuf,
        origin: ReadOrigin,
        wait: Duration,
    ) {
        let Ok(activity) = self.acquire(location) else {
            return;
        };
        let Some(parent) = file.parent().and_then(|parent| parent.canonicalize().ok()) else {
            return;
        };
        let Some(name) = file.file_name() else {
            return;
        };
        let file = parent.join(name);
        let (reply, receive) = oneshot::channel();
        let warm = Warm {
            file,
            text: String::new(),
            origin,
            removed: true,
            save: Some(Save {
                reply,
                _activity: activity,
            }),
        };
        if self.enqueue(location, warm).is_ok() {
            let _ = tokio::time::timeout(wait, receive).await;
        }
    }

    pub(crate) async fn feedback(
        &self,
        location: &Path,
        file: PathBuf,
        text: String,
        origin: ReadOrigin,
        wait: Duration,
        other: bool,
    ) -> String {
        let Ok(activity) = self.acquire(location) else {
            return String::new();
        };
        let (reply, receive) = oneshot::channel();
        let file = match file.canonicalize() {
            Ok(file) => file,
            Err(_) => return String::new(),
        };
        if self
            .enqueue(
                location,
                Warm {
                    file: file.clone(),
                    text,
                    origin,
                    removed: false,
                    save: Some(Save {
                        reply,
                        _activity: activity,
                    }),
                },
            )
            .is_err()
        {
            return String::new();
        }
        let Some(deadline) = Instant::now().checked_add(wait) else {
            return String::new();
        };
        let saved = tokio::time::timeout_at(deadline, receive)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        let snapshots = futures::future::join_all(saved.into_iter().map(|(handle, receipt)| {
            wait_feedback(handle, receipt, file.clone(), other, deadline)
        }))
        .await
        .into_iter()
        .flatten()
        .collect();
        render_feedback(snapshots, &file, location, other)
    }

    fn enqueue(&self, location: &Path, mut warm: Warm) -> Result<(), LspError> {
        if warm.text.len() > super::documents::MAX_DOCUMENT_BYTES {
            return Err(unavailable());
        }
        let location = location.canonicalize().map_err(|_| unavailable())?;
        if warm.removed {
            if !matches!(std::fs::symlink_metadata(&warm.file), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
            {
                return Err(unavailable());
            }
        } else {
            warm.file = warm.file.canonicalize().map_err(|_| unavailable())?;
        }
        if !warm.file.starts_with(&location) || (!warm.removed && !warm.file.is_file()) {
            return Err(unavailable());
        }
        let _activity = self.acquire(&location)?;
        let entry = self.start(&location)?;
        let mut activity = entry.activity.state.lock().map_err(|_| unavailable())?;
        if entry.cancel.is_cancelled() {
            return Err(unavailable());
        }
        let sent = entry.sender.try_send(warm);
        if sent.is_ok() {
            activity.touched = Instant::now();
            entry.activity.changed.notify_one();
        }
        drop(activity);
        sent.map_err(|_| unavailable())
    }
    fn start(&self, location: &Path) -> Result<Arc<Entry>, LspError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| unavailable())?;
        let location = location.canonicalize().map_err(|_| unavailable())?;
        let mut table = self.0.table.lock().map_err(|_| unavailable())?;
        if table.closed || table.fences.contains_key(&location) || !location.is_dir() {
            return Err(unavailable());
        }
        if table
            .entries
            .get(&location)
            .is_some_and(|entry| entry.retired.load(Ordering::Acquire))
        {
            table.entries.remove(&location);
        }
        if !has_capacity(&table, &location) {
            return Err(unavailable());
        }
        let activity = table
            .activities
            .entry(location.clone())
            .or_default()
            .clone();
        let entry = table
            .entries
            .entry(location.clone())
            .or_insert_with(|| {
                let (sender, receiver) = mpsc::channel(16);
                let entry = Arc::new(Entry {
                    cancel: CancellationToken::new(),
                    activity,
                    sender,
                    pool: OnceLock::new(),
                    retired: AtomicBool::new(false),
                    discovery_failed: AtomicBool::new(false),
                    worker: tokio::sync::Mutex::default(),
                });
                let worker = entry.clone();
                let factory = self.0.factory.clone();
                let idle = self.0.idle;
                entry.worker.try_lock().unwrap().task =
                    Some(runtime.spawn(async move {
                        run(worker, location, factory, idle, receiver).await
                    }));
                entry
            })
            .clone();
        Ok(entry)
    }

    pub(crate) async fn query_pool(
        &self,
        location: &Path,
        origin: ReadOrigin,
    ) -> Result<Pool, LspError> {
        let expected_location = location.canonicalize().map_err(|_| unavailable())?;
        let entry = self.start(location)?;
        let pool = tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                if entry.cancel.is_cancelled() {
                    return Err(unavailable());
                }
                if let Some(pool) = entry.pool.get() {
                    return Ok(pool.clone());
                }
                if entry
                    .worker
                    .lock()
                    .await
                    .task
                    .as_ref()
                    .is_none_or(|task| task.is_finished())
                {
                    return Err(unavailable());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| unavailable())??;
        if pool.location() != expected_location {
            return Err(unavailable());
        }
        let location = location.to_path_buf();
        let current = tokio::task::spawn_blocking(move || checkout_records(&location))
            .await
            .map_err(|_| unavailable())??;
        if current != origin.location {
            return Err(unavailable());
        }
        Ok(pool)
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
    pub async fn close_location(&self, location: &Path) -> Result<Vec<Settlement>, LspError> {
        let fence = self.fence(location)?;
        let result = settle_fence(&fence, false).await?;
        let table = self.0.table.lock().map_err(|_| unavailable())?;
        current_fence(&table, &fence)?;
        Ok(result)
    }

    /// Reopen admission only after the observed generation has fully settled; discovery stays lazy.
    pub async fn reload_location(&self, location: &Path) -> Result<Vec<Settlement>, LspError> {
        let fence = self.fence(location)?;
        let result = settle_fence(&fence, true).await?;
        let mut table = self.0.table.lock().map_err(|_| unavailable())?;
        current_fence(&table, &fence)?;
        if table.closed {
            return Err(unavailable());
        }
        table.entries.remove(&fence.location);
        table.fences.remove(&fence.location);
        if table
            .activities
            .get(&fence.location)
            .is_some_and(|state| state.state.lock().is_ok_and(|state| state.users == 0))
        {
            table.activities.remove(&fence.location);
        }
        Ok(result)
    }

    fn fence(&self, location: &Path) -> Result<Fence, LspError> {
        let location = location.canonicalize().map_err(|_| unavailable())?;
        if !location.is_dir() {
            return Err(unavailable());
        }
        let mut table = self.0.table.lock().map_err(|_| unavailable())?;
        if table.closed {
            return Err(unavailable());
        }
        if !has_capacity(&table, &location) {
            return Err(unavailable());
        }
        let token = Arc::new(());
        table.fences.insert(location.clone(), token.clone());
        let entry = table.entries.get(&location).cloned();
        if let Some(entry) = &entry {
            entry.cancel.cancel();
        }
        Ok(Fence {
            location,
            token,
            entry,
        })
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
            result.extend(settle_entry(&entry).await?);
        }
        Ok(result)
    }
}

fn has_capacity(table: &Table, location: &Path) -> bool {
    table.entries.contains_key(location)
        || table.fences.contains_key(location)
        || table
            .entries
            .keys()
            .chain(table.fences.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            < 128
}

fn current_fence(table: &Table, fence: &Fence) -> Result<(), LspError> {
    if !table
        .fences
        .get(&fence.location)
        .is_some_and(|token| Arc::ptr_eq(token, &fence.token))
    {
        return Err(LspError::Protocol("Location transition superseded"));
    }
    Ok(())
}

async fn settle_fence(fence: &Fence, reload: bool) -> Result<Vec<Settlement>, LspError> {
    let result = match &fence.entry {
        Some(entry) => match settle_entry(entry).await {
            Ok(result) => result,
            Err(_)
                if reload
                    && entry.discovery_failed.load(Ordering::Acquire)
                    && entry.pool.get().is_none() =>
            {
                vec![]
            }
            Err(error) => return Err(error),
        },
        None => vec![],
    };
    if result.iter().any(|settlement| !settlement.acknowledged) {
        return Err(unavailable());
    }
    Ok(result)
}

async fn settle_entry(entry: &Entry) -> Result<Vec<Settlement>, LspError> {
    let mut worker = entry.worker.lock().await;
    if let Some(task) = worker.task.as_mut() {
        let settled = task
            .await
            .map_err(|_| ())
            .and_then(|settled| settled.map_err(|_| ()));
        worker.result = Some(settled);
        worker.task.take();
    }
    worker
        .result
        .clone()
        .ok_or_else(unavailable)?
        .map_err(|_| unavailable())
}

async fn run(
    entry: Arc<Entry>,
    location: PathBuf,
    factory: PoolFactory,
    idle: Duration,
    mut receiver: mpsc::Receiver<Warm>,
) -> Result<Vec<Settlement>, LspError> {
    let observed_location = location.clone();
    let pool = discover(&entry, location, factory).await?;
    entry.pool.set(pool.clone()).map_err(|_| unavailable())?;
    while let Some(warm) = next_warm(&entry, idle, &mut receiver).await? {
        if !current_warm(&observed_location, &warm, &pool, &entry.cancel).await {
            continue;
        }
        if !deliver_warm(&entry, &pool, warm).await {
            break;
        }
    }
    close_pool(&entry, &pool).await
}

async fn deliver_warm(entry: &Entry, pool: &Pool, warm: Warm) -> bool {
    if warm.removed {
        tokio::select! { biased; _ = entry.cancel.cancelled() => return false, _ = pool.remove_observed(&warm.file, warm.origin.document) => {} }
        if let Some(save) = warm.save {
            let _ = save.reply.send(vec![]);
        }
        return true;
    }
    if let Some(save) = warm.save {
        let saved = tokio::select! { biased; _ = entry.cancel.cancelled() => return false, saved = pool.save_observed(&warm.file, warm.text, warm.origin.document, warm.origin.created) => saved };
        let _ = save.reply.send(saved);
    } else {
        tokio::select! { biased; _ = entry.cancel.cancelled() => return false, _ = pool.warm_observed(&warm.file, warm.text, warm.origin.document) => {} }
    }
    true
}

async fn discover(
    entry: &Entry,
    location: PathBuf,
    factory: PoolFactory,
) -> Result<Pool, LspError> {
    match tokio::task::spawn_blocking(move || factory(&location)).await {
        Ok(Ok(pool)) => Ok(pool),
        Ok(Err(error)) => {
            entry.discovery_failed.store(true, Ordering::Release);
            Err(error)
        }
        Err(_) => Err(unavailable()),
    }
}

async fn next_warm(
    entry: &Entry,
    idle: Duration,
    receiver: &mut mpsc::Receiver<Warm>,
) -> Result<Option<Warm>, LspError> {
    loop {
        let deadline = entry.activity.deadline(idle)?;
        let warm = tokio::select! {
            biased;
            _ = entry.cancel.cancelled() => return Ok(None),
            _ = wait_deadline(deadline) => {
                if entry.activity.expire(&entry.cancel, idle)? { return Ok(None); }
                continue;
            },
            _ = entry.activity.changed.notified() => continue,
            warm = receiver.recv() => warm
        };
        return Ok(warm);
    }
}

async fn wait_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
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
    let file = if warm.removed {
        let Some(parent) = warm.file.parent() else {
            return false;
        };
        parent.to_owned()
    } else {
        warm.file.clone()
    };
    let expected_location = warm.origin.location.clone();
    let expected_document = warm.origin.document.clone();
    let mut observation = tokio::task::spawn_blocking(move || {
        checkout_records(&location).ok() == Some(expected_location)
            && checkout_records(&file).ok() == Some(expected_document)
    });
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

async fn wait_feedback(
    handle: super::ServerHandle,
    receipt: Arc<super::pool::SaveReceipt>,
    file: PathBuf,
    other: bool,
    deadline: Instant,
) -> Vec<super::DiagnosticSnapshot> {
    let mut latest = vec![];
    loop {
        if Instant::now() >= deadline {
            return latest;
        }
        let packet = match tokio::time::timeout_at(
            deadline,
            handle.feedback(file.clone(), receipt.clone(), other),
        )
        .await
        {
            Ok(Ok(packet)) => packet,
            _ => return latest,
        };
        latest = packet.snapshots;
        if packet.complete || Instant::now() >= deadline {
            return latest;
        }
        let next = packet
            .retry_at
            .unwrap_or_else(|| Instant::now() + Duration::from_millis(25));
        tokio::time::sleep_until(next.min(deadline)).await;
    }
}

fn render_feedback(
    snapshots: Vec<super::DiagnosticSnapshot>,
    file: &Path,
    location: &Path,
    other: bool,
) -> String {
    let mut merged: BTreeMap<PathBuf, super::DiagnosticSnapshot> = BTreeMap::new();
    for snapshot in snapshots {
        match merged.get_mut(&snapshot.path) {
            None => {
                merged.insert(snapshot.path.clone(), snapshot);
            }
            Some(existing) => {
                for diagnostic in snapshot.diagnostics {
                    if !existing.diagnostics.contains(&diagnostic) {
                        existing.diagnostics.push(diagnostic);
                    }
                }
            }
        }
    }
    let mut blocks = vec![];
    if let Some(snapshot) = merged.remove(file)
        && let Some(block) = snapshot.error_block(location)
    {
        blocks.push(block);
    }
    if other {
        blocks.extend(
            merged
                .into_values()
                .filter_map(|snapshot| snapshot.error_block(location))
                .take(5),
        );
    }
    blocks.join("\n")
}

pub(crate) fn checkout_records(
    location: &Path,
) -> Result<Vec<cyber_core::worktrees::Managed>, LspError> {
    let path = location.canonicalize().map_err(|_| unavailable())?;
    let directory = if path.is_file() {
        path.parent().ok_or_else(unavailable)?
    } else {
        &path
    };
    cyber_core::worktrees::Repository::managed_locations_at(directory)
        .map(|locations| locations.into_iter().map(|(_, managed)| managed).collect())
        .map_err(|_| unavailable())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn rejected_save_drops_activity_after_releasing_the_queue_lock() {
        for closed in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().canonicalize().unwrap();
            let file = directory.join("file.rs");
            std::fs::write(&file, "saved").unwrap();
            let locations = Locations::new(Arc::new(|_| panic!("no discovery required")));
            let activity = locations.acquire(&directory).unwrap();
            let state = activity.state.clone();
            let (sender, receiver) = mpsc::channel(16);
            let mut receiver = Some(receiver);
            if closed {
                drop(receiver.take());
            } else {
                for _ in 0..16 {
                    sender
                        .try_send(Warm {
                            origin: read_origin(&directory, &file).unwrap(),
                            file: file.clone(),
                            text: "saved".into(),
                            removed: false,
                            save: None,
                        })
                        .ok()
                        .unwrap();
                }
            }
            let entry = Arc::new(Entry {
                cancel: CancellationToken::new(),
                activity: state.clone(),
                sender,
                pool: OnceLock::new(),
                retired: AtomicBool::new(false),
                discovery_failed: AtomicBool::new(false),
                worker: tokio::sync::Mutex::default(),
            });
            locations
                .0
                .table
                .lock()
                .unwrap()
                .entries
                .insert(directory.clone(), entry);
            let (reply, _) = oneshot::channel();
            let warm = Warm {
                origin: read_origin(&directory, &file).unwrap(),
                file,
                text: "saved".into(),
                removed: false,
                save: Some(Save {
                    reply,
                    _activity: activity,
                }),
            };
            assert!(locations.enqueue(&directory, warm).is_err());
            assert_eq!(state.state.lock().unwrap().users, 0);
        }
    }

    #[tokio::test]
    async fn activity_is_lazy_and_does_not_consume_service_slots() {
        let root = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let locations = Locations::new(Arc::new(move |directory| {
            observed.fetch_add(1, Ordering::SeqCst);
            Pool::new(
                directory,
                vec![],
                Arc::new(|_, _| Box::pin(async { unreachable!() })),
            )
        }));
        let mut guards = Vec::new();
        for index in 0..128 {
            let directory = root.path().join(index.to_string());
            std::fs::create_dir(&directory).unwrap();
            locations.close_location(&directory).await.unwrap();
            guards.push(locations.acquire(&directory).unwrap());
        }
        let other = root.path().join("other");
        std::fs::create_dir(&other).unwrap();
        let file = other.join("file.rs");
        std::fs::write(&file, "").unwrap();
        let next = locations.acquire(&other).unwrap();
        assert!(locations.warm(&other, file.clone(), "full".into()).is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(locations.status(&other).unwrap().is_empty());
        locations
            .reload_location(&root.path().join("0"))
            .await
            .unwrap();
        locations.warm(&other, file, "available".into()).unwrap();
        drop(next);
        drop(guards);
        locations.close().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(locations.0.table.lock().unwrap().activities.len(), 128);
    }

    #[tokio::test]
    async fn disposed_activity_without_service_entries_releases_its_tracking() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations::new(Arc::new(|_| panic!("must remain lazy")));
        let guard = locations.acquire(root.path()).unwrap();
        drop(guard);
        assert!(locations.0.table.lock().unwrap().activities.is_empty());
        locations.close().await.unwrap();
    }

    #[tokio::test]
    async fn activity_survives_lazy_reload_and_explicit_close() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations::new(Arc::new(|_| panic!("must remain lazy")));
        let first = locations.acquire(root.path()).unwrap();
        let second = locations.acquire(&root.path().join(".")).unwrap();
        locations.close_location(root.path()).await.unwrap();
        locations.reload_location(root.path()).await.unwrap();
        let state = locations
            .0
            .table
            .lock()
            .unwrap()
            .activities
            .values()
            .next()
            .unwrap()
            .clone();
        assert!(Arc::ptr_eq(&first.state, &state));
        assert_eq!(state.state.lock().unwrap().users, 2);
        drop(first);
        assert!(state.deadline(Duration::from_secs(60)).unwrap().is_none());
        drop(second);
        assert!(locations.0.table.lock().unwrap().activities.is_empty());
        locations.close().await.unwrap();
    }

    #[tokio::test]
    async fn scoped_close_fences_empty_locations_and_reload_stays_lazy() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let file = first.path().join("file.rs");
        let other = second.path().join("other.rs");
        std::fs::write(&file, "").unwrap();
        std::fs::write(&other, "").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let locations = Locations::new(Arc::new(move |directory| {
            observed.fetch_add(1, Ordering::SeqCst);
            Pool::new(
                directory,
                vec![],
                Arc::new(|_, _| Box::pin(async { unreachable!() })),
            )
        }));
        assert!(
            locations
                .close_location(first.path())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            locations
                .warm(first.path(), file.clone(), "closed".into())
                .is_err()
        );
        locations
            .warm(second.path(), other, "sibling".into())
            .unwrap();
        locations.reload_location(first.path()).await.unwrap();
        assert!(locations.status(first.path()).unwrap().is_empty());
        locations
            .warm(first.path(), file, "reloaded".into())
            .unwrap();
        locations.close().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(locations.reload_location(first.path()).await.is_err());
    }

    #[tokio::test]
    async fn cancelled_close_supersedes_pending_reload_and_retains_discovery() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file.rs");
        std::fs::write(&file, "").unwrap();
        let (release, blocked) = std::sync::mpsc::channel();
        let blocked = Mutex::new(blocked);
        let locations = Locations::new(Arc::new(move |directory| {
            blocked
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
        locations
            .warm(root.path(), file.clone(), "initial".into())
            .unwrap();
        let pending = locations.clone();
        let directory = root.path().canonicalize().unwrap();
        let target = directory.clone();
        let reload = tokio::spawn(async move { pending.reload_location(&target).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while !locations
                .0
                .table
                .lock()
                .unwrap()
                .fences
                .contains_key(&directory)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                locations.close_location(root.path())
            )
            .await
            .is_err()
        );
        release.send(()).unwrap();
        assert!(reload.await.unwrap().is_err());
        locations.close_location(root.path()).await.unwrap();
        assert!(
            locations
                .warm(root.path(), file, "still-closed".into())
                .is_err()
        );
        locations.reload_location(root.path()).await.unwrap();
        assert!(locations.status(root.path()).unwrap().is_empty());
        locations.close().await.unwrap();
    }

    #[tokio::test]
    async fn known_discovery_failure_reloads_lazily_after_join() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file.rs");
        std::fs::write(&file, "").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let locations = Locations::new(Arc::new(move |directory| {
            if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(unavailable());
            }
            Pool::new(
                directory,
                vec![],
                Arc::new(|_, _| Box::pin(async { unreachable!() })),
            )
        }));
        locations
            .warm(root.path(), file.clone(), "initial".into())
            .unwrap();
        assert!(locations.close_location(root.path()).await.is_err());
        assert!(locations.close_location(root.path()).await.is_err());
        assert!(
            locations
                .warm(root.path(), file.clone(), "failed".into())
                .is_err()
        );
        locations.reload_location(root.path()).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(locations.status(root.path()).unwrap().is_empty());
        locations.warm(root.path(), file, "fresh".into()).unwrap();
        locations.close().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn unverified_discovery_join_cannot_reload_or_implicitly_reopen() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file.rs");
        std::fs::write(&file, "").unwrap();
        let locations = Locations::new(Arc::new(|_| panic!("unverified discovery")));
        locations
            .warm(root.path(), file.clone(), "initial".into())
            .unwrap();
        assert!(locations.reload_location(root.path()).await.is_err());
        assert!(locations.reload_location(root.path()).await.is_err());
        assert!(locations.close_location(root.path()).await.is_err());
        assert!(locations.warm(root.path(), file, "failed".into()).is_err());
        assert!(locations.close().await.is_err());
    }

    #[tokio::test]
    async fn unacknowledged_root_cannot_reload_or_implicitly_reopen() {
        use cyber_core::intelligence::{DetectedServer, InstallMethod, ServerDefinition};
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file.rs");
        std::fs::write(&file, "").unwrap();
        let locations = Locations::new(Arc::new(|directory| {
            Pool::new(
                directory,
                vec![DetectedServer {
                    definition: ServerDefinition {
                        id: "fixture".into(),
                        command: vec!["fixture".into()],
                        extensions: vec![".rs".into()],
                        root_markers: vec![],
                        env: Default::default(),
                        initialization_options: None,
                        install: InstallMethod::Custom,
                    },
                    enabled: true,
                    installed: true,
                    executable: Some("/fixture".into()),
                }],
                Arc::new(|_, _| {
                    Box::pin(async {
                        Err(super::super::LaunchError {
                            error: unavailable(),
                            acknowledged: false,
                        })
                    })
                }),
            )
        }));
        locations
            .warm(root.path(), file.clone(), "initial".into())
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !locations
                .status(root.path())
                .unwrap()
                .first()
                .is_some_and(|row| row.status == super::super::ServerState::Broken)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(locations.reload_location(root.path()).await.is_err());
        assert!(locations.close_location(root.path()).await.is_err());
        assert!(locations.warm(root.path(), file, "unknown".into()).is_err());
        assert!(!locations.close().await.unwrap()[0].acknowledged);
    }

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
