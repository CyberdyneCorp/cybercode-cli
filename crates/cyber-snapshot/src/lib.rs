//! Shadow-Git and private non-Git snapshots with conflict-aware restore.
//!
//! Each worktree gets a separate git directory under `<data>/snapshot/<project_id>/` that
//! uses the project worktree as its work tree and borrows the project's objects through
//! alternates. The user's index, refs, stash and config are never touched.
//! Admitted Linux/macOS Locations outside Git use Location-bound BLAKE3 storage.

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod copy;
mod git;
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

pub struct GitSnapshots {
    data_dir: PathBuf,
    config: Arc<ConfigFn>,
    private_roots: Vec<PathBuf>,
    references: Option<Arc<ReferencesFn>>,
    unavailable: Mutex<HashMap<String, String>>,
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
        Arc::new(Self {
            data_dir,
            config,
            private_roots,
            references,
            unavailable: Mutex::new(HashMap::new()),
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
        Shadow::locate(&self.root(), location)
    }

    fn non_git(&self, directory: &str) -> bool {
        !self
            .private_roots
            .iter()
            .any(|root| Path::new(directory).starts_with(root))
            && (self.config)(Path::new(directory)).get("snapshots") != Some(&Value::Bool(false))
            && cyber_core::project::worktree_root(Path::new(directory)).is_none()
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

    pub async fn gc(&self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Some(references) = &self.references
            && let Ok(protected) = references()
        {
            let data = self.data_dir.clone();
            let config = self.config.clone();
            let _ = tokio::task::spawn_blocking(move || copy::gc(&data, &config, &protected)).await;
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = &self.references;
        for git_dir in git::shadow_dirs(&self.root()) {
            git::gc(&git_dir).await;
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
                // Git journaling and its lifetime proof are integrated separately.
                if cyber_core::project::worktree_root(Path::new(directory)).is_some() {
                    return Ok(None);
                }
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
        cfg!(any(target_os = "macos", target_os = "linux")) && self.non_git(directory)
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
                if !self.non_git(&request.directory) {
                    return Err(RestoreError::Failed(
                        "Owned file-copy restore is unavailable for this Location".into(),
                    ));
                }
                let config = (self.config)(Path::new(&request.directory));
                let (ignore, max_file) = Self::options_value(&config);
                let max_total = config
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
                let lock = cyber_server::workspace::mutation_lock(Path::new(&request.directory))
                    .map_err(|error| RestoreError::Failed(error.to_string()))?;
                let workspace = lock.lock_owned().await;
                worker
                    .run(move |work| {
                        let store = copy::Store::open(&data, &request.directory, options)
                            .map_err(RestoreError::Failed)?;
                        let paths = store.restore_worker(
                            &request.target,
                            &request.recorded,
                            &request.expected,
                            request.paths.as_deref(),
                            &|| Ok(()),
                            Some(work),
                        )?;
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
                    let result = self
                        .file_copy(directory.clone(), |store| store.track())
                        .await;
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
            if self.non_git(&directory) {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                return self
                    .file_copy(directory, move |store| store.changed(&from, &to))
                    .await;
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                return Err("Non-Git snapshots are unavailable on this host".into());
            }
            let Some(shadow) = self.shadow(&directory) else {
                return Ok(Vec::new());
            };
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
            if self.non_git(&directory) {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                return self
                    .file_copy(directory, move |store| store.diff(&from, &to))
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
            if self.non_git(&directory) {
                return self
                    .file_copy(directory, move |store| store.exclusions(&trees))
                    .await;
            }
            if let Some(shadow) = self.shadow(&directory) {
                return shadow.historical_exclusions(&trees).await;
            }
            Ok(vec![])
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
