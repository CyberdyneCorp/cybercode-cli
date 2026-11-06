//! Runtime ownership regressions use a deterministic filesystem-only worker barrier.
use super::*;
use futures::future::BoxFuture;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct Models;
impl ModelResolver for Models {
    fn role(&self, _: cyber_llm::catalog::ModelRole) -> Option<String> {
        None
    }
    fn resolve(&self, _: &str) -> Result<ResolvedModel, String> {
        Err("inference is forbidden in this test".into())
    }
}
struct Host(
    Arc<tokio::sync::Semaphore>,
    Arc<std::sync::atomic::AtomicBool>,
    Arc<std::sync::atomic::AtomicUsize>,
);
impl ToolHost for Host {
    fn binary_file_changed(
        &self,
        _: Arc<cyber_core::binary::editor::AcknowledgedChange>,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async {
            assert_eq!(
                self.0.available_permits(),
                1,
                "notification ran under the path mutation owner"
            );
            self.2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
    }
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn execute(&self, _: Invocation, _: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async { panic!("filesystem-only worker invoked a model tool") })
    }
    fn binary_file_decision<'a>(
        &'a self,
        _: &'a TurnContext,
        _: &'a str,
        _: &'a cyber_core::binary::Resource,
    ) -> BoxFuture<'a, FileDecision> {
        Box::pin(async {
            if self.1.load(std::sync::atomic::Ordering::SeqCst) {
                FileDecision::Deny
            } else {
                FileDecision::Allow
            }
        })
    }
    fn file_mutation_guard<'a>(
        &'a self,
        _: &'a TurnContext,
        _: &'a cyber_core::binary::Resource,
    ) -> BoxFuture<'a, Result<Box<dyn Send>, String>> {
        Box::pin(async {
            Ok(
                Box::new(self.0.clone().acquire_owned().await.map_err(|_| "closed")?)
                    as Box<dyn Send>,
            )
        })
    }
}
pub(super) async fn fixture() -> (
    tempfile::TempDir,
    Runtime,
    String,
    cyber_core::binary::Resource,
    Arc<tokio::sync::Semaphore>,
) {
    fixture_with_policy(Arc::new(std::sync::atomic::AtomicBool::new(false))).await
}
async fn fixture_with_policy(
    denied: Arc<std::sync::atomic::AtomicBool>,
) -> (
    tempfile::TempDir,
    Runtime,
    String,
    cyber_core::binary::Resource,
    Arc<tokio::sync::Semaphore>,
) {
    fixture_with_observer(denied, Arc::new(std::sync::atomic::AtomicUsize::new(0))).await
}
async fn fixture_with_observer(
    denied: Arc<std::sync::atomic::AtomicBool>,
    observed: Arc<std::sync::atomic::AtomicUsize>,
) -> (
    tempfile::TempDir,
    Runtime,
    String,
    cyber_core::binary::Resource,
    Arc<tokio::sync::Semaphore>,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::write(root.join("bytes"), [0, 255]).unwrap();
    let resource = cyber_core::binary::Document::open(&root, "bytes")
        .unwrap()
        .resource();
    let paths = Arc::new(tokio::sync::Semaphore::new(1));
    let runtime = Runtime::new(RuntimeOptions {
        store: Arc::new(
            cyber_store::Store::open(cyber_store::StoreOptions::new(
                cyber_core::paths::DatabaseLocation::Memory,
                Runtime::registry(),
            ))
            .unwrap(),
        ),
        resolver: Arc::new(Models),
        tools: Arc::new(Host(paths.clone(), denied, observed)),
        global_config_dir: root.join("config"),
        shell: "test".into(),
        claude_compat: false,
        compaction: Default::default(),
        retry: Default::default(),
        max_steps: None,
        today: None,
        interactive: false,
        snapshots: Arc::new(NoSnapshots),
    });
    let session = runtime
        .create_session(CreateSession {
            directory: root.to_str().unwrap().into(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    (temp, runtime, session, resource, paths)
}

#[tokio::test]
async fn binary_worker_refreshes_policy_after_preparation_without_releasing_owners() {
    let denied = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (temp, runtime, session, resource, paths) = fixture_with_policy(denied.clone()).await;
    let turn = runtime
        .file_turn(Some(&session), temp.path())
        .await
        .unwrap();
    let (started, running) = tokio::sync::oneshot::channel();
    let (finish, wait) = std::sync::mpsc::channel();
    let worker_paths = paths.clone();
    let task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .binary_file_work(
                    turn,
                    "bin_final_policy".into(),
                    resource,
                    (FileDecision::Allow, FileDecision::Allow),
                    move |work| {
                        work.verify().unwrap();
                        started.send(()).unwrap();
                        wait.recv().unwrap();
                        assert_eq!(worker_paths.available_permits(), 0);
                        work.verify()
                    },
                )
                .await
        })
    };
    running.await.unwrap();
    denied.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(paths.available_permits(), 0);
    finish.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap().unwrap(),
        Err(cyber_core::binary::Error::Changed)
    ));
    assert_eq!(paths.available_permits(), 1);
    assert_eq!(std::fs::read(temp.path().join("bytes")).unwrap(), [0, 255]);
    runtime.shutdown().await;
}

