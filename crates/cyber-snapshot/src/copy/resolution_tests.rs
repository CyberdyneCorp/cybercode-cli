//! Native backend fixtures. Synthetic origins confer no runtime acknowledgement authority.
use super::*;
use cyber_server::runtime::{
    SnapshotRecoveryAction as Action, SnapshotRecoveryConfirmation as Confirmation,
};

impl Prepared {
    fn managed() -> Self {
        let mut p = Self::new();
        p.record.origin = Some(cyber_server::runtime::SnapshotOrigin {
            version: 1,
            session_id: "ses_fixture".into(),
            session_created_id: "evt_fixture".into(),
            activity_id: "op_fixture".into(),
            worktree_id: None,
            binding: cyber_server::runtime::SnapshotBinding {
                directory: p.store.root.clone(),
                root_identity: p.store.root_id.clone(),
                namespace_identity: storage::dir_identity(&p.store.journals).unwrap(),
                journal_id: "restore_fixture".into(),
                journal_identity: storage::dir_identity(&p.dir).unwrap(),
                lock_identity: Some(p.store.lock_identity().unwrap()),
            },
        });
        Store::record(&p.dir, &p.record).unwrap();
        p
    }
    fn confirmation(&self, action: Action, id: &str) -> Confirmation {
        let review = self
            .store
            .recovery_review("restore_fixture", action)
            .unwrap();
        assert!(review.conflicts.is_empty(), "{:?}", review.conflicts);
        Confirmation {
            journal_id: "restore_fixture".into(),
            action,
            confirmation: review.confirmation.unwrap(),
            request_id: id.into(),
        }
    }
    fn resolve(&self, request: &Confirmation) -> cyber_server::runtime::SnapshotRecoveryReceipt {
        self.store
            .resolve_recovery(request, &|| Ok(()), None)
            .unwrap()
    }
    fn resolution_with_origin(
        &self,
        request: &Confirmation,
    ) -> (
        cyber_server::runtime::SnapshotRecoveryReceipt,
        cyber_server::runtime::SnapshotOrigin,
    ) {
        let receipt = self.resolve(request);
        let (dir, mut journal) = self.store.journal(&receipt.resolution_id).unwrap();
        let mut origin = self.record.origin.as_ref().unwrap().clone();
        origin.activity_id = "op_resolution_fixture".into();
        origin.binding.journal_id = receipt.resolution_id;
        origin.binding.journal_identity = storage::dir_identity(&dir).unwrap();
        journal.origin = Some(origin.clone());
        Store::record(&dir, &journal).unwrap();
        (self.resolve(request), origin)
    }
}

#[test]
fn reconciliation_rejects_changed_intent_and_unverifiable_legacy_writes() {
    let p = Prepared::managed();
    p.apply(0);
    let request = p.confirmation(Action::Rollback, "reconcile-identity");
    let (receipt, origin) = p.resolution_with_origin(&request);
    let mut legacy = receipt.clone();
    legacy.resolution_digest = None;
    assert!(
        p.store
            .reconcile_receipt(&origin, &legacy, &|| Ok(()))
            .is_err()
    );
    let (dir, mut journal) = p.store.journal(&receipt.resolution_id).unwrap();
    journal.context = Some(super::super::RestoreContext {
        target: format!("copy:{}", "0".repeat(64)),
        recorded: format!("copy:{}", "0".repeat(64)),
        expected: format!("copy:{}", "0".repeat(64)),
        exclusions: vec!["tampered".into()],
    });
    Store::record(&dir, &journal).unwrap();
    assert!(
        p.store
            .reconcile_receipt(&origin, &receipt, &|| Ok(()))
            .is_err()
    );
    assert!(!p.store.journal("restore_fixture").unwrap().1.acknowledged);
    assert!(
        !p.store
            .journal(&receipt.resolution_id)
            .unwrap()
            .1
            .acknowledged
    );
}

