//! Shared native identity. Unsupported platforms cannot establish object equality.
use super::MemoryStorageError;
use std::fs::File;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub(super) enum ObjectIdentity {
    #[cfg(unix)]
    Unix(u64, u64),
    #[cfg(windows)]
    Windows(super::windows::FileIdentity),
}

pub(super) fn file_identity(file: &File) -> Result<Option<ObjectIdentity>, MemoryStorageError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok(Some(ObjectIdentity::Unix(metadata.dev(), metadata.ino())))
    }
    #[cfg(windows)]
    {
        Ok(Some(ObjectIdentity::Windows(super::windows::identity(
            file,
        )?)))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(MemoryStorageError::Unsafe(
            "native memory identity is unsupported",
        ))
    }
}
pub(super) fn verify_identity(current: &File, retained: &File) -> Result<(), MemoryStorageError> {
    if file_identity(current)? != file_identity(retained)? {
        return Err(MemoryStorageError::ReviewConflict);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn unix_identity_preserves_existing_review_serialization_shape() {
        assert_eq!(
            serde_json::to_string(&Some(ObjectIdentity::Unix(5, 7))).unwrap(),
            "[5,7]"
        );
    }
    #[cfg(windows)]
    #[test]
    fn windows_identity_serialization_preserves_all_volume_and_file_bits() {
        let native = super::super::windows::FileIdentity {
            volume: u64::MAX,
            file: [0xff; 16],
        };
        let value = serde_json::to_value(Some(ObjectIdentity::Windows(native))).unwrap();
        assert_eq!(value["volume"].as_u64(), Some(u64::MAX));
        assert_eq!(value["file"].as_array().unwrap().len(), 16);
        assert!(
            value["file"]
                .as_array()
                .unwrap()
                .iter()
                .all(|byte| byte.as_u64() == Some(255))
        );
    }
    #[cfg(windows)]
    #[test]
    fn windows_shared_identity_checks_retained_handles_and_refuses_replacements() {
        use super::super::windows::{create_private_directory, create_private_file};
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        let root = tempfile::tempdir().unwrap();
        let parent = std::fs::OpenOptions::new()
            .access_mode(FILE_ALL_ACCESS)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(root.path())
            .unwrap();
        let directory = create_private_directory(&parent, "memory").unwrap();
        let retained = create_private_file(&directory, "note").unwrap();
        let expected = file_identity(&retained).unwrap();
        assert!(expected.is_some());
        verify_identity(&retained.try_clone().unwrap(), &retained).unwrap();
        std::fs::rename(
            root.path().join("memory").join("note"),
            root.path().join("memory").join("original"),
        )
        .unwrap();
        let replacement = create_private_file(&directory, "note").unwrap();
        assert_ne!(file_identity(&replacement).unwrap(), expected);
        assert!(matches!(
            verify_identity(&replacement, &retained),
            Err(MemoryStorageError::ReviewConflict)
        ));
    }
}
