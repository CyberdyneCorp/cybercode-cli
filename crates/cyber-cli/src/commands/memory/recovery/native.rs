//! Keep Windows SQLite ownership inspection bound to its named regular file.
use cyber_core::memory::{MemoryStorageError, windows as native};
use std::os::windows::fs::OpenOptionsExt;
use std::{
    fs::File,
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

pub(super) struct DatabaseInspection {
    parent: File,
    parent_path: PathBuf,
    parent_id: native::FileIdentity,
    file: File,
    path: PathBuf,
    id: native::FileIdentity,
}

fn open(path: &Path, directory: bool) -> Result<File, MemoryStorageError> {
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(flags)
        .open(path)?;
    if file.metadata()?.is_dir() != directory {
        return Err(MemoryStorageError::Unsafe(
            "Invalid memory database inspection object",
        ));
    }
    native::identity(&file)?;
    Ok(file)
}

impl DatabaseInspection {
    pub(super) fn new(path: &Path) -> Result<Self, MemoryStorageError> {
        let parent_path = path
            .parent()
            .ok_or(MemoryStorageError::Unsafe("Database parent is unavailable"))?;
        let parent = open(parent_path, true)?;
        let parent_id = native::identity(&parent)?;
        let file = open(path, false)?;
        let id = native::identity(&file)?;
        let inspection = Self {
            parent,
            parent_path: parent_path.into(),
            parent_id,
            file,
            path: path.into(),
            id,
        };
        inspection.verify()?;
        Ok(inspection)
    }

    pub(super) fn verify(&self) -> Result<(), MemoryStorageError> {
        if native::identity(&self.parent)? != self.parent_id
            || native::identity(&open(&self.parent_path, true)?)? != self.parent_id
            || native::identity(&self.file)? != self.id
            || native::identity(&open(&self.path, false)?)? != self.id
        {
            return Err(MemoryStorageError::ReviewConflict);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_inspection_pins_database_and_parent_until_release() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("data");
        std::fs::create_dir(&parent).unwrap();
        let database = parent.join("events.db");
        std::fs::write(&database, "Database witness").unwrap();
        let inspection = DatabaseInspection::new(&database).unwrap();
        assert!(std::fs::rename(&database, parent.join("moved.db")).is_err());
        assert!(std::fs::rename(&parent, root.path().join("moved-data")).is_err());
        inspection.verify().unwrap();
        drop(inspection);
        std::fs::rename(&database, parent.join("moved.db")).unwrap();
        std::fs::rename(&parent, root.path().join("moved-data")).unwrap();
    }

    #[test]
    fn native_inspection_refuses_existing_or_new_hard_link_without_repair() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.db");
        std::fs::write(&path, "User database").unwrap();
        let alias = root.path().join("alias.db");
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(DatabaseInspection::new(&path).is_err());
        std::fs::remove_file(&alias).unwrap();
        let inspection = DatabaseInspection::new(&path).unwrap();
        if std::fs::hard_link(&path, &alias).is_ok() {
            assert!(inspection.verify().is_err());
            assert!(DatabaseInspection::new(&path).is_err());
            assert_eq!(std::fs::read_to_string(&alias).unwrap(), "User database");
        } else {
            inspection.verify().unwrap();
            assert!(!alias.exists());
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "User database");
        drop(inspection);
        if alias.exists() {
            std::fs::remove_file(alias).unwrap();
        }
        DatabaseInspection::new(&path).unwrap().verify().unwrap();
    }

    #[test]
    fn native_inspection_allows_ordinary_reads_writes_and_refuses_non_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.db");
        std::fs::write(&path, "Original").unwrap();
        let inspection = DatabaseInspection::new(&path).unwrap();
        std::fs::write(&path, "Updated").unwrap();
        inspection.verify().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Updated");
        assert!(DatabaseInspection::new(root.path()).is_err());
        assert!(DatabaseInspection::new(&root.path().join("missing")).is_err());
    }
}