#[test]
fn reconciliation_cut_between_journal_writes_retains_fence_and_retry_preserves_owner_edits() {
    let p = Prepared::managed();
    p.apply(0);
    let request = p.confirmation(Action::Rollback, "reconcile-cut");
    let (receipt, origin) = p.resolution_with_origin(&request);
    let calls = std::cell::Cell::new(0);
    let stopped = || {
        calls.set(calls.get() + 1);
        if calls.get() == 3 {
            Err("cancelled before parent acknowledgement".into())
        } else {
            Ok(())
        }
    };
    assert!(
        p.store
            .reconcile_receipt(&origin, &receipt, &stopped)
            .is_err()
    );
    assert!(
        p.store
            .journal(&receipt.resolution_id)
            .unwrap()
            .1
            .acknowledged
    );
    assert!(!p.store.journal("restore_fixture").unwrap().1.acknowledged);
    assert!(p.store.ensure_recovered().is_err());
    std::fs::write(p.root.join("existing/replace"), b"later owner data").unwrap();
    p.store
        .reconcile_receipt(&origin, &receipt, &|| Ok(()))
        .unwrap();
    p.store.ensure_recovered().unwrap();
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"later owner data"
    );
    let (dir, _) = p.store.journal(&receipt.resolution_id).unwrap();
    let before =
        storage::identity(&storage::open(&dir, Path::new("intent.json")).unwrap()).unwrap();
    let mut legacy = receipt;
    legacy.resolution_digest = None;
    p.store
        .reconcile_receipt(&origin, &legacy, &|| Ok(()))
        .unwrap();
    assert_eq!(
        storage::identity(&storage::open(&dir, Path::new("intent.json")).unwrap()).unwrap(),
        before
    );
}

#[tokio::test]
async fn immutable_intent_handoff_rejects_changed_context_with_unchanged_files_and_receipt() {
    use cyber_server::runtime::SnapshotNativeGuard;
    let p = Prepared::managed();
    p.apply(0);
    let request = p.confirmation(Action::Rollback, "immutable-handoff");
    let receipt = p.resolve(&request);
    let (dir, mut journal) = p.store.journal(&receipt.resolution_id).unwrap();
    let mut origin = p.record.origin.as_ref().unwrap().clone();
    origin.activity_id = "op_resolution_fixture".into();
    origin.binding.journal_id = receipt.resolution_id.clone();
    origin.binding.journal_identity = storage::dir_identity(&dir).unwrap();
    journal.origin = Some(origin.clone());
    Store::record(&dir, &journal).unwrap();
    let receipt = p.resolve(&request);
    let digest = p.store.worker_digest(Some(&origin)).unwrap();
    let workspace = std::sync::Arc::new(tokio::sync::Mutex::new(()))
        .lock_owned()
        .await;
    let guard = crate::copy::WorkerGuard {
        store: p.store,
        workspace,
        origin: Some(origin.clone()),
        digest,
        receipt: Some(receipt),
    };
    guard.verify(Some(&origin)).unwrap();
    let snapshot = format!("copy:{}", "0".repeat(64));
    journal.context = Some(super::super::RestoreContext {
        target: snapshot.clone(),
        recorded: snapshot.clone(),
        expected: snapshot,
        exclusions: vec!["changed after handoff".into()],
    });
    Store::record(&dir, &journal).unwrap();
    guard.store.verify_worker_origin(Some(&origin)).unwrap();
    assert!(
        guard.verify(Some(&origin)).is_err(),
        "native intent changed after its completion handoff"
    );
    assert!(
        !guard
            .store
            .journal("restore_fixture")
            .unwrap()
            .1
            .acknowledged
    );
}

#[test]
fn changed_frozen_resolution_review_cannot_be_consumed_or_acknowledged() {
    let p = Prepared::managed();
    p.apply(0);
    let request = p.confirmation(Action::Rollback, "immutable-review");
    let receipt = p.resolve(&request);
    let (dir, mut journal) = p.store.journal(&receipt.resolution_id).unwrap();
    let mut origin = p.record.origin.as_ref().unwrap().clone();
    origin.activity_id = "op_resolution_fixture".into();
    origin.binding.journal_id = receipt.resolution_id.clone();
    origin.binding.journal_identity = storage::dir_identity(&dir).unwrap();
    journal.origin = Some(origin.clone());
    Store::record(&dir, &journal).unwrap();
    let before = p.store.worker_digest(Some(&origin)).unwrap();
    journal
        .resolution
        .as_mut()
        .unwrap()
        .reviewed_state
        .as_mut()
        .unwrap()
        .completed = true;
    Store::record(&dir, &journal).unwrap();
    assert_ne!(p.store.worker_digest(Some(&origin)).unwrap(), before);
    assert!(p.store.verify_worker_origin(Some(&origin)).is_err());
    assert!(p.store.acknowledge_worker(Some(&origin)).is_err());
    assert!(!p.store.journal("restore_fixture").unwrap().1.acknowledged);
    assert!(
        !p.store
            .journal(&receipt.resolution_id)
            .unwrap()
            .1
            .acknowledged
    );
}

