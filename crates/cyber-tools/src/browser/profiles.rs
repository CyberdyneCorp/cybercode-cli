//! Per-Session browser profiles: `<data>/browser/profiles/<session_id>/`, the browser
//! server's `TMPDIR` (the pinned server keeps its profile in memory, so only its
//! temporary files land there), and `<session_id>.lock`, held exclusively by the owning
//! process while the browser runs. `cyber browser cleanup` removes the profiles whose
//! lock nobody holds: those of a process that ended without stopping its browser.

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

pub(crate) struct Profile {
    dir: PathBuf,
    lock_path: PathBuf,
    lock: Mutex<Option<File>>,
}

fn profiles(root: &Path) -> PathBuf {
    root.join("profiles")
}

impl Profile {
    pub(crate) fn new(root: &Path, session_id: &str) -> Self {
        let profiles = profiles(root);
        Self {
            dir: profiles.join(session_id),
            lock_path: profiles.join(format!("{session_id}.lock")),
            lock: Mutex::new(None),
        }
    }

    /// Lock the profile and start it empty. Fails when another process holds it.
    pub(crate) fn create(&self) -> Result<PathBuf, String> {
        let unavailable = |error: std::io::Error| format!("browser profile unavailable: {error}");
        if let Some(parent) = self.lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(unavailable)?;
        }
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.lock_path)
            .map_err(unavailable)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(format!(
                    "browser profile {} is in use by another process",
                    self.dir.display()
                ));
            }
            Err(TryLockError::Error(error)) => return Err(unavailable(error)),
        }
        match std::fs::remove_dir_all(&self.dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(unavailable(error)),
        }
        private_dir(&self.dir).map_err(unavailable)?;
        *self.lock.lock().unwrap_or_else(PoisonError::into_inner) = Some(file);
        Ok(self.dir.clone())
    }

    /// Remove the profile and release its lock.
    pub(crate) fn remove(&self) {
        let lock = self
            .lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if lock.is_none() {
            return;
        }
        remove_logged(&self.dir);
        drop(lock);
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn remove_logged(dir: &Path) {
    if let Err(error) = std::fs::remove_dir_all(dir)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        cyber_core::log::warn(
            "browser",
            "profile removal failed",
            serde_json::json!({"path": dir.display().to_string(), "error": error.to_string()}),
        );
    }
}

/// What [`cleanup`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cleanup {
    /// Profiles removed: no running process held them.
    pub removed: Vec<PathBuf>,
    /// Profiles kept because a running browser holds their lock.
    pub in_use: Vec<PathBuf>,
}

/// Remove the profiles under `<root>/profiles` that no process holds.
pub fn cleanup(root: &Path) -> std::io::Result<Cleanup> {
    let mut report = Cleanup::default();
    let entries = match std::fs::read_dir(profiles(root)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(report),
        Err(error) => return Err(error),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let lock_path = path.with_extension("lock");
        let held = match File::options().write(true).open(&lock_path) {
            Ok(file) => matches!(file.try_lock(), Err(TryLockError::WouldBlock)),
            Err(_) => false,
        };
        if held {
            report.in_use.push(path);
            continue;
        }
        std::fs::remove_dir_all(&path)?;
        let _ = std::fs::remove_file(&lock_path);
        report.removed.push(path);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locked_profile_survives_cleanup_and_an_abandoned_one_is_removed() {
        let root = tempfile::tempdir().unwrap();
        let live = Profile::new(root.path(), "ses_live");
        let dir = live.create().unwrap();
        std::fs::write(dir.join("state"), "x").unwrap();
        let abandoned = profiles(root.path()).join("ses_gone");
        std::fs::create_dir_all(&abandoned).unwrap();
        std::fs::write(abandoned.with_extension("lock"), "").unwrap();

        let report = cleanup(root.path()).unwrap();
        assert_eq!(report.removed, vec![abandoned.clone()]);
        assert_eq!(report.in_use, vec![dir.clone()]);
        assert!(dir.join("state").exists() && !abandoned.exists());
        // A second owner of a held profile is refused.
        assert!(Profile::new(root.path(), "ses_live").create().is_err());
        live.remove();
        assert!(!dir.exists());
        assert!(cleanup(root.path()).unwrap().removed.is_empty());
    }

    #[test]
    fn create_starts_from_an_empty_profile() {
        let root = tempfile::tempdir().unwrap();
        let stale = profiles(root.path()).join("ses_1");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("cookies"), "old").unwrap();
        let profile = Profile::new(root.path(), "ses_1");
        let dir = profile.create().unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        profile.remove();
    }
}
