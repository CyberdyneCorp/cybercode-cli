mod support;

use cyber_server::runtime::*;
use futures::future::BoxFuture;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
use support::*;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Activity {
    active: AtomicUsize,
    unknown: AtomicBool,
    deny: AtomicBool,
    shell_unresponsive: AtomicBool,
    claim_unresponsive: AtomicBool,
}

struct Guard {
    activity: Arc<Activity>,
    settled: bool,
}

impl LocationGuard for Guard {
    fn settle(mut self: Box<Self>) -> Result<(), String> {
        self.settled = true;
        Ok(())
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.activity.active.fetch_sub(1, Ordering::SeqCst);
        if !self.settled {
            self.activity.unknown.store(true, Ordering::SeqCst);
        }
    }
}

struct Host {
    tools: Arc<Tools>,
    activity: Arc<Activity>,
}

impl ToolHost for Host {
    fn claim_location<'a>(
        &'a self,
        info: &'a SessionInfo,
        creating: bool,
        _: CancellationToken,
    ) -> BoxFuture<'a, Result<LocationLease, String>> {
        Box::pin(async move {
            if self.activity.deny.load(Ordering::SeqCst)
                || self.activity.unknown.load(Ordering::SeqCst)
            {
                return Err("checkout admission refused".into());
            }
            assert!(creating || info.worktree_id.as_deref() == Some("wt_test"));
            self.activity.active.fetch_add(1, Ordering::SeqCst);
            let lease = LocationLease::managed(
                "wt_test".into(),
                Box::new(Guard {
                    activity: Arc::clone(&self.activity),
                    settled: false,
                }),
            );
            if self.activity.claim_unresponsive.load(Ordering::SeqCst) {
                self.tools.started.notify_one();
                std::future::pending::<()>().await;
            }
            Ok(lease)
        })
    }

    fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        self.tools.definitions(turn)
    }
    fn execute(&self, call: Invocation, cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        self.tools.execute(call, cancel)
    }

    fn shell_owned(
        &self,
        _: &str,
        _: &str,
        _: &str,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            self.tools.started.notify_one();
            if self.activity.shell_unresponsive.load(Ordering::SeqCst) {
                return std::future::pending().await;
            }
            tokio::select! {
                _ = self.tools.release.notified() => Ok("shell completed".into()),
                _ = cancel.cancelled() => Err("shell cancelled and settled".into()),
            }
        })
    }
}

fn owned_runtime(h: &Harness) -> (Runtime, Arc<Activity>) {
    let activity = Arc::new(Activity::default());
    let runtime = Runtime::new(RuntimeOptions {
        store: Arc::clone(&h.store),
        resolver: Arc::clone(&h.models) as Arc<dyn ModelResolver>,
        tools: Arc::new(Host {
            tools: Arc::clone(&h.tools),
            activity: Arc::clone(&activity),
        }),
        global_config_dir: h.dir.path().join("global"),
        shell: "zsh".into(),
        claude_compat: true,
        compaction: CompactionConfig::default(),
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: Some("2026-10-03".into()),
        interactive: false,
        snapshots: Arc::new(NoSnapshots),
    });
    (runtime, activity)
}

async fn create(runtime: &Runtime, h: &Harness, model: &str) -> SessionInfo {
    runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: model.into(),
            ..Default::default()
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn idle_operations_refuse_location_admission_before_any_work() {
    let h = Harness::new(Setup::default());
    let (runtime, activity) = owned_runtime(&h);
    let info = create(&runtime, &h, "test/main").await;
    activity.deny.store(true, Ordering::SeqCst);
    let results = [
        runtime.shell(&info.id, "command").await.map(|_| ()),
        runtime.compact(&info.id, None).await,
        runtime.repair_context(&info.id).await,
        runtime
            .revert_stage(&info.id, "msg_missing", RevertTarget::Code)
            .await
            .map(|_| ()),
        runtime.revert_clear(&info.id).await,
    ];
    for result in results {
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("checkout admission refused")
        );
    }
    assert_eq!(activity.active.load(Ordering::SeqCst), 0);
    assert!(h.models.requests("test/main").is_empty());
}

