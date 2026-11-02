//! Process-local coordination shared by in-process editors and snapshot operations.
//! Native Location ownership and source identity/content checks are still required:
//! this lock cannot serialize arbitrary external processes or their open file handles.
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use tokio::sync::Mutex as AsyncMutex;

type Locks = HashMap<PathBuf, Weak<AsyncMutex<()>>>;
static LOCKS: OnceLock<Mutex<Locks>> = OnceLock::new();

/// Git Locations share their worktree root; non-Git Locations use their exact
/// canonical directory. Callers acquire this before any individual path lock.
pub fn mutation_lock(location: &Path) -> io::Result<Arc<AsyncMutex<()>>> {
    let root = cyber_core::project::worktree_root(location)
        .unwrap_or_else(|| location.to_path_buf())
        .canonicalize()?;
    let mut locks = LOCKS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() != 0);
    if let Some(lock) = locks.get(&root).and_then(Weak::upgrade) {
        return Ok(lock);
    }
    if locks.len() >= 1024 {
        return Err(io::Error::other("workspace mutation capacity exceeded"));
    }
    let lock = Arc::new(AsyncMutex::new(()));
    locks.insert(root, Arc::downgrade(&lock));
    Ok(lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn separate_owners_share_one_lock_until_the_owned_worker_releases_it() {
        let root = tempfile::tempdir().unwrap();
        let first = mutation_lock(root.path()).unwrap();
        let second = mutation_lock(root.path()).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let held = first.lock_owned().await;
        assert!(second.try_lock().is_err());
        let (release, wait) = tokio::sync::oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            let _held = held;
            wait.blocking_recv().unwrap();
        });
        // The native worker owns the guard independently of any request future.
        assert!(second.try_lock().is_err());
        release.send(()).unwrap();
        worker.await.unwrap();
        assert!(second.try_lock().is_ok());
        let other = tempfile::tempdir().unwrap();
        assert!(!Arc::ptr_eq(&second, &mutation_lock(other.path()).unwrap()));
    }
    #[test]
    fn nested_git_locations_share_the_checkout_lock() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(root.path())
                .status()
                .unwrap()
                .success()
        );
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let a = mutation_lock(root.path()).unwrap();
        let b = mutation_lock(&nested).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }
}