#[test]
fn acknowledged_resolution_settles_incomplete_parent_without_replaying_later_owner_edits() {
    let p = Prepared::managed();
    p.apply(0);
    let request = p.confirmation(Action::Rollback, "settlement-link");
    let receipt = p.resolve(&request);
    let (dir, mut resolution) = p.store.journal(&receipt.resolution_id).unwrap();
    let mut origin = p.record.origin.as_ref().unwrap().clone();
    origin.activity_id = "op_resolution_fixture".into();
    origin.binding.journal_id = receipt.resolution_id.clone();
    origin.binding.journal_identity = storage::dir_identity(&dir).unwrap();
    resolution.origin = Some(origin.clone());
    Store::record(&dir, &resolution).unwrap();
    // Model a cut after child acknowledgement but before the parent link. These
    // synthetic origins do not confer runtime acknowledgement authority.
    resolution.acknowledged = true;
    Store::record(&dir, &resolution).unwrap();
    assert!(p.store.ensure_recovered().is_err());
    std::fs::write(p.root.join("existing/replace"), b"later owner edit").unwrap();
    assert!(p.store.verify_worker_origin(Some(&origin)).is_err());
    // After a durable consuming receipt, acknowledgement reconciliation checks
    // immutable native evidence and must not revalidate/replay owner file edits.
    p.store.acknowledge_worker(Some(&origin)).unwrap();
    let (_, parent) = p.store.journal("restore_fixture").unwrap();
    assert!(!parent.completed);
    assert!(parent.acknowledged);
    assert_eq!(
        parent.settled_by.as_deref(),
        Some(receipt.resolution_id.as_str())
    );
    p.store.ensure_recovered().unwrap();
    p.store.acknowledge_worker(Some(&origin)).unwrap();
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"later owner edit"
    );
    assert_eq!(storage::read(&p.dir, "stage_0", 65536).unwrap(), b"before");
    let mut missing = parent;
    missing.settled_by = Some("restore_missing_resolution".into());
    Store::record(&p.dir, &missing).unwrap();
    assert!(
        p.store.ensure_recovered().is_err(),
        "missing reference licensed native settlement"
    );
}

#[test]
fn rollback_reverses_proven_files_and_retains_actual_displaced_live_inodes() {
    let p = Prepared::managed();
    for index in 0..3 {
        p.apply(index);
    }
    std::fs::write(p.root.join("owner"), b"keep owner data").unwrap();
    let live = storage::identity(
        &storage::open(
            &p.store.parent("existing/replace").unwrap(),
            Path::new("replace"),
        )
        .unwrap(),
    )
    .unwrap();
    let created = storage::identity(
        &storage::open(&p.store.parent("new/deep/file").unwrap(), Path::new("file")).unwrap(),
    )
    .unwrap();
    let request = p.confirmation(Action::Rollback, "rollback-one");
    let receipt = p.resolve(&request);
    assert!(receipt.filesystem_complete);
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"before"
    );
    assert_eq!(
        std::fs::read(p.root.join("existing/delete")).unwrap(),
        b"deleted"
    );
    assert!(!p.root.join("new/deep/file").exists());
    assert_eq!(
        std::fs::read(p.root.join("owner")).unwrap(),
        b"keep owner data"
    );
    let (dir, record) = p.store.journal(&receipt.resolution_id).unwrap();
    assert_eq!(
        storage::identity(&storage::open(&dir, Path::new("stage_0")).unwrap()).unwrap(),
        live
    );
    assert_eq!(
        storage::identity(&storage::open(&dir, Path::new("stage_2")).unwrap()).unwrap(),
        created
    );
    assert!(record.completed && !record.acknowledged);
    assert!(!p.store.journal("restore_fixture").unwrap().1.acknowledged);
    assert!(p.store.ensure_recovered().is_err());
    assert_eq!(storage::read(&p.dir, "stage_0", 65536).unwrap(), b"before");
    assert_eq!(storage::read(&p.dir, "stage_1", 65536).unwrap(), b"deleted");
}

