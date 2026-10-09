//! Real Unix note/index commits, retained preparation and user-edit conflicts.
#![cfg(unix)]
use cyber_core::memory::{MemoryStorageError, MemoryStore};

fn note(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: useful fact\ntype: user\n---\n{body}\n")
}

#[test]
fn writes_updates_and_deletes_keep_index_consistent_and_retain_original_versions() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let first = scope.write(&note("rule", "Original preference")).unwrap();
    assert_eq!(scope.read("rule").unwrap().body, "Original preference");
    assert_eq!(
        scope.index().unwrap().text,
        "- [rule](rule.md) — useful fact\n"
    );
    assert!(!store.path().join(".memory-transaction").exists());
    let update = scope.write(&note("rule", "Updated preference")).unwrap();
    assert_ne!(first.id, update.id);
    assert_eq!(scope.list().unwrap().memories.len(), 1);
    assert!(
        std::fs::read_to_string(
            store
                .path()
                .join(".memory-history")
                .join(&update.id)
                .join("note.before")
        )
        .unwrap()
        .contains("Original preference")
    );
    let deletion = scope.delete("rule").unwrap();
    assert!(deletion.deleted);
    assert!(scope.list().unwrap().memories.is_empty());
    assert_eq!(scope.index().unwrap().text, "");
    assert!(matches!(
        scope.read("rule"),
        Err(MemoryStorageError::NotFound)
    ));
    assert!(
        std::fs::read_to_string(
            store
                .path()
                .join(".memory-history")
                .join(deletion.id)
                .join("note.before")
        )
        .unwrap()
        .contains("Updated preference")
    );
    assert!(scope.recover().unwrap().is_none());
}

#[test]
fn disposed_preparation_fences_reads_and_recovers_once_after_reopening_the_scope() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "prj_test").unwrap();
    let mut scope = store.claim().unwrap();
    drop(
        scope
            .prepare_write(&note("rule", "Durable prepared fact"))
            .unwrap(),
    );
    assert!(matches!(
        scope.index(),
        Err(MemoryStorageError::RecoveryRequired)
    ));
    assert!(!store.path().join("rule.md").exists());
    drop(scope);
    let reopened = MemoryStore::open(data.path(), "prj_test").unwrap();
    let mut scope = reopened.claim().unwrap();
    let recovered = scope.recover().unwrap().unwrap();
    assert_eq!(recovered.name, "rule");
    assert_eq!(scope.read("rule").unwrap().body, "Durable prepared fact");
    assert!(scope.recover().unwrap().is_none());
}

#[test]
fn secret_invalid_and_missing_admissions_create_no_transaction_or_note_artifacts() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    for text in [
        note("rule", "password=private"),
        "malformed".into(),
        note("../escape", "A fact"),
    ] {
        assert!(scope.write(&text).is_err());
    }
    assert!(scope.delete("missing").is_err());
    assert!(!store.path().join(".memory-transaction").exists());
    assert!(!store.path().join(".memory-history").exists());
    assert!(scope.list().unwrap().memories.is_empty());
}

#[test]
fn user_edits_to_target_index_or_other_notes_are_retained_and_fence_recovery() {
    for changed in ["rule.md", "other.md", "MEMORY.md"] {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        scope.write(&note("rule", "Before")).unwrap();
        scope.write(&note("other", "Other before")).unwrap();
        drop(
            scope
                .prepare_write(&note("rule", "Desired update"))
                .unwrap(),
        );
        let edit = if changed == "MEMORY.md" {
            "User edited index\n".into()
        } else {
            note(changed.strip_suffix(".md").unwrap(), "User edit preserved")
        };
        std::fs::write(store.path().join(changed), &edit).unwrap();
        assert!(
            matches!(scope.recover(), Err(MemoryStorageError::Conflict)),
            "{changed}"
        );
        assert_eq!(
            std::fs::read_to_string(store.path().join(changed)).unwrap(),
            edit
        );
        assert!(store.path().join(".memory-transaction").exists());
        if changed != "rule.md" {
            assert!(
                std::fs::read_to_string(store.path().join("rule.md"))
                    .unwrap()
                    .contains("Before")
            );
        }
        assert!(matches!(
            scope.list(),
            Err(MemoryStorageError::RecoveryRequired)
        ));
    }
}

#[test]
fn corrupted_manifest_and_staged_content_remain_fenced_without_visible_note_effects() {
    for filename in ["intent.json", "note.after", "index.after"] {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        drop(scope.prepare_write(&note("rule", "Desired")).unwrap());
        std::fs::write(
            store.path().join(".memory-transaction").join(filename),
            "private corrupted staging bytes",
        )
        .unwrap();
        let error = scope.recover().unwrap_err().to_string();
        assert!(!error.contains("private corrupted staging bytes"));
        assert!(!store.path().join("rule.md").exists());
        assert!(!store.path().join("MEMORY.md").exists());
        assert!(store.path().join(".memory-transaction").exists());
    }
}

