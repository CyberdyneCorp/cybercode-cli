//! Instrumented worker ownership; native journal/namespace proof is tested separately.
use super::super::{Admission, Delivery, RuntimeOptions, events};
use super::*;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

struct Guard {
    origin: Option<SnapshotOrigin>,
    acknowledgements: Arc<AtomicUsize>,
}
impl SnapshotNativeGuard for Guard {
    fn verify(&self, origin: Option<&SnapshotOrigin>) -> Result<(), String> {
        if self.origin.as_ref() != origin {
            return Err("foreign worker receipt".into());
        }
        Ok(())
    }
    fn acknowledge(&mut self, origin: Option<&SnapshotOrigin>) -> Result<(), String> {
        self.verify(origin)?;
        self.acknowledgements.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn binding(directory: PathBuf) -> SnapshotBinding {
    SnapshotBinding {
        directory,
        root_identity: "instrumented_root".into(),
        namespace_identity: "instrumented_namespace".into(),
        journal_id: "restore_test".into(),
        journal_identity: "instrumented_journal".into(),
        lock_identity: Some("instrumented_lock".into()),
    }
}
async fn worker(runtime: &Runtime, session: &str) -> SnapshotWorker {
    let handle = runtime.inner.handle(session).await.unwrap();
    let state = handle.state.lock().await;
    runtime.snapshot_worker(&handle, &state).await.unwrap()
}
fn latest(runtime: &Runtime, session: &str) -> serde_json::Value {
    let session = session.to_owned();
    runtime.inner.store.read(move |db| {
        let data: String = db.query_row("SELECT data FROM event WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1 AND json_extract(data,'$.snapshot_worker')=1 ORDER BY rowid DESC LIMIT 1", [session], |row| row.get(0))?;
        Ok(serde_json::from_str(&data).unwrap())
    }).unwrap()
}

fn legacy_generic_unknown(runtime: &Runtime, handle: &Handle, session: &str) -> String {
    // Historical opaque evidence can coexist with a snapshot fence. This does
    // not launch new work through an already-fenced ordinary admission path.
    let id = cyber_core::ids::new_id("op");
    runtime.inner.store.append(&id, cyber_store::Expected::Seq(-1), vec![NewEvent::new(activity::CHANGED,
        serde_json::json!({"session_id":session,"status":"unknown","phase":"legacy_process_unknown"}))]).unwrap();
    handle.location_uncertain.store(true, Ordering::SeqCst);
    id
}
fn restart(runtime: &Runtime, root: &std::path::Path) -> Runtime {
    runtime_with_store(runtime, runtime.inner.store.clone(), root)
}
fn runtime_with_store(
    runtime: &Runtime,
    store: Arc<cyber_store::Store>,
    root: &std::path::Path,
) -> Runtime {
    Runtime::new(RuntimeOptions {
        store,
        resolver: runtime.inner.resolver.clone(),
        tools: runtime.inner.tools.clone(),
        global_config_dir: root.join("config"),
        shell: "test".into(),
        claude_compat: false,
        compaction: Default::default(),
        retry: Default::default(),
        max_steps: None,
        today: None,
        interactive: false,
        snapshots: Arc::new(super::super::NoSnapshots),
    })
}

#[tokio::test]
async fn recovery_pair_lost_commit_reply_is_lookup_only_after_reopening_the_real_store() {
    fn lost_reply(_: &rusqlite::hooks::Wal, _: i32) -> rusqlite::Result<()> {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("injected recovery pair reply loss".into()),
        ))
    }
    let (temp, base, _, _, _) = super::super::binary_documents_tests::fixture().await;
    let path = temp.path().join("recovery-pair.db");
    let store = cyber_store::Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::File(path.clone()),
        Runtime::registry(),
    ))
    .unwrap();
    let runtime = runtime_with_store(&base, Arc::new(store), temp.path());
    let session = runtime
        .create_session(super::super::CreateSession {
            directory: temp.path().to_str().unwrap().into(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    let request = recovery_request(original.clone());
    let handle = runtime.inner.handle(&session).await.unwrap();
    let state = handle.state.lock().await;
    let owner = runtime
        .snapshot_recovery_worker(&handle, &state, request.clone())
        .await
        .unwrap();
    drop(state);
    let count = Arc::new(AtomicUsize::new(0));
    let completed = finish_recovery(owner, request.clone(), count.clone()).await;
    runtime
        .inner
        .store
        .transaction(|tx| {
            tx.wal_hook(Some(lost_reply));
            Ok(())
        })
        .unwrap();
    let notifications = runtime.inner.store.subscribe();
    let state = handle.state.lock().await;
    assert!(completed.commit_recovery(&state).is_err());
    drop(state);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(
        notifications.try_recv().is_err(),
        "unknown commit reply published a notification"
    );
    let original_record = runtime
        .inner
        .store
        .read_events(&original.activity_id, -1, 10)
        .unwrap();
    assert_eq!(
        original_record.events.last().unwrap().data["status"],
        "settled"
    );
    let lookup = runtime
        .snapshot_recovery_confirm(&session, &request.confirmation)
        .await
        .unwrap();
    assert_eq!(
        lookup.state,
        super::super::SnapshotRecoveryState::Acknowledged
    );
    assert_eq!(lookup.journal_acknowledged, None);
    let receipt = lookup.receipt.unwrap();
    runtime.shutdown().await;
    drop(handle);
    drop(runtime);
    let reopened = cyber_store::Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::File(path),
        Runtime::registry(),
    ))
    .unwrap();
    let reloaded = runtime_with_store(&base, Arc::new(reopened), temp.path());
    let lookup = reloaded
        .snapshot_recovery_confirm(&session, &request.confirmation)
        .await
        .unwrap();
    assert_eq!(lookup.receipt, Some(receipt));
    assert_eq!(
        lookup.state,
        super::super::SnapshotRecoveryState::Acknowledged
    );
    assert_eq!(
        count.load(Ordering::SeqCst),
        0,
        "lookup replayed native work"
    );
    reloaded.shutdown().await;
    base.shutdown().await;
}

