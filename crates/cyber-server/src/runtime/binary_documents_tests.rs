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