#[test]
fn symlinked_staged_data_never_reads_or_writes_an_outside_target() {
    use std::os::unix::fs::symlink;
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    drop(scope.prepare_write(&note("rule", "Desired")).unwrap());
    let outside = data.path().join("outside");
    std::fs::write(&outside, "outside private bytes").unwrap();
    let staged = store.path().join(".memory-transaction/note.after");
    std::fs::remove_file(&staged).unwrap();
    symlink(&outside, &staged).unwrap();
    assert!(scope.recover().is_err());
    assert_eq!(
        std::fs::read_to_string(outside).unwrap(),
        "outside private bytes"
    );
    assert!(!store.path().join("rule.md").exists());
}

#[test]
fn memory_mutation_child() {
    let Some(data) = std::env::var_os("CYBER_MEMORY_MUTATION_CHILD") else {
        return;
    };
    let store = MemoryStore::open(std::path::Path::new(&data), "global").unwrap();
    let mut scope = store.claim().unwrap();
    drop(
        scope
            .prepare_write(&note("rule", "Prepared before process death"))
            .unwrap(),
    );
    std::fs::write(std::path::Path::new(&data).join("prepared"), "ready").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(30));
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn abrupt_owner_death_keeps_synced_preparation_for_explicit_recovery() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut child = Child(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "memory_mutation_child"])
            .env("CYBER_MEMORY_MUTATION_CHILD", data.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !data.path().join("prepared").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "memory preparation worker did not become ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let mut scope = store.claim().unwrap();
    assert!(matches!(
        scope.index(),
        Err(MemoryStorageError::RecoveryRequired)
    ));
    scope.recover().unwrap();
    assert_eq!(
        scope.read("rule").unwrap().body,
        "Prepared before process death"
    );
}

#[test]
fn changed_scope_or_staging_privacy_refuses_before_installation() {
    use std::os::unix::fs::PermissionsExt;
    for staging in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        let pending = scope.prepare_write(&note("rule", "Desired")).unwrap();
        let target = if staging {
            store.path().join(".memory-transaction")
        } else {
            store.path().to_path_buf()
        };
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(pending.commit().is_err());
        assert!(!store.path().join("rule.md").exists());
        assert!(!store.path().join("MEMORY.md").exists());
        assert!(store.path().join(".memory-transaction").exists());
    }
}

#[test]
fn editor_review_refuses_changed_target_or_index_before_journal_creation() {
    for target in ["coding-policy.md", "MEMORY.md"] {
        let data = tempfile::tempdir().unwrap();
        let store = cyber_core::memory::MemoryStore::open(data.path(), "global").unwrap();
        let text = "---\nname: coding-policy\ndescription: Coding policy\ntype: reference\n---\nOriginal fact\n";
        let mut owner = store.claim().unwrap();
        owner.write(text).unwrap();
        let review = owner.review_edit("coding-policy").unwrap();
        std::fs::write(store.path().join(target), "external user edit").unwrap();
        assert!(matches!(
            review.commit(&text.replace("Original fact", "Updated fact")),
            Err(cyber_core::memory::MemoryStorageError::ReviewConflict)
        ));
        assert_eq!(
            std::fs::read_to_string(store.path().join(target)).unwrap(),
            "external user edit"
        );
        assert!(!store.path().join(".memory-transaction").exists());
    }
}

#[test]
fn editor_review_admits_repair_but_refuses_secret_and_identity_changes() {
    let data = tempfile::tempdir().unwrap();
    let store = cyber_core::memory::MemoryStore::open(data.path(), "global").unwrap();
    let mut owner = store.claim().unwrap();
    let text = "---\nname: coding-policy\ndescription: Coding policy\ntype: reference\n---\nFact\n";
    owner.write(text).unwrap();
    std::fs::write(store.path().join("coding-policy.md"), "invalid frontmatter").unwrap();
    let review = owner.review_edit("coding-policy").unwrap();
    assert_eq!(review.original(), Some("invalid frontmatter"));
    review.commit(text).unwrap();
    assert_eq!(owner.read("coding-policy").unwrap().body, "Fact");
    for content in [
        text.replace("Fact", "password = never-store-this"),
        text.replace("coding-policy", "different-name"),
    ] {
        assert!(
            owner
                .review_edit("coding-policy")
                .unwrap()
                .commit(&content)
                .is_err()
        );
        assert!(!store.path().join(".memory-transaction").exists());
        assert_eq!(owner.read("coding-policy").unwrap().body, "Fact");
    }
}

#[test]
fn failed_external_acknowledgement_retains_completed_journal_and_read_fencing() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut owner = store.claim().unwrap();
    let prepared = owner
        .prepare_write(&note("policy", "durable fact"))
        .unwrap();
    let result = prepared.commit_with_acknowledgement(|_| {
        Err(MemoryStorageError::Unsafe("acknowledgement unavailable"))
    });
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(store.path().join(".memory-transaction/completed")).unwrap(),
        b"committed\n"
    );
    assert!(
        std::fs::read_to_string(store.path().join("policy.md"))
            .unwrap()
            .contains("durable fact")
    );
    assert!(matches!(
        owner.list(),
        Err(MemoryStorageError::RecoveryRequired)
    ));
    let receipt = owner.recover().unwrap().unwrap();
    assert_eq!(receipt.name, "policy");
    assert_eq!(owner.read("policy").unwrap().body, "durable fact");
}

