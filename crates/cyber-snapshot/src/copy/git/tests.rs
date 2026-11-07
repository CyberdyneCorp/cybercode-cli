//! Real Git image preparation followed by synchronous native journal execution.
use super::*;
use cyber_server::runtime::{SnapshotRestoreRequest, Snapshots};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

struct Repo {
    _temp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    snapshots: Arc<crate::GitSnapshots>,
}
impl Repo {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("work");
        let data = base.join("data");
        std::fs::create_dir(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        let snapshots = crate::GitSnapshots::new(data.clone(), Arc::new(|_| serde_json::json!({})));
        Self {
            _temp: temp,
            root,
            data,
            snapshots,
        }
    }
    async fn track(&self, directory: &Path) -> String {
        self.snapshots
            .track(directory.to_str().unwrap())
            .await
            .unwrap()
            .unwrap()
            .tree
    }
    fn store(&self, directory: &Path) -> Store {
        Store::open(
            &self.data,
            directory.to_str().unwrap(),
            super::super::Options {
                max_file: 65536,
                max_total: 8 * 1024 * 1024,
                ignore: vec![],
                private_roots: vec![],
            },
        )
        .unwrap()
    }
    async fn prepare(
        &self,
        directory: &Path,
        target: &str,
        recorded: &str,
        expected: &str,
    ) -> PreparedRestore {
        self.snapshots
            .shadow(directory.to_str().unwrap())
            .unwrap()
            .prepare_restore(
                &SnapshotRestoreRequest {
                    directory: directory.to_str().unwrap().into(),
                    target: target.into(),
                    recorded: recorded.into(),
                    expected: expected.into(),
                    paths: None,
                },
                65536,
                8 * 1024 * 1024,
            )
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn native_git_import_restores_files_modes_creations_deletions_and_retains_original_ids() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), b"before").unwrap();
    std::fs::set_permissions(
        repo.root.join("file"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::create_dir(repo.root.join("deleted")).unwrap();
    std::fs::write(repo.root.join("deleted/file"), b"restore parent").unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo.root)
            .args(["add", "file"])
            .status()
            .unwrap()
            .success()
    );
    let user_index = std::fs::read(repo.root.join(".git/index")).unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"after").unwrap();
    std::fs::set_permissions(
        repo.root.join("file"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    std::fs::remove_file(repo.root.join("deleted/file")).unwrap();
    std::fs::remove_dir(repo.root.join("deleted")).unwrap();
    std::fs::write(repo.root.join("added"), b"added").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    // Native execution must not ask Git for object bytes or use its index again.
    std::fs::rename(&shadow.git_dir, shadow.git_dir.with_extension("retained")).unwrap();
    let store = repo.store(&repo.root);
    assert_eq!(
        store.restore_git_worker(prepared, None).unwrap(),
        ["added", "deleted/file", "file"]
    );
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"before");
    assert_ne!(
        std::fs::metadata(repo.root.join("file"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    assert_eq!(
        std::fs::read(repo.root.join("deleted/file")).unwrap(),
        b"restore parent"
    );
    assert!(!repo.root.join("added").exists());
    assert_eq!(
        std::fs::read(repo.root.join(".git/index")).unwrap(),
        user_index
    );
    let journals = store.recovery().unwrap();
    assert_eq!(journals.len(), 1);
    let (dir, journal) = store.journal(&journals[0].id).unwrap();
    assert_eq!(journal.version, 5);
    let context = journal.context.as_ref().unwrap();
    assert_eq!(
        (&context.target, &context.recorded, &context.expected),
        (&target, &recorded, &recorded)
    );
    assert_eq!(context.git.as_ref().unwrap().worktree, repo.root);
    assert!(journal.completed);
    assert!(journal.ownership.is_some());
    assert_eq!(storage::read(&dir, "stage_2", 65536).unwrap(), b"after");
    assert!(
        !repo
            .snapshots
            .owned_restore_available(repo.root.to_str().unwrap())
    );
    let mut legacy_git = journal.clone();
    legacy_git.version = 4;
    legacy_git.backend = None;
    Store::record(&dir, &legacy_git).unwrap();
    assert!(store.journal(&journals[0].id).is_ok());
    let mut downgraded = journal.clone();
    downgraded.version = 3;
    Store::record(&dir, &downgraded).unwrap();
    assert!(store.journal(&journals[0].id).is_err());
    let mut missing_context = journal.clone();
    missing_context.context = None;
    Store::record(&dir, &missing_context).unwrap();
    assert!(store.journal(&journals[0].id).is_err());
    let mut missing_ownership = journal;
    missing_ownership.ownership = None;
    Store::record(&dir, &missing_ownership).unwrap();
    assert!(store.journal(&journals[0].id).is_err());
}

#[tokio::test]
async fn native_git_import_merges_owner_edits_and_rejects_stale_handoffs() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), b"one\ntwo\nthree\n").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"ONE\ntwo\nthree\n").unwrap();
    let recorded = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"ONE\ntwo\nTHREE\n").unwrap();
    let expected = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &expected)
        .await;
    let store = repo.store(&repo.root);
    store.restore_git_worker(prepared, None).unwrap();
    assert_eq!(
        std::fs::read(repo.root.join("file")).unwrap(),
        b"one\ntwo\nTHREE\n"
    );
    drop(store);
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &expected)
        .await;
    std::fs::write(repo.root.join("file"), b"later owner").unwrap();
    let store = repo.store(&repo.root);
    let before = store.recovery().unwrap().len();
    assert!(matches!(
        store.restore_git_worker(prepared, None),
        Err(RestoreError::Stale)
    ));
    assert_eq!(store.recovery().unwrap().len(), before);
    assert_eq!(
        std::fs::read(repo.root.join("file")).unwrap(),
        b"later owner"
    );
}

