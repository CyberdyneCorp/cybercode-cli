//! Exercise the real private journal pipeline without activating public mutation admission.
use super::*;

fn text(body: &str) -> String {
    format!("---\nname: rule\ndescription: useful fact\ntype: user\n---\n{body}\n")
}
fn prepare<'guard, 'store>(
    scope: &'guard mut MemoryScope<'store>,
    body: &str,
) -> PreparedMemory<'guard, 'store> {
    let document = MemoryDocument::for_write(&text(body)).unwrap();
    let rendered = document.render_for_write().unwrap();
    scope
        .prepare("rule", Some((document, rendered)), None, None)
        .unwrap()
}
fn object(dir: &Dir, name: &str) -> native::FileIdentity {
    native::identity(&optional_file(dir, name).unwrap().unwrap()).unwrap()
}

#[test]
fn native_live_intent_replacement_with_identical_bytes_refuses_before_effects() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let pending = prepare(&mut scope, "Proposed");
    let bytes = optional_bytes(&pending.dir, "intent.json", INTENT_LIMIT)
        .unwrap()
        .unwrap();
    let identity = object(&pending.dir, "intent.json");
    let intent = store.path().join(TRANSACTION).join("intent.json");
    let retained = intent.with_file_name("retained-intent");
    assert!(std::fs::rename(&intent, &retained).is_err());
    assert!(std::fs::remove_file(&intent).is_err());
    assert!(!retained.exists());
    assert_eq!(object(&pending.dir, "intent.json"), identity);
    assert_eq!(std::fs::read(&intent).unwrap(), bytes);
    assert!(!store.path().join("rule.md").exists());
    assert!(!store.path().join("MEMORY.md").exists());
    drop(pending);
    std::fs::rename(&intent, &retained).unwrap();
    std::fs::rename(&retained, &intent).unwrap();
    let recovered = scope.read_prepared().unwrap().unwrap();
    assert_eq!(object(&recovered.dir, "intent.json"), identity);
    assert!(std::fs::rename(&intent, &retained).is_err());
    recovered.commit().unwrap();
}

#[test]
fn native_live_intent_content_change_refuses_before_effects_or_acknowledgement() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let pending = prepare(&mut scope, "Proposed");
    let parent = pending.dir.try_clone().unwrap().into_std_file();
    let bytes = optional_bytes(&pending.dir, "intent.json", INTENT_LIMIT)
        .unwrap()
        .unwrap();
    let identity = object(&pending.dir, "intent.json");
    assert!(native::open_private_file(&parent, "intent.json", native::Access::DataWrite).is_err());
    assert!(native::open_private_file(&parent, "intent.json", native::Access::Write).is_err());
    assert_eq!(
        optional_bytes(&pending.dir, "intent.json", INTENT_LIMIT)
            .unwrap()
            .unwrap(),
        bytes
    );
    assert!(!store.path().join("rule.md").exists());
    assert!(!store.path().join("MEMORY.md").exists());
    drop(pending);
    let mut file =
        native::open_private_file(&parent, "intent.json", native::Access::DataWrite).unwrap();
    file.write_all(&bytes).unwrap();
    drop(file);
    // Release the fixture's writable journal directory before exclusive archival.
    drop(parent);
    let recovered = scope.read_prepared().unwrap().unwrap();
    assert_eq!(object(&recovered.dir, "intent.json"), identity);
    assert!(
        native::open_private_file(
            &recovered.dir.try_clone().unwrap().into_std_file(),
            "intent.json",
            native::Access::DataWrite
        )
        .is_err()
    );
    recovered.commit().unwrap();
}

#[test]
fn native_existing_intent_writer_refuses_recovery_admission_until_released() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let pending = prepare(&mut scope, "Proposed");
    let journal = pending.dir.try_clone().unwrap().into_std_file();
    drop(pending);
    let writer =
        native::open_private_file(&journal, "intent.json", native::Access::DataWrite).unwrap();
    assert!(scope.read_prepared().is_err());
    assert!(!store.path().join("rule.md").exists());
    assert!(!store.path().join("MEMORY.md").exists());
    drop(writer);
    drop(journal);
    scope.read_prepared().unwrap().unwrap().commit().unwrap();
    assert_eq!(scope.read("rule").unwrap().body, "Proposed");
}

#[test]
fn native_live_journal_name_is_pinned_until_disposal_or_exact_source_archival() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let pending = prepare(&mut scope, "Proposed");
    let journal = store.path().join(TRANSACTION);
    let moved = store.path().join("moved-journal");
    assert!(std::fs::rename(&journal, &moved).is_err());
    assert!(journal.exists());
    assert!(!moved.exists());
    let receipt = pending.commit().unwrap();
    assert!(!journal.exists());
    assert!(store.path().join(HISTORY).join(receipt.id).exists());
    let pending = prepare(&mut scope, "Second");
    assert!(std::fs::rename(&journal, &moved).is_err());
    drop(pending);
    std::fs::rename(&journal, &moved).unwrap();
    assert!(moved.join("intent.json").exists());
}