async fn failed_snapshot(runtime: &Runtime, session: &str, directory: PathBuf) -> SnapshotOrigin {
    let owner = worker(runtime, session).await;
    assert!(
        owner
            .run(move |work| {
                work.begin(binding(directory)).unwrap();
                Err(RestoreError::Failed("interrupted native restore".into()))
            })
            .await
            .is_err()
    );
    serde_json::from_value(latest(runtime, session)["snapshot_origin"].clone()).unwrap()
}

fn recovery_request(
    original: SnapshotOrigin,
) -> super::super::snapshot_recovery_types::RecoveryRequest {
    super::super::snapshot_recovery_types::RecoveryRequest {
        confirmation: super::super::SnapshotRecoveryConfirmation {
            journal_id: original.binding.journal_id.clone(),
            action: super::super::SnapshotRecoveryAction::Keep,
            confirmation: "a".repeat(64),
            request_id: "owned-recovery-request".into(),
        },
        original,
        authority: None,
    }
}

async fn finish_recovery(
    owner: SnapshotWorker,
    request: super::super::snapshot_recovery_types::RecoveryRequest,
    count: Arc<AtomicUsize>,
) -> SnapshotCompletion {
    owner
        .run(move |work| {
            let mut native_binding = request.original.binding.clone();
            native_binding.journal_id = "restore_resolution_owned".into();
            native_binding.journal_identity = "resolution_journal_identity".into();
            let origin = work.begin(native_binding).unwrap();
            work.effect(|| Ok(())).unwrap();
            let mut native = SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin.clone()),
                    acknowledgements: count,
                }),
            );
            native.recovery = Some(super::super::SnapshotRecoveryReceipt {
                journal_id: request.confirmation.journal_id,
                resolution_id: origin.binding.journal_id,
                request_id: request.confirmation.request_id,
                action: request.confirmation.action,
                confirmation: request.confirmation.confirmation,
                original: request.original,
                filesystem_complete: true,
            });
            Ok(native)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn writer_snapshot_fence_rejects_input_even_when_the_callers_precheck_is_stale() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let admitted = runtime
        .admit(&session, Admission::text("held seed", Delivery::Hold))
        .await
        .unwrap();
    let source = session.clone();
    let mut payload: serde_json::Value = runtime.inner.store.read(move |db| {
        let data: String = db.query_row("SELECT data FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq DESC LIMIT 1", [&source, events::ADMITTED], |row| row.get(0))?;
        Ok(serde_json::from_str(&data).unwrap())
    }).unwrap();
    payload["message_id"] = "msg_stale_admission".into();
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    let before = runtime.inner.store.aggregate_seq(&session).unwrap();
    let failed = runtime.inner.store.append(
        &session,
        cyber_store::Expected::Seq(before.unwrap()),
        vec![NewEvent::new(events::ADMITTED, payload)],
    );
    assert!(
        matches!(failed, Err(cyber_store::StoreError::Projector { reason, .. }) if reason == super::super::snapshot_recovery_record::FENCED)
    );
    assert_eq!(runtime.inner.store.aggregate_seq(&session).unwrap(), before);
    let state = runtime.state(&session).await.unwrap();
    assert!(state.input(&admitted.message_id).is_some());
    assert!(state.input("msg_stale_admission").is_none());
    assert_eq!(
        runtime
            .inner
            .store
            .aggregate_seq(&original.activity_id)
            .unwrap(),
        Some(2)
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn ordinary_snapshot_reservation_cannot_race_a_pending_generic_native_owner() {
    let (_temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut generic = activity::Scope::reserve(&runtime.inner, &handle)
        .await
        .unwrap();
    let source = session.clone();
    let events_before: i64 = runtime.inner.store.read(move |db| Ok(db.query_row("SELECT count(*) FROM event WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1", [source], |row| row.get(0))?)).unwrap();
    let state = handle.state.lock().await;
    assert!(runtime.snapshot_worker(&handle, &state).await.is_err());
    drop(state);
    assert!(!handle.snapshot_active.load(Ordering::SeqCst));
    let source = session.clone();
    let events_after: i64 = runtime.inner.store.read(move |db| Ok(db.query_row("SELECT count(*) FROM event WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1", [source], |row| row.get(0))?)).unwrap();
    assert_eq!(
        events_after, events_before,
        "failed admission published native ownership"
    );
    generic.launch().unwrap();
    generic.settle().unwrap();
    drop(generic);
    runtime.shutdown().await;
}

#[tokio::test]
async fn recovery_preserves_closed_admission_and_acknowledges_only_its_exact_native_pair() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    let handle = runtime.inner.handle(&session).await.unwrap();
    legacy_generic_unknown(&runtime, &handle, &session);
    let boundary = runtime.close_subtree_admissions(&session).await.unwrap();
    assert!(runtime.capture_child_admission(&session).is_err());
    let request = recovery_request(original.clone());
    let state = handle.state.lock().await;
    let sequence = state.last_seq;
    let owner = runtime
        .snapshot_recovery_worker(&handle, &state, request.clone())
        .await
        .unwrap();
    assert!(owner.scope.authority().is_err());
    drop(state);
    let reserved = runtime
        .snapshot_recovery_lookup(&session, &request.confirmation.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reserved.state, super::super::SnapshotRecoveryState::Pending);
    assert!(reserved.resolution.is_none() && reserved.receipt.is_none());
    let count = Arc::new(AtomicUsize::new(0));
    let completed = finish_recovery(owner, request.clone(), count.clone()).await;
    let state = handle.state.lock().await;
    completed.commit_recovery(&state).unwrap();
    assert_eq!(state.last_seq, sequence, "recovery moved the transcript");
    drop(state);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(
        handle.location_uncertain.load(Ordering::SeqCst),
        "generic unknown outcome was cleared"
    );
    assert!(
        !handle
            .snapshot_uncertain
            .lock()
            .unwrap()
            .contains(&original.activity_id)
    );
    assert!(runtime.capture_child_admission(&session).is_err());
    let source = session.clone();
    assert_eq!(
        runtime
            .inner
            .store
            .read(move |db| super::super::subtree_reopen::active_scope(db, &source))
            .unwrap(),
        Some(boundary)
    );
    let lookup = runtime
        .snapshot_recovery_confirm(&session, &request.confirmation)
        .await
        .unwrap();
    assert_eq!(
        lookup.state,
        super::super::SnapshotRecoveryState::Acknowledged
    );
    assert!(lookup.receipt.is_some());
    assert_eq!(lookup.journal_acknowledged, None);
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "retry replayed native acknowledgement"
    );
    let mut foreign = request.confirmation;
    foreign.journal_id = "restore_foreign".into();
    assert!(
        runtime
            .snapshot_recovery_confirm(&session, &foreign)
            .await
            .is_err()
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_new_stop_boundary_revokes_recovery_before_native_origin_or_effects() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    let request = recovery_request(original);
    let handle = runtime.inner.handle(&session).await.unwrap();
    let state = handle.state.lock().await;
    let owner = runtime
        .snapshot_recovery_worker(&handle, &state, request.clone())
        .await
        .unwrap();
    drop(state);
    runtime.fence_subtree_admissions(&session).await.unwrap();
    let native_binding = binding(temp.path().canonicalize().unwrap());
    assert!(
        owner
            .run(move |work| {
                assert!(work.authorize().is_err());
                assert!(work.begin(native_binding).is_err());
                assert!(
                    work.effect::<()>(|| panic!("revoked recovery changed source files"))
                        .is_err()
                );
                Err(RestoreError::Failed("recovery revoked".into()))
            })
            .await
            .is_err()
    );
    let lookup = runtime
        .snapshot_recovery_confirm(&session, &request.confirmation)
        .await
        .unwrap();
    assert_eq!(lookup.state, super::super::SnapshotRecoveryState::Unknown);
    assert!(lookup.resolution.is_none() && lookup.receipt.is_none());
    assert!(handle.has_location_uncertainty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn changed_session_rejects_recovery_pair_and_retry_only_looks_up_unknown_request() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    let request = recovery_request(original.clone());
    let handle = runtime.inner.handle(&session).await.unwrap();
    let state = handle.state.lock().await;
    let owner = runtime
        .snapshot_recovery_worker(&handle, &state, request.clone())
        .await
        .unwrap();
    drop(state);
    let count = Arc::new(AtomicUsize::new(0));
    let completed = finish_recovery(owner, request.clone(), count.clone()).await;
    let mut state = handle.state.lock().await;
    runtime
        .inner
        .commit_locked(
            &mut state,
            vec![events::event(
                events::TITLE_GENERATED,
                &events::Titled {
                    title: "changed revision".into(),
                    usage: None,
                    cost: None,
                    model: None,
                },
            )],
        )
        .unwrap();
    assert!(completed.commit_recovery(&state).is_err());
    drop(state);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(
        runtime
            .inner
            .store
            .aggregate_seq(&original.activity_id)
            .unwrap(),
        Some(2)
    );
    let lookup = runtime
        .snapshot_recovery_confirm(&session, &request.confirmation)
        .await
        .unwrap();
    assert_eq!(lookup.state, super::super::SnapshotRecoveryState::Unknown);
    assert!(lookup.receipt.is_none());
    runtime.shutdown().await;
}

#[tokio::test]
async fn late_snapshot_disposal_preserves_durable_acknowledgement_and_unrelated_uncertainty() {
    for (unrelated_uncertainty, other_snapshot) in [(false, false), (true, false), (false, true)] {
        let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let owner = worker(&runtime, &session).await;
        let native_binding = binding(temp.path().canonicalize().unwrap());
        let completed = owner
            .run(move |work| {
                let origin = work.begin(native_binding).unwrap();
                Ok(SnapshotNativeResult::new(
                    vec![],
                    Box::new(Guard {
                        origin: Some(origin),
                        acknowledgements: Arc::new(AtomicUsize::new(0)),
                    }),
                ))
            })
            .await
            .unwrap();
        let id = completed.worker.scope.id().to_owned();
        let mut receipt = latest(&runtime, &session);
        receipt["status"] = "settled".into();
        receipt["phase"] = "snapshot_recovery_committed".into();
        receipt["resolution_receipt"] = serde_json::json!({"request_id":"retained_request"});
        runtime
            .inner
            .store
            .append(
                &id,
                cyber_store::Expected::Seq(1),
                vec![NewEvent::new(activity::CHANGED, receipt.clone())],
            )
            .unwrap();
        let sequence = runtime.inner.store.aggregate_seq(&id).unwrap();
        let notifications = runtime.inner.store.subscribe();
        let handle = runtime.inner.handle(&session).await.unwrap();
        handle
            .location_uncertain
            .store(unrelated_uncertainty, Ordering::SeqCst);
        if other_snapshot {
            handle.mark_snapshot_uncertain("op_other_snapshot");
        }
        drop(completed);
        assert!(notifications.try_recv().is_err());
        assert_eq!(runtime.inner.store.aggregate_seq(&id).unwrap(), sequence);
        assert_eq!(latest(&runtime, &session), receipt);
        assert_eq!(
            handle.has_location_uncertainty(),
            unrelated_uncertainty || other_snapshot
        );
        assert_eq!(
            handle.location_uncertain.load(Ordering::SeqCst),
            unrelated_uncertainty
        );
        assert!(!handle.snapshot_uncertain.lock().unwrap().contains(&id));
        runtime.shutdown().await;
    }
}

#[tokio::test]
async fn late_snapshot_disposal_refuses_changed_native_ownership() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let completed = owner
        .run(move |work| {
            let origin = work.begin(native_binding).unwrap();
            Ok(SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin),
                    acknowledgements: Arc::new(AtomicUsize::new(0)),
                }),
            ))
        })
        .await
        .unwrap();
    let id = completed.worker.scope.id().to_owned();
    let mut foreign = latest(&runtime, &session);
    foreign["snapshot_origin"]["binding"]["journal_id"] = "restore_foreign".into();
    runtime
        .inner
        .store
        .append(
            &id,
            cyber_store::Expected::Seq(1),
            vec![NewEvent::new(activity::CHANGED, foreign.clone())],
        )
        .unwrap();
    let notifications = runtime.inner.store.subscribe();
    drop(completed);
    assert!(notifications.try_recv().is_err());
    assert_eq!(runtime.inner.store.aggregate_seq(&id).unwrap(), Some(2));
    assert_eq!(latest(&runtime, &session), foreign);
    assert!(
        runtime
            .inner
            .handle(&session)
            .await
            .unwrap()
            .has_location_uncertainty()
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn reload_preserves_generic_uncertainty_separately_from_snapshot_owners() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    assert!(
        owner
            .run(move |work| {
                work.begin(native_binding).unwrap();
                Err(RestoreError::Failed("interrupted snapshot".into()))
            })
            .await
            .is_err()
    );
    let handle = runtime.inner.handle(&session).await.unwrap();
    assert!(handle.has_location_uncertainty());
    assert!(!handle.location_uncertain.load(Ordering::SeqCst));
    let snapshot = latest(&runtime, &session);
    let activity_id = snapshot["snapshot_origin"]["activity_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        handle
            .snapshot_uncertain
            .lock()
            .unwrap()
            .contains(&activity_id)
    );
    assert!(
        activity::Scope::reserve(&runtime.inner, &handle)
            .await
            .is_err(),
        "snapshot fence admitted a new generic native scope"
    );
    legacy_generic_unknown(&runtime, &handle, &session);
    assert!(handle.location_uncertain.load(Ordering::SeqCst));
    let reloaded = restart(&runtime, temp.path());
    let restored = reloaded.inner.handle(&session).await.unwrap();
    assert!(restored.location_uncertain.load(Ordering::SeqCst));
    assert!(
        restored
            .snapshot_uncertain
            .lock()
            .unwrap()
            .contains(&activity_id)
    );
    assert!(
        reloaded
            .admit(
                &session,
                Admission::text("must stay fenced", Delivery::Hold)
            )
            .await
            .is_err()
    );
    // Simulate a separate writer's exact durable snapshot acknowledgement. Reload
    // must retain the generic fence even when no snapshot activity remains unknown.
    let mut receipt = snapshot;
    receipt["status"] = "settled".into();
    runtime
        .inner
        .store
        .append(
            &activity_id,
            cyber_store::Expected::Seq(2),
            vec![NewEvent::new(activity::CHANGED, receipt)],
        )
        .unwrap();
    let after_ack = restart(&runtime, temp.path());
    let restored = after_ack.inner.handle(&session).await.unwrap();
    assert!(restored.snapshot_uncertain.lock().unwrap().is_empty());
    assert!(restored.location_uncertain.load(Ordering::SeqCst));
    let state = restored.state.lock().await;
    assert!(after_ack.snapshot_worker(&restored, &state).await.is_err());
    drop(state);
    after_ack.shutdown().await;
    reloaded.shutdown().await;
    runtime.shutdown().await;
}

