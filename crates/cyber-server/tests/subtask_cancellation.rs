//! Controlled host handoff races; native child execution is covered by tools tests.
mod support;
use cyber_server::runtime::*;
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use support::{Harness, Setup};
use tokio_util::sync::CancellationToken;

struct Host {
    runtime: Mutex<Option<WeakRuntime>>,
    owner: CancellationToken,
    cancel_before_return: bool,
    foreign_parent: bool,
}
impl ToolHost for Host {
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn execute(&self, _: Invocation, _: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async { panic!("No tools in this handoff fixture") })
    }
    fn subtask_request(
        &self,
        turn: TurnContext,
        _: UserSubtask,
        _: CancellationToken,
    ) -> BoxFuture<'_, Result<Job, String>> {
        Box::pin(async move {
            let runtime = self
                .runtime
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .upgrade()
                .unwrap();
            let source = runtime.state(&turn.session_id).await.unwrap().info;
            let parent = if self.foreign_parent {
                runtime
                    .create_session(CreateSession {
                        directory: source.directory.clone(),
                        model: source.model.clone(),
                        ..Default::default()
                    })
                    .await
                    .unwrap()
                    .id
            } else {
                source.id
            };
            let child = runtime
                .create_session(CreateSession {
                    directory: source.directory,
                    model: source.model,
                    parent_id: Some(parent.clone()),
                    ..Default::default()
                })
                .await
                .unwrap();
            let job = runtime
                .start_child_job(
                    &parent,
                    &child.id,
                    "late".into(),
                    "Handoff race".into(),
                    None,
                    Box::pin(futures::future::pending()),
                )
                .await
                .unwrap();
            if self.cancel_before_return {
                self.owner.cancel();
            }
            Ok(job)
        })
    }
}
fn fixture(cancel_before_return: bool, foreign_parent: bool) -> (Harness, Runtime, Arc<Host>) {
    let h = Harness::new(Setup::default());
    let host = Arc::new(Host {
        runtime: Mutex::new(None),
        owner: CancellationToken::new(),
        cancel_before_return,
        foreign_parent,
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
    (h, runtime, host)
}
fn request() -> UserSubtask {
    UserSubtask {
        prompt: "child".into(),
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

#[tokio::test]
async fn cancellation_racing_with_handoff_returns_the_settled_owned_job() {
    let (h, runtime, host) = fixture(true, false);
    let parent = source(&runtime, &h).await;
    let job = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        runtime.subtask_request_owned(&parent, request(), host.owner.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(job.session_id, parent);
    assert_eq!(job.status, JobStatus::Cancelled);
    assert_eq!(runtime.job(&job.id).unwrap().status, JobStatus::Cancelled);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_foreign_handoff_cannot_redirect_owned_cancellation() {
    let (h, runtime, host) = fixture(true, true);
    let parent = source(&runtime, &h).await;
    let error = runtime
        .subtask_request_owned(&parent, request(), host.owner.clone())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("ownership changed"));
    let jobs = runtime.jobs(None).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_ne!(jobs[0].session_id, parent);
    assert_eq!(jobs[0].status, JobStatus::Running);
    runtime.cancel_job(&jobs[0].id).await.unwrap();
    runtime.shutdown().await;
}

#[tokio::test]
async fn returned_job_transfers_cancellation_ownership_to_its_job_control() {
    let (h, runtime, host) = fixture(false, false);
    let parent = source(&runtime, &h).await;
    let job = runtime
        .subtask_request_owned(&parent, request(), host.owner.clone())
        .await
        .unwrap();
    host.owner.cancel();
    tokio::task::yield_now().await;
    assert_eq!(runtime.job(&job.id).unwrap().status, JobStatus::Running);
    assert_eq!(
        runtime.cancel_job(&job.id).await.unwrap().status,
        JobStatus::Cancelled
    );
    runtime.shutdown().await;
}