#[tokio::test]
async fn disposed_binary_request_retains_worker_ownership_until_actual_exit() {
    let (temp, runtime, session, resource, paths) = fixture().await;
    let turn = runtime
        .file_turn(Some(&session), temp.path())
        .await
        .unwrap();
    let (started, running) = tokio::sync::oneshot::channel();
    let (finish, wait) = std::sync::mpsc::channel();
    let task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .binary_file_work(
                    turn,
                    "bin_worker".into(),
                    resource,
                    (FileDecision::Allow, FileDecision::Allow),
                    move |cancel| {
                        started.send(()).unwrap();
                        wait.recv().unwrap();
                        assert!(cancel.is_cancelled());
                    },
                )
                .await
        })
    };
    running.await.unwrap();
    assert_eq!(paths.available_permits(), 0);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let controls = runtime.inner.cancel_idle_activities(&session);
    assert_eq!(controls.len(), 1);
    assert!(
        runtime
            .inner
            .wait_idle_activities(&session, controls)
            .await
            .is_err()
    );
    assert_eq!(
        paths.available_permits(),
        0,
        "timed-out Stop released the actual worker guard"
    );
    let handle = runtime.inner.handle(&session).await.unwrap();
    assert!(!handle.has_location_uncertainty());
    let shutdown = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.shutdown().await })
    };
    tokio::task::yield_now().await;
    assert!(
        !shutdown.is_finished(),
        "shutdown outlived an unjoined filesystem worker"
    );
    finish.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(paths.available_permits(), 1);
    assert!(runtime.inner.activities.lock().unwrap().is_empty());
}

#[tokio::test]
async fn disposed_saved_binary_worker_hands_off_only_after_releasing_mutation_owners() {
    use cyber_core::binary::{
        Document, Offset,
        draft::{PatchBatch, Splice},
        editor::{DraftScope, Editor, Owner, SavePreparation},
        storage::StorageRoot,
    };
    let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (temp, runtime, session, resource, paths) = fixture_with_observer(
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        observed.clone(),
    )
    .await;
    let data = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageRoot::prepare(data.path()).unwrap());
    runtime.configure_binary_storage(storage.clone()).unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut source = Document::open(&root, "bytes").unwrap();
    let mut editor = Editor::create(
        storage.open("bin_post_commit").unwrap(),
        Owner {
            session_id: session.clone(),
            directory: root.clone(),
            root,
            scope: DraftScope::Manual,
        },
        &mut source,
        || false,
    )
    .unwrap();
    let turn = runtime
        .file_turn(Some(&session), temp.path())
        .await
        .unwrap();
    let (started, running) = tokio::sync::oneshot::channel();
    let (finish, wait) = std::sync::mpsc::channel();
    let task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .binary_file_work(
                    turn,
                    "bin_post_commit".into(),
                    resource,
                    (FileDecision::Allow, FileDecision::Allow),
                    move |work| {
                        editor
                            .patch(
                                "patch_one",
                                &PatchBatch {
                                    revision: editor.draft().revision(),
                                    splices: vec![Splice {
                                        offset: Offset(0),
                                        delete_length: Offset(1),
                                        expected_hex: "00".into(),
                                        insert_hex: "01".into(),
                                    }],
                                },
                            )
                            .unwrap();
                        let review = editor.review(0, 128, || false).unwrap();
                        let SavePreparation::Ready(prepared) = editor
                            .prepare_save("save_one", &review.fingerprint, || false)
                            .unwrap()
                        else {
                            panic!("save");
                        };
                        editor
                            .commit_save_checked(
                                *prepared,
                                || work.is_cancelled(),
                                || work.verify(),
                            )
                            .unwrap();
                        work.acknowledge_source(Arc::new(
                            editor.acknowledged_change("save_one").unwrap().unwrap(),
                        ))
                        .unwrap();
                        started.send(()).unwrap();
                        wait.recv().unwrap();
                    },
                )
                .await
        })
    };
    running.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(paths.available_permits(), 0);
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 0);
    finish.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while observed.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(paths.available_permits(), 1);
    assert_eq!(observed.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(temp.path().join("bytes")).unwrap(), [1, 255]);
    runtime.shutdown().await;
}