#[tokio::test]
async fn ordinary_restore_commit_cannot_consume_a_recovery_receipt_or_acknowledge_it() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let acknowledgements = Arc::new(AtomicUsize::new(0));
    let count = acknowledgements.clone();
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let completed = owner
        .run(move |work| {
            let origin = work.begin(native_binding).unwrap();
            let mut result = SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin.clone()),
                    acknowledgements: count,
                }),
            );
            result.recovery = Some(super::super::SnapshotRecoveryReceipt {
                journal_id: "restore_original".into(),
                resolution_id: "restore_resolution".into(),
                request_id: "request".into(),
                action: super::super::SnapshotRecoveryAction::Keep,
                confirmation: "a".repeat(64),
                original: origin,
                filesystem_complete: true,
            });
            Ok(result)
        })
        .await
        .unwrap();
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut state = handle.state.lock().await;
    let sequence = state.last_seq;
    assert!(completed.commit(&mut state, vec![]).is_err());
    assert_eq!(state.last_seq, sequence);
    assert_eq!(acknowledgements.load(Ordering::SeqCst), 0);
    assert_eq!(latest(&runtime, &session)["status"], "unknown");
    drop(state);
    runtime.shutdown().await;
}

#[tokio::test]
async fn non_utf8_location_binding_refuses_before_serializing_a_native_origin() {
    use std::os::unix::ffi::OsStringExt;
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let mut owner = worker(&runtime, &session).await;
    // Exercise the serialization boundary even on filesystems that reject these
    // names themselves. This is not a native path-admission acceptance test.
    let directory = temp
        .path()
        .join(std::ffi::OsString::from_vec(b"root_\xff".to_vec()));
    owner.directory = directory.clone();
    let native_binding = binding(directory);
    let result = owner
        .run(move |work| {
            assert!(work.begin(native_binding).unwrap_err().contains("UTF-8"));
            assert!(work.origin().is_none());
            Err(RestoreError::Stale)
        })
        .await;
    assert!(matches!(result, Err(RestoreError::Stale)));
    assert_eq!(latest(&runtime, &session)["status"], "settled");
    runtime.shutdown().await;
}

