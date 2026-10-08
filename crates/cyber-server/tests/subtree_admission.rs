//! Durable closure of new admission while a subtree cancellation owns the scope.
mod support;
use cyber_server::runtime::*;
use cyber_store::{Expected, NewEvent};
use serde_json::json;
use support::{Harness, Setup};

async fn child(h: &Harness, parent: &str) -> String {
    h.runtime
        .create_session(CreateSession {
            parent_id: Some(parent.into()),
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}
fn close(h: &Harness, root: &str) {
    h.store
        .append(
            root,
            Expected::Any,
            vec![NewEvent::new(
                "session.admission.fenced.1",
                json!({"closed":true,"scope_id":"op_closed"}),
            )],
        )
        .unwrap();
}
fn prompt(text: &str) -> Admission {
    let mut admission = Admission::text(text, Delivery::Queue);
    admission.resume = false;
    admission
}

#[tokio::test]
async fn closed_ancestor_refuses_fresh_capture_after_restart_and_generic_fence() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let nested = child(&h, &root).await;
    close(&h, &root);
    assert!(matches!(
        h.runtime.capture_child_admission(&nested),
        Err(RuntimeError::Conflict(_))
    ));
    h.restart().fence_subtree_admissions(&root).await.unwrap();
    assert!(matches!(
        h.restart().capture_child_admission(&nested),
        Err(RuntimeError::Conflict(_))
    ));
}

#[tokio::test]
async fn closure_refuses_root_and_descendant_input_without_changing_inbox() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let nested = child(&h, &root).await;
    close(&h, &root);
    let runtime = h.restart();
    for id in [&root, &nested] {
        assert!(matches!(
            runtime.admit(id, prompt("late")).await,
            Err(RuntimeError::Conflict(_))
        ));
        assert!(runtime.state(id).await.unwrap().inbox.is_empty());
    }
}

#[tokio::test]
async fn writer_refuses_unbound_root_input_after_closure() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    h.runtime.admit(&root, prompt("original")).await.unwrap();
    let mut payload = h
        .store
        .read_events(&root, -1, 20)
        .unwrap()
        .events
        .into_iter()
        .find(|event| event.kind == "session.prompt.admitted.1")
        .unwrap()
        .data;
    payload["message_id"] = json!("msg_late");
    close(&h, &root);
    let seq = h.store.aggregate_seq(&root).unwrap().unwrap();
    assert!(
        h.store
            .append(
                &root,
                Expected::Seq(seq),
                vec![NewEvent::new("session.prompt.admitted.1", payload)]
            )
            .is_err()
    );
    assert_eq!(h.store.aggregate_seq(&root).unwrap().unwrap(), seq);
}

#[tokio::test]
async fn job_handoff_checks_closed_child_when_parent_is_outside_scope() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let nested = child(&h, &parent).await;
    close(&h, &nested);
    let result = h
        .runtime
        .start_child_job(
            &parent,
            &nested,
            "late".into(),
            "late".into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await;
    assert!(matches!(result, Err(RuntimeError::Conflict(_))));
    assert!(h.runtime.jobs(Some(&parent)).unwrap().is_empty());
}

#[tokio::test]
async fn writer_refuses_child_creation_even_without_captured_bindings() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let nested = child(&h, &root).await;
    let mut payload = h.store.read_events(&nested, -1, 1).unwrap().events[0]
        .data
        .clone();
    payload
        .as_object_mut()
        .unwrap()
        .remove("admission_bindings");
    let id = cyber_core::ids::new_id("ses");
    payload["info"]["id"] = json!(id);
    close(&h, &root);
    assert!(
        h.store
            .append(
                &id,
                Expected::Seq(-1),
                vec![NewEvent::new("session.created.1", payload)]
            )
            .is_err()
    );
    assert!(h.store.read_events(&id, -1, 1).unwrap().events.is_empty());
}

