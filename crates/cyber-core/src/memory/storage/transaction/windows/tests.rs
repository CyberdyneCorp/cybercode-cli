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

fn assert_frozen(dir: &Dir, path: &std::path::Path, name: &str) {
    let bytes = optional_bytes(dir, name, INTENT_LIMIT).unwrap().unwrap();
    assert!(
        native::open_private_file(
            &dir.try_clone().unwrap().into_std_file(),
            name,
            native::Access::DataWrite
        )
        .is_err()
    );
    assert!(std::fs::rename(path, path.with_extension("blocked")).is_err());
    assert!(std::fs::remove_file(path).is_err());
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn native_acknowledgement_owns_installed_catalog_original_and_marker_objects() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    prepare(&mut scope, "Before").commit().unwrap();
    let other =
        MemoryDocument::for_write(&text("Other").replace("name: rule", "name: other")).unwrap();
    let rendered = other.render_for_write().unwrap();
    scope
        .prepare("other", Some((other, rendered)), None, None)
        .unwrap()
        .commit()
        .unwrap();
    let pending = prepare(&mut scope, "Proposed");
    let mut acknowledged = false;
    let receipt = pending
        .commit_with_acknowledgement(|_| {
            for name in ["rule.md", "MEMORY.md", "other.md"] {
                assert_frozen(&store.dir, &store.path().join(name), name);
            }
            let journal = existing_private_directory(&store.dir, TRANSACTION)
                .unwrap()
                .unwrap();
            for name in ["note.before", "index.before", "completed"] {
                assert_frozen(&journal, &store.path().join(TRANSACTION).join(name), name);
            }
            acknowledged = true;
            Ok(())
        })
        .unwrap();
    assert!(acknowledged);
    assert!(store.path().join(HISTORY).join(receipt.id).exists());
    assert!(!store.path().join(TRANSACTION).exists());
    assert!(
        native::open_private_file(
            &store.dir.try_clone().unwrap().into_std_file(),
            "rule.md",
            native::Access::DataWrite
        )
        .is_ok()
    );
}

#[test]
fn native_preexisting_installed_writer_fences_acknowledgement_and_releases_for_recovery() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let mut pending = prepare(&mut scope, "Proposed");
    pending.apply_note().unwrap();
    pending.apply_index().unwrap();
    let installed = object(&store.dir, "rule.md");
    let writer = native::open_private_file(
        &store.dir.try_clone().unwrap().into_std_file(),
        "rule.md",
        native::Access::DataWrite,
    )
    .unwrap();
    let mut acknowledged = false;
    assert!(
        pending
            .commit_with_acknowledgement(|_| {
                acknowledged = true;
                Ok(())
            })
            .is_err()
    );
    assert!(!acknowledged);
    assert!(!store.path().join(TRANSACTION).join("completed").exists());
    assert_eq!(object(&store.dir, "rule.md"), installed);
    drop(writer);
    scope.read_prepared().unwrap().unwrap().commit().unwrap();
    assert_eq!(object(&store.dir, "rule.md"), installed);
}

#[test]
fn native_failed_acknowledgement_releases_terminal_guards_without_rolling_back_files() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let pending = prepare(&mut scope, "Proposed");
    assert!(
        pending
            .commit_with_acknowledgement(|_| {
                assert_frozen(&store.dir, &store.path().join("rule.md"), "rule.md");
                Err(MemoryStorageError::Busy)
            })
            .is_err()
    );
    let installed = object(&store.dir, "rule.md");
    let bytes = std::fs::read(store.path().join("rule.md")).unwrap();
    let mut writer = native::open_private_file(
        &store.dir.try_clone().unwrap().into_std_file(),
        "rule.md",
        native::Access::DataWrite,
    )
    .unwrap();
    writer.write_all(&bytes).unwrap();
    drop(writer);
    scope.read_prepared().unwrap().unwrap().commit().unwrap();
    assert_eq!(object(&store.dir, "rule.md"), installed);
}