#[test]
fn detached_edit_review_survives_scope_release_and_permits_explicit_write_and_delete() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let original = note("rule", "Original fact");
    let review = {
        let mut scope = store.claim().unwrap();
        scope.write(&original).unwrap();
        scope.inspect_edit("rule").unwrap()
    };
    assert_eq!(
        review.original,
        Some(std::fs::read_to_string(store.path().join("rule.md")).unwrap())
    );
    assert_eq!(review.fingerprint.len(), 64);
    assert!(!store.path().join(".memory-transaction").exists());
    let mut scope = store.claim().unwrap();
    scope
        .prepare_write_reviewed(&note("rule", "Updated fact"), &review.fingerprint)
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(scope.read("rule").unwrap().body, "Updated fact");
    assert!(matches!(
        scope.prepare_delete_reviewed("rule", &review.fingerprint),
        Err(MemoryStorageError::ReviewConflict)
    ));
    let fresh = scope.inspect_edit("rule").unwrap();
    scope
        .prepare_delete_reviewed("rule", &fresh.fingerprint)
        .unwrap()
        .commit()
        .unwrap();
    assert!(scope.list().unwrap().memories.is_empty());
}

#[test]
fn detached_edit_review_preserves_changed_note_index_and_catalog_before_preparation() {
    for target in ["rule.md", "MEMORY.md", "other.md"] {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        scope.write(&note("rule", "Original fact")).unwrap();
        scope.write(&note("other", "Other fact")).unwrap();
        let review = scope.inspect_edit("rule").unwrap();
        let changed = if target == "MEMORY.md" {
            "External index edit".into()
        } else {
            note(target.trim_end_matches(".md"), "External fact")
        };
        std::fs::write(store.path().join(target), &changed).unwrap();
        assert!(matches!(
            scope.prepare_write_reviewed(&note("rule", "Draft"), &review.fingerprint),
            Err(MemoryStorageError::ReviewConflict)
        ));
        assert!(matches!(
            scope.prepare_delete_reviewed("rule", &review.fingerprint),
            Err(MemoryStorageError::ReviewConflict)
        ));
        assert_eq!(
            std::fs::read_to_string(store.path().join(target)).unwrap(),
            changed
        );
        assert!(!store.path().join(".memory-transaction").exists());
    }
}

#[test]
fn detached_edit_review_rejects_replaced_files_and_foreign_scope_or_note() {
    use std::os::unix::fs::PermissionsExt;
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let original = note("rule", "Original fact");
    scope.write(&original).unwrap();
    scope.write(&note("other", "Other fact")).unwrap();
    let review = scope.inspect_edit("rule").unwrap();
    assert!(matches!(
        scope.prepare_delete_reviewed("other", &review.fingerprint),
        Err(MemoryStorageError::ReviewConflict)
    ));
    let foreign = MemoryStore::open(data.path(), "prj_other").unwrap();
    let mut foreign_scope = foreign.claim().unwrap();
    foreign_scope.write(&original).unwrap();
    assert!(matches!(
        foreign_scope.prepare_delete_reviewed("rule", &review.fingerprint),
        Err(MemoryStorageError::ReviewConflict)
    ));
    for target in ["rule.md", "MEMORY.md"] {
        let fresh = scope.inspect_edit("rule").unwrap();
        let path = store.path().join(target);
        let bytes = std::fs::read(&path).unwrap();
        let replacement = store.path().join("replacement");
        std::fs::write(&replacement, bytes).unwrap();
        std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::rename(replacement, path).unwrap();
        assert!(matches!(
            scope.prepare_delete_reviewed("rule", &fresh.fingerprint),
            Err(MemoryStorageError::ReviewConflict)
        ));
        assert!(!store.path().join(".memory-transaction").exists());
    }
}

#[test]
fn detached_edit_review_of_missing_note_is_read_only_and_bounds_write_admission() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    let entries_before: Vec<_> = std::fs::read_dir(store.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    let review = scope.inspect_edit("rule").unwrap();
    assert!(review.original.is_none());
    let entries_after: Vec<_> = std::fs::read_dir(store.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries_before, entries_after);
    assert!(matches!(
        scope.prepare_write_reviewed(&note("rule", "Fact"), "invalid"),
        Err(MemoryStorageError::ReviewConflict)
    ));
    assert!(
        scope
            .prepare_write_reviewed(&note("rule", "password=private"), &review.fingerprint)
            .is_err()
    );
    assert!(matches!(
        scope.prepare_write_reviewed(&"x".repeat(1_048_577), &review.fingerprint),
        Err(MemoryStorageError::TooLarge)
    ));
    assert!(!store.path().join(".memory-transaction").exists());
    scope
        .prepare_write_reviewed(&note("rule", "Fact"), &review.fingerprint)
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(scope.read("rule").unwrap().body, "Fact");
}
