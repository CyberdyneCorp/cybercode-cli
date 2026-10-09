//! Private client checkpoints. Retained bytes grant no memory mutation authority.
use super::MemoryStorageError;
use super::storage::{
    private_builder, verify_private_directory, verify_private_file, verify_regular,
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::fs::{File, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const DIRECTORY: &str = "memory-client";
const CHECKPOINT: &str = "state.json";
const LOCK: &str = ".client.lock";
const LIMIT: usize = 16 * 1024 * 1024;

/// One retained owner; a second TUI must not overwrite this owner's draft.
pub struct MemoryClientStore {
    state: PathBuf,
    parent: Dir,
    dir: Dir,
    lock: File,
    previous: Option<Checkpoint>,
}
#[derive(PartialEq, Eq)]
struct Checkpoint {
    bytes: Vec<u8>,
    identity: Option<(u64, u64)>,
}
impl MemoryClientStore {
    /// Missing client storage remains missing. Existing unsafe storage is never repaired.
    pub fn existing(state: &Path) -> Result<Option<Self>, MemoryStorageError> {
        native()?;
        let parent = match Dir::open_ambient_dir(state, cap_std::ambient_authority()) {
            Ok(parent) => parent,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        match parent.symlink_metadata(DIRECTORY) {
            Ok(metadata) if !metadata.is_dir() => {
                return Err(MemoryStorageError::Unsafe(
                    "expected a client state directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let dir = match parent.open_dir_nofollow(DIRECTORY) {
            Ok(dir) => dir,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Self::claim(state, parent, dir).map(Some)
    }
    /// Creates only the private child of the caller's existing state directory.
    pub fn open(state: &Path) -> Result<Self, MemoryStorageError> {
        native()?;
        let parent = Dir::open_ambient_dir(state, cap_std::ambient_authority())?;
        match parent.create_dir_with(DIRECTORY, &private_builder()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let dir = parent.open_dir_nofollow(DIRECTORY)?;
        Self::claim(state, parent, dir)
    }
    fn claim(state: &Path, parent: Dir, dir: Dir) -> Result<Self, MemoryStorageError> {
        verify_private_directory(&dir)?;
        let mut options = private_options();
        options.read(true).write(true).create(true);
        let lock = dir.open_with(LOCK, &options)?.into_std();
        verify_regular(&lock)?;
        verify_private_file(&lock)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(MemoryStorageError::Busy),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        let mut store = Self {
            state: state.into(),
            parent,
            dir,
            lock,
            previous: None,
        };
        store.verify_binding()?;
        store.previous = store.read()?;
        Ok(store)
    }
    /// Bytes are evidence only. The consumer must validate its version and scoped identities.
    pub fn checkpoint(&self) -> Option<&[u8]> {
        self.previous
            .as_ref()
            .map(|snapshot| snapshot.bytes.as_slice())
    }

    pub fn save(&mut self, bytes: &[u8]) -> Result<(), MemoryStorageError> {
        if bytes.len() > LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        self.verify_binding()?;
        if self.read()? != self.previous {
            return Err(MemoryStorageError::ReviewConflict);
        }
        if self.checkpoint() == Some(bytes) {
            return self.sync();
        }
        let temporary = format!(".{}.pending", crate::ids::new_id("mcs"));
        let mut options = private_options();
        options.write(true).create_new(true);
        let mut file = self.dir.open_with(&temporary, &options)?.into_std();
        verify_regular(&file)?;
        verify_private_file(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        let identity = file_identity(&file)?;
        drop(file);
        self.verify_binding()?;
        if self.read()? != self.previous {
            return Err(MemoryStorageError::ReviewConflict);
        }
        self.dir.rename(&temporary, &self.dir, CHECKPOINT)?;
        self.previous = Some(Checkpoint {
            bytes: bytes.to_vec(),
            identity,
        });
        self.sync()
    }
    fn read(&self) -> Result<Option<Checkpoint>, MemoryStorageError> {
        match self.dir.symlink_metadata(CHECKPOINT) {
            Ok(metadata) if !metadata.is_file() => {
                return Err(MemoryStorageError::Unsafe(
                    "expected a regular client checkpoint",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let mut options = private_options();
        options.read(true);
        let mut file = match self.dir.open_with(CHECKPOINT, &options) {
            Ok(file) => file.into_std(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        verify_regular(&file)?;
        verify_private_file(&file)?;
        let mut bytes = Vec::new();
        let identity = file_identity(&file)?;
        (&mut file).take(LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        let current = self.dir.open_with(CHECKPOINT, &options)?.into_std();
        verify_identity(&current, &file)?;
        Ok(Some(Checkpoint { bytes, identity }))
    }
    fn verify_binding(&self) -> Result<(), MemoryStorageError> {
        verify_private_directory(&self.dir)?;
        verify_regular(&self.lock)?;
        verify_private_file(&self.lock)?;
        verify_identity(
            &self.parent.open_dir_nofollow(DIRECTORY)?.into_std_file(),
            &self.dir.try_clone()?.into_std_file(),
        )?;
        verify_identity(
            &Dir::open_ambient_dir(&self.state, cap_std::ambient_authority())?.into_std_file(),
            &self.parent.try_clone()?.into_std_file(),
        )?;
        let mut options = private_options();
        options.read(true);
        let lock = self.dir.open_with(LOCK, &options)?.into_std();
        verify_regular(&lock)?;
        verify_private_file(&lock)?;
        verify_identity(&lock, &self.lock)
    }
    fn sync(&self) -> Result<(), MemoryStorageError> {
        #[cfg(unix)]
        {
            super::storage::directory_file(&self.dir)?.sync_all()?;
            super::storage::directory_file(&self.parent)?.sync_all()?;
        }
        Ok(())
    }
}
fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
fn native() -> Result<(), MemoryStorageError> {
    if !cfg!(unix) {
        return Err(MemoryStorageError::Unsafe(
            "client checkpoint privacy and durability require native support",
        ));
    }
    Ok(())
}
fn file_identity(file: &File) -> Result<Option<(u64, u64)>, MemoryStorageError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok(Some((metadata.dev(), metadata.ino())))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(None)
    }
}
fn verify_identity(current: &File, retained: &File) -> Result<(), MemoryStorageError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current = current.metadata()?;
        let retained = retained.metadata()?;
        if (current.dev(), current.ino()) != (retained.dev(), retained.ino()) {
            return Err(MemoryStorageError::ReviewConflict);
        }
    }
    #[cfg(not(unix))]
    let _ = (current, retained);
    Ok(())
}