#[tokio::test]
async fn rejected_origin_append_cannot_authorize_source_effects_even_if_the_adapter_continues() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    runtime
        .inner
        .store
        .set_append_fault(Some(Arc::new(|_, event| {
            event.kind == activity::CHANGED
                && event.data["snapshot_worker"] == true
                && event.data["phase"] == "launching"
                && event.data["status"] == "pending"
        })));
    let result = owner
        .run(move |work| {
            assert!(work.begin(native_binding).is_err());
            assert!(work.origin().is_none());
            assert!(
                work.effect::<()>(|| panic!("unacknowledged origin authorized a source mutation"))
                    .is_err()
            );
            Err(RestoreError::Failed("origin append refused".into()))
        })
        .await;
    assert!(result.is_err());
    runtime.inner.store.set_append_fault(None);
    assert_eq!(latest(&runtime, &session)["status"], "settled");
    assert!(latest(&runtime, &session)["snapshot_origin"].is_null());
    runtime.shutdown().await;
}

#[tokio::test]
async fn snapshot_completion_commits_session_and_exact_origin_before_native_ack() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut unrelated = activity::Scope::reserve(&runtime.inner, &handle)
        .await
        .unwrap();
    let fake = SnapshotOrigin {
        version: 1,
        session_id: session.clone(),
        session_created_id: "untrusted_attempt".into(),
        activity_id: unrelated.id().to_owned(),
        worktree_id: None,
        binding: binding(temp.path().canonicalize().unwrap()),
    };
    assert!(
        unrelated.launch_snapshot(fake).is_err(),
        "generic activity adopted a snapshot origin"
    );
    unrelated.settle().unwrap();
    drop(unrelated);
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let acknowledgements = Arc::new(AtomicUsize::new(0));
    let seen = acknowledgements.clone();
    let completed = owner
        .run(move |work| {
            let origin = work.begin(native_binding.clone()).unwrap();
            assert!(
                work.begin(native_binding).is_err(),
                "origin replacement accepted"
            );
            work.effect(|| Ok(())).unwrap();
            Ok(SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin),
                    acknowledgements: seen,
                }),
            ))
        })
        .await
        .unwrap();
    assert_eq!(acknowledgements.load(Ordering::SeqCst), 0);
    let handle = runtime.inner.handle(&session).await.unwrap();
    assert!(
        activity::Scope::reserve(&runtime.inner, &handle)
            .await
            .is_err()
    );
    let unrelated_id = legacy_generic_unknown(&runtime, &handle, &session);
    let mut state = handle.state.lock().await;
    completed
        .commit(
            &mut state,
            vec![events::event(
                events::TITLE_GENERATED,
                &events::Titled {
                    title: "restored".into(),
                    usage: None,
                    cost: None,
                    model: None,
                },
            )],
        )
        .unwrap();
    assert_eq!(state.info.title, "restored");
    drop(state);
    let record = latest(&runtime, &session);
    assert_eq!(record["status"], "settled");
    assert_eq!(record["phase"], "snapshot_restore_committed");
    assert_eq!(record["snapshot_origin"]["session_id"], session);
    let created = runtime
        .inner
        .store
        .read_events(&session, -1, 1)
        .unwrap()
        .events[0]
        .id
        .clone();
    assert_eq!(record["snapshot_origin"]["session_created_id"], created);
    assert!(record.get("binary_document").is_none());
    assert_eq!(acknowledgements.load(Ordering::SeqCst), 1);
    let unrelated = runtime
        .inner
        .store
        .read_events(&unrelated_id, -1, 10)
        .unwrap();
    assert_eq!(unrelated.events.last().unwrap().data["status"], "unknown");
    assert!(
        handle.has_location_uncertainty(),
        "snapshot receipt cleared another process's unknown outcome"
    );
    assert!(!handle.snapshot_active.load(Ordering::SeqCst));
    runtime.shutdown().await;
}

