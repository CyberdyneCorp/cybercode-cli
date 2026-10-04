//! The advisory lock that makes one process the writer owner (`<state>/server.lock`).

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::StoreError;

/// Held for the lifetime of the owning process; the OS releases it on exit or crash.
#[derive(Debug)]
pub struct OwnershipLock {
    _file: File,
    path: PathBuf,
}

impl OwnershipLock {
    pub fn acquire(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(StoreError::LockHeld(path.to_path_buf())),
            Err(TryLockError::Error(e)) => return Err(e.into()),
        }
        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