#[tokio::test]
async fn unwound_binary_worker_does_not_claim_file_success_or_poison_native_process_recovery() {
    let (temp, runtime, session, resource, paths) = fixture().await;
    let turn = runtime
        .file_turn(Some(&session), temp.path())
        .await
        .unwrap();
    let result = runtime
        .binary_file_work(
            turn,
            "bin_panic".into(),
            resource,
            (FileDecision::Allow, FileDecision::Allow),
            |_| panic!("injected filesystem unwind"),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(paths.available_permits(), 1);
    let handle = runtime.inner.handle(&session).await.unwrap();
    assert!(!handle.has_location_uncertainty());
    assert!(runtime.inner.activities.lock().unwrap().is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn session_deletion_does_not_infer_settlement_from_a_missing_local_binary_control() {
    let (_temp, runtime, session, _resource, _paths) = fixture().await;
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut activity = super::activity::Scope::reserve_binary(
        &runtime.inner,
        &handle,
        "bin_external_worker".into(),
    )
    .await
    .unwrap();
    activity.launch().unwrap();
    // Simulate a worker owned by a different process/runtime registry. Durable
    // activity remains authoritative even before a document journal exists.
    runtime
        .inner
        .activities
        .lock()
        .unwrap()
        .remove(activity.id());
    let result = runtime.delete(&session).await;
    assert!(
        matches!(result, Err(RuntimeError::Conflict(message)) if message.contains("Binary filesystem activity"))
    );
    assert!(runtime.inner.handle(&session).await.is_ok());
    drop(activity);
    runtime.shutdown().await;
}

#[tokio::test]
async fn recovery_requires_the_configured_exclusive_document_and_leaves_generic_activity_alone() {
    use cyber_core::binary::{
        Document, Offset,
        draft::{PatchBatch, Splice},
        editor::{DraftScope, Editor, Owner},
        storage::StorageRoot,
    };
    let (_temp, runtime, session, resource, _paths) = fixture().await;
    let data = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageRoot::prepare(data.path()).unwrap());
    runtime.configure_binary_storage(storage.clone()).unwrap();
    let root = resource.path().parent().unwrap().to_owned();
    storage.admit_project(&root).unwrap();
    let owner = Owner {
        session_id: session.clone(),
        directory: root.clone(),
        root,
        scope: DraftScope::Manual,
    };
    let mut source = Document::open(&owner.root, "bytes").unwrap();
    let mut editor = Editor::create(
        storage.open("bin_recovery").unwrap(),
        owner.clone(),
        &mut source,
        || false,
    )
    .unwrap();
    let patch = editor
        .patch(
            "retained_patch",
            &PatchBatch {
                revision: editor.draft().revision(),
                splices: vec![Splice {
                    offset: Offset(0),
                    delete_length: Offset(1),
                    expected_hex: "00".into(),
                    insert_hex: "aa".into(),
                }],
            },
        )
        .unwrap();
    let revision = editor.draft().revision();
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut binary =
        super::activity::Scope::reserve_binary(&runtime.inner, &handle, "bin_recovery".into())
            .await
            .unwrap();
    binary.launch().unwrap();
    let generic = super::activity::Scope::reserve(&runtime.inner, &handle)
        .await
        .unwrap();
    let latest = |id: &str| {
        let id = id.to_owned();
        runtime
            .inner
            .store
            .read(move |db| {
                db.query_row(
                    "SELECT data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
                    [id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(Into::into)
            })
            .unwrap()
    };
    assert!(
        runtime
            .recover_binary_activity(editor.recovery_proof().unwrap())
            .is_err(),
        "live local worker was marked finished"
    );
    drop(editor);
    // The exclusive native lock must be reacquired after the old document closes.
    let editor = Editor::resume(storage.open_existing("bin_recovery").unwrap(), &owner).unwrap();
    runtime.inner.activities.lock().unwrap().remove(binary.id());
    let other_data = tempfile::tempdir().unwrap();
    let wrong_storage = StorageRoot::prepare(other_data.path()).unwrap();
    let wrong = Editor::create(
        wrong_storage.open("bin_recovery").unwrap(),
        owner.clone(),
        &mut source,
        || false,
    )
    .unwrap();
    assert!(
        runtime
            .recover_binary_activity(wrong.recovery_proof().unwrap())
            .is_err(),
        "another private root forged document settlement"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&latest(binary.id())).unwrap()["status"],
        "pending"
    );
    assert_eq!(
        runtime
            .recover_binary_activity(editor.recovery_proof().unwrap())
            .unwrap(),
        1
    );
    assert_eq!(
        runtime
            .recover_binary_activity(editor.recovery_proof().unwrap())
            .unwrap(),
        0,
        "recovery replay appended another disposition"
    );
    let disposition: serde_json::Value = serde_json::from_str(&latest(binary.id())).unwrap();
    assert_eq!(disposition["status"], "settled");
    assert_eq!(disposition["phase"], "filesystem_recovered_document_locked");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&latest(generic.id())).unwrap()["status"],
        "pending"
    );
    assert_eq!(editor.draft().revision(), revision);
    assert_eq!(
        editor
            .lookup_operation("retained_patch")
            .unwrap()
            .unwrap()
            .payload_sha256,
        patch.payload_sha256
    );
    assert!(
        editor.lookup_operation("unproven_save").unwrap().is_none(),
        "execution recovery fabricated a file receipt"
    );
    assert_eq!(std::fs::read(owner.root.join("bytes")).unwrap(), [0, 255]);
    drop((generic, binary));
    runtime.shutdown().await;
}
