//! Project command templates that run shell commands need an exact digest approval.
use cyber_core::commands::template_digest;
use cyber_core::trust::TrustStore;

#[test]
fn template_approval_is_exact_checkout_scoped_and_revocable() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = tmp.path().join("repo");
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let store = TrustStore::new(tmp.path().join("trust.json"));
    let digest = template_digest("command", "status", "!`git status`");
    assert!(!store.is_template_approved(&checkout, &digest).unwrap());
    store.approve_template(&checkout, &digest).unwrap();
    assert!(store.is_template_approved(&checkout, &digest).unwrap());
    assert!(!store.is_template_approved(&other, &digest).unwrap());
    let changed = template_digest("command", "status", "!`git status -s`");
    assert!(!store.is_template_approved(&checkout, &changed).unwrap());
    assert!(
        store
            .is_template_approved(&checkout, "not-a-digest")
            .is_err()
    );
    assert!(store.revoke_template(&checkout, &digest).unwrap());
    assert!(!store.is_template_approved(&checkout, &digest).unwrap());
    store.approve_template(&checkout, &digest).unwrap();
    assert!(
        store
            .revoke(&std::fs::canonicalize(&checkout).unwrap())
            .unwrap()
    );
    assert!(!store.is_template_approved(&checkout, &digest).unwrap());
}