#[tokio::test]
async fn close_primitive_persists_identity_and_refuses_wake_resume_and_release() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let mut held = Admission::text("held", Delivery::Hold);
    held.resume = false;
    let receipt = h.runtime.admit(&root, held).await.unwrap();
    let scope = h.runtime.close_subtree_admissions(&root).await.unwrap();
    let events = h.store.read_events(&root, -1, 20).unwrap().events;
    let boundary = events.last().unwrap();
    assert_eq!(boundary.data["scope_id"], scope);
    assert_eq!(boundary.data["closed"], true);
    assert!(matches!(
        h.runtime.wake(&root).await,
        Err(RuntimeError::Conflict(_))
    ));
    assert!(matches!(
        h.runtime.resume(&root).await,
        Err(RuntimeError::Conflict(_))
    ));
    assert!(matches!(
        h.runtime
            .release(&root, &receipt.message_id, Delivery::Queue)
            .await,
        Err(RuntimeError::Conflict(_))
    ));
    assert!(matches!(
        h.runtime.compact(&root, None).await,
        Err(RuntimeError::Conflict(_))
    ));
    assert!(matches!(
        h.runtime.repair_context(&root).await,
        Err(RuntimeError::Conflict(_))
    ));
    assert!(!h.runtime.is_running(&root));
    assert_eq!(
        h.runtime
            .state(&root)
            .await
            .unwrap()
            .input(&receipt.message_id)
            .unwrap()
            .delivery,
        Delivery::Hold
    );
}

#[tokio::test]
async fn terminal_job_cancellation_stays_writable_after_closure() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let nested = child(&h, &root).await;
    let job = h
        .runtime
        .start_child_job(
            &root,
            &nested,
            "owned".into(),
            "owned".into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    h.runtime.close_subtree_admissions(&root).await.unwrap();
    assert_eq!(
        h.runtime.cancel_job(&job.id).await.unwrap().status,
        JobStatus::Cancelled
    );
}

use futures::future::BoxFuture;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;

struct LocationHost {
    wait_location: bool,
    location_entered: tokio::sync::Notify,
    location_release: tokio::sync::Notify,
    shell_entered: tokio::sync::Notify,
    shell_release: tokio::sync::Notify,
    shell_count: AtomicUsize,
}
impl ToolHost for LocationHost {
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn execute(&self, _: Invocation, _: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async { panic!("No inference in Location closure fixture") })
    }
    fn claim_location<'a>(
        &'a self,
        _: &'a SessionInfo,
        _: bool,
        _: CancellationToken,
    ) -> BoxFuture<'a, Result<LocationLease, String>> {
        Box::pin(async move {
            if self.wait_location {
                self.location_entered.notify_one();
                self.location_release.notified().await;
            }
            Ok(LocationLease::unmanaged())
        })
    }
    fn shell_owned(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: CancellationToken,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            self.shell_count.fetch_add(1, Ordering::SeqCst);
            self.shell_entered.notify_one();
            self.shell_release.notified().await;
            Ok("native receipt".into())
        })
    }
}
fn location_runtime(h: &Harness, wait_location: bool) -> (Runtime, Arc<LocationHost>) {
    let host = Arc::new(LocationHost {
        wait_location,
        location_entered: Default::default(),
        location_release: Default::default(),
        shell_entered: Default::default(),
        shell_release: Default::default(),
        shell_count: Default::default(),
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
        interactive: false,
        snapshots: Arc::new(NoSnapshots),
    });
    (runtime, host)
}
#[tokio::test]
async fn shell_waiting_for_location_cannot_dispatch_after_closure() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let (runtime, host) = location_runtime(&h, true);
    let task_runtime = runtime.clone();
    let task_root = root.clone();
    let task = tokio::spawn(async move { task_runtime.shell(&task_root, "late").await });
    host.location_entered.notified().await;
    runtime.close_subtree_admissions(&root).await.unwrap();
    host.location_release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(RuntimeError::Conflict(_))
    ));
    assert_eq!(host.shell_count.load(Ordering::SeqCst), 0);
    assert!(runtime.state(&root).await.unwrap().inbox.is_empty());
    runtime.shutdown().await;
}
#[tokio::test]
async fn started_shell_records_terminal_receipt_after_closure_without_new_work() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let (runtime, host) = location_runtime(&h, false);
    let task_runtime = runtime.clone();
    let task_root = root.clone();
    let task = tokio::spawn(async move { task_runtime.shell(&task_root, "started").await });
    host.shell_entered.notified().await;
    runtime.close_subtree_admissions(&root).await.unwrap();
    host.shell_release.notify_one();
    assert_eq!(task.await.unwrap().unwrap(), "native receipt");
    let state = runtime.state(&root).await.unwrap();
    assert_eq!(state.inbox.len(), 1);
    assert_eq!(state.inbox[0].status, InputStatus::Promoted);
    assert!(!runtime.is_running(&root));
    let mut spoofed = prompt("new shell input");
    spoofed.source = "shell".into();
    assert!(matches!(
        runtime.admit(&root, spoofed).await,
        Err(RuntimeError::Conflict(_))
    ));
    assert!(matches!(
        runtime.shell(&root, "new").await,
        Err(RuntimeError::Conflict(_))
    ));
    assert_eq!(host.shell_count.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;
}
