//! Native data, metadata and storage-cache flushing without fallback acknowledgement.
use super::*;
use std::ptr::null;
use windows_sys::Wdk::Storage::FileSystem::NtFlushBuffersFileEx;
use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

/// Flushes the exact retained private object. Unsupported or read-only flushes refuse.
/// Callers must additionally synchronize affected parent directories after installation.
pub fn sync_private(file: &File) -> Result<(), MemoryStorageError> {
    let before = verify_private(file)?;
    let mut status = IO_STATUS_BLOCK::default();
    // Normal flags flush data, metadata and the storage cache. The native flush operation
    // is synchronous, so neither IO_STATUS_BLOCK nor input storage can outlive this call.
    let result = unsafe { NtFlushBuffersFileEx(file.as_raw_handle(), 0, null(), 0, &mut status) };
    if result < 0 {
        return Err(
            io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(result) } as i32).into(),
        );
    }
    if result != 0 || unsafe { status.Anonymous.Status } != 0 {
        return Err(refusal("native memory flush returned unknown settlement"));
    }
    if verify_private(file)? != before {
        return Err(refusal("native memory flush identity changed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    fn parent(path: &std::path::Path) -> File {
        std::fs::OpenOptions::new()
            .access_mode(FILE_ALL_ACCESS)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap()
    }
    #[test]
    fn native_normal_flush_accepts_private_file_and_directory_with_identity_preserved() {
        let root = tempfile::tempdir().unwrap();
        let directory = create_private_directory(&parent(root.path()), "memory").unwrap();
        let directory_identity = identity(&directory).unwrap();
        let mut file = create_private_file(&directory, "note").unwrap();
        file.write_all(b"private bytes").unwrap();
        let file_identity = identity(&file).unwrap();
        sync_private(&file).unwrap();
        sync_private(&directory).unwrap();
        assert_eq!(identity(&file).unwrap(), file_identity);
        assert_eq!(identity(&directory).unwrap(), directory_identity);
        assert_eq!(
            std::fs::read(root.path().join("memory").join("note")).unwrap(),
            b"private bytes"
        );
    }
    #[test]
    fn native_flush_refuses_read_only_broad_and_aliased_objects_without_fallback() {
        let root = tempfile::tempdir().unwrap();
        let bootstrap = parent(root.path());
        let directory = create_private_directory(&bootstrap, "memory").unwrap();
        let mut file = create_private_file(&directory, "note").unwrap();
        file.write_all(b"user bytes").unwrap();
        let read_file = open_private_file(&directory, "note", Access::Read).unwrap();
        let read_directory = open_private_directory(&bootstrap, "memory", Access::Read).unwrap();
        assert!(sync_private(&read_file).is_err());
        assert!(sync_private(&read_directory).is_err());
        assert!(sync_private(&bootstrap).is_err());
        std::fs::hard_link(
            root.path().join("memory").join("note"),
            root.path().join("alias"),
        )
        .unwrap();
        assert!(sync_private(&file).is_err());
        assert_eq!(
            std::fs::read(root.path().join("alias")).unwrap(),
            b"user bytes"
        );
    }
}
