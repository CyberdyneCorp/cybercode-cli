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
fn restart(runtime: &Runtime, root: &std::path::Path) -> Runtime {
    Runtime::new(RuntimeOptions {
        store: runtime.inner.store.clone(),
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
    let mut generic = activity::Scope::reserve(&runtime.inner, &handle)
        .await
        .unwrap();
    generic.launch().unwrap();
    drop(generic);
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
    let mut unrelated = activity::Scope::reserve(&runtime.inner, &handle)
        .await
        .unwrap();
    let unrelated_id = unrelated.id().to_owned();
    let origin = completed.worker.scope.snapshot_origin().unwrap().clone();
    assert!(
        unrelated.launch_snapshot(origin).is_err(),
        "generic activity adopted a snapshot origin"
    );
    unrelated.launch().unwrap();
    drop(unrelated);
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
