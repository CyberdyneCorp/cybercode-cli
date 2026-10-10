//! Public native checkpoint admission and private capability/refusal boundaries.
use super::*;
fn state() -> tempfile::TempDir {
    let outer = tempfile::tempdir().unwrap();
    let parent = Dir::open_ambient_dir(outer.path(), cap_std::ambient_authority()).unwrap();
    drop(native::create_private_directory(&parent.into_std_file(), "state").unwrap());
    outer
}
fn admit(path: &Path) -> MemoryClientStore {
    let parent = state_directory(path).unwrap();
    let dir = directory(&parent, true).unwrap().unwrap();
    let store = MemoryClientStore::claim(path, parent, dir).unwrap();
    store.sync().unwrap();
    store
}
fn write(dir: &Dir, name: &str, bytes: &[u8]) {
    let mut file =
        native::create_private_file(&dir.try_clone().unwrap().into_std_file(), name).unwrap();
    file.write_all(bytes).unwrap();
    native::sync_private(&file).unwrap();
}
#[test]
fn native_client_existing_only_preserves_absence_and_public_open_claims_native_storage() {
    let data = state();
    let path = data.path().join("state");
    let parent = state_directory(&path).unwrap();
    assert!(directory(&parent, false).unwrap().is_none());
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
    assert!(state_directory(&path.join("missing")).is_err());
    assert!(!path.join("missing").exists());
    assert!(MemoryClientStore::existing(&path).unwrap().is_none());
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
    let owner = MemoryClientStore::open(&path).unwrap();
    assert!(owner.checkpoint().is_none());
    native::verify_private(&owner.dir.try_clone().unwrap().into_std_file()).unwrap();
}
#[test]
fn native_client_private_claim_reads_reopens_and_excludes_another_owner() {
    let data = state();
    let path = data.path().join("state");
    let mut owner = MemoryClientStore::open(&path).unwrap();
    assert!(owner.checkpoint().is_none());
    let parent = state_directory(&path).unwrap();
    let dir = directory(&parent, true).unwrap().unwrap();
    assert!(matches!(
        MemoryClientStore::claim(&path, parent, dir),
        Err(MemoryStorageError::Busy)
    ));
    owner
        .save(b"retained draft, no mutation authority")
        .unwrap();
    drop(owner);
    let reopened = MemoryClientStore::existing(&path).unwrap().unwrap();
    assert_eq!(
        reopened.checkpoint(),
        Some(b"retained draft, no mutation authority".as_slice())
    );
    reopened.verify_binding().unwrap();
    assert!(!path.join("memory").exists());
}
#[test]
fn native_client_state_directory_and_lock_names_are_pinned_until_disposal() {
    let data = state();
    let path = data.path().join("state");
    let owner = admit(&path);
    for child in [
        path.clone(),
        path.join(DIRECTORY),
        path.join(DIRECTORY).join(LOCK),
    ] {
        assert!(std::fs::rename(&child, child.with_extension("moved")).is_err());
    }
    owner.verify_binding().unwrap();
    drop(owner);
    let lock = path.join(DIRECTORY).join(LOCK);
    std::fs::rename(&lock, lock.with_extension("moved")).unwrap();
    std::fs::rename(lock.with_extension("moved"), &lock).unwrap();
    std::fs::rename(&path, path.with_extension("moved")).unwrap();
}
#[test]
fn native_client_checkpoint_aliases_and_preexisting_writers_refuse_without_repair() {
    let data = state();
    let path = data.path().join("state");
    let owner = admit(&path);
    write(&owner.dir, CHECKPOINT, b"original retained draft");
    let writer = native::open_private_file(
        &owner.dir.try_clone().unwrap().into_std_file(),
        CHECKPOINT,
        native::Access::DataWrite,
    )
    .unwrap();
    assert!(owner.read().is_err());
    drop(writer);
    assert_eq!(
        owner.read().unwrap().unwrap().bytes,
        b"original retained draft"
    );
    let checkpoint = path.join(DIRECTORY).join(CHECKPOINT);
    let frozen = file(&owner.dir, CHECKPOINT).unwrap();
    assert!(
        native::open_private_file(
            &owner.dir.try_clone().unwrap().into_std_file(),
            CHECKPOINT,
            native::Access::DataWrite
        )
        .is_err()
    );
    assert!(std::fs::rename(&checkpoint, checkpoint.with_extension("moved")).is_err());
    assert_eq!(
        std::fs::read(&checkpoint).unwrap(),
        b"original retained draft"
    );
    drop(frozen);
    let alias = path.join("user-alias");
    std::fs::hard_link(&checkpoint, &alias).unwrap();
    assert!(owner.read().is_err());
    assert_eq!(std::fs::read(&alias).unwrap(), b"original retained draft");
    std::fs::remove_file(alias).unwrap();
    assert_eq!(
        owner.read().unwrap().unwrap().bytes,
        b"original retained draft"
    );
}
#[test]
fn native_client_oversized_checkpoint_is_preserved_and_refused() {
    let data = state();
    let path = data.path().join("state");
    let owner = admit(&path);
    write(&owner.dir, CHECKPOINT, &vec![b'x'; LIMIT + 1]);
    assert!(matches!(owner.read(), Err(MemoryStorageError::TooLarge)));
    assert_eq!(
        std::fs::metadata(path.join(DIRECTORY).join(CHECKPOINT))
            .unwrap()
            .len(),
        (LIMIT + 1) as u64
    );
    assert!(owner.checkpoint().is_none());
}
#[test]
fn native_client_unsafe_existing_directory_and_lock_are_not_repaired() {
    let data = state();
    let path = data.path().join("state");
    std::fs::create_dir(path.join(DIRECTORY)).unwrap();
    let parent = state_directory(&path).unwrap();
    assert!(directory(&parent, false).is_err());
    assert!(!path.join(DIRECTORY).join(LOCK).exists());
    drop(parent);
    std::fs::remove_dir(path.join(DIRECTORY)).unwrap();
    let owner = admit(&path);
    drop(owner);
    let lock = path.join(DIRECTORY).join(LOCK);
    let retained = lock.with_extension("retained");
    std::fs::rename(&lock, &retained).unwrap();
    std::fs::hard_link(&retained, &lock).unwrap();
    let parent = state_directory(&path).unwrap();
    let dir = directory(&parent, false).unwrap().unwrap();
    assert!(MemoryClientStore::claim(&path, parent, dir).is_err());
    assert!(retained.exists());
    assert!(lock.exists());
}