#[tokio::test]
async fn native_git_import_keeps_subdirectory_and_historical_exclusion_boundaries() {
    let repo = Repo::new();
    let directory = repo.root.join("[ab]");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("file"), b"before").unwrap();
    std::fs::write(directory.join("ignored"), b"owner ignored before").unwrap();
    std::fs::write(directory.join(".gitignore"), b"ignored\n").unwrap();
    let target = repo.track(&directory).await;
    std::fs::write(directory.join("file"), b"after").unwrap();
    std::fs::write(directory.join("ignored"), b"later ignored owner").unwrap();
    std::fs::write(repo.root.join("sibling"), b"sibling owner").unwrap();
    let recorded = repo.track(&directory).await;
    let prepared = repo
        .prepare(&directory, &target, &recorded, &recorded)
        .await;
    let store = repo.store(&directory);
    assert_eq!(store.restore_git_worker(prepared, None).unwrap(), ["file"]);
    assert_eq!(std::fs::read(directory.join("file")).unwrap(), b"before");
    assert_eq!(
        std::fs::read(directory.join("ignored")).unwrap(),
        b"later ignored owner"
    );
    assert_eq!(
        std::fs::read(repo.root.join("sibling")).unwrap(),
        b"sibling owner"
    );
    let recovery = store.recovery().unwrap();
    let (_, journal) = store.journal(&recovery[0].id).unwrap();
    let context = journal.context.unwrap();
    assert_eq!(context.git.as_ref().unwrap().prefix, "[ab]");
    assert!(
        context
            .exclusions
            .iter()
            .any(|path| path.contains("ignored"))
    );
    let mut invalid = context.clone();
    invalid.git.as_mut().unwrap().prefix = "../foreign".into();
    assert!(invalid.validate(&directory, 4).is_err());
    invalid = context;
    invalid.target = format!("copy:{}", "0".repeat(64));
    assert!(invalid.validate(&directory, 4).is_err());
}

#[tokio::test]
async fn native_git_import_refuses_escaping_links_before_any_journal_or_source_effect() {
    let repo = Repo::new();
    std::os::unix::fs::symlink("../destination", repo.root.join("link")).unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::remove_file(repo.root.join("link")).unwrap();
    std::fs::write(repo.root.join("link"), b"current owner").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let store = repo.store(&repo.root);
    assert!(
        matches!(store.restore_git_worker(prepared, None), Err(RestoreError::Failed(error)) if error.contains("outside the worktree"))
    );
    assert!(store.recovery().unwrap().is_empty());
    assert_eq!(
        std::fs::read(repo.root.join("link")).unwrap(),
        b"current owner"
    );
}