#[test]
fn keep_preserves_reviewed_foreign_owner_images_without_assigning_restore_ownership() {
    let p = Prepared::managed();
    p.apply(0);
    p.replace_same_bytes(&p.root.join("existing/replace"), b"my later edit");
    let rollback = p
        .store
        .recovery_review("restore_fixture", Action::Rollback)
        .unwrap();
    assert!(rollback.confirmation.is_none() && !rollback.conflicts.is_empty());
    let keep = p
        .store
        .recovery_review("restore_fixture", Action::Keep)
        .unwrap();
    assert_eq!(keep.paths[0].state, "foreign");
    assert!(keep.paths.iter().all(|path| !path.changes));
    let request = p.confirmation(Action::Keep, "keep-one");
    let receipt = p.resolve(&request);
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"my later edit"
    );
    let (dir, record) = p.store.journal(&receipt.resolution_id).unwrap();
    assert!(record.changes.is_empty() && !record.acknowledged);
    assert!(p.store.verify_resolution(&dir, &record).is_ok());
    std::fs::write(p.root.join("existing/replace"), b"changed after completion").unwrap();
    assert!(p.store.verify_resolution(&dir, &record).is_err());
    let retry = p
        .store
        .resolve_recovery(
            &request,
            &|| panic!("retry must not authorize effects"),
            None,
        )
        .unwrap();
    assert_eq!(retry, receipt);
    assert!(p.store.ensure_recovered().is_err());
}

#[test]
fn stale_generation_and_whole_plan_conflicts_refuse_before_intent_or_effect() {
    let p = Prepared::managed();
    for index in 0..3 {
        p.apply(index);
    }
    let request = p.confirmation(Action::Rollback, "stale-one");
    std::fs::File::open(p.root.join("existing/replace"))
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(17))
        .unwrap();
    assert!(matches!(
        p.store.resolve_recovery(&request, &|| Ok(()), None),
        Err(RestoreError::Stale)
    ));
    let request = p.confirmation(Action::Rollback, "stale-two");
    std::fs::write(p.root.join("existing/delete"), b"new owner file").unwrap();
    assert!(
        p.store
            .resolve_recovery(&request, &|| Ok(()), None)
            .is_err()
    );
    assert_eq!(
        storage::names(&p.store.journals, 100).unwrap(),
        ["restore_fixture"]
    );
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"after"
    );
    assert_eq!(
        std::fs::read(p.root.join("existing/delete")).unwrap(),
        b"new owner file"
    );
}

#[test]
fn a_same_inode_generation_change_before_the_effect_is_not_adopted_as_new_ownership() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let p = Prepared::managed();
    p.apply(0);
    let request = p.confirmation(Action::Rollback, "generation-during-prepare");
    let calls = AtomicUsize::new(0);
    let authorize = || {
        if calls.fetch_add(1, Ordering::SeqCst) == 1 {
            std::fs::File::open(p.root.join("existing/replace"))
                .unwrap()
                .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(31))
                .unwrap();
        }
        Ok(())
    };
    assert!(
        p.store
            .resolve_recovery(&request, &authorize, None)
            .is_err()
    );
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"after"
    );
    let ids = storage::names(&p.store.journals, 100).unwrap();
    assert_eq!(ids.len(), 2);
    let (_, record) = p
        .store
        .journal(
            ids.iter()
                .find(|id| id.as_str() != "restore_fixture")
                .unwrap(),
        )
        .unwrap();
    assert!(record.applied.is_empty() && !record.completed);
    assert!(p.store.ensure_recovered().is_err());
}