#[tokio::test]
async fn setup_shutdown_settles_only_acknowledged_activity() {
    for unresponsive in [false, true] {
        let h = Harness::new(Setup::default());
        let (runtime, activity) = owned_runtime(&h);
        let info = create(&runtime, &h, "test/main").await;
        let worker = runtime.clone();
        let tools = Arc::clone(&h.tools);
        let cancel = CancellationToken::new();
        let job = tokio::spawn(async move {
            worker
                .own_session_worktree_setup(&info.id, cancel.clone(), async move {
                    tools.started.notify_one();
                    if unresponsive {
                        return std::future::pending::<std::io::Result<()>>().await;
                    }
                    cancel.cancelled().await;
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "setup acknowledged cancellation",
                    ))
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), h.tools.started.notified())
            .await
            .unwrap();
        assert_eq!(activity.active.load(Ordering::SeqCst), 1);
        tokio::time::timeout(Duration::from_secs(5), runtime.shutdown())
            .await
            .unwrap();
        assert_eq!(
            job.await.unwrap().unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        );
        assert_eq!(activity.active.load(Ordering::SeqCst), 0);
        assert_eq!(activity.unknown.load(Ordering::SeqCst), unresponsive);
    }
}

#[tokio::test]
async fn setup_caller_disposal_and_pending_claim_shutdown_preserve_unknown_activity() {
    for pending_claim in [false, true] {
        let h = Harness::new(Setup::default());
        let (runtime, activity) = owned_runtime(&h);
        let info = create(&runtime, &h, "test/main").await;
        activity
            .claim_unresponsive
            .store(pending_claim, Ordering::SeqCst);
        let worker = runtime.clone();
        let tools = Arc::clone(&h.tools);
        let job = tokio::spawn(async move {
            worker
                .own_session_worktree_setup(&info.id, CancellationToken::new(), async move {
                    tools.started.notify_one();
                    std::future::pending::<std::io::Result<()>>().await
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), h.tools.started.notified())
            .await
            .unwrap();
        assert_eq!(activity.active.load(Ordering::SeqCst), 1);
        if pending_claim {
            tokio::time::timeout(Duration::from_secs(5), runtime.shutdown())
                .await
                .unwrap();
            assert!(job.await.unwrap().is_err());
        } else {
            job.abort();
            assert!(job.await.unwrap_err().is_cancelled());
        }
        assert_eq!(activity.active.load(Ordering::SeqCst), 0);
        assert!(activity.unknown.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn idle_shell_holds_location_until_acknowledged_completion() {
    let h = Harness::new(Setup::default());
    let (runtime, activity) = owned_runtime(&h);
    let info = create(&runtime, &h, "test/main").await;
    let worker = runtime.clone();
    let id = info.id.clone();
    let job = tokio::spawn(async move { worker.shell(&id, "command").await });
    tokio::time::timeout(Duration::from_secs(5), h.tools.started.notified())
        .await
        .unwrap();
    assert_eq!(activity.active.load(Ordering::SeqCst), 1);
    h.tools.release.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), job)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "shell completed"
    );
    assert_eq!(activity.active.load(Ordering::SeqCst), 0);
    assert!(!activity.unknown.load(Ordering::SeqCst));
    assert_eq!(runtime.state(&info.id).await.unwrap().entries.len(), 1);
}

#[tokio::test]
async fn shutdown_settles_cooperative_shell_and_preserves_unacknowledged_activity() {
    for unresponsive in [false, true] {
        let h = Harness::new(Setup::default());
        let (runtime, activity) = owned_runtime(&h);
        let info = create(&runtime, &h, "test/main").await;
        activity
            .shell_unresponsive
            .store(unresponsive, Ordering::SeqCst);
        let worker = runtime.clone();
        let id = info.id.clone();
        let job = tokio::spawn(async move { worker.shell(&id, "command").await });
        tokio::time::timeout(Duration::from_secs(5), h.tools.started.notified())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), runtime.shutdown())
            .await
            .unwrap();
        assert!(job.await.unwrap().is_err());
        assert_eq!(activity.active.load(Ordering::SeqCst), 0);
        assert_eq!(activity.unknown.load(Ordering::SeqCst), unresponsive);
    }
}