#[test]
fn legacy_copy_context_keeps_its_encoding_and_cannot_be_reinterpreted_as_git() {
    let value = serde_json::json!({"target": format!("copy:{}", "a".repeat(64)),
        "recorded": format!("copy:{}", "b".repeat(64)), "expected": format!("copy:{}", "c".repeat(64)), "exclusions": []});
    let context: RestoreContext = serde_json::from_value(value.clone()).unwrap();
    context.validate(Path::new("/fixture"), 3).unwrap();
    assert_eq!(serde_json::to_value(&context).unwrap(), value);
    let mut invalid = context;
    invalid.git = Some(GitContext {
        worktree: "/fixture".into(),
        prefix: String::new(),
    });
    assert!(invalid.validate(Path::new("/fixture"), 4).is_err());
}

#[tokio::test]
async fn imported_git_preimages_support_native_keep_and_rollback_with_retained_git_context() {
    use cyber_server::runtime::{
        SnapshotBinding, SnapshotOrigin, SnapshotRecoveryAction as Action,
        SnapshotRecoveryConfirmation,
    };
    for action in [Action::Keep, Action::Rollback] {
        let repo = Repo::new();
        std::fs::write(repo.root.join("file"), b"target before").unwrap();
        let target = repo.track(&repo.root).await;
        std::fs::write(repo.root.join("file"), b"recorded after").unwrap();
        let recorded = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        let store = repo.store(&repo.root);
        store.restore_git_worker(prepared, None).unwrap();
        let id = store.recovery().unwrap()[0].id.clone();
        let (dir, mut journal) = store.journal(&id).unwrap();
        // Synthetic fixture origin exercises the native backend only. It grants no
        // runtime/Store acknowledgement authority or worker lifetime proof.
        journal.origin = Some(SnapshotOrigin {
            version: 1,
            session_id: "ses_fixture".into(),
            session_created_id: "evt_fixture".into(),
            activity_id: "op_fixture".into(),
            worktree_id: None,
            binding: SnapshotBinding {
                directory: store.root.clone(),
                root_identity: store.root_id.clone(),
                namespace_identity: storage::dir_identity(&store.journals).unwrap(),
                journal_id: id.clone(),
                journal_identity: storage::dir_identity(&dir).unwrap(),
                lock_identity: Some(store.lock_identity().unwrap()),
            },
        });
        Store::record(&dir, &journal).unwrap();
        std::fs::write(repo.root.join("owner-file"), b"later owner").unwrap();
        let review = store.recovery_review(&id, action).unwrap();
        assert!(review.conflicts.is_empty(), "{:?}", review.conflicts);
        let confirmation = SnapshotRecoveryConfirmation {
            journal_id: id,
            action,
            confirmation: review.confirmation.unwrap(),
            request_id: "git-native-fixture".into(),
        };
        let receipt = store
            .resolve_recovery(&confirmation, &|| Ok(()), None)
            .unwrap();
        let (_, resolution) = store.journal(&receipt.resolution_id).unwrap();
        assert_eq!(resolution.version, 5);
        assert_eq!(resolution.context.as_ref().unwrap().target, target);
        assert_eq!(
            std::fs::read(repo.root.join("file")).unwrap(),
            if action == Action::Keep {
                b"target before".as_slice()
            } else {
                b"recorded after".as_slice()
            }
        );
        assert_eq!(
            std::fs::read(repo.root.join("owner-file")).unwrap(),
            b"later owner"
        );
        std::fs::write(repo.root.join("file"), b"owner after resolution").unwrap();
        assert_eq!(
            store
                .resolve_recovery(&confirmation, &|| Ok(()), None)
                .unwrap(),
            receipt
        );
        assert_eq!(
            std::fs::read(repo.root.join("file")).unwrap(),
            b"owner after resolution"
        );
    }
}

#[tokio::test]
async fn git_image_handoff_enforces_byte_bounds_without_source_effects() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), b"bounded image").unwrap();
    let snapshot = repo.track(&repo.root).await;
    let request = SnapshotRestoreRequest {
        directory: repo.root.to_str().unwrap().into(),
        target: snapshot.clone(),
        recorded: snapshot.clone(),
        expected: snapshot,
        paths: None,
    };
    let shadow = repo.snapshots.shadow(&request.directory).unwrap();
    assert!(shadow.prepare_restore(&request, 65536, 1).await.is_err());
    assert_eq!(
        std::fs::read(repo.root.join("file")).unwrap(),
        b"bounded image"
    );
    assert!(repo.store(&repo.root).recovery().unwrap().is_empty());
}

