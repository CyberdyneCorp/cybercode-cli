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
        skill_command: None,
        admission_id: None,
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

struct DelayedLaunch {
    delay_location: bool,
    delay_after_job: bool,
    runtime: Mutex<Option<WeakRuntime>>,
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl ToolHost for DelayedLaunch {
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn execute(&self, _: Invocation, _: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async { panic!("Unexpected inference in delayed admission fixture") })
    }
    fn durable_user_delegation(&self) -> bool {
        true
    }
    fn claim_location<'a>(
        &'a self,
        info: &'a SessionInfo,
        _: bool,
        _: CancellationToken,
    ) -> BoxFuture<'a, Result<LocationLease, String>> {
        Box::pin(async move {
            if self.delay_location && info.parent_id.is_some() {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(LocationLease::unmanaged())
        })
    }

    fn subtask_request(
        &self,
        turn: TurnContext,
        request: UserSubtask,
        _: CancellationToken,
    ) -> BoxFuture<'_, Result<Job, String>> {
        Box::pin(async move {
            if !self.delay_after_job && !self.delay_location {
                self.started.notify_one();
                self.release.notified().await;
            }
            let runtime = self
                .runtime
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .upgrade()
                .unwrap();
            if let Some(id) = request.admission_id {
                runtime
                    .mark_delegation_launching(&turn.session_id, &id)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            let source = runtime
                .state(&turn.session_id)
                .await
                .map_err(|e| e.to_string())?
                .info;
            let child = runtime
                .create_session(CreateSession {
                    directory: source.directory,
                    model: source.model,
                    parent_id: Some(source.id.clone()),
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?;
            let job = runtime
                .start_child_job(
                    &source.id,
                    &child.id,
                    "late".into(),
                    "Delayed launch".into(),
                    None,
                    Box::pin(futures::future::pending()),
                )
                .await
                .map_err(|e| e.to_string())?;
            if self.delay_after_job {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(job)
        })
    }
}

#[tokio::test]
async fn delayed_user_callbacks_cannot_recapture_authority_after_cancellation() {
    for durable in [false, true] {
        for boundary in ["source", "subtree", "caller"] {
            let h = Harness::new(Setup::default());
            let host = Arc::new(DelayedLaunch {
                delay_after_job: false,
                delay_location: false,
                runtime: Mutex::new(None),
                started: Default::default(),
                release: Default::default(),
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
            *host.runtime.lock().unwrap() = Some(runtime.downgrade());
            let parent = source(&runtime, &h).await;
            let owner = CancellationToken::new();
            let task_owner = owner.clone();
            let task_runtime = runtime.clone();
            let task_parent = parent.clone();
            let task = if durable {
                runtime
                    .start_delegation(&parent, "op_delayed_boundary", request())
                    .await
                    .unwrap();
                None
            } else {
                Some(tokio::spawn(async move {
                    task_runtime
                        .subtask_request_owned(&task_parent, request(), task_owner)
                        .await
                }))
            };
            tokio::time::timeout(std::time::Duration::from_secs(2), host.started.notified())
                .await
                .unwrap();
            match boundary {
                "subtree" => runtime.fence_subtree_admissions(&parent).await.unwrap(),
                "caller" if durable => {
                    runtime
                        .cancel_delegation(&parent, "op_delayed_boundary")
                        .await
                        .unwrap();
                }
                "caller" => {
                    owner.cancel();
                    tokio::task::yield_now().await;
                }
                _ => runtime.interrupt(&parent).await.unwrap(),
            }
            host.release.notify_one();
            if let Some(task) = task {
                let error = task
                    .await
                    .unwrap()
                    .expect_err("Delayed callback must not regain launch authority");
                assert!(error.to_string().contains("fenced"), "{error}");
            } else {
                let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    loop {
                        let result = runtime
                            .delegation(&parent, "op_delayed_boundary")
                            .unwrap()
                            .unwrap();
                        if !matches!(
                            result.status,
                            DelegationStatus::Pending | DelegationStatus::Cancelling
                        ) {
                            break result;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                assert_eq!(result.status, DelegationStatus::Cancelled);
                assert_eq!(result.phase, DelegationPhase::Reserved);
                assert!(result.job_id.is_none());
            }
            assert!(runtime.jobs(Some(&parent)).unwrap().is_empty());
            assert_eq!(runtime.list(&Default::default()).unwrap().sessions.len(), 1);
            runtime.shutdown().await;
        }
    }
}

#[tokio::test]
async fn parent_interrupt_preserves_job_registered_before_delayed_delegation_reply() {
    let h = Harness::new(Setup::default());
    let host = Arc::new(DelayedLaunch {
        delay_after_job: true,
        delay_location: false,
        runtime: Mutex::new(None),
        started: Default::default(),
        release: Default::default(),
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
    *host.runtime.lock().unwrap() = Some(runtime.downgrade());
    let parent = source(&runtime, &h).await;
    runtime
        .start_delegation(&parent, "op_registered_reply", request())
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), host.started.notified())
        .await
        .unwrap();
    let job = runtime.jobs(Some(&parent)).unwrap().pop().unwrap();
    assert_eq!(job.status, JobStatus::Running);
    runtime.interrupt(&parent).await.unwrap();
    host.release.notify_one();
    let admission = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let admission = runtime
                .delegation(&parent, "op_registered_reply")
                .unwrap()
                .unwrap();
            if admission.status == DelegationStatus::Admitted {
                break admission;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(admission.job_id.as_deref(), Some(job.id.as_str()));
    assert_eq!(runtime.job(&job.id).unwrap().status, JobStatus::Running);
    runtime.cancel_job(&job.id).await.unwrap();
    runtime.shutdown().await;
}

#[tokio::test]
async fn durable_request_cancellation_fences_child_writer_after_location_preflight() {
    let h = Harness::new(Setup::default());
    let host = Arc::new(DelayedLaunch {
        delay_after_job: false,
        delay_location: true,
        runtime: Mutex::new(None),
        started: Default::default(),
        release: Default::default(),
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
    *host.runtime.lock().unwrap() = Some(runtime.downgrade());
    let parent = source(&runtime, &h).await;
    runtime
        .start_delegation(&parent, "op_location_cancel", request())
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), host.started.notified())
        .await
        .unwrap();
    runtime
        .cancel_delegation(&parent, "op_location_cancel")
        .await
        .unwrap();
    host.release.notify_one();
    let data = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let data = runtime
                .delegation(&parent, "op_location_cancel")
                .unwrap()
                .unwrap();
            if !matches!(
                data.status,
                DelegationStatus::Pending | DelegationStatus::Cancelling
            ) {
                break data;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(data.phase, DelegationPhase::Launching);
    assert_eq!(data.status, DelegationStatus::Unknown);
    assert!(data.job_id.is_none());
    assert!(runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert_eq!(
        runtime.list(&Default::default()).unwrap().sessions.len(),
        1,
        "Cancelled durable authority must roll back child creation after Location preflight"
    );
    runtime.shutdown().await;
}

struct AcknowledgingHost {
    entered: tokio::sync::Notify,
    cancelled: tokio::sync::Notify,
    release: tokio::sync::Notify,
    acknowledged: std::sync::atomic::AtomicBool,
}
impl ToolHost for AcknowledgingHost {
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn execute(&self, _: Invocation, _: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async { panic!("No inference in acknowledgement fixture") })
    }
    fn subtask_request(
        &self,
        _: TurnContext,
        _: UserSubtask,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Job, String>> {
        Box::pin(async move {
            self.entered.notify_one();
            cancel.cancelled().await;
            self.cancelled.notify_one();
            self.release.notified().await;
            self.acknowledged
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Err("Native cancellation acknowledged".into())
        })
    }
}

#[tokio::test]
async fn disposed_legacy_caller_retains_host_until_native_cancellation_acknowledges() {
    let h = Harness::new(Setup::default());
    let host = Arc::new(AcknowledgingHost {
        entered: Default::default(),
        cancelled: Default::default(),
        release: Default::default(),
        acknowledged: Default::default(),
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
    let parent = source(&runtime, &h).await;
    let task_runtime = runtime.clone();
    let task_parent = parent.clone();
    let task =
        tokio::spawn(async move { task_runtime.subtask_request(&task_parent, request()).await });
    host.entered.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(2), host.cancelled.notified())
        .await
        .expect("Disposed caller must leave the host running to acknowledge cancellation");
    assert!(!host.acknowledged.load(std::sync::atomic::Ordering::SeqCst));
    host.release.notify_one();
    runtime.shutdown().await;
    assert!(host.acknowledged.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn disposed_reply_stops_registered_job_before_handoff_acceptance() {
    let (h, runtime, host) = fixture(false, false);
    let parent = source(&runtime, &h).await;
    let mut caller =
        Box::pin(runtime.subtask_request_owned(&parent, request(), host.owner.clone()));
    assert!(futures::poll!(&mut caller).is_pending());
    let job = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(job) = runtime.jobs(Some(&parent)).unwrap().pop() {
                break job;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(caller);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if runtime.job(&job.id).unwrap().status == JobStatus::Cancelled {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    runtime.shutdown().await;
}

struct OwnerlessHandoff {
    job: Job,
    caller: CancellationToken,
}
impl ToolHost for OwnerlessHandoff {
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn execute(&self, _: Invocation, _: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async { panic!("No inference in ownerless handoff fixture") })
    }
    fn subtask_request(
        &self,
        _: TurnContext,
        _: UserSubtask,
        _: CancellationToken,
    ) -> BoxFuture<'_, Result<Job, String>> {
        Box::pin(async move {
            self.caller.cancel();
            Ok(self.job.clone())
        })
    }
}

#[tokio::test]
async fn cancelled_ownerless_handoff_records_unknown_and_refuses_success() {
    let (h, original, _) = fixture(false, false);
    let parent = source(&original, &h).await;
    let job = original.subtask_request(&parent, request()).await.unwrap();
    let caller = CancellationToken::new();
    // A second runtime can observe the durable Job but does not own its native actor.
    let runtime = Runtime::new(RuntimeOptions {
        store: h.store.clone(),
        resolver: h.models.clone(),
        tools: Arc::new(OwnerlessHandoff {
            job: job.clone(),
            caller: caller.clone(),
        }),
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
    let error = runtime
        .subtask_request_owned(&parent, request(), caller)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cancellation was not acknowledged")
    );
    let identity: String = h.store.read(|db| {
        Ok(db.query_row(
            "SELECT aggregate_id FROM event WHERE type='delegation.changed.1' ORDER BY rowid DESC LIMIT 1",
            [],
            |row| row.get(0),
        )?)
    }).unwrap();
    let record = runtime.delegation(&parent, &identity).unwrap().unwrap();
    assert_eq!(record.status, DelegationStatus::Unknown);
    assert_eq!(record.job_id.as_deref(), Some(job.id.as_str()));
    assert_eq!(runtime.job(&job.id).unwrap().status, JobStatus::Running);
    assert!(record.error.unwrap().contains("not acknowledged"));
    runtime.shutdown().await;
    original.cancel_job(&job.id).await.unwrap();
    original.shutdown().await;
}