#[test]
fn interrupted_resolution_is_independent_evidence_and_retry_never_redispatches() {
    let p = Prepared::managed();
    for index in 0..3 {
        p.apply(index);
    }
    let request = p.confirmation(Action::Rollback, "partial-one");
    let first = p.root.join("existing/replace");
    let authorize = || {
        if std::fs::read(&first).unwrap() == b"before" {
            Err("admission revoked".into())
        } else {
            Ok(())
        }
    };
    assert!(
        p.store
            .resolve_recovery(&request, &authorize, None)
            .is_err()
    );
    let ids = storage::names(&p.store.journals, 100).unwrap();
    assert_eq!(ids.len(), 2);
    let id = ids
        .iter()
        .find(|id| id.as_str() != "restore_fixture")
        .unwrap();
    let (_, record) = p.store.journal(id).unwrap();
    assert!(!record.completed);
    assert_eq!(record.applied, ["existing/replace"]);
    assert!(record.resolution.as_ref().unwrap().outcome.is_none());
    assert!(
        p.store
            .resolve_recovery(
                &request,
                &|| panic!("partial retry must not dispatch"),
                None
            )
            .is_err()
    );
    assert_eq!(std::fs::read(&first).unwrap(), b"before");
    assert!(!p.root.join("existing/delete").exists());
    assert_eq!(
        std::fs::read(p.root.join("new/deep/file")).unwrap(),
        b"after"
    );
    assert!(p.store.ensure_recovered().is_err());
}

#[test]
fn same_request_id_with_different_action_or_fingerprint_refuses_without_replay() {
    let p = Prepared::managed();
    p.apply(0);
    let request = p.confirmation(Action::Rollback, "owned-id");
    let receipt = p.resolve(&request);
    for action in [Action::Keep, Action::Rollback] {
        let mut changed = request.clone();
        changed.action = action;
        changed.confirmation = "0".repeat(64);
        assert!(
            p.store
                .resolve_recovery(&changed, &|| panic!("ID conflict must not dispatch"), None)
                .is_err()
        );
    }
    assert_eq!(p.resolve(&request), receipt);
    assert_eq!(storage::names(&p.store.journals, 100).unwrap().len(), 2);
}

#[test]
fn cap_refusal_preserves_both_review_and_preimages_until_capacity_is_available() {
    let mut p = Prepared::managed();
    p.apply(0);
    let request = p.confirmation(Action::Rollback, "cap-one");
    p.store.options.max_total = 1;
    assert!(
        p.store
            .resolve_recovery(&request, &|| Ok(()), None)
            .is_err()
    );
    assert_eq!(
        storage::names(&p.store.journals, 100).unwrap(),
        ["restore_fixture"]
    );
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"after"
    );
    assert_eq!(storage::read(&p.dir, "stage_0", 65536).unwrap(), b"before");
    p.store.options.max_total = 4 * 1024 * 1024;
    assert!(p.resolve(&request).filesystem_complete);
}

#[test]
fn rollback_reads_the_actual_retained_preimage_independently_of_capture_objects() {
    let p = Prepared::managed();
    p.apply(0);
    let hash = &p.record.changes[0].before.as_ref().unwrap().object;
    p.store.objects.remove_file(hash).unwrap();
    let receipt = p.resolve(&p.confirmation(Action::Rollback, "retained-preimage"));
    assert!(receipt.filesystem_complete);
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"before"
    );
    assert_eq!(storage::read(&p.dir, "stage_0", 65536).unwrap(), b"before");
    assert!(p.store.objects.symlink_metadata(hash).is_err());
}

#[test]
fn legacy_and_unprepared_journals_do_not_acquire_rollback_proof() {
    let mut p = Prepared::managed();
    p.record.ownership = None;
    Store::record(&p.dir, &p.record).unwrap();
    assert!(
        p.store
            .recovery_review("restore_fixture", Action::Rollback)
            .unwrap()
            .confirmation
            .is_none()
    );
    assert!(
        p.resolve(&p.confirmation(Action::Keep, "unprepared"))
            .filesystem_complete
    );
    p.record.origin = None;
    p.record.version = 1;
    Store::record(&p.dir, &p.record).unwrap();
    let review = p
        .store
        .recovery_review("restore_fixture", Action::Keep)
        .unwrap();
    assert!(review.confirmation.is_none());
    assert!(review.exclusions.is_none());
}