#[tokio::test]
async fn shutdown_disposes_pending_location_admission_without_losing_its_guard() {
    let h = Harness::new(Setup::default());
    let (runtime, activity) = owned_runtime(&h);
    let info = create(&runtime, &h, "test/main").await;
    activity.claim_unresponsive.store(true, Ordering::SeqCst);
    let worker = runtime.clone();
    let id = info.id.clone();
    let job = tokio::spawn(async move { worker.shell(&id, "command").await });
    tokio::time::timeout(Duration::from_secs(5), h.tools.started.notified())
        .await
        .unwrap();
    assert_eq!(activity.active.load(Ordering::SeqCst), 1);
    tokio::time::timeout(Duration::from_secs(5), runtime.shutdown())
        .await
        .unwrap();
    assert!(matches!(
        job.await.unwrap(),
        Err(RuntimeError::ShuttingDown)
    ));
    assert_eq!(activity.active.load(Ordering::SeqCst), 0);
    assert!(activity.unknown.load(Ordering::SeqCst));
}

#[tokio::test]
async fn binding_survives_replay_and_fork_and_model_errors_settle_the_lease() {
    let h = Harness::new(Setup::default());
    let (runtime, activity) = owned_runtime(&h);
    let info = create(&runtime, &h, "missing/model").await;
    assert_eq!(info.worktree_id.as_deref(), Some("wt_test"));
    assert_eq!(activity.active.load(Ordering::SeqCst), 0);
    assert_eq!(
        runtime.fork(&info.id, None).await.unwrap().worktree_id,
        info.worktree_id
    );
    runtime
        .admit(&info.id, Admission::text("run", Delivery::Queue))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), runtime.wait_idle(&info.id))
        .await
        .unwrap();
    assert!(!activity.unknown.load(Ordering::SeqCst));
    assert_eq!(activity.active.load(Ordering::SeqCst), 0);
    let (restarted, _) = owned_runtime(&h);
    assert_eq!(
        restarted.state(&info.id).await.unwrap().info.worktree_id,
        info.worktree_id
    );
}

#[tokio::test]
async fn admission_refusal_precedes_prompt_promotion_and_model_access() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("must not run")])],
        ..Default::default()
    });
    let (runtime, activity) = owned_runtime(&h);
    let info = create(&runtime, &h, "test/main").await;
    activity.deny.store(true, Ordering::SeqCst);
    runtime
        .admit(&info.id, Admission::text("run", Delivery::Queue))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), runtime.wait_idle(&info.id))
        .await
        .unwrap();
    assert!(h.models.requests("test/main").is_empty());
    assert_eq!(
        runtime.state(&info.id).await.unwrap().inbox[0].status,
        InputStatus::Pending
    );
    assert_eq!(activity.active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn tool_panic_and_read_only_cancellation_timeout_retain_unknown_activity() {
    for panic in [true, false] {
        let h = Harness::new(Setup {
            scripts: vec![("test/main", vec![tools(&[("lease_call", "read", "{}")])])],
            ..Default::default()
        });
        h.tools.set(
            "read",
            if panic {
                Behavior::Panic
            } else {
                Behavior::Gated
            },
        );
        let (runtime, activity) = owned_runtime(&h);
        let info = create(&runtime, &h, "test/main").await;
        runtime
            .admit(&info.id, Admission::text("run", Delivery::Queue))
            .await
            .unwrap();
        if !panic {
            tokio::time::timeout(Duration::from_secs(5), h.tools.started.notified())
                .await
                .unwrap();
            assert_eq!(activity.active.load(Ordering::SeqCst), 1);
            runtime.interrupt(&info.id).await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), runtime.wait_idle(&info.id))
            .await
            .unwrap();
        assert!(activity.unknown.load(Ordering::SeqCst));
        assert_eq!(activity.active.load(Ordering::SeqCst), 0);
        if !panic {
            assert_eq!(
                runtime.state(&info.id).await.unwrap().calls["lease_call"].status,
                CallStatus::Interrupted
            );
        }
    }
}
