//! Metadata-only retention checked against the still-pinned source lookup chain.
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::fs::OpenOptionsExt,
    path::Path,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};

pub(super) fn retain_identity(path: &Path) -> io::Result<File> {
    // The caller retains every original lookup handle until all identities match.
    // Zero desired access supports metadata queries without retaining access rights.
    // Reject final reparse points through the identity check,
    // Ancestor rename compatibility is still under native investigation.
    OpenOptions::new()
        .read(true)
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn directory_identity_handle_allows_renaming_that_directory() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("source");
        std::fs::create_dir(&parent).unwrap();
        let retained = retain_identity(&parent).unwrap();
        let identity = crate::memory::windows::identity(&retained).unwrap();
        let moved = temp.path().join("moved");
        std::fs::rename(&parent, &moved).expect("rename with only the directory itself retained");
        assert_eq!(
            crate::memory::windows::identity(&retained).unwrap(),
            identity
        );
        let reopened = retain_identity(&moved).unwrap();
        assert_eq!(
            crate::memory::windows::identity(&reopened).unwrap(),
            identity
        );
    }

    #[test]
    fn child_directory_identity_handle_allows_renaming_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("source");
        let child = parent.join("nested");
        std::fs::create_dir_all(&child).unwrap();
        let retained = retain_identity(&child).unwrap();
        let identity = crate::memory::windows::identity(&retained).unwrap();
        let moved = temp.path().join("moved");
        std::fs::rename(&parent, &moved).expect("rename with only a child directory retained");
        assert_eq!(
            crate::memory::windows::identity(&retained).unwrap(),
            identity
        );
        let reopened = retain_identity(&moved.join("nested")).unwrap();
        assert_eq!(
            crate::memory::windows::identity(&reopened).unwrap(),
            identity
        );
    }

    #[test]
    fn file_identity_handle_allows_renaming_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("source");
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("config.json");
        std::fs::write(&path, b"private-source-value").unwrap();
        let retained = retain_identity(&path).unwrap();
        let identity = crate::memory::windows::identity(&retained).unwrap();
        let moved = temp.path().join("moved");
        std::fs::rename(&parent, &moved).expect("rename with only the child file retained");
        assert_eq!(
            crate::memory::windows::identity(&retained).unwrap(),
            identity
        );
        let reopened = retain_identity(&moved.join("config.json")).unwrap();
        assert_eq!(
            crate::memory::windows::identity(&reopened).unwrap(),
            identity
        );
    }

    // This probe does not implement directory or empty-file identity retention.
    #[test]
    fn read_only_section_without_file_handle_allows_ancestor_move_and_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("source");
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("config.json");
        std::fs::write(&path, b"private-source-value").unwrap();
        let original = File::open(&path).unwrap();
        let identity = crate::memory::windows::identity(&original).unwrap();
        let section = read_only_section(&original).unwrap();
        drop(original);
        let moved = temp.path().join("moved");
        std::fs::rename(&parent, &moved)
            .expect("rename retaining only an unnamed read-only section");
        let reopened = File::open(moved.join("config.json")).unwrap();
        assert_eq!(
            crate::memory::windows::identity(&reopened).unwrap(),
            identity
        );
        drop(reopened);
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(&path, b"private-source-value").unwrap();
        let replacement = File::open(&path).unwrap();
        assert_ne!(
            crate::memory::windows::identity(&replacement).unwrap(),
            identity
        );
        drop(section);
    }

    #[allow(unsafe_code)] // Test-only native section ownership; no views or source mutations.
    fn read_only_section(file: &File) -> io::Result<std::os::windows::io::OwnedHandle> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::Memory::{CreateFileMappingW, PAGE_READONLY};
        // The borrowed live file has read access. Null security/name creates a private,
        // non-inheritable section; zero sizes use the existing nonempty file length.
        let raw = unsafe {
            CreateFileMappingW(
                file.as_raw_handle(),
                std::ptr::null(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // Successful creation transfers exactly one CloseHandle obligation to OwnedHandle.
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }

    #[test]
    fn metadata_retention_preserves_identity_without_data_access_or_ancestor_rename_locks() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("source");
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("config.json");
        std::fs::write(&path, b"private-source-value").unwrap();
        let identity = {
            let original = File::open(&path).unwrap();
            crate::memory::windows::identity(&original).unwrap()
        };
        let directory = retain_identity(&parent).unwrap();
        let mut file = retain_identity(&path).unwrap();
        assert_eq!(crate::memory::windows::identity(&file).unwrap(), identity);
        assert_eq!(file.metadata().unwrap().len(), 20);
        let mut bytes = Vec::new();
        assert!(file.read_to_end(&mut bytes).is_err());
        assert!(bytes.is_empty());
        let moved = temp.path().join("moved-source");
        std::fs::rename(&parent, &moved).unwrap();
        assert!(directory.metadata().unwrap().is_dir());
        assert_eq!(crate::memory::windows::identity(&file).unwrap(), identity);
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(&path, b"private-source-value").unwrap();
        let replacement = File::open(&path).unwrap();
        assert_ne!(
            crate::memory::windows::identity(&replacement).unwrap(),
            identity
        );
    }
}
