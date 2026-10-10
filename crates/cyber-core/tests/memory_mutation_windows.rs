//! Public native core storage; application/tool/CLI admission remains independent.
#![cfg(windows)]
use cyber_core::memory::{MemoryStorageError, MemoryStore};
fn note(body: &str) -> String {
    format!("---\nname: policy\ndescription: Policy\ntype: user\n---\n{body}\n")
}
#[test]
fn native_public_create_conditional_update_delete_and_scope_isolation() {
    let data = tempfile::tempdir().unwrap();
    let global = MemoryStore::open(data.path(), "global").unwrap();
    let project = MemoryStore::open(data.path(), "prj_test").unwrap();
    let mut scope = global.claim().unwrap();
    let mut other = project.claim().unwrap();
    scope.write(&note("Original")).unwrap();
    other.write(&note("Project preference")).unwrap();
    assert!(matches!(global.claim(), Err(MemoryStorageError::Busy)));
    let review = scope.inspect_edit("policy").unwrap();
    let update = scope
        .prepare_write_reviewed(&note("Updated"), &review.fingerprint)
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(scope.read("policy").unwrap().body, "Updated");
    assert_eq!(other.read("policy").unwrap().body, "Project preference");
    assert!(
        std::fs::read_to_string(
            global
                .path()
                .join(".memory-history")
                .join(update.id)
                .join("note.before")
        )
        .unwrap()
        .contains("Original")
    );
    let review = scope.inspect_edit("policy").unwrap();
    let deleted = scope
        .prepare_delete_reviewed("policy", &review.fingerprint)
        .unwrap()
        .commit()
        .unwrap();
    assert!(deleted.deleted);
    assert!(scope.list().unwrap().memories.is_empty());
    assert_eq!(scope.index().unwrap().text, "");
    assert_eq!(other.read("policy").unwrap().body, "Project preference");
    assert!(scope.recover().unwrap().is_none());
}
#[test]
fn native_public_stale_edit_refusal_and_reviewed_reopening_preserve_user_content() {
    let data = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path(), "global").unwrap();
    let mut scope = store.claim().unwrap();
    scope.write(&note("Original")).unwrap();
    let stale = scope.inspect_edit("policy").unwrap();
    std::fs::write(store.path().join("policy.md"), note("User edit")).unwrap();
    assert!(matches!(
        scope.prepare_write_reviewed(&note("Rejected"), &stale.fingerprint),
        Err(MemoryStorageError::ReviewConflict)
    ));
    assert!(!store.path().join(".memory-transaction").exists());
    assert_eq!(scope.read("policy").unwrap().body, "User edit");
    let fresh = scope.inspect_edit("policy").unwrap();
    let pending = scope
        .prepare_write_reviewed(&note("Recovered"), &fresh.fingerprint)
        .unwrap();
    let identity = pending.journal_identity().unwrap();
    drop(pending);
    let review = scope.inspect_recovery().unwrap().unwrap();
    assert_eq!(review.journal, identity);
    drop(scope);
    drop(store);
    let store = MemoryStore::existing(data.path(), "global")
        .unwrap()
        .unwrap();
    let mut scope = store.claim().unwrap();
    assert_eq!(
        scope.recover_reviewed(&review.fingerprint).unwrap(),
        identity.receipt
    );
    assert_eq!(scope.read("policy").unwrap().body, "Recovered");
    assert!(scope.inspect_recovery().unwrap().is_none());
    assert!(
        std::fs::read_to_string(
            store
                .path()
                .join(".memory-history")
                .join(identity.receipt.id)
                .join("note.before")
        )
        .unwrap()
        .contains("User edit")
    );
}