#[test]
fn native_client_actual_startup_inherited_container_saves_retains_and_reopens_private_state() {
    let data = tempfile::tempdir().unwrap();
    let paths = crate::paths::Paths {
        data: data.path().join("data"),
        config: data.path().join("config"),
        state: data.path().join("state"),
        cache: data.path().join("cache"),
        tmp: data.path().join("tmp"),
    };
    paths.ensure().unwrap();
    let mut store = MemoryClientStore::open(&paths.state).unwrap();
    let parent_id = native::identity(&store.parent.try_clone().unwrap().into_std_file()).unwrap();
    assert!(native::verify_private(&store.parent.try_clone().unwrap().into_std_file()).is_err());
    native::verify_private(&store.dir.try_clone().unwrap().into_std_file()).unwrap();
    std::fs::write(paths.state.join("unrelated-state"), b"preserve me").unwrap();
    for index in 0..6 {
        store
            .save(format!("startup checkpoint {index}").as_bytes())
            .unwrap();
    }
    let bytes = store.checkpoint().unwrap().to_vec();
    let file = native::open_private_file(
        &store.dir.try_clone().unwrap().into_std_file(),
        CHECKPOINT,
        native::Access::Read,
    )
    .unwrap();
    let id = native::verify_private(&file).unwrap();
    drop(file);
    assert_eq!(
        std::fs::read_dir(paths.state.join(DIRECTORY).join(".checkpoint-history"))
            .unwrap()
            .count(),
        2
    );
    assert_eq!(
        native::identity(&store.parent.try_clone().unwrap().into_std_file()).unwrap(),
        parent_id
    );
    assert!(native::verify_private(&store.parent.try_clone().unwrap().into_std_file()).is_err());
    drop(store);
    let restored = MemoryClientStore::existing(&paths.state).unwrap().unwrap();
    assert_eq!(restored.checkpoint(), Some(bytes.as_slice()));
    let file = native::open_private_file(
        &restored.dir.try_clone().unwrap().into_std_file(),
        CHECKPOINT,
        native::Access::Read,
    )
    .unwrap();
    assert_eq!(native::verify_private(&file).unwrap(), id);
    assert_eq!(
        std::fs::read(paths.state.join("unrelated-state")).unwrap(),
        b"preserve me"
    );
}