#[tokio::test]
async fn disposed_snapshot_request_retains_real_worker_owners_and_stop_remains_available() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let mut retry = Admission::text("retained held input", Delivery::Hold);
    retry.message_id = Some("msg_snapshot_exact_retry".into());
    let admitted = runtime.admit(&session, retry.clone()).await.unwrap();
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = permits.clone().acquire_owned().await.unwrap();
    let (started, running) = tokio::sync::oneshot::channel();
    let (finish, gate) = std::sync::mpsc::channel();
    let task = tokio::spawn(owner.run(move |work| {
        let _permit = permit;
        work.begin(native_binding).unwrap();
        started.send(()).unwrap();
        gate.recv().unwrap();
        assert!(work.authorize().is_err());
        Err(RestoreError::Failed(
            "interrupted worker retained its intent".into(),
        ))
    }));
    running.await.unwrap();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert_eq!(permits.available_permits(), 0);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), runtime.state(&session))
            .await
            .unwrap()
            .is_ok()
    );
    assert!(
        runtime
            .admit(&session, Admission::text("must not run", Delivery::Queue))
            .await
            .is_err()
    );
    assert_eq!(
        runtime
            .admit(&session, retry.clone())
            .await
            .unwrap()
            .message_id,
        admitted.message_id
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(3), runtime.interrupt(&session))
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        permits.available_permits(),
        0,
        "interruption released a live worker owner"
    );
    let stopped = tokio::time::timeout(Duration::from_secs(4), runtime.stop_subtree(&session))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.status, super::super::SubtreeStopStatus::Unknown);
    assert!(stopped.persisted);
    assert_eq!(
        permits.available_permits(),
        0,
        "subtree Stop released a live worker owner"
    );
    let shutdown = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.shutdown().await })
    };
    tokio::task::yield_now().await;
    assert!(
        !shutdown.is_finished(),
        "shutdown outlived its worker ownership"
    );
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(permits.available_permits(), 1);
    assert_eq!(latest(&runtime, &session)["status"], "unknown");
    let restarted = restart(&runtime, temp.path());
    assert!(
        restarted
            .admit(&session, Admission::text("still fenced", Delivery::Queue))
            .await
            .is_err()
    );
    restarted.shutdown().await;
}

