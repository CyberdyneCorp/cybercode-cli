//! Cancellation must settle native preparation ownership before any new child input.
mod support;

use std::sync::Arc;
use std::time::Duration;

use cyber_server::runtime::*;
use futures::future::BoxFuture;
use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use support::{Harness, Setup, tools};

#[derive(Clone, Copy)]
enum Behavior {
    LateSuccess,
    Ignore,
    Panic,
}

struct Host {
    tools: Arc<support::Tools>,
    behavior: Behavior,
    started: Notify,
    cancelled: Notify,
    release: Notify,
}

impl ToolHost for Host {
    fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        self.tools.definitions(turn)
    }
    fn execute(&self, call: Invocation, cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        self.tools.execute(call, cancel)
    }
    fn prepare_child_continuation<'a>(
        &'a self,
        _parent: &'a SessionInfo,
        _child: &'a SessionState,
        _owner: &'a ChildExecution,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<Box<dyn ChildContinuation>>, String>> {
        Box::pin(async move {
            self.started.notify_one();
            match self.behavior {
                Behavior::Ignore => std::future::pending().await,
                Behavior::Panic => panic!("controlled preparation panic"),
                Behavior::LateSuccess => {
                    cancel.cancelled().await;
                    self.cancelled.notify_one();
                    self.release.notified().await;
                    Ok(None)
                }
            }
        })
    }
}

async fn fixture(behavior: Behavior) -> (Harness, Runtime, Arc<Host>, String, String) {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![tools(&[("first", "return_result", "1")])])],
        ..Default::default()
    });
    let host = Arc::new(Host {
        tools: h.tools.clone(),
        behavior,
        started: Notify::new(),
        cancelled: Notify::new(),
        release: Notify::new(),
    });
    let runtime = Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: h.models.clone(),
        tools: host.clone(),
        global_config_dir: h.dir.path().join("global"),
        shell: "bash".into(),
        claude_compat: true,
        compaction: Default::default(),
        retry: Default::default(),
        max_steps: None,
        today: None,
        interactive: true,
        snapshots: Arc::new(NoSnapshots),
    });
    let parent = runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let child = runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            output_schema: Some(StructuredSchema::new(json!({"type":"integer"})).unwrap()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    runtime
        .admit(&child, Admission::text("initial", Delivery::Queue))
        .await
        .unwrap();
    runtime.wait_idle(&child).await;
    (h, runtime, host, parent, child)
}

#[tokio::test]
async fn unacknowledged_or_panicked_preparation_refuses_dispatch_after_restart() {
    for behavior in [Behavior::Ignore, Behavior::Panic] {
        let (h, runtime, host, _, child) = fixture(behavior).await;
        let before = runtime.state(&child).await.unwrap();
        let running = runtime.clone();
        let id = child.clone();
        let request = tokio::spawn(async move {
            running
                .admit_user(&id, Admission::text("follow up", Delivery::Queue))
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), host.started.notified())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), runtime.interrupt(&child))
            .await
            .unwrap()
            .unwrap();
        assert!(request.await.unwrap().is_err());
        runtime.shutdown().await;
        let restored = h.restart();
        let state = restored.state(&child).await.unwrap();
        assert_eq!(state.inbox, before.inbox);
        assert_eq!(state.structured_result(), Some(&json!(1)));
        for result in [
            restored
                .admit_user(&child, Admission::text("retry", Delivery::Queue))
                .await
                .map(|_| ()),
            restored.wake(&child).await,
            restored.resume(&child).await,
        ] {
            assert!(result.unwrap_err().to_string().contains("recovery"));
        }
        assert_eq!(h.models.requests("test/main").len(), 1);
        let events = h.store.read_events(&child, -1, 200).unwrap().events;
        assert!(
            events
                .iter()
                .any(|event| event.kind == "session.child.continuation_settled.1"
                    && event.data["unknown"] == true
                    && event.data["phase"] == "preparation")
        );
    }
}

#[tokio::test]
async fn disposed_request_keeps_preparation_owned_until_late_acknowledgement() {
    let (h, runtime, host, parent, child) = fixture(Behavior::LateSuccess).await;
    let before = runtime.state(&child).await.unwrap();
    let running = runtime.clone();
    let id = child.clone();
    let request = tokio::spawn(async move {
        running
            .admit_user(&id, Admission::text("follow up", Delivery::Queue))
            .await
    });
    host.started.notified().await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(3), host.cancelled.notified())
        .await
        .unwrap();
    assert!(runtime.claim_child_execution(&parent, &child).is_err());
    let running = runtime.clone();
    let id = child.clone();
    let interruption = tokio::spawn(async move { running.interrupt(&id).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while runtime.state(&child).await.unwrap().last_seq == before.last_seq {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!interruption.is_finished());
    host.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5), interruption)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(runtime.state(&child).await.unwrap().inbox, before.inbox);
    assert_eq!(h.models.requests("test/main").len(), 1);
    assert!(runtime.claim_child_execution(&parent, &child).is_ok());
    runtime.shutdown().await;
}

#[tokio::test]
async fn late_preparation_success_cannot_release_held_input_after_interrupt() {
    let (h, runtime, host, parent, child) = fixture(Behavior::LateSuccess).await;
    let receipt = runtime
        .admit_user(&child, Admission::text("held follow up", Delivery::Hold))
        .await
        .unwrap();
    let before = runtime.state(&child).await.unwrap();
    let running = runtime.clone();
    let id = child.clone();
    let message = receipt.message_id.clone();
    let request =
        tokio::spawn(async move { running.release(&id, &message, Delivery::Queue).await });
    host.started.notified().await;
    let running = runtime.clone();
    let id = child.clone();
    let interruption = tokio::spawn(async move { running.interrupt(&id).await });
    host.cancelled.notified().await;
    assert!(runtime.claim_child_execution(&parent, &child).is_err());
    host.release.notify_one();
    assert!(request.await.unwrap().is_err());
    tokio::time::timeout(Duration::from_secs(5), interruption)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let state = runtime.state(&child).await.unwrap();
    assert_eq!(state.inbox, before.inbox);
    assert_eq!(state.structured_result(), Some(&json!(1)));
    assert_eq!(
        state.input(&receipt.message_id).unwrap().status,
        InputStatus::Held
    );
    assert_eq!(h.models.requests("test/main").len(), 1);
    runtime.shutdown().await;
}
