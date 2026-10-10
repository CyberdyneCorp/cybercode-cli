//! Native exact-source disposal, including failure after visible namespace effects.
use super::*;
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
fn fixture() -> (tempfile::TempDir, File) {
    let data = tempfile::tempdir().unwrap();
    let bootstrap = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(data.path())
        .unwrap();
    let parent = create_private_directory(&bootstrap, "private").unwrap();
    (data, parent)
}
fn staged(parent: &File, name: &str, bytes: &[u8]) -> FileIdentity {
    let mut file = create_private_file(parent, name).unwrap();
    file.write_all(bytes).unwrap();
    sync_private(&file).unwrap();
    identity(&file).unwrap()
}
#[test]
fn native_disposal_excludes_independent_access_and_removes_exact_file_durably() {
    let (data, parent) = fixture();
    let expected = staged(&parent, "source", b"owned bytes");
    let source = dispose_private_file(&parent, "source", expected).unwrap();
    let path = data.path().join("private/source");
    assert!(std::fs::read(&path).is_err());
    assert!(open_private_file(&parent, "source", Access::DataWrite).is_err());
    assert!(std::fs::remove_file(&path).is_err());
    assert!(std::fs::rename(&path, path.with_extension("moved")).is_err());
    let mut bytes = Vec::new();
    source.file().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"owned bytes");
    let parent_id = identity(&parent).unwrap();
    source.remove_durable().unwrap();
    assert!(!path.exists());
    assert_eq!(identity(&parent).unwrap(), parent_id);
}
#[test]
fn native_disposal_refuses_nonempty_directory_and_allows_owned_child_then_empty_directory() {
    let (data, parent) = fixture();
    let child = create_private_directory(&parent, "journal").unwrap();
    let dir_id = identity(&child).unwrap();
    let file_id = staged(&child, "before", b"captured draft");
    drop(child);
    assert!(
        dispose_private_directory(&parent, "journal", dir_id)
            .unwrap()
            .remove_durable()
            .is_err()
    );
    assert_eq!(
        std::fs::read(data.path().join("private/journal/before")).unwrap(),
        b"captured draft"
    );
    let directory = dispose_private_directory(&parent, "journal", dir_id).unwrap();
    dispose_private_file(directory.file(), "before", file_id)
        .unwrap()
        .remove_durable()
        .unwrap();
    directory.remove_durable().unwrap();
    assert!(!data.path().join("private/journal").exists());
}
#[test]
fn native_disposal_preexisting_readers_and_writers_refuse_without_effects() {
    for access in [Access::Read, Access::DataWrite] {
        let (data, parent) = fixture();
        let id = staged(&parent, "source", b"retained");
        let reader = open_private_file(&parent, "source", access).unwrap();
        assert!(dispose_private_file(&parent, "source", id).is_err());
        assert_eq!(
            std::fs::read(data.path().join("private/source")).unwrap(),
            b"retained"
        );
        drop(reader);
        dispose_private_file(&parent, "source", id)
            .unwrap()
            .remove_durable()
            .unwrap();
    }
}
#[test]
fn native_disposal_wrong_identity_aliases_types_and_unsafe_existing_objects_are_preserved() {
    let (data, parent) = fixture();
    let id = staged(&parent, "source", b"user original");
    let wrong = staged(&parent, "other", b"other bytes");
    assert!(dispose_private_file(&parent, "source", wrong).is_err());
    assert!(dispose_private_directory(&parent, "source", id).is_err());
    let path = data.path().join("private/source");
    let alias = path.with_extension("alias");
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(dispose_private_file(&parent, "source", id).is_err());
    assert_eq!(std::fs::read(&alias).unwrap(), b"user original");
    let unsafe_path = data.path().join("private/unprotected");
    std::fs::write(&unsafe_path, b"unprotected user bytes").unwrap();
    let unprotected = std::fs::File::open(&unsafe_path).unwrap();
    let unsafe_id = identity(&unprotected).unwrap();
    assert!(verify_private(&unprotected).is_err());
    drop(unprotected);
    assert!(dispose_private_file(&parent, "unprotected", unsafe_id).is_err());
    assert_eq!(
        std::fs::read(&unsafe_path).unwrap(),
        b"unprotected user bytes"
    );
    assert!(verify_private(&std::fs::File::open(&unsafe_path).unwrap()).is_err());
}
#[test]
fn native_disposal_failed_preflight_preserves_source_and_failed_postflush_does_not_acknowledge() {
    for failure in [1, 2, 3] {
        let (data, parent) = fixture();
        let id = staged(&parent, "source", b"evidence");
        let source = dispose_private_file(&parent, "source", id).unwrap();
        let mut calls = 0;
        let result = source.remove_with(|file| {
            calls += 1;
            if calls == failure {
                return Err(io::Error::other("injected disposal flush failure").into());
            }
            sync_private(file)
        });
        assert!(result.is_err());
        let path = data.path().join("private/source");
        if failure < 3 {
            assert_eq!(std::fs::read(path).unwrap(), b"evidence");
        } else {
            assert!(!path.exists());
        }
    }
}
#[test]
fn native_disposal_recreated_name_is_preserved_and_refuses_acknowledgement() {
    let (data, parent) = fixture();
    let id = staged(&parent, "source", b"original");
    let source = dispose_private_file(&parent, "source", id).unwrap();
    let mut calls = 0;
    let result = source.remove_with(|file| {
        calls += 1;
        if calls == 3 {
            staged(&parent, "source", b"user replacement");
        }
        sync_private(file)
    });
    assert!(matches!(result, Err(MemoryStorageError::ReviewConflict)));
    assert_eq!(
        std::fs::read(data.path().join("private/source")).unwrap(),
        b"user replacement"
    );
}
#[test]
fn native_disposal_duplicate_owned_handle_cannot_falsely_acknowledge_pending_removal() {
    let (data, parent) = fixture();
    let id = staged(&parent, "source", b"pending");
    let source = dispose_private_file(&parent, "source", id).unwrap();
    let duplicate = source.file().try_clone().unwrap();
    assert!(source.remove_durable().is_err());
    drop(duplicate);
    sync_private(&parent).unwrap();
    assert!(!data.path().join("private/source").exists());
}
#[test]
fn native_disposal_pending_or_unknown_status_is_never_success() {
    assert!(settlement(0).is_ok());
    assert!(settlement(0x103).is_err());
    assert!(settlement(-1).is_err());
}
