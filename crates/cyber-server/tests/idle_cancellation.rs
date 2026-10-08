mod support;
use cyber_server::runtime::*;
use futures::future::BoxFuture;
use std::{sync::Arc, time::Duration};
use support::{Harness, Setup};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Host {
    started: Notify,
    cancelled: Notify,
    release: Notify,
}
impl ToolHost for Host {
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn execute(&self, _: Invocation, _: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async { ToolOutcome::Aborted })
    }
    fn shell_owned(
        &self,
        _: &str,
        _: &str,
        _: &str,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            self.started.notify_one();
            cancel.cancelled().await;
            self.cancelled.notify_one();
            self.release.notified().await;
            Err("Native cancellation acknowledged".into())
        })
    }
}
fn runtime(h: &Harness, host: Arc<Host>) -> Runtime {
    Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: h.models.clone(),
        tools: host,
        global_config_dir: h.dir.path().join("global"),
        shell: "bash".into(),
        claude_compat: true,
        compaction: Default::default(),
        retry: Default::default(),
        max_steps: None,
        today: None,
        interactive: false,
        snapshots: Arc::new(NoSnapshots),
    })
}

fn latest_activity(h: &Harness, source: &str) -> String {
    let source = source.to_owned();
    h.store.read(move |db| {
        Ok(db.query_row("SELECT json_extract(data,'$.status') FROM event WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1 ORDER BY rowid DESC LIMIT 1", [source], |row| row.get(0))?)
    }).unwrap()
}

#[tokio::test]
async fn disposed_idle_shell_keeps_unknown_evidence_after_restart() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let host = Arc::new(Host::default());
    let runtime = runtime(&h, host.clone());
    let shell = {
        let r = runtime.clone();
        let id = root.clone();
        tokio::spawn(async move { r.shell(&id, "wait").await })
    };
    host.started.notified().await;
    shell.abort();
    assert!(shell.await.unwrap_err().is_cancelled());
    assert_eq!(latest_activity(&h, &root), "unknown");
    let report = h.restart().stop_subtree(&root).await.unwrap();
    assert_eq!(report.status, SubtreeStopStatus::Unknown);
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("Idle operation"))
    );
    assert!(h.restart().capture_child_admission(&root).is_err());
    runtime.shutdown().await;
}

#[tokio::test]
async fn missing_idle_acknowledgement_is_bounded_and_never_reopens_scope() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let host = Arc::new(Host::default());
    let runtime = runtime(&h, host.clone());
    let shell = {
        let r = runtime.clone();
        let id = root.clone();
        tokio::spawn(async move { r.shell(&id, "wait").await })
    };
    host.started.notified().await;
    let report = tokio::time::timeout(Duration::from_secs(3), runtime.stop_subtree(&root))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(report.status, SubtreeStopStatus::Unknown);
    assert!(report.persisted);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), shell)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(latest_activity(&h, &root), "unknown");
    assert!(h.restart().capture_child_admission(&root).is_err());
    runtime.shutdown().await;
}

#[tokio::test]
async fn interrupt_signals_idle_shell_and_waits_for_native_acknowledgement() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let host = Arc::new(Host::default());
    let runtime = runtime(&h, host.clone());
    let shell = {
        let r = runtime.clone();
        let id = root.clone();
        tokio::spawn(async move { r.shell(&id, "wait").await })
    };
    host.started.notified().await;
    let interrupt = {
        let r = runtime.clone();
        let id = root.clone();
        tokio::spawn(async move { r.interrupt(&id).await })
    };
    let cancelled =
        tokio::time::timeout(Duration::from_millis(500), host.cancelled.notified()).await;
    if cancelled.is_err() {
        shell.abort();
        interrupt.abort();
        panic!("Interrupt did not signal idle native cancellation");
    }
    assert!(
        !interrupt.is_finished(),
        "Interrupt returned before native acknowledgement"
    );
    host.release.notify_one();
    assert!(shell.await.unwrap().is_err());
    interrupt.await.unwrap().unwrap();
    runtime.shutdown().await;
}

#[tokio::test]
async fn subtree_stop_retains_worker_after_caller_disposal_until_idle_ack() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let host = Arc::new(Host::default());
    let runtime = runtime(&h, host.clone());
    let shell = {
        let r = runtime.clone();
        let id = root.clone();
        tokio::spawn(async move { r.shell(&id, "wait").await })
    };
    host.started.notified().await;
    let stop = {
        let r = runtime.clone();
        let id = root.clone();
        tokio::spawn(async move { r.stop_subtree(&id).await })
    };
    tokio::time::timeout(Duration::from_secs(1), host.cancelled.notified())
        .await
        .unwrap();
    assert!(!stop.is_finished());
    stop.abort();
    host.release.notify_one();
    assert!(shell.await.unwrap().is_err());
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let events = h.store.read_events(&root, -1, 100).unwrap().events;
            if let Some(event) = events
                .iter()
                .find(|e| e.kind == "session.subtree.stopped.1")
            {
                assert_eq!(event.data["status"], "acknowledged");
                assert_eq!(event.data["persisted"], true);
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(runtime.capture_child_admission(&root).is_err());
    runtime.shutdown().await;
}