#[test]
fn native_new_catalog_name_during_acknowledgement_is_preserved_and_fences_disposal() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let pending = prepare(&mut scope, "Proposed");
    let extra = text("User added").replace("name: rule", "name: extra");
    let mut acknowledged = false;
    assert!(matches!(
        pending.commit_with_acknowledgement(|_| {
            create_file(&store.dir, "extra.md", extra.as_bytes())?;
            acknowledged = true;
            Ok(())
        }),
        Err(MemoryStorageError::Conflict)
    ));
    assert!(acknowledged);
    assert_eq!(
        std::fs::read(store.path().join("extra.md")).unwrap(),
        extra.as_bytes()
    );
    assert!(store.path().join(TRANSACTION).join("completed").exists());
    assert!(scope.read_prepared().unwrap().unwrap().commit().is_err());
    assert_eq!(
        std::fs::read(store.path().join("extra.md")).unwrap(),
        extra.as_bytes()
    );
}

#[test]
fn native_recovery_refuses_identical_byte_replacements_of_every_recorded_file() {
    for target in [
        "intent.json",
        "note.after",
        "index.after",
        "rule.md",
        "MEMORY.md",
        "other.md",
    ] {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        prepare(&mut scope, "Before").commit().unwrap();
        let other_text = text("Other").replace("name: rule", "name: other");
        let other = MemoryDocument::for_write(&other_text).unwrap();
        let rendered = other.render_for_write().unwrap();
        scope
            .prepare("other", Some((other, rendered)), None, None)
            .unwrap()
            .commit()
            .unwrap();
        let pending = prepare(&mut scope, "Proposed");
        let dir = if target.ends_with(".md") {
            store.dir.try_clone().unwrap()
        } else {
            existing_private_directory(&store.dir, TRANSACTION)
                .unwrap()
                .unwrap()
        };
        let bytes = optional_bytes(&dir, target, INTENT_LIMIT).unwrap().unwrap();
        let old = object(&dir, target);
        drop(pending);
        let base = if target.ends_with(".md") {
            store.path().to_owned()
        } else {
            store.path().join(TRANSACTION)
        };
        let original = base.join("retained-original");
        std::fs::rename(base.join(target), &original).unwrap();
        create_file(&dir, target, &bytes).unwrap();
        let replacement = object(&dir, target);
        assert_ne!(replacement, old);
        drop(dir);
        drop(scope);
        drop(store);
        let store = MemoryStore::existing(data.path(), "global")
            .unwrap()
            .unwrap();
        let mut scope = store.claim().unwrap();
        assert!(scope.read_prepared().is_err(), "{target}");
        assert_eq!(std::fs::read(base.join(target)).unwrap(), bytes);
        assert_eq!(std::fs::read(&original).unwrap(), bytes);
        assert!(!store.path().join(TRANSACTION).join("completed").exists());
        std::fs::remove_file(base.join(target)).unwrap();
        std::fs::rename(original, base.join(target)).unwrap();
        scope.read_prepared().unwrap().unwrap().commit().unwrap();
        assert_eq!(scope.read("rule").unwrap().body, "Proposed");
    }
}