#[tokio::test]
async fn snapshot_pre_effect_refusal_does_not_manufacture_an_unknown_native_effect() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let wrong = binding(temp.path().join("foreign"));
    let result = owner
        .run(move |work| {
            assert!(
                work.effect::<()>(|| panic!("unbound source effect ran"))
                    .is_err()
            );
            assert!(work.begin(wrong).is_err());
            Err(RestoreError::Stale)
        })
        .await;
    assert!(matches!(result, Err(RestoreError::Stale)));
    let record = latest(&runtime, &session);
    assert_eq!(record["status"], "settled");
    assert!(record.get("snapshot_origin").is_none());
    let handle = runtime.inner.handle(&session).await.unwrap();
    assert!(!handle.snapshot_active.load(Ordering::SeqCst));
    assert!(!handle.has_location_uncertainty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn changed_session_refuses_the_next_source_effect_and_never_acknowledges_it() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let source = temp.path().join("must_not_change");
    std::fs::write(&source, "owner bytes").unwrap();
    let worker_source = source.clone();
    let (started, running) = tokio::sync::oneshot::channel();
    let (finish, gate) = std::sync::mpsc::channel();
    let task = tokio::spawn(owner.run(move |work| {
        work.begin(native_binding).unwrap();
        started.send(()).unwrap();
        gate.recv().unwrap();
        assert!(
            work.effect(|| std::fs::write(worker_source, "wrong").map_err(|e| e.to_string()))
                .is_err()
        );
        Err(RestoreError::Stale)
    }));
    running.await.unwrap();
    runtime
        .rename(&session, "owner changed Session")
        .await
        .unwrap();
    finish.send(()).unwrap();
    assert!(matches!(task.await.unwrap(), Err(RestoreError::Stale)));
    assert_eq!(std::fs::read_to_string(source).unwrap(), "owner bytes");
    assert_eq!(latest(&runtime, &session)["status"], "unknown");
    assert_eq!(
        runtime.state(&session).await.unwrap().info.title,
        "owner changed Session"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn rejected_runtime_commit_keeps_origin_unknown_and_never_acknowledges_the_journal() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let acknowledgements = Arc::new(AtomicUsize::new(0));
    let observed = acknowledgements.clone();
    let completed = owner
        .run(move |work| {
            let origin = work.begin(native_binding).unwrap();
            Ok(SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin),
                    acknowledgements: observed,
                }),
            ))
        })
        .await
        .unwrap();
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut state = handle.state.lock().await;
    let sequence = state.last_seq;
    assert!(
        completed
            .commit(
                &mut state,
                vec![NewEvent::new("test.unregistered.1", serde_json::json!({}))]
            )
            .is_err()
    );
    assert_eq!(state.last_seq, sequence);
    assert_eq!(acknowledgements.load(Ordering::SeqCst), 0);
    drop(state);
    assert_eq!(latest(&runtime, &session)["status"], "unknown");
    runtime.shutdown().await;
    let restarted = restart(&runtime, temp.path());
    assert!(
        restarted
            .admit(
                &session,
                Admission::text("unresolved origin must fence direct input", Delivery::Queue)
            )
            .await
            .is_err()
    );
    assert!(
        restarted
            .inner
            .handle(&session)
            .await
            .unwrap()
            .has_location_uncertainty()
    );
    restarted.shutdown().await;
}
