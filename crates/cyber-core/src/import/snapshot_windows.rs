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
    // and permit user renames after lookup and file data-read handles are released.
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