#[test]
fn native_recovery_refuses_new_directory_context_even_with_original_file_objects() {
    for target in ["data", "root", "scope", "journal"] {
        let container = tempfile::tempdir().unwrap();
        let data = container.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let store = MemoryStore::open(&data, "global").unwrap();
        let mut scope = store.claim().unwrap();
        prepare(&mut scope, "Before").commit().unwrap();
        drop(prepare(&mut scope, "Proposed"));
        let path = store.path().to_owned();
        let source = match target {
            "data" => data.clone(),
            "root" => data.join("memory"),
            "scope" => path.clone(),
            _ => path.join(TRANSACTION),
        };
        drop(scope);
        drop(store);
        let retained = container.path().join("retained");
        std::fs::rename(&source, &retained).unwrap();
        if target == "data" {
            std::fs::create_dir(&data).unwrap();
        }
        let store = MemoryStore::open(&data, "global").unwrap();
        let old_scope = match target {
            "data" => retained.join("memory").join("global"),
            "root" => retained.join("global"),
            "scope" => retained.clone(),
            _ => path.clone(),
        };
        if target == "journal" {
            drop(
                native::create_private_directory(
                    &store.dir.try_clone().unwrap().into_std_file(),
                    TRANSACTION,
                )
                .unwrap(),
            );
            for entry in std::fs::read_dir(&retained).unwrap() {
                let entry = entry.unwrap();
                std::fs::rename(entry.path(), path.join(TRANSACTION).join(entry.file_name()))
                    .unwrap();
            }
        } else {
            for name in ["rule.md", "MEMORY.md", TRANSACTION] {
                std::fs::rename(old_scope.join(name), store.path().join(name)).unwrap();
            }
        }
        let mut scope = store.claim().unwrap();
        let note = object(&store.dir, "rule.md");
        assert!(scope.read_prepared().is_err(), "{target}");
        assert_eq!(object(&store.dir, "rule.md"), note);
        assert!(!store.path().join(TRANSACTION).join("completed").exists());
    }
}

#[test]
fn native_legacy_or_incomplete_object_plan_refuses_without_file_effects() {
    for legacy in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        drop(prepare(&mut scope, "Proposed"));
        let dir = existing_private_directory(&store.dir, TRANSACTION)
            .unwrap()
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(
            &optional_bytes(&dir, "intent.json", INTENT_LIMIT)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        if legacy {
            value["version"] = 1.into();
            value.as_object_mut().unwrap().remove("objects");
        } else {
            value["objects"]["intent"] = serde_json::Value::Null;
        }
        let mut file = native::open_private_file(
            &dir.try_clone().unwrap().into_std_file(),
            "intent.json",
            native::Access::DataWrite,
        )
        .unwrap();
        file.set_len(0).unwrap();
        file.write_all(&serde_json::to_vec(&value).unwrap())
            .unwrap();
        drop(file);
        assert!(scope.read_prepared().is_err());
        assert!(!store.path().join("rule.md").exists());
        assert!(!store.path().join("MEMORY.md").exists());
    }
}

struct Owner(std::process::Child);
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn native_identity_owner_child() {
    let Some(data) = std::env::var_os("CYBER_MEMORY_IDENTITY_OWNER_DATA") else {
        return;
    };
    let store = MemoryStore::open(std::path::Path::new(&data), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let mut pending = prepare(&mut scope, "After owner death");
    pending.apply_note().unwrap();
    std::fs::write(std::path::Path::new(&data).join("prepared-ready"), b"ready").unwrap();
    loop {
        std::thread::park();
    }
}
#[test]
fn native_killed_owner_recovery_preserves_recorded_installed_identity() {
    let data = tempfile::tempdir().unwrap();
    let mut owner = Owner(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "memory::storage::transaction::windows::tests::native_identity_owner_child",
                "--nocapture",
            ])
            .env("CYBER_MEMORY_IDENTITY_OWNER_DATA", data.path())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !data.path().join("prepared-ready").exists() {
        assert!(
            owner.0.try_wait().unwrap().is_none(),
            "owner exited before preparation"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "owner readiness timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    let store = MemoryStore::existing(data.path(), "global")
        .unwrap()
        .unwrap();
    let installed = object(&store.dir, "rule.md");
    let mut scope = store.claim().unwrap();
    scope.read_prepared().unwrap().unwrap().commit().unwrap();
    assert_eq!(object(&store.dir, "rule.md"), installed);
    assert_eq!(scope.read("rule").unwrap().body, "After owner death");
    assert!(!store.path().join(TRANSACTION).exists());
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
