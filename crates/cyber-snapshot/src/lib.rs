//! Shadow-Git and private non-Git snapshots with conflict-aware restore.
//!
//! Each worktree gets a separate git directory under `<data>/snapshot/<project_id>/` that
//! uses the project worktree as its work tree and borrows the project's objects through
//! alternates. The user's index, refs, stash and config are never touched.
//! Admitted Linux/macOS Locations outside Git use Location-bound BLAKE3 storage.

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod copy;
mod git;
pub mod reclamation;
mod restore;
pub use cyber_server::runtime::SnapshotRecovery;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyber_server::runtime::{FileDiff, RestoreError, Snapshot, Snapshots};
use futures::future::BoxFuture;
use serde_json::Value;

use git::Shadow;

/// Resolved configuration for a Location.
pub type ConfigFn = dyn Fn(&Path) -> Value + Send + Sync;
/// Durable roots supplied by the application. Failure disables pruning.
pub type ReferencesFn = dyn Fn() -> Result<BTreeSet<String>, String> + Send + Sync;

const DEFAULT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

fn copy_comparison(from: &str, to: &str) -> Result<bool, String> {
    match (from.starts_with("copy:"), to.starts_with("copy:")) {
        (true, true) => Ok(true),
        (false, false) => Ok(false),
        _ => {
            Err("Snapshots use different backends; cross-backend comparison is unavailable".into())
        }
    }
}

pub struct GitSnapshots {
    data_dir: PathBuf,
    config: Arc<ConfigFn>,
    private_roots: Vec<PathBuf>,
    references: Option<Arc<ReferencesFn>>,
    unavailable: Mutex<HashMap<String, String>>,
    reclamation: Option<Arc<dyn reclamation::Policy>>,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    retirement_cursor: Mutex<Option<String>>,
}

impl GitSnapshots {
    /// `data_dir` is the Cyber data directory; snapshots live under `<data_dir>/snapshot`.
    pub fn new(data_dir: PathBuf, config: Arc<ConfigFn>) -> Arc<Self> {
        Self::with_policy(data_dir, config, vec![], None)
    }

    pub fn with_policy(
        data_dir: PathBuf,
        config: Arc<ConfigFn>,
        private_roots: Vec<PathBuf>,
        references: Option<Arc<ReferencesFn>>,
    ) -> Arc<Self> {
        Self::with_reclamation(data_dir, config, private_roots, references, None)
    }

