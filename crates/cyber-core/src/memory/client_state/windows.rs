//! Native checkpoint capabilities; public activation awaits durable replacement/recovery.
use super::*;
use crate::memory::windows as native;
use std::io::{self, Write};
#[path = "journal.rs"]
mod journal;
pub(super) use journal::{recover, save};
use std::os::windows::fs::OpenOptionsExt;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

pub(super) fn state_directory(path: &Path) -> Result<Dir, MemoryStorageError> {
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    native::identity(&file)?;
    if !file.metadata()?.is_dir() {
        return Err(MemoryStorageError::Unsafe(
            "expected a client state directory",
        ));
    }
    Ok(Dir::from_std_file(file))
}
pub(super) fn directory(parent: &Dir, create: bool) -> Result<Option<Dir>, MemoryStorageError> {
    child_directory(parent, DIRECTORY, create)
}
fn child_directory(
    parent: &Dir,
    name: &str,
    create: bool,
) -> Result<Option<Dir>, MemoryStorageError> {
    let parent = parent.try_clone()?.into_std_file();
    match native::open_pinned_private_directory(&parent, name, true) {
        Ok(file) => return Ok(Some(Dir::from_std_file(file))),
        Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if !create {
        return Ok(None);
    }
    let expected = match native::create_private_directory(&parent, name) {
        Ok(file) => {
            native::sync_private(&file)?;
            Some(native::identity(&file)?)
        }
        Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => None,
        Err(error) => return Err(error),
    };
    let file = native::open_pinned_private_directory(&parent, name, true)?;
    if let Some(expected) = expected
        && native::identity(&file)? != expected
    {
        return Err(MemoryStorageError::ReviewConflict);
    }
    Ok(Some(Dir::from_std_file(file)))
}
pub(super) fn lock(dir: &Dir) -> Result<File, MemoryStorageError> {
    let parent = dir.try_clone()?.into_std_file();
    match native::open_pinned_private_file(&parent, LOCK) {
        Ok(file) => return Ok(file),
        Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let expected = match native::create_private_file(&parent, LOCK) {
        Ok(file) => {
            native::sync_private(&file)?;
            native::sync_private(&parent)?;
            Some(native::identity(&file)?)
        }
        Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => None,
        Err(error) => return Err(error),
    };
    let file = native::open_pinned_private_file(&parent, LOCK)?;
    if let Some(expected) = expected
        && native::identity(&file)? != expected
    {
        return Err(MemoryStorageError::ReviewConflict);
    }
    Ok(file)
}
pub(super) fn file(dir: &Dir, name: &str) -> Result<File, MemoryStorageError> {
    let parent = dir.try_clone()?.into_std_file();
    let file = native::open_private_file(&parent, name, native::Access::Read)?;
    if name != CHECKPOINT {
        return Ok(file);
    }
    let expected = native::identity(&file)?;
    drop(file);
    native::freeze_private_file(&parent, name, expected)
}
pub(super) fn sync(dir: &Dir, parent: &Dir) -> Result<(), MemoryStorageError> {
    native::sync_private(&dir.try_clone()?.into_std_file())?;
    native::sync_namespace_directory(&parent.try_clone()?.into_std_file())
}
#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
