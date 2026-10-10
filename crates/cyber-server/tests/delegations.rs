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
    finish: CancellationToken,
    returned: CancellationToken,
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
            tokio::select! {_=cancel.cancelled()=>{},_=self.finish.cancelled()=>{}}
            self.returned.cancel();
            Err("Host settled without a Job response".into())
        })
    }
}
fn runtime(h: &Harness, launching: bool, panic: bool) -> (Runtime, Arc<Host>) {
    let host = Arc::new(Host {
        runtime: Mutex::new(None),
        entered: CancellationToken::new(),
        finish: CancellationToken::new(),
        returned: CancellationToken::new(),
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
        skill_command: None,
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
        DelegationStatus::Cancelled
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
        DelegationStatus::Cancelled
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

#[tokio::test]
async fn recovered_cancellation_fences_a_live_foreign_actor_and_preserves_input_binding() {
    let h = Harness::new(Setup::default());
    let (first, host) = runtime(&h, false, false);
    let parent = source(&first, &h).await;
    first
        .start_delegation(&parent, "op_fenced", request())
        .await
        .unwrap();
    host.entered.cancelled().await;
    let (second, replacement) = runtime(&h, false, false);
    let stopped = second
        .cancel_delegation(&parent, "op_fenced")
        .await
        .unwrap();
    assert_eq!(stopped.status, DelegationStatus::Cancelled);
    assert_eq!(stopped.phase, DelegationPhase::Reserved);
    assert!(stopped.job_id.is_none());
    assert!(
        first
            .mark_delegation_launching(&parent, "op_fenced")
            .await
            .is_err()
    );
    assert_eq!(
        first
            .start_delegation(&parent, "op_fenced", request())
            .await
            .unwrap()
            .status,
        DelegationStatus::Cancelled
    );
    let mut changed = request();
    changed.prompt = "different prompt".into();
    assert!(matches!(
        second.start_delegation(&parent, "op_fenced", changed).await,
        Err(RuntimeError::Conflict(_))
    ));
    assert_eq!(host.calls.load(Ordering::Relaxed), 1);
    assert_eq!(replacement.calls.load(Ordering::Relaxed), 0);
    host.finish.cancel();
    host.returned.cancelled().await;
    first.shutdown().await;
    assert_eq!(
        second
            .delegation(&parent, "op_fenced")
            .unwrap()
            .unwrap()
            .status,
        DelegationStatus::Cancelled
    );
    second.shutdown().await;
}

#[tokio::test]
async fn launching_foreign_actor_cannot_be_acknowledged_as_no_dispatch() {
    let h = Harness::new(Setup::default());
    let (first, host) = runtime(&h, true, false);
    let parent = source(&first, &h).await;
    first
        .start_delegation(&parent, "op_foreign_launch", request())
        .await
        .unwrap();
    host.entered.cancelled().await;
    let (second, replacement) = runtime(&h, false, false);
    let stopped = second
        .cancel_delegation(&parent, "op_foreign_launch")
        .await
        .unwrap();
    assert_eq!(stopped.status, DelegationStatus::Unknown);
    assert_eq!(stopped.phase, DelegationPhase::Launching);
    assert_eq!(
        second
            .start_delegation(&parent, "op_foreign_launch", request())
            .await
            .unwrap()
            .status,
        DelegationStatus::Unknown
    );
    assert_eq!(replacement.calls.load(Ordering::Relaxed), 0);
    first.shutdown().await;
    second.shutdown().await;
}

#[tokio::test]
async fn killed_admission_owner_worker() {
    let Some(db) = std::env::var_os("CYBER_DELEGATION_KILL_DB") else {
        return;
    };
    let mut h = Harness::new(Setup::default());
    h.store = Arc::new(support::open_store(std::path::Path::new(&db)));
    let (reserved, reserved_host) = runtime(&h, false, false);
    let parent = source(&reserved, &h).await;
    reserved
        .start_delegation(&parent, "op_killed_reserved", request())
        .await
        .unwrap();
    reserved_host.entered.cancelled().await;
    let (launching, launching_host) = runtime(&h, true, false);
    launching
        .start_delegation(&parent, "op_killed_launch", request())
        .await
        .unwrap();
    launching_host.entered.cancelled().await;
    let ready = std::path::PathBuf::from(std::env::var_os("CYBER_DELEGATION_KILL_READY").unwrap());
    let temporary = ready.with_extension("tmp");
    std::fs::write(&temporary, parent).unwrap();
    std::fs::rename(temporary, ready).unwrap();
    std::future::pending::<()>().await;
    drop((reserved, launching));
}

struct KilledOwner(std::process::Child);
impl Drop for KilledOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn kill_owner(
    db: &std::path::Path,
    ready: &std::path::Path,
    temporary: &std::path::Path,
) -> String {
    use std::process::{Command, Stdio};
    let mut owner = KilledOwner(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "killed_admission_owner_worker", "--nocapture"])
            .env("CYBER_DELEGATION_KILL_DB", db)
            .env("CYBER_DELEGATION_KILL_READY", ready)
            .env("TMPDIR", temporary)
            .env("TEMP", temporary)
            .env("TMP", temporary)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let parent = loop {
        if let Ok(parent) = std::fs::read_to_string(ready) {
            break parent;
        }
        if let Some(status) = owner.0.try_wait().unwrap() {
            use std::io::Read;
            let mut error = String::new();
            owner
                .0
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut error)
                .unwrap();
            panic!("Admission worker exited before durable readiness: {status}: {error}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Worker did not commit both admission phases"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    owner.0.kill().unwrap();
    let status = owner.0.wait().unwrap();
    assert!(!status.success());
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
    }
    parent
}

#[tokio::test]
async fn abrupt_process_death_recovers_only_the_proven_pre_effect_reservation() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("owner.db");
    let parent = kill_owner(&db, &dir.path().join("ready"), dir.path());
    let mut h = Harness::new(Setup::default());
    h.store = Arc::new(support::open_store(&db));
    let (recovered, host) = runtime(&h, false, false);
    for id in ["op_killed_reserved", "op_killed_launch"] {
        assert_eq!(
            recovered.delegation(&parent, id).unwrap().unwrap().status,
            DelegationStatus::Unknown
        );
        assert_eq!(
            recovered
                .start_delegation(&parent, id, request())
                .await
                .unwrap()
                .status,
            DelegationStatus::Unknown
        );
    }
    let cancelled = recovered
        .cancel_delegation(&parent, "op_killed_reserved")
        .await
        .unwrap();
    assert_eq!(cancelled.status, DelegationStatus::Cancelled);
    assert_eq!(cancelled.phase, DelegationPhase::Reserved);
    assert!(cancelled.job_id.is_none());
    assert!(
        recovered
            .mark_delegation_launching(&parent, "op_killed_reserved")
            .await
            .is_err()
    );
    assert_eq!(
        recovered
            .start_delegation(&parent, "op_killed_reserved", request())
            .await
            .unwrap()
            .status,
        DelegationStatus::Cancelled
    );
    let mut different = request();
    different.prompt = "changed after death".into();
    assert!(matches!(
        recovered
            .start_delegation(&parent, "op_killed_reserved", different)
            .await,
        Err(RuntimeError::Conflict(_))
    ));
    let launching = recovered
        .cancel_delegation(&parent, "op_killed_launch")
        .await
        .unwrap();
    assert_eq!(launching.status, DelegationStatus::Unknown);
    assert_eq!(launching.phase, DelegationPhase::Launching);
    assert!(launching.job_id.is_none());
    assert_eq!(host.calls.load(Ordering::Relaxed), 0);
    recovered.shutdown().await;
}

#[tokio::test]
async fn missing_admission_owner_cannot_advance_a_reserved_launch_marker() {
    let h = Harness::new(Setup::default());
    let (first, host) = runtime(&h, false, false);
    let parent = source(&first, &h).await;
    first
        .start_delegation(&parent, "op_missing_marker", request())
        .await
        .unwrap();
    host.entered.cancelled().await;
    drop(first);
    let (second, _) = runtime(&h, false, false);
    let error = second
        .mark_delegation_launching(&parent, "op_missing_marker")
        .await
        .expect_err("Missing ownership must not authorize a native launch marker");
    assert!(error.to_string().contains("unavailable"), "{error}");
    let record = second
        .delegation(&parent, "op_missing_marker")
        .unwrap()
        .unwrap();
    assert_eq!(record.status, DelegationStatus::Unknown);
    assert_eq!(record.phase, DelegationPhase::Reserved);
    second
        .cancel_delegation(&parent, "op_missing_marker")
        .await
        .unwrap();
    second.shutdown().await;
}