#[test]
fn native_journal_commit_update_delete_preserves_original_objects_and_private_history() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    assert!(
        scope
            .write(&text("Public admission is still closed"))
            .is_err()
    );
    assert!(!store.path().join(TRANSACTION).exists());
    prepare(&mut scope, "Original").commit().unwrap();
    let original = object(&store.dir, "rule.md");
    let original_index = object(&store.dir, "MEMORY.md");
    let update = prepare(&mut scope, "Updated").commit().unwrap();
    let history = existing_private_directory(&store.dir, HISTORY)
        .unwrap()
        .unwrap();
    let saved = existing_private_directory(&history, &update.id)
        .unwrap()
        .unwrap();
    assert_eq!(object(&saved, "note.before"), original);
    assert_eq!(object(&saved, "index.before"), original_index);
    assert_eq!(scope.read("rule").unwrap().body, "Updated");
    assert_eq!(
        scope.index().unwrap().text,
        "- [rule](rule.md) — useful fact\n"
    );
    let updated = object(&store.dir, "rule.md");
    let deletion = scope
        .prepare("rule", None, None, None)
        .unwrap()
        .commit()
        .unwrap();
    assert!(deletion.deleted);
    let saved = existing_private_directory(&history, &deletion.id)
        .unwrap()
        .unwrap();
    assert_eq!(object(&saved, "note.before"), updated);
    assert!(scope.list().unwrap().memories.is_empty());
    assert_eq!(scope.index().unwrap().text, "");
    assert!(!store.path().join(TRANSACTION).exists());
}

#[test]
fn native_partial_install_reopens_and_recovers_without_replaying_note_effects() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    prepare(&mut scope, "Original").commit().unwrap();
    let mut pending = prepare(&mut scope, "Recovered");
    pending.apply_note().unwrap();
    let installed = object(&store.dir, "rule.md");
    let receipt = pending.journal_identity().unwrap().receipt;
    drop(pending);
    drop(scope);
    drop(store);
    let store = MemoryStore::existing(data.path(), "global")
        .unwrap()
        .unwrap();
    let mut scope = store.claim().unwrap();
    assert!(matches!(
        scope.read("rule"),
        Err(MemoryStorageError::RecoveryRequired)
    ));
    let pending = scope.read_prepared().unwrap().unwrap();
    pending.verify_desired().unwrap();
    pending.verify_slots().unwrap();
    assert_eq!(pending.commit().unwrap(), receipt);
    assert_eq!(object(&store.dir, "rule.md"), installed);
    assert_eq!(scope.read("rule").unwrap().body, "Recovered");
    assert!(scope.read_prepared().unwrap().is_none());
}

#[test]
fn native_failed_acknowledgement_retains_completed_evidence_and_exact_installed_objects() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let pending = prepare(&mut scope, "Installed");
    let receipt = pending.journal_identity().unwrap().receipt;
    assert!(
        pending
            .commit_with_acknowledgement(|_| Err(MemoryStorageError::Busy))
            .is_err()
    );
    let note = object(&store.dir, "rule.md");
    let index = object(&store.dir, "MEMORY.md");
    let pending = scope.read_prepared().unwrap().unwrap();
    assert_eq!(
        optional_bytes(&pending.dir, "completed", NOTE_LIMIT)
            .unwrap()
            .unwrap(),
        b"committed\n"
    );
    let recovered = pending
        .commit_with_acknowledgement(|actual| {
            assert_eq!(actual, &receipt);
            Ok(())
        })
        .unwrap();
    assert_eq!(recovered, receipt);
    assert_eq!(object(&store.dir, "rule.md"), note);
    assert_eq!(object(&store.dir, "MEMORY.md"), index);
}

#[test]
fn native_conflicts_preserve_targets_and_completed_history_collision_evidence() {
    for history_collision in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        let mut pending = prepare(&mut scope, "Proposed");
        if history_collision {
            pending.apply_note().unwrap();
            pending.apply_index().unwrap();
            let history = private_directory(&store.dir, HISTORY).unwrap();
            let collision = native::create_private_directory(
                &history.try_clone().unwrap().into_std_file(),
                &pending.intent.id,
            )
            .unwrap();
            drop(collision);
        } else {
            create_file(&store.dir, "rule.md", text("User edit").as_bytes()).unwrap();
        }
        let identity = object(&store.dir, "rule.md");
        let mut acknowledged = false;
        let result = pending.commit_with_acknowledgement(|_| {
            acknowledged = true;
            Ok(())
        });
        assert!(result.is_err());
        // History collision is currently checked after acknowledgement; receipt remains
        // durable and the journal fences disposal. A target conflict cannot acknowledge.
        assert_eq!(acknowledged, history_collision);
        assert_eq!(object(&store.dir, "rule.md"), identity);
        assert!(store.path().join(TRANSACTION).exists());
    }
}

#[test]
fn native_journal_alias_refuses_without_link_normalization_or_file_effects() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let pending = prepare(&mut scope, "Proposed");
    let staged = store.path().join(TRANSACTION).join("note.after");
    let alias = store.path().join("alias");
    std::fs::hard_link(&staged, &alias).unwrap();
    assert!(pending.commit().is_err());
    assert!(staged.exists());
    assert!(alias.exists());
    assert!(!store.path().join("rule.md").exists());
    assert!(!store.path().join("MEMORY.md").exists());
}