#[tokio::test]
async fn host_review_ignores_capture_disable_and_lowered_capture_size_without_writing() {
    use cyber_server::runtime::Snapshots;
    let p = Prepared::managed();
    p.apply(0);
    let before = storage::names(&p.store.objects, 100).unwrap();
    for config in [
        serde_json::json!({"snapshots":false}),
        serde_json::json!({"snapshots":{"max_file_bytes":1,"fallback_max_bytes":1}}),
    ] {
        let snapshots = crate::GitSnapshots::new(
            p.store.data_path.clone(),
            std::sync::Arc::new(move |_| config.clone()),
        );
        let view = snapshots
            .recovery_review(&p.root.to_string_lossy(), "restore_fixture", Action::Keep)
            .await
            .unwrap();
        assert!(view.confirmation.is_some());
        assert_eq!(view.paths[0].current.as_ref().unwrap().bytes, 5);
    }
    assert_eq!(storage::names(&p.store.objects, 100).unwrap(), before);
    assert_eq!(
        storage::names(&p.store.journals, 100).unwrap(),
        ["restore_fixture"]
    );
}

#[test]
fn replaced_lock_file_cannot_prove_a_live_original_worker_has_exited() {
    let p = Prepared::managed();
    let request = p.confirmation(Action::Keep, "lock-bound");
    let namespace = p
        .private_path("intent.json")
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let lock = namespace.join("lock");
    std::fs::rename(&lock, namespace.join("old_lock")).unwrap();
    std::fs::write(&lock, b"").unwrap();
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o600)).unwrap();
    // A different lock inode really can be acquired while the old owner is live.
    let foreign = Store::recover_existing(
        &p.store.data_path,
        &p.root.to_string_lossy(),
        p.store.options.clone(),
    )
    .unwrap();
    assert_ne!(
        foreign.lock_identity().unwrap(),
        p.store.lock_identity().unwrap()
    );
    let review = foreign
        .recovery_review("restore_fixture", Action::Keep)
        .unwrap();
    assert!(review.confirmation.is_none());
    assert!(
        review
            .conflicts
            .iter()
            .any(|message| message.contains("lock identity changed"))
    );
    assert!(
        foreign
            .resolve_recovery(&request, &|| Ok(()), None)
            .is_err()
    );
    assert!(
        p.store
            .verify_worker_namespace(p.record.origin.as_ref().unwrap())
            .is_err()
    );
    assert_eq!(
        std::fs::read(p.root.join("existing/replace")).unwrap(),
        b"before"
    );
}

#[test]
fn legacy_origin_without_exact_lock_binding_remains_inspectable_and_fenced() {
    let mut p = Prepared::managed();
    p.record.origin.as_mut().unwrap().binding.lock_identity = None;
    Store::record(&p.dir, &p.record).unwrap();
    let view = p
        .store
        .recovery_review("restore_fixture", Action::Keep)
        .unwrap();
    assert!(view.confirmation.is_none());
    assert!(
        view.conflicts
            .iter()
            .any(|message| message.contains("no exact lock identity"))
    );
    assert!(p.store.ensure_recovered().is_err());
}

#[test]
fn recovery_requires_an_existing_namespace_and_real_exclusive_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("work");
    std::fs::create_dir(&root).unwrap();
    let data = temp.path().join("data");
    let options = super::super::super::Options {
        max_file: 65536,
        max_total: 4 * 1024 * 1024,
        ignore: vec![],
        private_roots: vec![],
    };
    assert!(Store::recover_existing(&data, &root.to_string_lossy(), options.clone()).is_err());
    assert!(!data.exists());
    let first = Store::open(&data, &root.to_string_lossy(), options.clone()).unwrap();
    assert!(Store::recover_existing(&data, &root.to_string_lossy(), options.clone()).is_err());
    let identity = first.lock_identity().unwrap();
    drop(first);
    let second = Store::recover_existing(&data, &root.to_string_lossy(), options).unwrap();
    assert_eq!(second.lock_identity().unwrap(), identity);
}
