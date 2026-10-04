//! Shadow-git working-tree snapshots and conflict-aware restore (`snapshots-checkpoints`).
//!
//! Each worktree gets a separate git directory under `<data>/snapshot/<project_id>/` that
//! uses the project worktree as its work tree and borrows the project's objects through
//! alternates. The user's index, refs, stash and config are never touched.

mod git;
mod restore;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cyber_server::runtime::{FileDiff, RestoreError, Snapshot, Snapshots};
use futures::future::BoxFuture;
use serde_json::Value;

use git::Shadow;

/// Resolved configuration for a Location.
pub type ConfigFn = dyn Fn(&Path) -> Value + Send + Sync;

const DEFAULT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub struct GitSnapshots {
    data_dir: PathBuf,
    config: Arc<ConfigFn>,
    /// Workspace mutation locks, one per worktree.
    locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl GitSnapshots {
    /// `data_dir` is the Cyber data directory; snapshots live under `<data_dir>/snapshot`.
    pub fn new(data_dir: PathBuf, config: Arc<ConfigFn>) -> Arc<Self> {
        Arc::new(Self {
            data_dir,
            config,
            locks: Mutex::default(),
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

    fn lock(&self, worktree: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(locks.entry(worktree.to_path_buf()).or_default())
    }

    fn options(&self, location: &Path) -> (Vec<String>, u64) {
        let config = (self.config)(location);
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
        for git_dir in git::shadow_dirs(&self.root()) {
            git::gc(&git_dir).await;
        }
    }
}

impl Snapshots for GitSnapshots {
    fn track(&self, directory: &str) -> BoxFuture<'_, Result<Option<Snapshot>, String>> {
        let directory = directory.to_string();
        Box::pin(async move {
            let Some(shadow) = self.shadow(&directory) else {
                return Ok(None);
            };
            let (ignore, max) = self.options(Path::new(&directory));
            let lock = self.lock(&shadow.worktree);
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
            let Some(shadow) = self.shadow(&directory) else {
                return Ok(Vec::new());
            };
            shadow.diff(&from, &to).await
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
            let shadow = self.shadow(&directory).ok_or_else(|| {
                RestoreError::Failed("snapshots are unavailable for this Location".into())
            })?;
            let lock = self.lock(&shadow.worktree);
            let _guard = lock.lock().await;
            let backups = self
                .root()
                .join("backups")
                .join(ulid::Ulid::new().to_string().to_lowercase());
            restore::restore(&shadow, &target, &recorded, &backups).await
        })
    }
}
