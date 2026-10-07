//! A missing admission actor never authorizes replay of native effects.
mod support;
use cyber_server::runtime::*;
use futures::future::BoxFuture;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use support::{Harness, Setup};
use tokio_util::sync::CancellationToken;
struct Host {
    runtime: Mutex<Option<WeakRuntime>>,
    entered: CancellationToken,
    calls: AtomicUsize,
    launching: bool,
    panic: bool,
}
impl ToolHost for Host {
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn execute(&self, _: Invocation, _: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async { panic!("No tools") })
    }
    fn durable_user_delegation(&self) -> bool {
        true
    }
    fn subtask_request(
        &self,
        turn: TurnContext,
        request: UserSubtask,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Job, String>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.launching {
                let runtime = self
                    .runtime
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .upgrade()
                    .unwrap();
                runtime
                    .mark_delegation_launching(
                        &turn.session_id,
                        request.admission_id.as_deref().unwrap(),
                    )
                    .await
                    .unwrap();
            }
            self.entered.cancel();
            assert!(!self.panic, "Controlled admission panic");
            cancel.cancelled().await;
            Err("Host settled without a Job response".into())
        })
    }
}
fn runtime(h: &Harness, launching: bool, panic: bool) -> (Runtime, Arc<Host>) {
    let host = Arc::new(Host {
        runtime: Mutex::new(None),
        entered: CancellationToken::new(),
        calls: AtomicUsize::new(0),
        launching,
        panic,
    });
    let runtime = Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: h.models.clone(),
        tools: host.clone(),
        global_config_dir: h.dir.path().join("global"),
        shell: "zsh".into(),
        claude_compat: true,
        compaction: CompactionConfig::default(),
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: Some("2026-10-07".into()),
        interactive: false,
        snapshots: Arc::new(NoSnapshots),
    });
    *host.runtime.lock().unwrap() = Some(runtime.downgrade());
    (runtime, host)
}
fn request() -> UserSubtask {
    UserSubtask {
        admission_id: None,
        prompt: "inspect project".into(),
        agent: None,
        attachments: vec![],
        max_steps: None,
    }
}
async fn source(runtime: &Runtime, h: &Harness) -> String {
    runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}
async fn settle(runtime: &Runtime, parent: &str, id: &str) -> Delegation {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let data = runtime.delegation(parent, id).unwrap().unwrap();
            if !matches!(
                data.status,
                DelegationStatus::Pending | DelegationStatus::Cancelling
            ) {
                return data;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn cancelled_launching_admission_remains_unknown_and_cannot_redispatch() {
    let h = Harness::new(Setup::default());
    let (runtime, host) = runtime(&h, true, false);
    let parent = source(&runtime, &h).await;
    runtime
        .start_delegation(&parent, "op_launch", request())
        .await
        .unwrap();
    host.entered.cancelled().await;
    runtime
        .cancel_delegation(&parent, "op_launch")
        .await
        .unwrap();
    let data = settle(&runtime, &parent, "op_launch").await;
    assert_eq!(data.status, DelegationStatus::Unknown);
    assert_eq!(data.phase, DelegationPhase::Launching);
    assert_eq!(
        runtime
            .start_delegation(&parent, "op_launch", request())
            .await
            .unwrap()
            .status,
        DelegationStatus::Unknown
    );
    assert_eq!(host.calls.load(Ordering::Relaxed), 1);
    runtime.shutdown().await;
}
#[tokio::test]
async fn lost_actor_after_restart_preserves_uncertainty_and_does_not_retain_runtime() {
    let h = Harness::new(Setup::default());
    let (first, host) = runtime(&h, false, false);
    let parent = source(&first, &h).await;
    first
        .start_delegation(&parent, "op_restart", request())
        .await
        .unwrap();
    host.entered.cancelled().await;
    let weak = first.downgrade();
    drop(first);
    assert!(
        weak.upgrade().is_none(),
        "Admission actor kept its runtime alive"
    );
    let (second, replacement) = runtime(&h, false, false);
    assert_eq!(
        second
            .delegation(&parent, "op_restart")
            .unwrap()
            .unwrap()
            .status,
        DelegationStatus::Unknown
    );
    assert_eq!(
        second
            .start_delegation(&parent, "op_restart", request())
            .await
            .unwrap()
            .status,
        DelegationStatus::Unknown
    );
    assert_eq!(replacement.calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        second
            .cancel_delegation(&parent, "op_restart")
            .await
            .unwrap()
            .status,
        DelegationStatus::Unknown
    );
    second.shutdown().await;
}
#[tokio::test]
async fn panicked_admission_is_unknown_instead_of_a_permanent_live_owner() {
    let h = Harness::new(Setup::default());
    let (runtime, host) = runtime(&h, false, true);
    let parent = source(&runtime, &h).await;
    runtime
        .start_delegation(&parent, "op_panic", request())
        .await
        .unwrap();
    host.entered.cancelled().await;
    assert_eq!(
        settle(&runtime, &parent, "op_panic").await.status,
        DelegationStatus::Unknown
    );
    assert_eq!(
        runtime
            .cancel_delegation(&parent, "op_panic")
            .await
            .unwrap()
            .status,
        DelegationStatus::Unknown
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn recorded_job_without_a_live_owner_cannot_acknowledge_cancellation() {
    use cyber_store::{Expected, NewEvent};
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let child = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let job = Job {
        id: "job_owner_lost".into(),
        session_id: parent.clone(),
        child_id: child.id,
        name: "lost".into(),
        kind: "subagent".into(),
        description: "Lost control fixture".into(),
        status: JobStatus::Running,
        started_ms: 1,
        ended_ms: None,
        exit_code: None,
        output_path: None,
        result: None,
        error: None,
        cost: 0.0,
        unpriced: false,
        tokens: 0,
        notified: false,
    };
    h.store
        .append(
            &parent,
            Expected::Any,
            vec![NewEvent::new(
                "job.started.1",
                serde_json::to_value(&job).unwrap(),
            )],
        )
        .unwrap();
    let data = Delegation {
        id: "op_owner_lost".into(),
        session_id: parent.clone(),
        status: DelegationStatus::Admitted,
        phase: DelegationPhase::Launching,
        job_id: Some(job.id.clone()),
        error: None,
    };
    h.store
        .append(
            &data.id,
            Expected::Seq(-1),
            vec![NewEvent::new(
                "delegation.changed.1",
                serde_json::json!({"data":data,"hash":"bound-input"}),
            )],
        )
        .unwrap();
    let stopped = h
        .runtime
        .cancel_delegation(&parent, &data.id)
        .await
        .unwrap();
    assert_eq!(stopped.status, DelegationStatus::Unknown);
    assert_eq!(stopped.job_id.as_deref(), Some(job.id.as_str()));
    assert!(
        stopped
            .error
            .unwrap()
            .contains("Cancellation was not acknowledged")
    );
    assert_eq!(h.runtime.job(&job.id).unwrap().status, JobStatus::Running);
    assert_eq!(
        h.runtime
            .delegation(&parent, &data.id)
            .unwrap()
            .unwrap()
            .status,
        DelegationStatus::Unknown
    );
}
