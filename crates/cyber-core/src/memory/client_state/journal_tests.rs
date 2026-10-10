//! Actual private native save/recovery, with public activation kept closed.
use super::*;
fn fixture() -> (tempfile::TempDir, MemoryClientStore) {
    let data = tempfile::tempdir().unwrap();
    let parent = Dir::open_ambient_dir(data.path(), cap_std::ambient_authority()).unwrap();
    drop(native::create_private_directory(&parent.into_std_file(), "state").unwrap());
    let path = data.path().join("state");
    let parent = state_directory(&path).unwrap();
    let dir = directory(&parent, true).unwrap().unwrap();
    let store = MemoryClientStore::claim(&path, parent, dir).unwrap();
    (data, store)
}
fn reopen(path: &Path) -> MemoryClientStore {
    let parent = state_directory(path).unwrap();
    let dir = directory(&parent, false).unwrap().unwrap();
    MemoryClientStore::claim(path, parent, dir).unwrap()
}
#[test]
fn native_checkpoint_save_update_and_same_bytes_preserve_installed_identity() {
    let (_data, mut store) = fixture();
    save(&mut store, b"First").unwrap();
    let original = snapshot(&store.dir, CHECKPOINT).unwrap().unwrap();
    save(&mut store, b"First").unwrap();
    assert_eq!(
        snapshot(&store.dir, CHECKPOINT).unwrap(),
        Some(original.clone())
    );
    let pending = Journal::prepare(&store, b"Second").unwrap();
    let expected = pending.intent.after.clone();
    let id = pending.intent.id.clone();
    pending.commit(&store).unwrap();
    assert_eq!(snapshot(&store.dir, CHECKPOINT).unwrap(), Some(expected));
    let history = child_directory(&store.dir, HISTORY, false)
        .unwrap()
        .unwrap();
    let archived = child_directory(&history, &id, false).unwrap().unwrap();
    assert_eq!(snapshot(&archived, "before").unwrap(), Some(original));
    assert!(Journal::open(&store).unwrap().is_none());
    let path = store.state.clone();
    drop(archived);
    drop(history);
    drop(store);
    let restored = reopen(&path);
    assert_eq!(restored.checkpoint(), Some(b"Second".as_slice()));
}
#[test]
fn native_checkpoint_restart_recovers_original_captured_and_installed_slots() {
    for phase in [Phase::Original, Phase::Captured, Phase::Installed] {
        let (_data, mut store) = fixture();
        save(&mut store, b"Before").unwrap();
        let pending = Journal::prepare(&store, b"After").unwrap();
        let expected = pending.intent.after.clone();
        let original = pending.intent.before.clone().unwrap();
        let id = pending.intent.id.clone();
        if phase != Phase::Original {
            move_file(&store.dir, CHECKPOINT, &pending.dir, "before", &original).unwrap();
        }
        if phase == Phase::Installed {
            pending.install(&store).unwrap();
        }
        let path = store.state.clone();
        drop(pending);
        drop(store);
        let restored = reopen(&path);
        assert_eq!(restored.checkpoint(), Some(b"After".as_slice()));
        assert_eq!(snapshot(&restored.dir, CHECKPOINT).unwrap(), Some(expected));
        let history = child_directory(&restored.dir, HISTORY, false)
            .unwrap()
            .unwrap();
        let archived = child_directory(&history, &id, false).unwrap().unwrap();
        assert_eq!(snapshot(&archived, "before").unwrap(), Some(original));
        assert!(Journal::open(&restored).unwrap().is_none());
    }
}
#[test]
fn native_checkpoint_changed_user_file_refuses_before_preparation_or_installation() {
    let (_data, mut store) = fixture();
    save(&mut store, b"Original").unwrap();
    let mut writer = native::open_private_file(
        &descriptor(&store.dir).unwrap(),
        CHECKPOINT,
        native::Access::DataWrite,
    )
    .unwrap();
    writer.write_all(b"User edit").unwrap();
    drop(writer);
    assert!(matches!(
        save(&mut store, b"Proposed"),
        Err(MemoryStorageError::ReviewConflict)
    ));
    assert!(
        child_directory(&store.dir, PENDING, false)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        std::fs::read(store.state.join(DIRECTORY).join(CHECKPOINT)).unwrap(),
        b"User edit"
    );
}
#[test]
fn native_checkpoint_identical_byte_stage_replacement_refuses_and_preserves_evidence() {
    let (_data, store) = fixture();
    let pending = Journal::prepare(&store, b"Proposed").unwrap();
    let expected = pending.intent.after.clone();
    let path = store.state.join(DIRECTORY).join(PENDING).join("after");
    drop(pending);
    std::fs::rename(&path, path.with_extension("retained")).unwrap();
    let dir = child_directory(&store.dir, PENDING, false)
        .unwrap()
        .unwrap();
    create(&dir, "after", b"Proposed").unwrap();
    assert_ne!(snapshot(&dir, "after").unwrap().unwrap(), expected);
    assert!(Journal::open(&store).is_err());
    assert!(!store.state.join(DIRECTORY).join(CHECKPOINT).exists());
    assert!(path.with_extension("retained").exists());
}
#[test]
fn native_checkpoint_incomplete_intent_and_existing_target_collision_are_preserved() {
    let (_data, store) = fixture();
    let pending = Journal::prepare(&store, b"Proposed").unwrap();
    create(&store.dir, CHECKPOINT, b"User created").unwrap();
    assert!(pending.commit(&store).is_err());
    assert_eq!(
        std::fs::read(store.state.join(DIRECTORY).join(CHECKPOINT)).unwrap(),
        b"User created"
    );
    let dir = child_directory(&store.dir, PENDING, false)
        .unwrap()
        .unwrap();
    let mut writer = native::open_private_file(
        &descriptor(&dir).unwrap(),
        "intent.json",
        native::Access::DataWrite,
    )
    .unwrap();
    writer.set_len(0).unwrap();
    writer.write_all(b"{}").unwrap();
    drop(writer);
    assert!(Journal::open(&store).is_err());
    assert!(
        store
            .state
            .join(DIRECTORY)
            .join(PENDING)
            .join("after")
            .exists()
    );
}
#[test]
fn native_checkpoint_invalid_or_premature_completion_refuses_before_file_effects() {
    for marker in [b"invalid\n".as_slice(), b"committed\n".as_slice()] {
        let (_data, mut store) = fixture();
        save(&mut store, b"Original").unwrap();
        let original = snapshot(&store.dir, CHECKPOINT).unwrap();
        let pending = Journal::prepare(&store, b"Proposed").unwrap();
        create(&pending.dir, "completed", marker).unwrap();
        assert!(pending.commit(&store).is_err());
        assert_eq!(snapshot(&store.dir, CHECKPOINT).unwrap(), original);
        assert!(Journal::open(&store).is_err());
    }
}
fn replace_recorded_file(name: &str) {
    let (_data, mut store) = fixture();
    save(&mut store, b"Original").unwrap();
    let pending = Journal::prepare(&store, b"Proposed").unwrap();
    if name == "before" {
        move_file(
            &store.dir,
            CHECKPOINT,
            &pending.dir,
            "before",
            pending.intent.before.as_ref().unwrap(),
        )
        .unwrap();
    }
    if name == CHECKPOINT {
        pending.install(&store).unwrap();
    }
    let root_file = name == CHECKPOINT;
    let dir = if root_file { &store.dir } else { &pending.dir };
    let observed =
        native::open_private_file(&descriptor(dir).unwrap(), name, native::Access::Read).unwrap();
    let expected = native::identity(&observed).unwrap();
    drop(observed);
    let path = if root_file {
        store.state.join(DIRECTORY).join(name)
    } else {
        store.state.join(DIRECTORY).join(PENDING).join(name)
    };
    let bytes = std::fs::read(&path).unwrap();
    drop(pending);
    std::fs::rename(&path, path.with_extension("retained")).unwrap();
    let journal = child_directory(&store.dir, PENDING, false)
        .unwrap()
        .unwrap();
    let dir = if root_file { &store.dir } else { &journal };
    create(dir, name, &bytes).unwrap();
    assert_ne!(snapshot(dir, name).unwrap().unwrap().id, expected);
    assert!(Journal::open(&store).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        std::fs::read(path.with_extension("retained")).unwrap(),
        bytes
    );
}
#[test]
fn native_checkpoint_every_recorded_file_class_refuses_identical_byte_replacement() {
    for name in ["intent.json", "after", "before", CHECKPOINT] {
        replace_recorded_file(name);
    }
}
