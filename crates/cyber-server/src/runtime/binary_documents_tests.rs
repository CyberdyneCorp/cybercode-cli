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
struct Host(Arc<tokio::sync::Semaphore>);
impl ToolHost for Host {
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
        Box::pin(async { FileDecision::Allow })
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
async fn fixture() -> (
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
        tools: Arc::new(Host(paths.clone())),
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
    assert!(
        !handle
            .location_uncertain
            .load(std::sync::atomic::Ordering::SeqCst)
    );
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
    assert!(
        !handle
            .location_uncertain
            .load(std::sync::atomic::Ordering::SeqCst)
    );
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