#[tokio::test]
async fn native_git_import_rechecks_the_handed_off_blob_identity_before_reading_source() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), b"before").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"after").unwrap();
    let recorded = repo.track(&repo.root).await;
    let mut prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    prepared.target.images.get_mut("file").unwrap().bytes = b"substituted".to_vec();
    let store = repo.store(&repo.root);
    assert!(
        matches!(store.restore_git_worker(prepared, None), Err(RestoreError::Failed(error)) if error.contains("object identity"))
    );
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
    assert!(store.recovery().unwrap().is_empty());
}

#[tokio::test]
async fn git_handoff_limits_changes_to_exact_reviewed_paths_inside_the_location() {
    let repo = Repo::new();
    let directory = repo.root.join("[ab]");
    std::fs::create_dir(&directory).unwrap();
    for name in ["first", "second"] {
        std::fs::write(directory.join(name), b"before").unwrap();
    }
    let target = repo.track(&directory).await;
    for name in ["first", "second"] {
        std::fs::write(directory.join(name), b"after").unwrap();
    }
    let recorded = repo.track(&directory).await;
    let mut request = SnapshotRestoreRequest {
        directory: directory.to_str().unwrap().into(),
        target,
        recorded: recorded.clone(),
        expected: recorded,
        paths: Some(vec!["[ab]/first".into()]),
    };
    let shadow = repo.snapshots.shadow(&request.directory).unwrap();
    let prepared = shadow
        .prepare_restore(&request, 65536, 8 * 1024 * 1024)
        .await
        .unwrap();
    let store = repo.store(&directory);
    assert_eq!(store.restore_git_worker(prepared, None).unwrap(), ["first"]);
    assert_eq!(std::fs::read(directory.join("first")).unwrap(), b"before");
    assert_eq!(std::fs::read(directory.join("second")).unwrap(), b"after");
    request.paths = Some(vec!["[absolutely]/first".into()]);
    assert!(
        shadow
            .prepare_restore(&request, 65536, 8 * 1024 * 1024)
            .await
            .is_err()
    );
}

/// Synthetic native fixture only; no runtime acknowledgement authority is granted.
fn own_native_journal(store: &Store, id: &str) {
    use cyber_server::runtime::{SnapshotBinding, SnapshotOrigin};
    let (dir, mut journal) = store.journal(id).unwrap();
    journal.origin = Some(SnapshotOrigin {
        version: 1,
        session_id: "ses_fixture".into(),
        session_created_id: "evt_fixture".into(),
        activity_id: "op_fixture".into(),
        worktree_id: None,
        binding: SnapshotBinding {
            directory: store.root.clone(),
            root_identity: store.root_id.clone(),
            namespace_identity: storage::dir_identity(&store.journals).unwrap(),
            journal_id: id.into(),
            journal_identity: storage::dir_identity(&dir).unwrap(),
            lock_identity: Some(store.lock_identity().unwrap()),
        },
    });
    Store::record(&dir, &journal).unwrap();
}
fn put_node(root: &Path, node: &str) {
    let path = root.join("entry");
    if std::fs::symlink_metadata(&path).is_ok() {
        std::fs::remove_file(&path).unwrap();
    }
    match node {
        "absent" => {}
        "file" => std::fs::write(path, b"regular owner bytes").unwrap(),
        target => std::os::unix::fs::symlink(target, path).unwrap(),
    }
}
fn assert_node(root: &Path, node: &str) {
    let path = root.join("entry");
    match node {
        "absent" => assert!(std::fs::symlink_metadata(path).is_err()),
        "file" => assert_eq!(std::fs::read(path).unwrap(), b"regular owner bytes"),
        target => assert_eq!(std::fs::read_link(path).unwrap(), Path::new(target)),
    }
}

