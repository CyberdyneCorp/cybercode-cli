//! Native Windows evidence for the review handoff in `snapshot.rs`: why snapshots release
//! their handles after capture, and why full file identities still refuse replacements.
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

/// The least intrusive handle Windows offers: zero desired access, full sharing.
fn metadata_handle(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

const ACCESS_DENIED: i32 = 5;

mod tests {
    use super::*;
    use crate::memory::windows::identity;

    #[test]
    fn directory_identity_handle_allows_renaming_that_directory() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("source");
        std::fs::create_dir(&parent).unwrap();
        let retained = metadata_handle(&parent).unwrap();
        let before = identity(&retained).unwrap();
        let moved = temp.path().join("moved");
        std::fs::rename(&parent, &moved).expect("rename with only the directory itself open");
        assert_eq!(identity(&retained).unwrap(), before);
        assert_eq!(identity(&metadata_handle(&moved).unwrap()).unwrap(), before);
    }

    /// NTFS refuses an ancestor rename while any descendant is open, even a zero-access,
    /// fully shared handle. This is why a returned snapshot holds no handles on Windows.
    #[test]
    fn open_descendants_block_ancestor_renames_until_released() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("source");
        let child = parent.join("nested");
        std::fs::create_dir_all(&child).unwrap();
        let file = parent.join("config.json");
        std::fs::write(&file, b"private-source-value").unwrap();
        let moved = temp.path().join("moved");
        for descendant in [&child, &file] {
            let handle = metadata_handle(descendant).unwrap();
            let refused = std::fs::rename(&parent, &moved).unwrap_err();
            assert_eq!(
                refused.raw_os_error(),
                Some(ACCESS_DENIED),
                "{descendant:?}"
            );
            drop(handle);
        }
        let before = identity(&metadata_handle(&file).unwrap()).unwrap();
        std::fs::rename(&parent, &moved).expect("rename once descendants are released");
        let after = identity(&metadata_handle(&moved.join("config.json")).unwrap()).unwrap();
        assert_eq!(after, before);
    }

    /// Without live handles, a recreated object still gets a different full identity:
    /// beside a moved original, and after the original is deleted (NTFS bumps the file
    /// record's sequence number on reuse).
    #[test]
    fn released_sources_keep_distinct_identities_from_replacements() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("source");
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("config.json");
        std::fs::write(&path, b"private-source-value").unwrap();
        let original = identity(&File::open(&path).unwrap()).unwrap();
        let directory = identity(&metadata_handle(&parent).unwrap()).unwrap();
        let moved = temp.path().join("moved-source");
        std::fs::rename(&parent, &moved).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(&path, b"private-source-value").unwrap();
        assert_ne!(identity(&File::open(&path).unwrap()).unwrap(), original);
        assert_ne!(
            identity(&metadata_handle(&parent).unwrap()).unwrap(),
            directory
        );
        std::fs::remove_dir_all(&moved).unwrap();
        for _ in 0..16 {
            std::fs::remove_file(&path).unwrap();
            std::fs::write(&path, b"private-source-value").unwrap();
            assert_ne!(identity(&File::open(&path).unwrap()).unwrap(), original);
        }
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
}