    pub fn with_reclamation(
        data_dir: PathBuf,
        config: Arc<ConfigFn>,
        private_roots: Vec<PathBuf>,
        references: Option<Arc<ReferencesFn>>,
        reclamation: Option<Arc<dyn reclamation::Policy>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            reclamation,
            data_dir,
            config,
            private_roots,
            references,
            unavailable: Mutex::new(HashMap::new()),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            retirement_cursor: Mutex::new(None),
        })
    }

    fn root(&self) -> PathBuf {
        self.data_dir.join("snapshot")
    }

    /// The shadow repository for a Location, or `None` when disabled or outside git.
    fn shadow(&self, directory: &str) -> Option<Shadow> {
        let location = Path::new(directory);
        if (self.config)(location).get("snapshots") == Some(&Value::Bool(false)) {
            return None;
        }
        let mut shadow = Shadow::locate(&self.root(), location)?;
        shadow
            .private_roots
            .extend(self.private_roots.iter().cloned());
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let (ignore, max_untracked) = self.options(location);
            shadow.capture_settings = git::prepared::CaptureSettings {
                ignore,
                max_untracked,
            };
        }
        shadow.private_roots.sort();
        shadow.private_roots.dedup();
        Some(shadow)
    }

    fn non_git(&self, directory: &str) -> bool {
        !self
            .private_roots
            .iter()
            .any(|root| Path::new(directory).starts_with(root))
            && (self.config)(Path::new(directory)).get("snapshots") != Some(&Value::Bool(false))
            && cyber_core::project::worktree_root(Path::new(directory)).is_none()
            && !git::repository_marker(Path::new(directory))
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn file_copy<T: Send + 'static>(
        &self,
        directory: String,
        operation: impl FnOnce(copy::Store) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let config = (self.config)(Path::new(&directory));
        if config.get("snapshots") == Some(&Value::Bool(false)) {
            return Err("Snapshots are disabled for this Location".into());
        }
        let (ignore, max_file) = Self::options_value(&config);
        let max_total = config
            .pointer("/snapshots/fallback_max_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(200 * 1024 * 1024);
        let data = self.data_dir.clone();
        let private_roots = self.private_roots.clone();
        let lock = cyber_server::workspace::mutation_lock(Path::new(&directory))
            .map_err(|e| e.to_string())?;
        let guard = lock.lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let store = copy::Store::open(
                &data,
                &directory,
                copy::Options {
                    ignore,
                    max_file: max_file.min(max_total).min(64 * 1024 * 1024),
                    max_total,
                    private_roots,
                },
            )?;
            operation(store)
        })
        .await
        .map_err(|e| format!("Snapshot worker did not acknowledge completion: {e}"))?
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn read_file_copy<T: Send + 'static>(
        &self,
        directory: String,
        operation: impl FnOnce(copy::Store) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let options = self.copy_options(&directory);
        let data = self.data_dir.clone();
        let lock = cyber_server::workspace::mutation_lock(Path::new(&directory))
            .map_err(|error| error.to_string())?;
        let guard = lock.lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            // Keep the existing native namespace lock through immutable reads;
            // opening for inspection alone would lose the former reader fence.
            let store = copy::Store::recover_existing(&data, &directory, options)?;
            operation(store)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn retained_exclusions(
        &self,
        directory: String,
        trees: Vec<String>,
    ) -> Result<Vec<String>, String> {
        if trees.is_empty() {
            return Ok(vec![]);
        }
        if self.non_git(&directory) {
            return self
                .read_file_copy(directory, move |store| store.exclusions(&trees))
                .await;
        }
        let (copies, git): (Vec<_>, Vec<_>) =
            trees.into_iter().partition(|id| id.starts_with("copy:"));
        let mut exclusions = BTreeSet::new();
        if !copies.is_empty() {
            exclusions.extend(
                self.read_file_copy(directory.clone(), move |store| store.exclusions(&copies))
                    .await?,
            );
        }
        if !git.is_empty() {
            let shadow = self
                .shadow(&directory)
                .ok_or("Git snapshot exclusion history is unavailable")?;
            exclusions.extend(shadow.historical_exclusions(&git).await?);
        }
        Ok(exclusions.into_iter().collect())
    }

    /// Inspect retained journals without restoring files or acknowledging native effects.
    pub async fn recovery(&self, directory: &str) -> Result<Vec<SnapshotRecovery>, String> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let (ignore, max_file) = self.options(Path::new(directory));
            let max_total = (self.config)(Path::new(directory))
                .pointer("/snapshots/fallback_max_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(200 * 1024 * 1024);
            let options = copy::Options {
                ignore,
                max_file: max_file.min(max_total).min(64 * 1024 * 1024),
                max_total,
                private_roots: self.private_roots.clone(),
            };
            let data = self.data_dir.clone();
            let directory = directory.to_owned();
            tokio::task::spawn_blocking(move || {
                copy::Store::existing(&data, &directory, options)?
                    .map(|store| store.recovery())
                    .unwrap_or_else(|| Ok(vec![]))
            })
            .await
            .map_err(|e| e.to_string())?
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = directory;
            Err("Snapshot recovery inspection is unavailable on this host".into())
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn git_preview(
        &self,
        request: cyber_server::runtime::SnapshotRestoreRequest,
    ) -> Result<Vec<FileDiff>, RestoreError> {
        self.recovery_location(&request.directory)
            .map_err(RestoreError::Failed)?;
        let shadow = self
            .shadow(&request.directory)
            .ok_or_else(|| RestoreError::Failed("Git snapshots are unavailable".into()))?;
        let options = self.git_options(&request.directory);
        let lock = cyber_server::workspace::mutation_lock(&shadow.worktree)
            .map_err(|error| RestoreError::Failed(error.to_string()))?;
        let guard = lock.lock_owned().await;
        let prepared = shadow
            .prepare_restore(&request, options.max_file, options.max_total)
            .await
            .map_err(RestoreError::Failed)?;
        let data = self.data_dir.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let store = copy::Store::open(&data, &request.directory, options)
                .map_err(RestoreError::Failed)?;
            store.preview_git(&prepared)
        })
        .await
        .map_err(|error| RestoreError::Failed(error.to_string()))?
    }

    fn options(&self, location: &Path) -> (Vec<String>, u64) {
        Self::options_value(&(self.config)(location))
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn copy_options(&self, directory: &str) -> copy::Options {
        let config = (self.config)(Path::new(directory));
        let (ignore, max_file) = Self::options_value(&config);
        let max_total = config
            .pointer("/snapshots/fallback_max_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(200 * 1024 * 1024);
        copy::Options {
            ignore,
            max_file: max_file.min(max_total).min(64 * 1024 * 1024),
            max_total,
            private_roots: self.private_roots.clone(),
        }
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn git_options(&self, directory: &str) -> copy::Options {
        let mut options = self.copy_options(directory);
        // The Git configured file threshold omits only untracked files. Tracked
        // images retain the raw Git reader's separate bound and total budget.
        options.max_file = options.max_total.min(64 * 1024 * 1024);
        options
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn recovery_options(&self, directory: &str) -> copy::Options {
        let mut options = self.copy_options(directory);
        // Retained files remain inspectable after capture limits are lowered or
        // snapshots are disabled. Resolution writes still obey the current cap.
        options.max_file = 64 * 1024 * 1024;
        options
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn recovery_location(&self, directory: &str) -> Result<(), String> {
        let directory = Path::new(directory)
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if self
            .private_roots
            .iter()
            .any(|root| directory.starts_with(root.canonicalize().unwrap_or_else(|_| root.clone())))
        {
            return Err("Recovery Location is a protected private root".into());
        }
        Ok(())
    }
    fn options_value(config: &Value) -> (Vec<String>, u64) {
        let ignore = config
            .pointer("/snapshots/ignore")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        let max = config
            .pointer("/snapshots/max_file_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_MAX_FILE_BYTES);
        (ignore, max)
    }

    /// Run `git gc --prune=7.days` on every shadow repository about a minute after start,
    /// then every 24 hours.
    pub fn spawn_gc(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            loop {
                this.gc().await;
                tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
            }
        })
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn track_with_reclamation(&self, directory: &str) -> Result<Snapshot, String> {
        let result = self
            .file_copy(directory.to_owned(), |store| store.track())
            .await;
        let Some(policy) = &self.reclamation else {
            return result;
        };
        if !result
            .as_ref()
            .is_err_and(|error| error.starts_with("Non-Git snapshot storage cap reached"))
        {
            return result;
        }
        match self.reclaim(directory).await {
            Ok(bytes) if bytes > 0 => {
                self.file_copy(directory.to_owned(), |store| store.track())
                    .await
            }
            Ok(_) => {
                if self.retire_for_capacity(directory, policy).await > 0 {
                    self.file_copy(directory.to_owned(), |store| store.track())
                        .await
                } else {
                    result
                }
            }
            Err(error) => {
                policy.refused(directory, &error);
                result
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn retire_for_capacity(
        &self,
        directory: &str,
        policy: &Arc<dyn reclamation::Policy>,
    ) -> u64 {
        let batch = match policy.retirement_batch(Some(directory), None) {
            Ok(batch) => batch,
            Err(error) => {
                policy.refused(directory, &error);
                return 0;
            }
        };
        let mut freed = 0u64;
        for receipt in batch.receipts {
            match self.retire(directory, receipt).await {
                Ok(bytes) => freed = freed.saturating_add(bytes),
                Err(error) => policy.refused(directory, &error),
            }
        }
        freed
    }

    /// Retire exact private metadata for an already completed payload plan.
    /// The Store receipt remains authoritative; this never replays source work.
    pub async fn retire(
        &self,
        directory: &str,
        receipt: cyber_server::runtime::snapshot_reclamation::Receipt,
    ) -> Result<u64, String> {
        let policy = self
            .reclamation
            .clone()
            .ok_or("Snapshot retirement authority is unavailable")?;
        let origin = receipt.origin().map_err(|error| error.to_string())?;
        if origin.binding.directory != Path::new(directory) {
            return Err("Metadata retirement belongs to another Location".into());
        }
        let record = policy
            .record(&receipt)?
            .ok_or("Missing payload retirement receipt")?;
        if record.retired {
            return Ok(0);
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            self.recovery_location(directory)?;
            let source = directory.to_owned();
            let data = self.data_dir.clone();
            let options = self.recovery_options(directory);
            let config = self.config.clone();
            let lock = cyber_server::workspace::mutation_lock(Path::new(directory))
                .map_err(|error| error.to_string())?;
            let workspace = lock
                .try_lock_owned()
                .map_err(|_| "Snapshot reader or writer is active")?;
            tokio::task::spawn_blocking(move || {
                let store = copy::Store::recover_existing(&data, &source, options)?;
                copy::reclamation::retirement::retire(store, workspace, config, policy, receipt)
            })
            .await
            .map_err(|error| error.to_string())?
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            Err("Native snapshot metadata retirement is unsupported on this host".into())
        }
    }

    /// Reclaim one independently settled ordinary journal. Intent/lifetime
    /// metadata and durable retry receipts remain retained. No source effects.
    pub async fn reclaim(&self, directory: &str) -> Result<u64, String> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let policy = self
                .reclamation
                .clone()
                .ok_or("Snapshot reclamation authority is unavailable")?;
            self.recovery_location(directory)?;
            let directory = directory.to_owned();
            let data = self.data_dir.clone();
            let options = self.recovery_options(&directory);
            let lock = cyber_server::workspace::mutation_lock(Path::new(&directory))
                .map_err(|error| error.to_string())?;
            let reader = lock
                .clone()
                .try_lock_owned()
                .map_err(|_| "Snapshot reader or writer is active")?;
            let (source, native_data, native_options) =
                (directory.clone(), data.clone(), options.clone());
            let journals = tokio::task::spawn_blocking(move || {
                let _reader = reader;
                copy::Store::recover_existing(&native_data, &source, native_options)?
                    .reclamation_journals()
            })
            .await
            .map_err(|error| error.to_string())??;
            let mut refusal = None;
            for journal in journals {
                let workspace = lock
                    .clone()
                    .try_lock_owned()
                    .map_err(|_| "Snapshot reader or writer is active")?;
                let (source, native_data, native_options, native_policy, config) = (
                    directory.clone(),
                    data.clone(),
                    options.clone(),
                    policy.clone(),
                    self.config.clone(),
                );
                let result = tokio::task::spawn_blocking(move || {
                    let store =
                        copy::Store::recover_existing(&native_data, &source, native_options)?;
                    copy::reclamation::reclaim(store, workspace, config, native_policy, &journal)
                })
                .await
                .map_err(|error| error.to_string())?;
                match result {
                    Ok(0) => {}
                    Ok(bytes) => return Ok(bytes),
                    Err(error) => {
                        refusal.get_or_insert(error);
                    }
                }
            }
            refusal.map_or(Ok(0), Err)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = directory;
            Err("Native snapshot reclamation is unsupported on this host".into())
        }
    }

    pub async fn gc(&self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            if let Some(policy) = &self.reclamation {
                let data = self.data_dir.clone();
                let locations =
                    tokio::task::spawn_blocking(move || copy::reclamation::locations(&data)).await;
                match locations {
                    Ok(Ok(locations)) => {
                        for directory in locations {
                            if let Err(error) = self.reclaim(&directory).await {
                                policy.refused(&directory, &error);
                            }
                        }
                    }
                    Ok(Err(error)) => policy.refused("snapshot inventory", &error),
                    Err(error) => policy.refused("snapshot inventory", &error.to_string()),
                }
                self.gc_metadata(policy).await;
                let (data, config, policy) =
                    (self.data_dir.clone(), self.config.clone(), policy.clone());
                let report = policy.clone();
                let result = tokio::task::spawn_blocking(move || {
                    policy.collect(Box::new(move |protected| {
                        copy::gc(&data, &config, protected)
                    }))
                })
                .await;
                if let Err(error) = result
                    .map_err(|error| error.to_string())
                    .and_then(|value| value)
                {
                    report.refused("snapshot collection", &error);
                }
            } else if let Some(references) = &self.references
                && let Ok(protected) = references()
            {
                let (data, config) = (self.data_dir.clone(), self.config.clone());
                let _ =
                    tokio::task::spawn_blocking(move || copy::gc(&data, &config, &protected)).await;
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = (&self.references, &self.reclamation);
        for git_dir in git::shadow_dirs(&self.root()) {
            git::gc(&git_dir).await;
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn gc_metadata(&self, policy: &Arc<dyn reclamation::Policy>) {
        let after = self
            .retirement_cursor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        match policy.retirement_batch(None, after.as_deref()) {
            Ok(batch) => {
                for receipt in batch.receipts {
                    let source = receipt
                        .origin()
                        .ok()
                        .and_then(|origin| origin.binding.directory.to_str())
                        .map(str::to_owned);
                    if let Some(source) = source {
                        if let Err(error) = self.retire(&source, receipt).await {
                            policy.refused(&source, &error);
                        }
                    } else {
                        policy.refused("metadata retirement discovery", "Missing native Location");
                    }
                }
                *self
                    .retirement_cursor
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = batch.after;
            }
            Err(error) => policy.refused("metadata retirement discovery", &error),
        }
    }
}

impl Snapshots for GitSnapshots {
    fn lock_worker(
        &self,
        binding: cyber_server::runtime::SnapshotLifetimeBinding,
    ) -> BoxFuture<'_, Result<Box<dyn cyber_server::runtime::SnapshotLifetime>, String>> {
        Box::pin(async move {
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                let _ = binding;
                Err("Native snapshot lifetime recovery is unavailable".into())
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                let directory = binding
                    .owner
                    .directory
                    .to_str()
                    .ok_or("Invalid snapshot lifetime Location")?;
                self.recovery_location(directory)?;
                let directory = directory.to_owned();
                let options = self.recovery_options(&directory);
                let data = self.data_dir.clone();
                tokio::task::spawn_blocking(move || {
                    let store = copy::Store::recover_existing(&data, &directory, options)?;
                    Ok(Box::new(store.lock_lifetime(&binding)?)
                        as Box<dyn cyber_server::runtime::SnapshotLifetime>)
                })
                .await
                .map_err(|error| error.to_string())?
            }
        })
    }
    fn reserve_worker(
        &self,
        request: cyber_server::runtime::SnapshotLifetimeRequest,
    ) -> BoxFuture<'_, Result<Option<Box<dyn cyber_server::runtime::SnapshotLifetime>>, String>>
    {
        Box::pin(async move {
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                let _ = request;
                Ok(None)
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                let directory = request
                    .directory
                    .to_str()
                    .ok_or("Invalid snapshot lifetime Location")?;
                // Reservation is metadata-only and independent of the snapshot
                // image backend. Ordinary Git source admission remains gated by
                // owned_restore_available, not by weakening lifetime ownership.
                self.recovery_location(directory)?;
                let data = self.data_dir.clone();
                let options = self.recovery_options(directory);
                let directory = directory.to_owned();
                tokio::task::spawn_blocking(move || {
                    let store = copy::Store::open(&data, &directory, options)?;
                    let lifetime = store.reserve_lifetime(request)?;
                    Ok(Some(
                        Box::new(lifetime) as Box<dyn cyber_server::runtime::SnapshotLifetime>
                    ))
                })
                .await
                .map_err(|error| error.to_string())?
            }
        })
    }
    fn recovery_review(
        &self,
        directory: &str,
        journal: &str,
        action: cyber_server::runtime::SnapshotRecoveryAction,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::SnapshotRecoveryReview, String>> {
        let directory = directory.to_owned();
        let journal = journal.to_owned();
        Box::pin(async move {
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                let _ = (directory, journal, action);
                Err("Native recovery review is unavailable on this host".into())
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                self.recovery_location(&directory)?;
                let data = self.data_dir.clone();
                let options = self.recovery_options(&directory);
                tokio::task::spawn_blocking(move || {
                    let store = copy::Store::existing(&data, &directory, options)?
                        .ok_or("No retained file-copy namespace")?;
                    store.recovery_review(&journal, action)
                })
                .await
                .map_err(|error| error.to_string())?
            }
        })
    }
    fn resolve_recovery(
        &self,
        directory: String,
        request: cyber_server::runtime::SnapshotRecoveryConfirmation,
        worker: cyber_server::runtime::SnapshotWorker,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::SnapshotCompletion, RestoreError>> {
        Box::pin(async move {
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                let _ = (directory, request, worker);
                Err(RestoreError::Failed(
                    "Native recovery is unavailable on this host".into(),
                ))
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                self.recovery_location(&directory)
                    .map_err(RestoreError::Failed)?;
                let options = self.recovery_options(&directory);
                let data = self.data_dir.clone();
                let lock = cyber_server::workspace::mutation_lock(Path::new(&directory))
                    .map_err(|error| RestoreError::Failed(error.to_string()))?;
                let workspace = lock.lock_owned().await;
                worker
                    .run(move |work| {
                        let store = copy::Store::recover_existing(&data, &directory, options)
                            .map_err(RestoreError::Failed)?;
                        let receipt = store.resolve_recovery(&request, &|| Ok(()), Some(work))?;
                        let origin = work.origin().cloned();
                        let digest = store
                            .worker_digest(origin.as_ref())
                            .map_err(RestoreError::Failed)?;
                        let mut native = cyber_server::runtime::SnapshotNativeResult::new(
                            vec![],
                            Box::new(copy::WorkerGuard {
                                store,
                                workspace,
                                origin,
                                digest,
                                receipt: Some(receipt.clone()),
                            }),
                        );
                        native.recovery = Some(receipt);
                        Ok(native)
                    })
                    .await
            }
        })
    }
    fn acknowledge_recovery(
        &self,
        directory: String,
        worker: cyber_server::runtime::SnapshotAcknowledgementWorker,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                let _ = (directory, worker);
                Err("Native snapshot acknowledgement is unavailable on this host".into())
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                self.recovery_location(&directory)?;
                let options = self.recovery_options(&directory);
                let data = self.data_dir.clone();
                let lock = cyber_server::workspace::mutation_lock(Path::new(&directory))
                    .map_err(|error| error.to_string())?;
                let workspace = lock.lock_owned().await;
                worker
                    .run(move |work| {
                        let _workspace = workspace;
                        let store = copy::Store::recover_existing(&data, &directory, options)?;
                        store.reconcile_acknowledgement(work)
                    })
                    .await
            }
        })
    }
    fn owned_restore_available(&self, directory: &str) -> bool {
        cfg!(any(target_os = "macos", target_os = "linux"))
            && (self.non_git(directory)
                || (cfg!(feature = "native-git-acceptance")
                    && (self.config)(Path::new(directory)).get("snapshots")
                        != Some(&Value::Bool(false))
                    && !self
                        .private_roots
                        .iter()
                        .any(|root| Path::new(directory).starts_with(root))
                    && git::repository_marker(Path::new(directory))))
    }

    fn restore_owned(
        &self,
        request: cyber_server::runtime::SnapshotRestoreRequest,
        worker: cyber_server::runtime::SnapshotWorker,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::SnapshotCompletion, RestoreError>> {
        Box::pin(async move {
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                let _ = (request, worker);
                Err(RestoreError::Failed(
                    "Owned snapshot restore is unavailable on this host".into(),
                ))
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                // Direct callers share the runtime availability gate. Candidate
                // Git admission is confined to the explicit acceptance build.
                if !self.owned_restore_available(&request.directory) {
                    return Err(RestoreError::Failed(
                        "Owned snapshot restore is unavailable for this Location".into(),
                    ));
                }
                self.recovery_location(&request.directory)
                    .map_err(RestoreError::Failed)?;
                let shadow = if self.non_git(&request.directory) {
                    None
                } else {
                    Some(self.shadow(&request.directory).ok_or_else(|| {
                        RestoreError::Failed(
                            "Owned snapshot restore is unavailable for this Location".into(),
                        )
                    })?)
                };
                let options = if shadow.is_some() {
                    self.git_options(&request.directory)
                } else {
                    self.copy_options(&request.directory)
                };
                let prepared = match shadow {
                    Some(shadow) => Some(
                        shadow
                            .prepare_restore(&request, options.max_file, options.max_total)
                            .await
                            .map_err(RestoreError::Failed)?,
                    ),
                    None => None,
                };
                let data = self.data_dir.clone();
                let lock = cyber_server::workspace::mutation_lock(Path::new(&request.directory))
                    .map_err(|error| RestoreError::Failed(error.to_string()))?;
                let workspace = lock.lock_owned().await;
                worker
                    .run(move |work| {
                        let store = copy::Store::open(&data, &request.directory, options)
                            .map_err(RestoreError::Failed)?;
                        let paths = if let Some(prepared) = prepared {
                            store.restore_git_worker(prepared, Some(work))?
                        } else {
                            store.restore_worker(
                                &request.target,
                                &request.recorded,
                                &request.expected,
                                request.paths.as_deref(),
                                &|| Ok(()),
                                Some(work),
                            )?
                        };
                        let origin = work.origin().cloned();
                        let digest = store
                            .worker_digest(origin.as_ref())
                            .map_err(RestoreError::Failed)?;
                        Ok(cyber_server::runtime::SnapshotNativeResult::new(
                            paths,
                            Box::new(copy::WorkerGuard {
                                store,
                                workspace,
                                origin,
                                digest,
                                receipt: None,
                            }),
                        ))
                    })
                    .await
            }
        })
    }
    fn recovery(&self, directory: &str) -> BoxFuture<'_, Result<Vec<SnapshotRecovery>, String>> {
        let directory = directory.to_owned();
        Box::pin(async move { GitSnapshots::recovery(self, &directory).await })
    }
    fn unavailable_reason(&self, directory: &str) -> Option<String> {
        self.unavailable
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(directory)
            .cloned()
    }
    fn track(&self, directory: &str) -> BoxFuture<'_, Result<Option<Snapshot>, String>> {
        let directory = directory.to_string();
        Box::pin(async move {
            if self.non_git(&directory) {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                {
                    let result = self.track_with_reclamation(&directory).await;
                    let mut unavailable = self
                        .unavailable
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    return match result {
                        Ok(snapshot) => {
                            unavailable.remove(&directory);
                            Ok(Some(snapshot))
                        }
                        Err(error) => {
                            let repeated = unavailable.get(&directory) == Some(&error);
                            if unavailable.len() < 1024 || unavailable.contains_key(&directory) {
                                unavailable.insert(directory, error.clone());
                            }
                            if repeated { Ok(None) } else { Err(error) }
                        }
                    };
                }
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    let mut unavailable = self
                        .unavailable
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if unavailable.len() < 1024 {
                        unavailable.insert(directory,"Non-Git snapshots require native private storage support on this host; conversation rewind remains available".into());
                    }
                    return Ok(None);
                }
            }
            let Some(shadow) = self.shadow(&directory) else {
                if (self.config)(Path::new(&directory)).get("snapshots")
                    != Some(&Value::Bool(false))
                    && git::repository_marker(Path::new(&directory))
                {
                    return Err("Git snapshots are unavailable for this repository; non-Git fallback is disabled".into());
                }
                return Ok(None);
            };
            let (ignore, max) = self.options(Path::new(&directory));
            let lock = cyber_server::workspace::mutation_lock(&shadow.worktree)
                .map_err(|error| error.to_string())?;
            let _guard = lock.lock().await;
            shadow.track(&ignore, max).await.map(Some)
        })
    }

    fn changed(
        &self,
        directory: &str,
        from: &str,
        to: &str,
    ) -> BoxFuture<'_, Result<Vec<String>, String>> {
        let (directory, from, to) = (directory.to_string(), from.to_string(), to.to_string());
        Box::pin(async move {
            if copy_comparison(&from, &to)? || self.non_git(&directory) {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                return self
                    .read_file_copy(directory, move |store| store.changed(&from, &to))
                    .await;
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                return Err("Non-Git snapshots are unavailable on this host".into());
            }
            let Some(shadow) = self.shadow(&directory) else {
                return Ok(Vec::new());
            };
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            if self.owned_restore_available(&directory) {
                return shadow.changed_native(&from, &to).await;
            }
            shadow.changed(&from, &to).await
        })
    }

    fn diff(
        &self,
        directory: &str,
        from: &str,
        to: &str,
    ) -> BoxFuture<'_, Result<Vec<FileDiff>, String>> {
        let (directory, from, to) = (directory.to_string(), from.to_string(), to.to_string());
        Box::pin(async move {
            if copy_comparison(&from, &to)? || self.non_git(&directory) {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                return self
                    .read_file_copy(directory, move |store| store.diff(&from, &to))
                    .await;
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                return Err("Non-Git snapshots are unavailable on this host".into());
            }
            let Some(shadow) = self.shadow(&directory) else {
                return Ok(Vec::new());
            };
            shadow.diff(&from, &to).await
        })
    }

    fn exclusions(
        &self,
        directory: &str,
        trees: Vec<String>,
    ) -> BoxFuture<'_, Result<Vec<String>, String>> {
        let directory = directory.to_owned();
        Box::pin(async move {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            {
                self.retained_exclusions(directory, trees).await
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                if let Some(shadow) = self.shadow(&directory) {
                    return shadow.historical_exclusions(&trees).await;
                }
                Ok(vec![])
            }
        })
    }
    fn reviewed_restore_available(&self) -> bool {
        true
    }

    fn preview_restore(
        &self,
        directory: &str,
        target: &str,
        recorded: &str,
        expected: &str,
    ) -> BoxFuture<'_, Result<Vec<FileDiff>, RestoreError>> {
        let (directory, target, recorded, expected) = (
            directory.to_owned(),
            target.to_owned(),
            recorded.to_owned(),
            expected.to_owned(),
        );
        Box::pin(async move {
            if self.non_git(&directory) {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                return self
                    .file_copy(directory, move |store| {
                        Ok(store.preview(&target, &recorded, &expected, None))
                    })
                    .await
                    .map_err(RestoreError::Failed)?;
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                return Err(RestoreError::Failed(
                    "Non-Git restore is unavailable on this host".into(),
                ));
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            if self.owned_restore_available(&directory) {
                return self
                    .git_preview(cyber_server::runtime::SnapshotRestoreRequest {
                        directory,
                        target,
                        recorded,
                        expected,
                        paths: None,
                    })
                    .await;
            }
            let shadow = self
                .shadow(&directory)
                .ok_or_else(|| RestoreError::Failed("Snapshots are unavailable".into()))?;
            let lock = cyber_server::workspace::mutation_lock(&shadow.worktree)
                .map_err(|error| RestoreError::Failed(error.to_string()))?;
            let _guard = lock.lock().await;
            let (ignore, max) = self.options(Path::new(&directory));
            let current = shadow
                .track(&ignore, max)
                .await
                .map_err(RestoreError::Failed)?;
            if current.tree != expected {
                return Err(RestoreError::Stale);
            }
            restore::preview(&shadow, &target, &recorded, &expected).await
        })
    }

    fn restore_reviewed(
        &self,
        directory: &str,
        target: &str,
        recorded: &str,
        expected: &str,
        authorize: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    ) -> BoxFuture<'_, Result<Vec<String>, RestoreError>> {
        let (directory, target, recorded, expected) = (
            directory.to_owned(),
            target.to_owned(),
            recorded.to_owned(),
            expected.to_owned(),
        );
        Box::pin(async move {
            if self.non_git(&directory) {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                return self
                    .file_copy(directory, move |store| {
                        Ok(store.restore(&target, &recorded, &expected, None, authorize.as_ref()))
                    })
                    .await
                    .map_err(RestoreError::Failed)?;
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                return Err(RestoreError::Failed(
                    "Non-Git restore is unavailable on this host".into(),
                ));
            }
            let shadow = self.shadow(&directory).ok_or_else(|| {
                RestoreError::Failed("Snapshots are unavailable for this Location".into())
            })?;
            let lock = cyber_server::workspace::mutation_lock(&shadow.worktree)
                .map_err(|error| RestoreError::Failed(error.to_string()))?;
            let _guard = lock.lock().await;
            authorize().map_err(RestoreError::Failed)?;
            let (ignore, max) = self.options(Path::new(&directory));
            let current = shadow
                .track(&ignore, max)
                .await
                .map_err(RestoreError::Failed)?;
            if current.tree != expected {
                return Err(RestoreError::Stale);
            }
            let backups = self
                .root()
                .join("backups")
                .join(ulid::Ulid::new().to_string().to_lowercase());
            restore::restore_authorized(&shadow, &target, &recorded, &backups, authorize.as_ref())
                .await
        })
    }

    fn reviewed_staging_available(&self) -> bool {
        true
    }

    fn preview_restore_paths(
        &self,
        directory: &str,
        target: &str,
        recorded: &str,
        expected: &str,
        paths: Vec<String>,
    ) -> BoxFuture<'_, Result<Vec<FileDiff>, RestoreError>> {
        let (directory, target, recorded, expected) = (
            directory.to_owned(),
            target.to_owned(),
            recorded.to_owned(),
            expected.to_owned(),
        );
        Box::pin(async move {
            if self.non_git(&directory) {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                return self
                    .file_copy(directory, move |store| {
                        Ok(store.preview(&target, &recorded, &expected, Some(&paths)))
                    })
                    .await
                    .map_err(RestoreError::Failed)?;
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                return Err(RestoreError::Failed(
                    "Non-Git restore is unavailable on this host".into(),
                ));
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            if self.owned_restore_available(&directory) {
                return self
                    .git_preview(cyber_server::runtime::SnapshotRestoreRequest {
                        directory,
                        target,
                        recorded,
                        expected,
                        paths: Some(paths),
                    })
                    .await;
            }
            let shadow = self
                .shadow(&directory)
                .ok_or_else(|| RestoreError::Failed("Snapshots are unavailable".into()))?;
            let lock = cyber_server::workspace::mutation_lock(&shadow.worktree)
                .map_err(|e| RestoreError::Failed(e.to_string()))?;
            let _guard = lock.lock().await;
            let (ignore, max) = self.options(Path::new(&directory));
            let current = shadow
                .track(&ignore, max)
                .await
                .map_err(RestoreError::Failed)?;
            if current.tree != expected {
                return Err(RestoreError::Stale);
            }
            restore::preview_scoped(&shadow, &target, &recorded, &expected, Some(&paths)).await
        })
    }
    fn restore_reviewed_paths(
        &self,
        directory: &str,
        target: &str,
        recorded: &str,
        expected: &str,
        authorize: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
        paths: Vec<String>,
    ) -> BoxFuture<'_, Result<Vec<String>, RestoreError>> {
        let (directory, target, recorded, expected) = (
            directory.to_owned(),
            target.to_owned(),
            recorded.to_owned(),
            expected.to_owned(),
        );
        Box::pin(async move {
            if self.non_git(&directory) {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                return self
                    .file_copy(directory, move |store| {
                        Ok(store.restore(
                            &target,
                            &recorded,
                            &expected,
                            Some(&paths),
                            authorize.as_ref(),
                        ))
                    })
                    .await
                    .map_err(RestoreError::Failed)?;
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                return Err(RestoreError::Failed(
                    "Non-Git restore is unavailable on this host".into(),
                ));
            }
            let shadow = self
                .shadow(&directory)
                .ok_or_else(|| RestoreError::Failed("Snapshots are unavailable".into()))?;
            let lock = cyber_server::workspace::mutation_lock(&shadow.worktree)
                .map_err(|e| RestoreError::Failed(e.to_string()))?;
            let _guard = lock.lock().await;
            authorize().map_err(RestoreError::Failed)?;
            let (ignore, max) = self.options(Path::new(&directory));
            let current = shadow
                .track(&ignore, max)
                .await
                .map_err(RestoreError::Failed)?;
            if current.tree != expected {
                return Err(RestoreError::Stale);
            }
            let backups = self
                .root()
                .join("backups")
                .join(ulid::Ulid::new().to_string().to_lowercase());
            restore::restore_authorized_scoped(
                &shadow,
                &target,
                &recorded,
                &backups,
                authorize.as_ref(),
                Some(&paths),
            )
            .await
        })
    }

    fn restore(
        &self,
        directory: &str,
        target: &str,
        recorded: &str,
    ) -> BoxFuture<'_, Result<Vec<String>, RestoreError>> {
        let (directory, target, recorded) = (
            directory.to_string(),
            target.to_string(),
            recorded.to_string(),
        );
        Box::pin(async move {
            if self.non_git(&directory) {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                return self
                    .file_copy(directory, move |store| {
                        Ok(match store.track() {
                            Ok(current) => {
                                store.restore(&target, &recorded, &current.tree, None, &|| Ok(()))
                            }
                            Err(error) => Err(RestoreError::Failed(error)),
                        })
                    })
                    .await
                    .map_err(RestoreError::Failed)?;
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                return Err(RestoreError::Failed(
                    "Non-Git restore is unavailable on this host".into(),
                ));
            }
            let shadow = self.shadow(&directory).ok_or_else(|| {
                RestoreError::Failed("snapshots are unavailable for this Location".into())
            })?;
            let lock = cyber_server::workspace::mutation_lock(&shadow.worktree)
                .map_err(|error| RestoreError::Failed(error.to_string()))?;
            let _guard = lock.lock().await;
            let backups = self
                .root()
                .join("backups")
                .join(ulid::Ulid::new().to_string().to_lowercase());
            restore::restore(&shadow, &target, &recorded, &backups).await
        })
    }
}