#[tokio::test]
async fn native_git_link_changes_and_rollback_never_follow_target_files() {
    use cyber_server::runtime::{SnapshotRecoveryAction as Action, SnapshotRecoveryConfirmation};
    for (want, recorded_node) in [
        ("one", "two"),
        ("one", "file"),
        ("file", "two"),
        ("one", "absent"),
        ("absent", "two"),
        ("missing/child", "file"),
    ] {
        let repo = Repo::new();
        std::fs::write(repo.root.join("one"), b"first target owner data").unwrap();
        std::fs::write(repo.root.join("two"), b"second target owner data").unwrap();
        put_node(&repo.root, want);
        let target = repo.track(&repo.root).await;
        put_node(&repo.root, recorded_node);
        let recorded = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        let store = repo.store(&repo.root);
        assert_eq!(
            store.restore_git_worker(prepared, None).unwrap(),
            ["entry"],
            "{want}/{recorded_node}"
        );
        assert_node(&repo.root, want);
        let id = store.recovery().unwrap()[0].id.clone();
        own_native_journal(&store, &id);
        let review = store.recovery_review(&id, Action::Rollback).unwrap();
        assert!(review.conflicts.is_empty(), "{:?}", review.conflicts);
        let desired = review
            .paths
            .iter()
            .find(|path| path.path == "entry")
            .unwrap();
        if want != "file" && want != "absent" {
            assert_eq!(
                desired.current.as_ref().unwrap().kind,
                super::super::ImageKind::Symlink
            );
        }
        let confirmation = SnapshotRecoveryConfirmation {
            journal_id: id,
            action: Action::Rollback,
            confirmation: review.confirmation.unwrap(),
            request_id: "link-rollback".into(),
        };
        let receipt = store
            .resolve_recovery(&confirmation, &|| Ok(()), None)
            .unwrap();
        assert_node(&repo.root, recorded_node);
        assert_eq!(
            std::fs::read(repo.root.join("one")).unwrap(),
            b"first target owner data"
        );
        assert_eq!(
            std::fs::read(repo.root.join("two")).unwrap(),
            b"second target owner data"
        );
        let (_, journal) = store.journal(&receipt.resolution_id).unwrap();
        assert_eq!(journal.version, 5);
        std::fs::write(repo.root.join("later-owner"), b"owner after resolution").unwrap();
        assert_eq!(
            store
                .resolve_recovery(&confirmation, &|| Ok(()), None)
                .unwrap(),
            receipt
        );
    }
}

#[tokio::test]
async fn native_git_link_target_aliases_cannot_escape_or_enter_protected_namespaces() {
    for target_name in [
        "alias/secret",
        "alias/../secret",
        ".git/config",
        ".GIT/config",
        "../outside/secret",
    ] {
        let repo = Repo::new();
        if target_name == ".GIT/config" && !repo.root.join(target_name).exists() {
            continue;
        }
        let outside = repo.root.parent().unwrap().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"private target data").unwrap();
        std::fs::create_dir(outside.join("child")).unwrap();
        std::os::unix::fs::symlink(outside.join("child"), repo.root.join("alias")).unwrap();
        put_node(&repo.root, target_name);
        let target = repo.track(&repo.root).await;
        put_node(&repo.root, "file");
        let recorded = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        let store = repo.store(&repo.root);
        assert!(store.restore_git_worker(prepared, None).is_err());
        assert_node(&repo.root, "file");
        assert!(store.recovery().unwrap().is_empty());
        assert_eq!(
            std::fs::read(outside.join("secret")).unwrap(),
            b"private target data"
        );
    }
}

#[tokio::test]
async fn native_link_targets_cannot_use_case_aliases_of_configured_private_roots() {
    let repo = Repo::new();
    let private = repo.root.join("Private");
    std::fs::create_dir(&private).unwrap();
    if !repo.root.join("PRIVATE").exists() {
        return;
    }
    put_node(&repo.root, "PRIVATE/missing");
    let target = repo.track(&repo.root).await;
    put_node(&repo.root, "file");
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let mut store = repo.store(&repo.root);
    store.options.private_roots.push(private);
    let result = store.restore_git_worker(prepared, None);
    assert!(
        matches!(&result, Err(RestoreError::Failed(error)) if error.contains("private root") || error.contains("protected")),
        "{result:?}"
    );
    assert_node(&repo.root, "file");
    assert!(store.recovery().unwrap().is_empty());
}
