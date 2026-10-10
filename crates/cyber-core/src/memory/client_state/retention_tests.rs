//! Native retention through the real checkpoint journal and durable cleanup plan.
use super::super::tests::{fixture, reopen};
use super::*;
fn history(store: &MemoryClientStore) -> Dir {
    child_directory(&store.dir, HISTORY, false)
        .unwrap()
        .unwrap()
}
fn append(store: &mut MemoryClientStore, count: usize) {
    for index in 0..count {
        Journal::prepare(store, format!("checkpoint {index}").as_bytes())
            .unwrap()
            .commit(store)
            .unwrap();
        store.previous = store.read().unwrap();
    }
}
fn planned(store: &MemoryClientStore) -> Cleanup {
    let history = history(store);
    let targets = candidates(&history)
        .unwrap()
        .into_iter()
        .map(|name| capture(store, &history, &name).unwrap())
        .collect();
    let current = snapshot(&store.dir, CHECKPOINT)
        .unwrap()
        .map(|proof| proof.id);
    Cleanup::prepare(store, &history, select(targets, current).unwrap()).unwrap()
}
#[test]
fn native_checkpoint_retention_bounds_real_saves_and_preserves_foreign_names() {
    let (_data, mut store) = fixture();
    save(&mut store, b"initial").unwrap();
    create(&history(&store), "user-kept.txt", b"user content").unwrap();
    for index in 0..8 {
        let bytes = format!("save {index}");
        save(&mut store, bytes.as_bytes()).unwrap();
        assert_eq!(store.checkpoint(), Some(bytes.as_bytes()));
        assert!(candidates(&history(&store)).unwrap().len() <= KEEP);
    }
    assert_eq!(
        bounded(
            &native::open_private_file(
                &descriptor(&history(&store)).unwrap(),
                "user-kept.txt",
                native::Access::Read
            )
            .unwrap(),
            LIMIT as u64
        )
        .unwrap(),
        b"user content"
    );
    let before = snapshot(&store.dir, CHECKPOINT).unwrap();
    let path = store.state.clone();
    drop(store);
    let restored = reopen(&path);
    assert_eq!(snapshot(&restored.dir, CHECKPOINT).unwrap(), before);
    assert_eq!(candidates(&history(&restored)).unwrap().len(), KEEP);
}
#[test]
fn native_checkpoint_retention_batches_old_history_and_protects_current_object() {
    let (_data, mut store) = fixture();
    append(&mut store, 14);
    let before = snapshot(&store.dir, CHECKPOINT).unwrap();
    let cleanup = planned(&store);
    assert_eq!(cleanup.plan.targets.len(), BATCH);
    assert!(encoded(&cleanup.plan).unwrap().len() as u64 <= INTENT_LIMIT);
    cleanup.run(&store).unwrap();
    assert_eq!(candidates(&history(&store)).unwrap().len(), 6);
    prune(&store).unwrap();
    assert_eq!(candidates(&history(&store)).unwrap().len(), KEEP);
    assert_eq!(snapshot(&store.dir, CHECKPOINT).unwrap(), before);
    assert!(Cleanup::open(&store).unwrap().is_none());
}
#[test]
fn native_checkpoint_retention_reopens_partial_file_and_absent_directory_cleanup() {
    for whole_directory in [false, true] {
        let (_data, mut store) = fixture();
        append(&mut store, 5);
        let before = snapshot(&store.dir, CHECKPOINT).unwrap();
        let cleanup = planned(&store);
        let target = &cleanup.plan.targets[0];
        let history = history(&store);
        if whole_directory {
            remove_target(&history, target).unwrap();
        } else {
            let dir = child_directory(&history, &target.name, false)
                .unwrap()
                .unwrap();
            let proof = &target.files["completed"];
            native::dispose_private_file(&descriptor(&dir).unwrap(), "completed", proof.id)
                .unwrap()
                .remove_durable()
                .unwrap();
        }
        let path = store.state.clone();
        drop(history);
        drop(cleanup);
        drop(store);
        let restored = reopen(&path);
        assert_eq!(snapshot(&restored.dir, CHECKPOINT).unwrap(), before);
        assert_eq!(candidates(&self::history(&restored)).unwrap().len(), KEEP);
        assert!(Cleanup::open(&restored).unwrap().is_none());
    }
}
#[test]
fn native_checkpoint_retention_preflights_every_target_before_deleting_any() {
    for unexpected in [false, true] {
        let (_data, mut store) = fixture();
        append(&mut store, 5);
        let before = snapshot(&store.dir, CHECKPOINT).unwrap();
        let cleanup = planned(&store);
        let history = history(&store);
        let target = cleanup.plan.targets.last().unwrap();
        let dir = child_directory(&history, &target.name, false)
            .unwrap()
            .unwrap();
        if unexpected {
            create(&dir, "user-created", b"preserve").unwrap();
        } else {
            let mut writer = native::open_private_file(
                &descriptor(&dir).unwrap(),
                "completed",
                native::Access::DataWrite,
            )
            .unwrap();
            writer.write_all(b"user edit").unwrap();
        }
        drop(dir);
        assert!(cleanup.run(&store).is_err());
        assert_eq!(candidates(&history).unwrap().len(), 5);
        assert_eq!(snapshot(&store.dir, CHECKPOINT).unwrap(), before);
        assert!(snapshot(&store.dir, CLEANUP).unwrap().is_some());
    }
}
#[test]
fn native_checkpoint_retention_refuses_identical_byte_replacement_and_incomplete_plan() {
    for corrupt in [false, true] {
        let (_data, mut store) = fixture();
        append(&mut store, 4);
        let before = snapshot(&store.dir, CHECKPOINT).unwrap();
        let cleanup = planned(&store);
        let original = encoded(&cleanup.plan).unwrap();
        drop(cleanup);
        let path = store.state.join(DIRECTORY).join(CLEANUP);
        std::fs::rename(&path, path.with_extension("retained")).unwrap();
        create(&store.dir, CLEANUP, if corrupt { b"{" } else { &original }).unwrap();
        assert!(recover(&store).is_err());
        assert_eq!(candidates(&history(&store)).unwrap().len(), 4);
        assert_eq!(snapshot(&store.dir, CHECKPOINT).unwrap(), before);
        assert!(path.with_extension("retained").exists());
    }
}

#[test]
fn native_checkpoint_retention_rejects_cleanup_authority_for_current_archive() {
    let (_data, mut store) = fixture();
    append(&mut store, 3);
    let history = history(&store);
    let current = snapshot(&store.dir, CHECKPOINT).unwrap().unwrap();
    let target = candidates(&history)
        .unwrap()
        .into_iter()
        .map(|name| capture(&store, &history, &name).unwrap())
        .find(|target| target.after == current.id)
        .unwrap();
    assert!(Cleanup::prepare(&store, &history, vec![target]).is_err());
    assert_eq!(candidates(&history).unwrap().len(), 3);
    assert_eq!(snapshot(&store.dir, CHECKPOINT).unwrap(), Some(current));
}
