//! Instrumented worker ownership; native journal/namespace proof is tested separately.
use super::super::{Admission, Delivery, RuntimeOptions, Snapshots, events};
use super::*;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

struct Guard {
    origin: Option<SnapshotOrigin>,
    acknowledgements: Arc<AtomicUsize>,
}
impl SnapshotNativeGuard for Guard {
    fn intent_digest(&self) -> Option<&str> {
        Some("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd")
    }
    fn verify(&self, origin: Option<&SnapshotOrigin>) -> Result<(), String> {
        if self.origin.as_ref() != origin {
            return Err("foreign worker receipt".into());
        }
        Ok(())
    }
    fn acknowledge(&mut self, origin: Option<&SnapshotOrigin>) -> Result<(), String> {
        self.verify(origin)?;
        self.acknowledgements.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn binding(directory: PathBuf) -> SnapshotBinding {
    SnapshotBinding {
        directory,
        root_identity: "instrumented_root".into(),
        namespace_identity: "instrumented_namespace".into(),
        journal_id: "restore_test".into(),
        journal_identity: "instrumented_journal".into(),
        lock_identity: Some("instrumented_lock".into()),
    }
}
async fn worker(runtime: &Runtime, session: &str) -> SnapshotWorker {
    let handle = runtime.inner.handle(session).await.unwrap();
    let state = handle.state.lock().await;
    runtime.snapshot_worker(&handle, &state).await.unwrap()
}
fn latest(runtime: &Runtime, session: &str) -> serde_json::Value {
    let session = session.to_owned();
    runtime.inner.store.read(move |db| {
        let data: String = db.query_row("SELECT data FROM event WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1 AND json_extract(data,'$.snapshot_worker')=1 ORDER BY rowid DESC LIMIT 1", [session], |row| row.get(0))?;
        Ok(serde_json::from_str(&data).unwrap())
    }).unwrap()
}

fn legacy_generic_unknown(runtime: &Runtime, handle: &Handle, session: &str) -> String {
    // Historical opaque evidence can coexist with a snapshot fence. This does
    // not launch new work through an already-fenced ordinary admission path.
    let id = cyber_core::ids::new_id("op");
    runtime.inner.store.append(&id, cyber_store::Expected::Seq(-1), vec![NewEvent::new(activity::CHANGED,
        serde_json::json!({"session_id":session,"status":"unknown","phase":"legacy_process_unknown"}))]).unwrap();
    handle.location_uncertain.store(true, Ordering::SeqCst);
    id
}
fn restart(runtime: &Runtime, root: &std::path::Path) -> Runtime {
    runtime_with_store(runtime, runtime.inner.store.clone(), root)
}
fn runtime_with_store(
    runtime: &Runtime,
    store: Arc<cyber_store::Store>,
    root: &std::path::Path,
) -> Runtime {
    runtime_with_snapshots(runtime, store, root, Arc::new(super::super::NoSnapshots))
}
fn runtime_with_snapshots(
    runtime: &Runtime,
    store: Arc<cyber_store::Store>,
    root: &std::path::Path,
    snapshots: Arc<dyn Snapshots>,
) -> Runtime {
    runtime_with_snapshot_tools(runtime, store, root, snapshots, runtime.inner.tools.clone())
}
fn runtime_with_snapshot_tools(
    runtime: &Runtime,
    store: Arc<cyber_store::Store>,
    root: &std::path::Path,
    snapshots: Arc<dyn Snapshots>,
    tools: Arc<dyn super::super::ToolHost>,
) -> Runtime {
    Runtime::new(RuntimeOptions {
        store,
        resolver: runtime.inner.resolver.clone(),
        tools,
        global_config_dir: root.join("config"),
        shell: "test".into(),
        claude_compat: false,
        compaction: Default::default(),
        retry: Default::default(),
        max_steps: None,
        today: None,
        interactive: false,
        snapshots,
    })
}

#[derive(Default)]
struct SnapshotClaims {
    ordinary_forbidden: std::sync::atomic::AtomicBool,
    claimed: std::sync::Mutex<Vec<String>>,
    reacquisition: Option<(Arc<AtomicUsize>, Arc<AtomicUsize>)>,
    reacquire_stop: bool,
    native_ack: Option<Arc<NativeAckFixture>>,
    acquired_settlements: Option<Arc<AtomicUsize>>,
    reacquire_inner: std::sync::Mutex<std::sync::Weak<super::super::Inner>>,
}
impl super::super::ToolHost for SnapshotClaims {
    fn definitions(&self, _: &super::super::TurnContext) -> Vec<super::super::ToolDef> {
        vec![]
    }
    fn execute(
        &self,
        _: super::super::Invocation,
        _: tokio_util::sync::CancellationToken,
    ) -> futures::future::BoxFuture<'_, super::super::ToolOutcome> {
        Box::pin(async { super::super::ToolOutcome::Failed("unused fixture tool".into()) })
    }
    fn reacquire_snapshot_location(
        &self,
        info: &super::super::SessionInfo,
        _: &super::super::SnapshotLifetimeBinding,
        _: &[cyber_core::worktrees::CheckoutBinding],
    ) -> Result<Box<dyn super::super::SnapshotLocationGuard>, String> {
        let (calls, fail_at) = self
            .reacquisition
            .as_ref()
            .ok_or("no reacquisition fixture")?;
        if self.reacquire_stop {
            let inner = self.reacquire_inner.lock().unwrap().upgrade().unwrap();
            let stop = inner
                .activities
                .lock()
                .unwrap()
                .values()
                .find(|control| control.source == info.id && control.snapshot_worker)
                .unwrap()
                .stop
                .clone();
            return Ok(Box::new(StopCheckoutProof {
                calls: calls.clone(),
                stop_at: fail_at.clone(),
                stop,
            }));
        }
        let proof = CheckoutProof {
            calls: calls.clone(),
            fail_at: fail_at.clone(),
        };
        match &self.native_ack {
            Some(state) => Ok(Box::new(ReceiptCheckoutProof {
                proof,
                state: state.clone(),
            })),
            None => Ok(Box::new(proof)),
        }
    }
    fn claim_location<'a>(
        &'a self,
        _: &'a super::super::SessionInfo,
        _: bool,
        _: tokio_util::sync::CancellationToken,
    ) -> futures::future::BoxFuture<'a, Result<super::super::LocationLease, String>> {
        Box::pin(async move {
            if self.ordinary_forbidden.load(Ordering::SeqCst) {
                return Err("Snapshot reused the ordinary Session claim".into());
            }
            Ok(super::super::LocationLease::unmanaged())
        })
    }
    fn claim_snapshot_location<'a>(
        &'a self,
        _: &'a super::super::SessionInfo,
        activity: &'a str,
        _: tokio_util::sync::CancellationToken,
    ) -> futures::future::BoxFuture<'a, Result<super::super::LocationLease, String>> {
        Box::pin(async move {
            self.claimed.lock().unwrap().push(activity.into());
            match &self.acquired_settlements {
                Some(settlements) => Ok(super::super::LocationLease::guarded(
                    None,
                    Box::new(BindingLease {
                        bindings: vec![],
                        settlements: settlements.clone(),
                    }),
                )),
                None => Ok(super::super::LocationLease::unmanaged()),
            }
        })
    }
}

#[tokio::test]
async fn snapshot_location_port_receives_the_reserved_native_activity_instead_of_the_session() {
    let (temp, base, _, _, _) = super::super::binary_documents_tests::fixture().await;
    let host = Arc::new(SnapshotClaims::default());
    let runtime = Runtime::new(RuntimeOptions {
        store: base.inner.store.clone(),
        resolver: base.inner.resolver.clone(),
        tools: host.clone(),
        global_config_dir: temp.path().join("config"),
        shell: "test".into(),
        claude_compat: false,
        compaction: Default::default(),
        retry: Default::default(),
        max_steps: None,
        today: None,
        interactive: false,
        snapshots: Arc::new(super::super::NoSnapshots),
    });
    let session = runtime
        .create_session(super::super::CreateSession {
            directory: temp.path().to_str().unwrap().into(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    host.ordinary_forbidden.store(true, Ordering::SeqCst);
    let owner = worker(&runtime, &session.id).await;
    assert_eq!(*host.claimed.lock().unwrap(), [owner.scope.id().to_owned()]);
    let record = latest(&runtime, &session.id);
    assert_eq!(record["snapshot_worker"], true);
    assert_eq!(record["session_id"], session.id);
    assert_eq!(record["phase"], "reserved");
    drop(owner);
    runtime.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn default_host_refuses_managed_snapshot_claim_without_explicit_support() {
    use super::super::ToolHost;
    let (_temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let mut info = runtime.session_info(&session).await.unwrap();
    info.worktree_id = Some("wt_fixture".into());
    let host = super::super::NoTools;
    assert!(
        host.claim_snapshot_location(&info, "op_fixture", Default::default())
            .await
            .is_err()
    );
    info.worktree_id = None;
    host.claim_snapshot_location(&info, "op_fixture", Default::default())
        .await
        .unwrap()
        .settle()
        .unwrap();
    runtime.shutdown().await;
}

struct Acknowledgements(Arc<AtomicUsize>);

struct BindingLease {
    bindings: Vec<cyber_core::worktrees::CheckoutBinding>,
    settlements: Arc<AtomicUsize>,
}
impl super::super::LocationGuard for BindingLease {
    fn settle(self: Box<Self>) -> Result<(), String> {
        self.settlements.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn settle_retained(self: Box<Self>) -> Result<Box<dyn Send>, String> {
        self.settlements.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(()))
    }
    fn snapshot_bindings(&self) -> Result<Vec<cyber_core::worktrees::CheckoutBinding>, String> {
        Ok(self.bindings.clone())
    }
    fn settle_snapshot_retained(
        self: Box<Self>,
    ) -> Result<Box<dyn super::super::SnapshotLocationGuard>, String> {
        self.settlements.fetch_add(1, Ordering::SeqCst);
        Ok(self)
    }
}
impl super::super::SnapshotLocationGuard for BindingLease {
    fn verify(&self) -> Result<(), String> {
        Ok(())
    }
    fn bindings(&self) -> Result<Vec<cyber_core::worktrees::CheckoutBinding>, String> {
        Ok(self.bindings.clone())
    }
}

#[tokio::test]
async fn checkout_lifetime_context_is_checked_before_any_native_settlement() {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            foreign: false,
            reject_resources: false,
            resources: Default::default(),
        }),
    );
    let owner = worker(&runtime, &session).await;
    let lifetime = owner.scope.snapshot_lifetime().unwrap();
    let context = lifetime.checkout_owner().unwrap();
    let mut changed = lifetime.clone();
    changed.file_identity.push_str("_replacement");
    assert_ne!(
        changed.checkout_owner().unwrap().lifetime_digest,
        context.lifetime_digest
    );
    changed.version = 1;
    assert!(changed.checkout_owner().is_err());
    let binding = cyber_core::worktrees::CheckoutBinding {
        version: 2,
        owner_id: owner.scope.id().into(),
        worktree_id: "wt_fixture".into(),
        name: "fixture".into(),
        common_dir: temp.path().to_path_buf(),
        common_identity: "fixture".into(),
        root_identity: "fixture".into(),
        directory_identity: "fixture".into(),
        file_identity: "fixture".into(),
        creation_directory_identity: "fixture".into(),
        creation_file_identity: "fixture".into(),
        creation_digest: "a".repeat(64),
        lifetime: Some(context),
        staging_name: Some("preparing_fixture".into()),
    };
    for case in [
        "valid",
        "activity",
        "creation",
        "digest",
        "missing",
        "version",
        "duplicate",
        "empty",
    ] {
        let mut bindings = vec![binding.clone()];
        match case {
            "activity" => bindings[0].owner_id = "op_foreign".into(),
            "creation" => {
                bindings[0].lifetime.as_mut().unwrap().session_created_id = "evt_foreign".into()
            }
            "digest" => bindings[0].lifetime.as_mut().unwrap().lifetime_digest = "b".repeat(64),
            "missing" => bindings[0].lifetime = None,
            "version" => bindings[0].version = 3,
            "duplicate" => bindings.push(binding.clone()),
            "empty" => bindings.clear(),
            _ => {}
        }
        let settlements = Arc::new(AtomicUsize::new(0));
        let lease = super::super::LocationLease::managed(
            "wt_fixture".into(),
            Box::new(BindingLease {
                bindings,
                settlements: settlements.clone(),
            }),
        );
        let result = SnapshotWorker::checkout_proof(&owner.scope, lease, Some("wt_fixture"));
        assert_eq!(result.is_ok(), case == "valid", "{case}");
        assert_eq!(
            settlements.load(Ordering::SeqCst),
            usize::from(case == "valid"),
            "{case}"
        );
    }
    drop(owner);
    runtime.shutdown().await;
    base.shutdown().await;
}
struct CheckoutProof {
    calls: Arc<AtomicUsize>,
    fail_at: Arc<AtomicUsize>,
}

#[derive(Default)]
struct NativeAckFixture {
    attempts: AtomicUsize,
    refuse_prepare: std::sync::atomic::AtomicBool,
    refuse_publish: std::sync::atomic::AtomicBool,
}
struct ReceiptCheckoutProof {
    proof: CheckoutProof,
    state: Arc<NativeAckFixture>,
}
impl super::super::SnapshotLocationGuard for ReceiptCheckoutProof {
    fn verify(&self) -> Result<(), String> {
        self.proof.verify()
    }
    fn prepare_preparation_acknowledgement(
        &mut self,
        receipt: &super::super::SnapshotPreparationReceipt,
    ) -> Result<(), String> {
        receipt.checkout_acknowledgement_digest()?;
        if self.state.refuse_prepare.load(Ordering::SeqCst) {
            return Err("foreign existing native receipt".into());
        }
        Ok(())
    }
    fn acknowledge_preparation(
        &mut self,
        receipt: &super::super::SnapshotPreparationReceipt,
    ) -> Result<(), String> {
        receipt.checkout_acknowledgement_digest()?;
        self.state.attempts.fetch_add(1, Ordering::SeqCst);
        if self.state.refuse_publish.load(Ordering::SeqCst) {
            return Err("native acknowledgement publication cut".into());
        }
        Ok(())
    }
}

struct StopCheckoutProof {
    calls: Arc<AtomicUsize>,
    stop_at: Arc<AtomicUsize>,
    stop: tokio_util::sync::CancellationToken,
}
impl super::super::SnapshotLocationGuard for StopCheckoutProof {
    fn verify(&self) -> Result<(), String> {
        if self.calls.fetch_add(1, Ordering::SeqCst) + 1 == self.stop_at.load(Ordering::SeqCst) {
            self.stop.cancel(); // Model Stop arriving during the writer's final native check.
        }
        Ok(())
    }
}

#[tokio::test]
async fn stop_during_writer_checkout_verification_refuses_consuming_acknowledgement() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let mut owner = worker(&runtime, &session).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let stop_at = Arc::new(AtomicUsize::new(usize::MAX));
    owner._location = Some(Box::new(StopCheckoutProof {
        calls: calls.clone(),
        stop_at: stop_at.clone(),
        stop: owner.scope.control.stop.clone(),
    }));
    let binding = binding(temp.path().canonicalize().unwrap());
    let acknowledgements = Arc::new(AtomicUsize::new(0));
    let acknowledged = acknowledgements.clone();
    let completed = owner
        .run(move |work| {
            let origin = work.begin(binding).unwrap();
            Ok(SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin),
                    acknowledgements: acknowledged,
                }),
            ))
        })
        .await
        .unwrap();
    stop_at.store(calls.load(Ordering::SeqCst) + 2, Ordering::SeqCst);
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut state = handle.state.lock().await;
    let before = state.last_seq;
    let result = completed.commit(
        &mut state,
        vec![events::event(
            events::TITLE_GENERATED,
            &events::Titled {
                title: "must not commit after Stop".into(),
                usage: None,
                cost: None,
                model: None,
            },
        )],
    );
    assert!(
        matches!(result, Err(RuntimeError::Conflict(ref reason)) if reason.contains("cancelled"))
    );
    assert_eq!(state.last_seq, before);
    assert_eq!(acknowledgements.load(Ordering::SeqCst), 0);
    drop(state);
    assert_eq!(latest(&runtime, &session)["status"], "unknown");
    runtime.shutdown().await;
}
impl super::super::SnapshotLocationGuard for CheckoutProof {
    fn verify(&self) -> Result<(), String> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call >= self.fail_at.load(Ordering::SeqCst) {
            return Err("checkout native route changed".into());
        }
        Ok(())
    }
}

#[tokio::test]
async fn changed_checkout_proof_refuses_the_source_callback() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let fail_at = Arc::new(AtomicUsize::new(usize::MAX));
    let mut owner = worker(&runtime, &session).await;
    owner._location = Some(Box::new(CheckoutProof {
        calls: calls.clone(),
        fail_at: fail_at.clone(),
    }));
    let source_calls = Arc::new(AtomicUsize::new(0));
    let source = source_calls.clone();
    let binding = binding(temp.path().canonicalize().unwrap());
    assert!(
        owner
            .run(move |work| {
                work.begin(binding).unwrap();
                fail_at.store(calls.load(Ordering::SeqCst) + 1, Ordering::SeqCst);
                assert!(
                    work.effect(|| {
                        source.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .is_err()
                );
                Err(RestoreError::Failed(
                    "changed checkout before source effect".into(),
                ))
            })
            .await
            .is_err()
    );
    assert_eq!(source_calls.load(Ordering::SeqCst), 0);
    assert_eq!(latest(&runtime, &session)["status"], "unknown");
    runtime.shutdown().await;
}

#[tokio::test]
async fn consuming_writer_verifies_retained_checkout_proof_and_rolls_back_both_owners_on_change() {
    for changed in [false, true] {
        let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let fail_at = Arc::new(AtomicUsize::new(usize::MAX));
        let mut owner = worker(&runtime, &session).await;
        owner._location = Some(Box::new(CheckoutProof {
            calls: calls.clone(),
            fail_at: fail_at.clone(),
        }));
        let binding = binding(temp.path().canonicalize().unwrap());
        let acknowledgements = Arc::new(AtomicUsize::new(0));
        let acknowledged = acknowledgements.clone();
        let completed = owner
            .run(move |work| {
                let origin = work.begin(binding).unwrap();
                Ok(SnapshotNativeResult::new(
                    vec![],
                    Box::new(Guard {
                        origin: Some(origin),
                        acknowledgements: acknowledged,
                    }),
                ))
            })
            .await
            .unwrap();
        let before_checks = calls.load(Ordering::SeqCst);
        if changed {
            fail_at.store(before_checks + 2, Ordering::SeqCst);
        }
        let handle = runtime.inner.handle(&session).await.unwrap();
        let mut state = handle.state.lock().await;
        let before = state.last_seq;
        let result = completed.commit(
            &mut state,
            vec![events::event(
                events::TITLE_GENERATED,
                &events::Titled {
                    title: "checkout consuming batch".into(),
                    usage: None,
                    cost: None,
                    model: None,
                },
            )],
        );
        assert_eq!(result.is_err(), changed);
        if changed {
            assert!(matches!(result, Err(RuntimeError::Conflict(_))));
        }
        assert_eq!(calls.load(Ordering::SeqCst), before_checks + 2);
        assert_eq!(state.last_seq, before + i64::from(!changed));
        assert_eq!(
            acknowledgements.load(Ordering::SeqCst),
            usize::from(!changed)
        );
        drop(state);
        assert_eq!(
            latest(&runtime, &session)["status"],
            if changed { "unknown" } else { "settled" }
        );
        runtime.shutdown().await;
    }
}

struct Lifetimes {
    inner: Acknowledgements,
    permits: Arc<tokio::sync::Semaphore>,
    foreign: bool,
    reject_resources: bool,
    resources: Arc<std::sync::Mutex<Option<Vec<cyber_core::worktrees::CheckoutBinding>>>>,
}
struct LifetimePermit {
    binding: super::super::SnapshotLifetimeBinding,
    _permit: tokio::sync::OwnedSemaphorePermit,
    reject_resources: bool,
    resources: Arc<std::sync::Mutex<Option<Vec<cyber_core::worktrees::CheckoutBinding>>>>,
}
impl super::super::SnapshotLifetime for LifetimePermit {
    fn bind_checkouts(
        &self,
        bindings: &[cyber_core::worktrees::CheckoutBinding],
    ) -> Result<(), String> {
        if self.reject_resources {
            return Err("injected resource publication failure".into());
        }
        if bindings.is_empty() {
            *self.resources.lock().unwrap() = Some(vec![]);
            Ok(())
        } else {
            Err("instrumented lifetime supports unmanaged resource fixtures only".into())
        }
    }
    fn checkout_bindings(
        &self,
    ) -> Result<Option<Vec<cyber_core::worktrees::CheckoutBinding>>, String> {
        Ok(self.resources.lock().unwrap().clone())
    }
    fn admit_source(&self, _: &SnapshotOrigin) -> Result<(), String> {
        Ok(())
    }
    fn binding(&self) -> &super::super::SnapshotLifetimeBinding {
        &self.binding
    }
    fn verify(&self) -> Result<(), String> {
        Ok(())
    }
    fn verify_preparation(&self, _: Option<&SnapshotOrigin>) -> Result<(), String> {
        Ok(())
    }
    fn prepare_preparation_acknowledgement(
        &self,
        _: &super::super::SnapshotPreparationReceipt,
    ) -> Result<(), String> {
        Ok(()) // Instrumented journal ownership; actual native receipts are tested in the backend.
    }
    fn acknowledge_preparation(
        &mut self,
        _: &super::super::SnapshotPreparationReceipt,
    ) -> Result<(), String> {
        Ok(())
    }
}
impl Snapshots for Lifetimes {
    fn lock_worker(
        &self,
        binding: super::super::SnapshotLifetimeBinding,
    ) -> futures::future::BoxFuture<'_, Result<Box<dyn super::super::SnapshotLifetime>, String>>
    {
        Box::pin(async move {
            let permit = self
                .permits
                .clone()
                .try_acquire_owned()
                .map_err(|error| error.to_string())?;
            Ok(Box::new(LifetimePermit {
                binding,
                reject_resources: self.reject_resources,
                resources: self.resources.clone(),
                _permit: permit,
            }) as Box<dyn super::super::SnapshotLifetime>)
        })
    }
    fn reserve_worker(
        &self,
        mut request: super::super::SnapshotLifetimeRequest,
    ) -> futures::future::BoxFuture<
        '_,
        Result<Option<Box<dyn super::super::SnapshotLifetime>>, String>,
    > {
        Box::pin(async move {
            let permit = self
                .permits
                .clone()
                .acquire_owned()
                .await
                .map_err(|error| error.to_string())?;
            if self.foreign {
                request.session_id = "ses_foreign".into();
            }
            Ok(Some(Box::new(LifetimePermit {
                binding: super::super::SnapshotLifetimeBinding {
                    version: 2,
                    owner: request,
                    root_identity: "instrumented_root".into(),
                    namespace_identity: "fixture-lifetime-namespace".into(),
                    directory_identity: "fixture-directory".into(),
                    file_identity: "fixture-file".into(),
                    source_phase: Some("phase_fixture".into()),
                },
                reject_resources: self.reject_resources,
                resources: self.resources.clone(),
                _permit: permit,
            })
                as Box<dyn super::super::SnapshotLifetime>))
        })
    }
    fn acknowledge_recovery(
        &self,
        directory: String,
        worker: super::super::SnapshotAcknowledgementWorker,
    ) -> futures::future::BoxFuture<'_, Result<(), String>> {
        self.inner.acknowledge_recovery(directory, worker)
    }
    fn track(
        &self,
        directory: &str,
    ) -> futures::future::BoxFuture<'_, Result<Option<super::super::Snapshot>, String>> {
        self.inner.track(directory)
    }
    fn changed(
        &self,
        directory: &str,
        before: &str,
        after: &str,
    ) -> futures::future::BoxFuture<'_, Result<Vec<String>, String>> {
        self.inner.changed(directory, before, after)
    }
    fn diff(
        &self,
        directory: &str,
        before: &str,
        after: &str,
    ) -> futures::future::BoxFuture<'_, Result<Vec<super::super::FileDiff>, String>> {
        self.inner.diff(directory, before, after)
    }
    fn restore(
        &self,
        directory: &str,
        before: &str,
        after: &str,
    ) -> futures::future::BoxFuture<'_, Result<Vec<String>, RestoreError>> {
        self.inner.restore(directory, before, after)
    }
}

fn crash_unstarted(owner: SnapshotWorker) -> String {
    let id = owner.scope.id().to_owned();
    let store = owner.runtime.inner.store.clone();
    let target = id.clone();
    store.transaction(move |tx| {
        tx.execute_batch("CREATE TEMP TABLE preparation_cut(owner TEXT); CREATE TEMP TRIGGER preparation_cut BEFORE INSERT ON event WHEN EXISTS(SELECT 1 FROM preparation_cut WHERE owner=NEW.aggregate_id) BEGIN SELECT RAISE(ABORT,'simulated abrupt preparation death'); END;")?;
        tx.execute("INSERT INTO preparation_cut(owner) VALUES (?1)", [target])?;
        Ok(())
    }).unwrap();
    drop(owner); // Suppress only Drop's record to model an abrupt process boundary.
    store
        .transaction(|tx| {
            tx.execute_batch("DROP TRIGGER preparation_cut; DROP TABLE preparation_cut;")?;
            Ok(())
        })
        .unwrap();
    id
}

#[tokio::test]
async fn published_origin_with_positive_false_admission_can_cancel_but_admitted_source_cannot() {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            foreign: false,
            reject_resources: false,
            resources: Default::default(),
        }),
    );
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let result = worker(&runtime, &session)
        .await
        .run(move |work| {
            work.begin(native_binding).unwrap();
            Err(RestoreError::Failed(
                "cut before intent or source admission".into(),
            ))
        })
        .await;
    assert!(result.is_err());
    let origin: SnapshotOrigin =
        serde_json::from_value(latest(&runtime, &session)["snapshot_origin"].clone()).unwrap();
    let review = runtime
        .snapshot_preparation_review(&session, &origin.activity_id)
        .await
        .unwrap();
    assert_eq!(review.origin, Some(origin.clone()));
    let receipt = runtime
        .snapshot_preparation_confirm(
            &session,
            &origin.activity_id,
            &super::super::SnapshotPreparationConfirmation {
                confirmation: review.confirmation,
                request_id: "cancel-before-source".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(receipt.source_admitted, Some(false));
    assert_eq!(receipt.origin, Some(origin));
    let admitted = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    assert!(
        runtime
            .snapshot_preparation_review(&session, &admitted.activity_id)
            .await
            .is_err()
    );
    assert_eq!(latest(&runtime, &session)["snapshot_source_admitted"], true);
    runtime.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn rejected_source_admission_prevents_the_callback_and_retains_positive_preparation() {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            foreign: false,
            reject_resources: false,
            resources: Default::default(),
        }),
    );
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    runtime
        .inner
        .store
        .set_append_fault(Some(Arc::new(|_, event| {
            event.data["phase"] == "snapshot_source_admitted"
        })));
    assert!(
        owner
            .run(move |work| {
                work.begin(native_binding).unwrap();
                assert!(
                    work.effect(|| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .is_err()
                );
                Err(RestoreError::Failed("admission rejected".into()))
            })
            .await
            .is_err()
    );
    runtime.inner.store.set_append_fault(None);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let record = latest(&runtime, &session);
    assert_eq!(record["snapshot_source_admitted"], false);
    let activity = record["snapshot_origin"]["activity_id"].as_str().unwrap();
    runtime
        .snapshot_preparation_review(&session, activity)
        .await
        .unwrap();
    runtime.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn lost_source_admission_reply_never_calls_the_source_and_never_becomes_false_proof() {
    fn lost_reply(_: &rusqlite::hooks::Wal, _: i32) -> rusqlite::Result<()> {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("lost source-admission reply".into()),
        ))
    }
    let (temp, base, _, _, _) = super::super::binary_documents_tests::fixture().await;
    let store = cyber_store::Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::File(temp.path().join("source-admission.db")),
        Runtime::registry(),
    ))
    .unwrap();
    let runtime = runtime_with_snapshots(
        &base,
        Arc::new(store),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            foreign: false,
            reject_resources: false,
            resources: Default::default(),
        }),
    );
    let session = runtime
        .create_session(super::super::CreateSession {
            directory: temp.path().to_str().unwrap().into(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let owner = worker(&runtime, &session).await;
    let id = owner.scope.id().to_owned();
    let binding = binding(temp.path().canonicalize().unwrap());
    let store = runtime.inner.store.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let effects = calls.clone();
    assert!(
        owner
            .run(move |work| {
                work.begin(binding).unwrap();
                store
                    .transaction(|tx| {
                        tx.wal_hook(Some(lost_reply));
                        Ok(())
                    })
                    .unwrap();
                assert!(
                    work.effect(|| {
                        effects.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .is_err()
                );
                Err(RestoreError::Failed(
                    "admission outcome requires journal recovery".into(),
                ))
            })
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let record = latest(&runtime, &session);
    assert_eq!(record["snapshot_source_admitted"], true);
    assert_eq!(record["phase"], "snapshot_source_admitted");
    assert!(
        runtime
            .snapshot_preparation_review(&session, &id)
            .await
            .is_err()
    );
    runtime.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn preparation_lost_commit_reply_is_lookup_only_after_reopening_the_real_store() {
    fn lost_reply(_: &rusqlite::hooks::Wal, _: i32) -> rusqlite::Result<()> {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("lost preparation reply".into()),
        ))
    }
    let (temp, base, _, _, _) = super::super::binary_documents_tests::fixture().await;
    let path = temp.path().join("preparation-pair.db");
    let store = cyber_store::Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::File(path.clone()),
        Runtime::registry(),
    ))
    .unwrap();
    let runtime = runtime_with_snapshots(
        &base,
        Arc::new(store),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            foreign: false,
            reject_resources: false,
            resources: Default::default(),
        }),
    );
    let session = runtime
        .create_session(super::super::CreateSession {
            directory: temp.path().to_str().unwrap().into(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let id = crash_unstarted(worker(&runtime, &session).await);
    let review = runtime
        .snapshot_preparation_review(&session, &id)
        .await
        .unwrap();
    let confirmation = super::super::SnapshotPreparationConfirmation {
        confirmation: review.confirmation,
        request_id: "lost-preparation".into(),
    };
    runtime
        .inner
        .store
        .transaction(|tx| {
            tx.wal_hook(Some(lost_reply));
            Ok(())
        })
        .unwrap();
    let notifications = runtime.inner.store.subscribe();
    assert!(
        runtime
            .snapshot_preparation_confirm(&session, &id, &confirmation)
            .await
            .is_err()
    );
    assert!(notifications.try_recv().is_err());
    let receipt = runtime
        .snapshot_preparation_lookup(&session, &id, &confirmation.request_id)
        .await
        .unwrap()
        .unwrap();
    runtime.shutdown().await;
    drop(runtime);
    let reopened = cyber_store::Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::File(path),
        Runtime::registry(),
    ))
    .unwrap();
    let reloaded = runtime_with_store(&base, Arc::new(reopened), temp.path());
    // NoSnapshots cannot reacquire any proof. This must use only the original pair.
    assert_eq!(
        reloaded
            .snapshot_preparation_confirm(&session, &id, &confirmation)
            .await
            .unwrap(),
        receipt
    );
    reloaded.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn preparation_cancellation_is_paired_and_retry_never_reopens_native_proof() {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: permits.clone(),
            foreign: false,
            reject_resources: false,
            resources: Default::default(),
        }),
    );
    let owner = worker(&runtime, &session).await;
    let id = owner.scope.id().to_owned();
    assert!(
        runtime
            .snapshot_preparation_review(&session, &id)
            .await
            .is_err()
    );
    assert_eq!(
        runtime.snapshot_preparations(&session).await.unwrap()[0].activity_id,
        id
    );
    crash_unstarted(owner);
    let handle = runtime.inner.handle(&session).await.unwrap();
    handle.mark_snapshot_uncertain(&id);
    handle.mark_snapshot_uncertain("op_other");
    legacy_generic_unknown(&runtime, &handle, &session);
    let before = runtime.state(&session).await.unwrap().last_seq;
    let review = runtime
        .snapshot_preparation_review(&session, &id)
        .await
        .unwrap();
    let confirmation = super::super::SnapshotPreparationConfirmation {
        confirmation: review.confirmation,
        request_id: "preparation-one".into(),
    };
    let receipt = runtime
        .snapshot_preparation_confirm(&session, &id, &confirmation)
        .await
        .unwrap();
    assert_eq!(receipt.lifetime, review.lifetime);
    assert_eq!(runtime.state(&session).await.unwrap().last_seq, before);
    assert!(handle.location_uncertain.load(Ordering::SeqCst));
    assert!(!handle.snapshot_uncertain.lock().unwrap().contains(&id));
    assert!(
        handle
            .snapshot_uncertain
            .lock()
            .unwrap()
            .contains("op_other")
    );
    permits.close();
    assert_eq!(
        runtime
            .snapshot_preparation_confirm(&session, &id, &confirmation)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        runtime
            .snapshot_preparation_lookup(&session, &id, &confirmation.request_id)
            .await
            .unwrap(),
        Some(receipt)
    );
    assert!(
        runtime
            .snapshot_preparation_confirm(&session, "op_other", &confirmation)
            .await
            .is_err()
    );
    let mut changed = confirmation;
    changed.confirmation = "0".repeat(64);
    assert!(
        runtime
            .snapshot_preparation_confirm(&session, &id, &changed)
            .await
            .is_err()
    );
    runtime.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn preparation_writer_failure_rolls_back_both_owners_and_changed_session_refuses_confirmation()
 {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: permits.clone(),
            foreign: false,
            reject_resources: false,
            resources: Default::default(),
        }),
    );
    let id = crash_unstarted(worker(&runtime, &session).await);
    let review = runtime
        .snapshot_preparation_review(&session, &id)
        .await
        .unwrap();
    let confirmation = super::super::SnapshotPreparationConfirmation {
        confirmation: review.confirmation,
        request_id: "preparation-failure".into(),
    };
    let notifications = runtime.inner.store.subscribe();
    runtime.inner.store.transaction(|tx| { tx.execute_batch("CREATE TEMP TRIGGER preparation_pair_failure BEFORE INSERT ON event WHEN NEW.type='snapshot.preparation.cancelled.1' BEGIN SELECT RAISE(ABORT,'preparation pair failure'); END;")?; Ok(()) }).unwrap();
    assert!(
        runtime
            .snapshot_preparation_confirm(&session, &id, &confirmation)
            .await
            .is_err()
    );
    runtime
        .inner
        .store
        .transaction(|tx| {
            tx.execute_batch("DROP TRIGGER preparation_pair_failure;")?;
            Ok(())
        })
        .unwrap();
    assert!(notifications.try_recv().is_err());
    assert_eq!(latest(&runtime, &session)["status"], "pending");
    assert!(
        runtime
            .snapshot_preparation_lookup(&session, &id, &confirmation.request_id)
            .await
            .unwrap()
            .is_none()
    );
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut state = handle.state.lock().await;
    runtime
        .inner
        .commit_locked(
            &mut state,
            vec![events::event(
                events::TITLE_GENERATED,
                &events::Titled {
                    title: "later revision".into(),
                    usage: None,
                    cost: None,
                    model: None,
                },
            )],
        )
        .unwrap();
    drop(state);
    assert!(
        runtime
            .snapshot_preparation_confirm(&session, &id, &confirmation)
            .await
            .is_err()
    );
    assert_eq!(permits.available_permits(), 1);
    runtime.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn cancelling_an_unstarted_resolution_keeps_its_original_failed_restore_fenced() {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: Arc::new(tokio::sync::Semaphore::new(2)),
            foreign: false,
            reject_resources: false,
            resources: Default::default(),
        }),
    );
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    assert!(
        runtime
            .snapshot_preparation_review(&session, &original.activity_id)
            .await
            .is_err()
    );
    let request = recovery_request(original.clone());
    let handle = runtime.inner.handle(&session).await.unwrap();
    let state = handle.state.lock().await;
    let owner = runtime
        .snapshot_recovery_worker(&handle, &state, request.clone())
        .await
        .unwrap();
    drop(state);
    let id = crash_unstarted(owner);
    let review = runtime
        .snapshot_preparation_review(&session, &id)
        .await
        .unwrap();
    runtime
        .snapshot_preparation_confirm(
            &session,
            &id,
            &super::super::SnapshotPreparationConfirmation {
                confirmation: review.confirmation,
                request_id: "cancel-resolution".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        runtime
            .snapshot_preparations(&session)
            .await
            .unwrap()
            .iter()
            .all(|candidate| candidate.activity_id != id)
    );
    let history = runtime
        .snapshot_preparations_with_history(&session, true)
        .await
        .unwrap();
    assert!(
        history
            .iter()
            .any(|candidate| candidate.activity_id == id && candidate.status == "settled")
    );
    let receipt = runtime
        .snapshot_preparation_receipt(&session, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.request_id, "cancel-resolution");
    assert_eq!(
        runtime
            .snapshot_preparation_lookup(&session, &id, &receipt.request_id)
            .await
            .unwrap(),
        Some(receipt)
    );
    assert!(
        runtime
            .snapshot_preparation_receipt(&session, "op_missing")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        runtime
            .snapshot_preparation_receipt("ses_foreign", &id)
            .await
            .is_err()
    );
    let discovered = runtime
        .snapshot_recovery_requests(&session, &original.binding.journal_id)
        .await
        .unwrap();
    assert_eq!(discovered, vec![request.confirmation.clone()]);
    assert!(
        runtime
            .snapshot_recovery_requests(&session, "restore_other")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        runtime
            .snapshot_recovery_requests(&session, "../restore_other")
            .await
            .is_err()
    );
    let lookup = runtime
        .snapshot_recovery_lookup(&session, &request.confirmation.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        lookup.state,
        super::super::SnapshotRecoveryState::CancelledBeforeOrigin
    );
    let source = original.activity_id.clone();
    let status: String = runtime.inner.store.read(move |db| Ok(db.query_row("SELECT json_extract(data,'$.status') FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1", [source], |row| row.get(0))?)).unwrap();
    assert_eq!(status, "unknown");
    let state = handle.state.lock().await;
    assert!(runtime.snapshot_worker(&handle, &state).await.is_err());
    drop(state);
    runtime.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn pre_origin_lifetime_is_bound_before_reservation_and_retained_through_request_disposal() {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: permits.clone(),
            foreign: false,
            reject_resources: false,
            resources: Default::default(),
        }),
    );
    let owner = worker(&runtime, &session).await;
    let id = owner.scope.id().to_owned();
    assert_eq!(permits.available_permits(), 0);
    let record = latest(&runtime, &session);
    assert!(record["snapshot_origin"].is_null());
    assert_eq!(record["snapshot_lifetime"]["owner"]["activity_id"], id);
    assert_eq!(record["snapshot_lifetime"]["owner"]["session_id"], session);
    let (started, running) = tokio::sync::oneshot::channel();
    let (finish, gate) = std::sync::mpsc::channel();
    let task = tokio::spawn(owner.run(move |work| {
        work.authorize().unwrap();
        started.send(()).unwrap();
        gate.recv().unwrap();
        assert!(work.effect(|| Ok(())).is_err());
        Err(RestoreError::Failed(
            "cancelled pre-origin preparation".into(),
        ))
    }));
    running.await.unwrap();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert_eq!(permits.available_permits(), 0);
    let mut shutdown = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.shutdown().await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut shutdown)
            .await
            .is_err()
    );
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(permits.available_permits(), 1);
    base.shutdown().await;
}

#[tokio::test]
async fn preparation_consuming_writer_revalidates_original_checkout_proof_and_rolls_back_both_owners()
 {
    for stop in [false, true] {
        let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let fail_at = Arc::new(AtomicUsize::new(usize::MAX));
        let host = Arc::new(SnapshotClaims {
            reacquisition: Some((calls.clone(), fail_at.clone())),
            reacquire_stop: stop,
            ..Default::default()
        });
        let runtime = runtime_with_snapshot_tools(
            &base,
            base.inner.store.clone(),
            temp.path(),
            Arc::new(Lifetimes {
                inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
                permits: Arc::new(tokio::sync::Semaphore::new(1)),
                foreign: false,
                reject_resources: false,
                resources: Default::default(),
            }),
            host.clone(),
        );
        *host.reacquire_inner.lock().unwrap() = Arc::downgrade(&runtime.inner);
        let id = crash_unstarted(worker(&runtime, &session).await);
        let review = runtime
            .snapshot_preparation_review(&session, &id)
            .await
            .unwrap();
        let before = calls.load(Ordering::SeqCst);
        fail_at.store(before + 2, Ordering::SeqCst); // factory succeeds; consuming writer refuses.
        let confirmation = super::super::SnapshotPreparationConfirmation {
            confirmation: review.confirmation,
            request_id: "checkout-writer-proof".into(),
        };
        assert!(
            runtime
                .snapshot_preparation_confirm(&session, &id, &confirmation)
                .await
                .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), before + 2);
        assert!(
            runtime
                .snapshot_preparation_lookup(&session, &id, &confirmation.request_id)
                .await
                .unwrap()
                .is_none()
        );
        assert_ne!(latest(&runtime, &session)["status"], "settled");
        fail_at.store(usize::MAX, Ordering::SeqCst);
        let receipt = runtime
            .snapshot_preparation_confirm(&session, &id, &confirmation)
            .await
            .unwrap();
        assert_eq!(
            receipt.checkout_resources_digest,
            review.checkout_resources_digest
        );
        runtime.shutdown().await;
        base.shutdown().await;
    }
}

#[tokio::test]
async fn preparation_native_ack_is_post_commit_and_only_explicit_reconciliation_retries_publication()
 {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let native = Arc::new(NativeAckFixture::default());
    let host = Arc::new(SnapshotClaims {
        reacquisition: Some((
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(usize::MAX)),
        )),
        native_ack: Some(native.clone()),
        ..Default::default()
    });
    let runtime = runtime_with_snapshot_tools(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            foreign: false,
            reject_resources: false,
            resources: Default::default(),
        }),
        host,
    );
    let id = crash_unstarted(worker(&runtime, &session).await);
    let review = runtime
        .snapshot_preparation_review(&session, &id)
        .await
        .unwrap();
    let confirmation = super::super::SnapshotPreparationConfirmation {
        confirmation: review.confirmation,
        request_id: "native-ack-cut".into(),
    };
    native.refuse_prepare.store(true, Ordering::SeqCst);
    assert!(
        runtime
            .snapshot_preparation_confirm(&session, &id, &confirmation)
            .await
            .is_err()
    );
    assert!(
        runtime
            .snapshot_preparation_lookup(&session, &id, &confirmation.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(native.attempts.load(Ordering::SeqCst), 0);
    native.refuse_prepare.store(false, Ordering::SeqCst);
    native.refuse_publish.store(true, Ordering::SeqCst);
    assert!(
        runtime
            .snapshot_preparation_confirm(&session, &id, &confirmation)
            .await
            .is_err()
    );
    let receipt = runtime
        .snapshot_preparation_lookup(&session, &id, &confirmation.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest(&runtime, &session)["status"], "settled");
    assert_eq!(native.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime
            .snapshot_preparation_confirm(&session, &id, &confirmation)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(native.attempts.load(Ordering::SeqCst), 1);
    native.refuse_publish.store(false, Ordering::SeqCst);
    assert_eq!(
        runtime
            .snapshot_preparation_acknowledge(&session, &id, &confirmation.request_id)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(native.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        runtime
            .snapshot_preparation_lookup(&session, &id, &confirmation.request_id)
            .await
            .unwrap(),
        Some(receipt)
    );
    assert_eq!(native.attempts.load(Ordering::SeqCst), 2);
    runtime.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn preparation_receipt_binds_resources_and_lookup_never_reopens_changed_native_evidence() {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let resources = Arc::new(std::sync::Mutex::new(None));
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            foreign: false,
            reject_resources: false,
            resources: resources.clone(),
        }),
    );
    let id = crash_unstarted(worker(&runtime, &session).await);
    let review = runtime
        .snapshot_preparation_review(&session, &id)
        .await
        .unwrap();
    let digest = review.lifetime.checkout_resources_digest(&[]).unwrap();
    assert_eq!(review.checkout_resources_digest.as_ref(), Some(&digest));
    let confirmation = super::super::SnapshotPreparationConfirmation {
        confirmation: review.confirmation,
        request_id: "resource-bound-cancel".into(),
    };
    *resources.lock().unwrap() = None;
    assert!(
        runtime
            .snapshot_preparation_confirm(&session, &id, &confirmation)
            .await
            .is_err()
    );
    assert!(
        runtime
            .snapshot_preparation_lookup(&session, &id, &confirmation.request_id)
            .await
            .unwrap()
            .is_none()
    );
    *resources.lock().unwrap() = Some(vec![]);
    let receipt = runtime
        .snapshot_preparation_confirm(&session, &id, &confirmation)
        .await
        .unwrap();
    assert_eq!(receipt.checkout_resources_digest, Some(digest));
    *resources.lock().unwrap() = None;
    assert_eq!(
        runtime
            .snapshot_preparation_lookup(&session, &id, &confirmation.request_id)
            .await
            .unwrap(),
        Some(receipt)
    );
    assert!(
        runtime
            .snapshot_preparation_acknowledge(&session, &id, &confirmation.request_id)
            .await
            .is_err()
    );
    runtime.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn complete_resource_publication_precedes_any_checkout_admission_settlement() {
    for reject in [false, true] {
        let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let settlements = Arc::new(AtomicUsize::new(0));
        let runtime = runtime_with_snapshot_tools(
            &base,
            base.inner.store.clone(),
            temp.path(),
            Arc::new(Lifetimes {
                inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
                permits: Arc::new(tokio::sync::Semaphore::new(1)),
                foreign: false,
                reject_resources: reject,
                resources: Default::default(),
            }),
            Arc::new(SnapshotClaims {
                acquired_settlements: Some(settlements.clone()),
                ..Default::default()
            }),
        );
        let handle = runtime.inner.handle(&session).await.unwrap();
        let state = handle.state.lock().await;
        let result = runtime.snapshot_worker(&handle, &state).await;
        assert_eq!(result.is_err(), reject);
        assert_eq!(settlements.load(Ordering::SeqCst), usize::from(!reject));
        drop(result);
        drop(state);
        runtime.shutdown().await;
        base.shutdown().await;
    }
}

#[tokio::test]
async fn resource_publication_failure_retains_unknown_reservation_without_origin_or_source_work() {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let acknowledgements = Arc::new(AtomicUsize::new(0));
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(acknowledgements.clone()),
            permits: permits.clone(),
            foreign: false,
            reject_resources: true,
            resources: Default::default(),
        }),
    );
    let handle = runtime.inner.handle(&session).await.unwrap();
    let state = handle.state.lock().await;
    let error = match runtime.snapshot_worker(&handle, &state).await {
        Ok(_) => panic!("worker admitted without durable resource publication"),
        Err(error) => error,
    };
    assert!(
        matches!(error, RuntimeError::Conflict(message) if message.contains("resource publication failure"))
    );
    drop(state);
    assert_eq!(permits.available_permits(), 1);
    assert!(!handle.snapshot_active.load(Ordering::SeqCst));
    let activity = latest(&runtime, &session);
    assert_eq!(activity["status"], "unknown");
    assert!(activity["snapshot_origin"].is_null());
    assert_ne!(activity["snapshot_source_admitted"], true);
    assert!(handle.has_location_uncertainty());
    assert!(!handle.location_uncertain.load(Ordering::SeqCst));
    assert_eq!(acknowledgements.load(Ordering::SeqCst), 0);
    runtime.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn foreign_lifetime_proof_is_rejected_before_any_native_reservation() {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: permits.clone(),
            foreign: true,
            reject_resources: false,
            resources: Default::default(),
        }),
    );
    let handle = runtime.inner.handle(&session).await.unwrap();
    let state = handle.state.lock().await;
    assert!(runtime.snapshot_worker(&handle, &state).await.is_err());
    drop(state);
    assert_eq!(permits.available_permits(), 1);
    assert!(!handle.snapshot_active.load(Ordering::SeqCst));
    let count: i64 = runtime
        .inner
        .store
        .read(|db| {
            Ok(db.query_row(
                "SELECT COUNT(*) FROM event WHERE type='native.activity.changed.1'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 0);
    runtime.shutdown().await;
    base.shutdown().await;
}
impl Snapshots for Acknowledgements {
    fn track(
        &self,
        _: &str,
    ) -> futures::future::BoxFuture<'_, Result<Option<super::super::Snapshot>, String>> {
        Box::pin(async { Ok(None) })
    }
    fn changed(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> futures::future::BoxFuture<'_, Result<Vec<String>, String>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn diff(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> futures::future::BoxFuture<'_, Result<Vec<super::super::FileDiff>, String>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn restore(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> futures::future::BoxFuture<'_, Result<Vec<String>, RestoreError>> {
        Box::pin(async { panic!("acknowledgement replayed a source restore") })
    }
    fn acknowledge_recovery(
        &self,
        _: String,
        worker: super::super::SnapshotAcknowledgementWorker,
    ) -> futures::future::BoxFuture<'_, Result<(), String>> {
        let count = self.0.clone();
        Box::pin(async move {
            worker
                .run(move |work| {
                    work.authorize()?;
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
        })
    }
}

#[tokio::test]
async fn ordinary_restore_lost_pair_reply_retains_exact_receipt_without_replaying_the_worker() {
    fn lost_reply(_: &rusqlite::hooks::Wal, _: i32) -> rusqlite::Result<()> {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("lost ordinary restore reply".into()),
        ))
    }
    let (temp, base, _, _, _) = super::super::binary_documents_tests::fixture().await;
    let path = temp.path().join("ordinary-owner.db");
    let store = cyber_store::Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::File(path.clone()),
        Runtime::registry(),
    ))
    .unwrap();
    let acknowledged = Arc::new(AtomicUsize::new(0));
    let runtime = runtime_with_snapshots(
        &base,
        Arc::new(store),
        temp.path(),
        Arc::new(Acknowledgements(acknowledged.clone())),
    );
    let session = runtime
        .create_session(super::super::CreateSession {
            directory: temp.path().to_str().unwrap().into(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let source_ack = Arc::new(AtomicUsize::new(0));
    let count = source_ack.clone();
    let binding = binding(temp.path().canonicalize().unwrap());
    let completed = worker(&runtime, &session)
        .await
        .run(move |work| {
            let origin = work.begin(binding).map_err(RestoreError::Failed)?;
            Ok(SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin),
                    acknowledgements: count,
                }),
            ))
        })
        .await
        .unwrap();
    let activity = completed.worker.scope.id().to_owned();
    runtime
        .inner
        .store
        .transaction(|db| {
            db.wal_hook(Some(lost_reply));
            Ok(())
        })
        .unwrap();
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut state = handle.state.lock().await;
    let before = state.last_seq;
    let notifications = runtime.inner.store.subscribe();
    assert!(
        completed
            .commit(
                &mut state,
                vec![events::event(
                    events::TITLE_GENERATED,
                    &events::Titled {
                        title: "durable restore batch".into(),
                        usage: None,
                        cost: None,
                        model: None
                    }
                )]
            )
            .is_err()
    );
    drop(state);
    assert!(notifications.try_recv().is_err());
    let receipt = runtime
        .snapshot_restore_lookup(&session, &activity)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.session_before, before);
    assert_eq!(receipt.session_after, before + 1);
    assert_eq!(source_ack.load(Ordering::SeqCst), 0);
    assert!(
        runtime
            .snapshot_restore_acknowledge(&session, &activity)
            .await
            .is_err()
    );
    assert_eq!(acknowledged.load(Ordering::SeqCst), 0);
    runtime.shutdown().await;
    drop(handle);
    drop(runtime);
    let reopened = cyber_store::Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::File(path.clone()),
        Runtime::registry(),
    ))
    .unwrap();
    let reloaded = runtime_with_snapshots(
        &base,
        Arc::new(reopened),
        temp.path(),
        Arc::new(Acknowledgements(acknowledged.clone())),
    );
    assert_eq!(
        reloaded
            .snapshot_restore_lookup(&session, &activity)
            .await
            .unwrap(),
        Some(receipt.clone())
    );
    let handle = reloaded.inner.handle(&session).await.unwrap();
    let mut state = handle.state.lock().await;
    reloaded
        .inner
        .commit_locked(
            &mut state,
            vec![events::event(
                events::TITLE_GENERATED,
                &events::Titled {
                    title: "later owner revision".into(),
                    usage: None,
                    cost: None,
                    model: None,
                },
            )],
        )
        .unwrap();
    drop(state);
    let reconciled = reloaded
        .snapshot_restore_acknowledge(&session, &activity)
        .await
        .unwrap();
    assert_eq!(reconciled, receipt);
    assert_eq!(acknowledged.load(Ordering::SeqCst), 1);
    assert_eq!(source_ack.load(Ordering::SeqCst), 0);
    reloaded.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn recovery_pair_lost_commit_reply_is_lookup_only_after_reopening_the_real_store() {
    fn lost_reply(_: &rusqlite::hooks::Wal, _: i32) -> rusqlite::Result<()> {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("injected recovery pair reply loss".into()),
        ))
    }
    let (temp, base, _, _, _) = super::super::binary_documents_tests::fixture().await;
    let path = temp.path().join("recovery-pair.db");
    let store = cyber_store::Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::File(path.clone()),
        Runtime::registry(),
    ))
    .unwrap();
    let acknowledged = Arc::new(AtomicUsize::new(0));
    let snapshots = Arc::new(Acknowledgements(acknowledged.clone()));
    let runtime = runtime_with_snapshots(&base, Arc::new(store), temp.path(), snapshots.clone());
    let session = runtime
        .create_session(super::super::CreateSession {
            directory: temp.path().to_str().unwrap().into(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    let request = recovery_request(original.clone());
    let handle = runtime.inner.handle(&session).await.unwrap();
    let state = handle.state.lock().await;
    let owner = runtime
        .snapshot_recovery_worker(&handle, &state, request.clone())
        .await
        .unwrap();
    drop(state);
    let count = Arc::new(AtomicUsize::new(0));
    let completed = finish_recovery(owner, request.clone(), count.clone()).await;
    legacy_generic_unknown(&runtime, &handle, &session);
    runtime
        .inner
        .store
        .transaction(|tx| {
            tx.wal_hook(Some(lost_reply));
            Ok(())
        })
        .unwrap();
    let notifications = runtime.inner.store.subscribe();
    let state = handle.state.lock().await;
    assert!(completed.commit_recovery(&state).is_err());
    drop(state);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(
        notifications.try_recv().is_err(),
        "unknown commit reply published a notification"
    );
    let original_record = runtime
        .inner
        .store
        .read_events(&original.activity_id, -1, 10)
        .unwrap();
    assert_eq!(
        original_record.events.last().unwrap().data["status"],
        "settled"
    );
    let lookup = runtime
        .snapshot_recovery_confirm(&session, &request.confirmation)
        .await
        .unwrap();
    assert_eq!(
        lookup.state,
        super::super::SnapshotRecoveryState::Acknowledged
    );
    assert_eq!(lookup.journal_acknowledged, None);
    let receipt = lookup.receipt.unwrap();
    assert!(
        handle
            .snapshot_uncertain
            .lock()
            .unwrap()
            .contains(&original.activity_id)
    );
    let reconciled = runtime
        .snapshot_recovery_acknowledge(&session, &request.confirmation.request_id)
        .await
        .unwrap();
    assert_eq!(reconciled.journal_acknowledged, Some(true));
    assert_eq!(acknowledged.load(Ordering::SeqCst), 1);
    assert!(handle.snapshot_uncertain.lock().unwrap().is_empty());
    assert!(
        handle.location_uncertain.load(Ordering::SeqCst),
        "reconciliation cleared unrelated generic uncertainty"
    );
    assert_eq!(
        count.load(Ordering::SeqCst),
        0,
        "reconciliation replayed the source worker"
    );
    runtime.shutdown().await;
    drop(handle);
    drop(runtime);
    let reopened = cyber_store::Store::open(cyber_store::StoreOptions::new(
        cyber_core::paths::DatabaseLocation::File(path),
        Runtime::registry(),
    ))
    .unwrap();
    let reloaded = runtime_with_snapshots(&base, Arc::new(reopened), temp.path(), snapshots);
    let lookup = reloaded
        .snapshot_recovery_confirm(&session, &request.confirmation)
        .await
        .unwrap();
    assert_eq!(lookup.receipt, Some(receipt));
    assert_eq!(
        lookup.state,
        super::super::SnapshotRecoveryState::Acknowledged
    );
    assert_eq!(
        count.load(Ordering::SeqCst),
        0,
        "lookup replayed native work"
    );
    reloaded.shutdown().await;
    base.shutdown().await;
}

async fn failed_snapshot(runtime: &Runtime, session: &str, directory: PathBuf) -> SnapshotOrigin {
    let owner = worker(runtime, session).await;
    assert!(
        owner
            .run(move |work| {
                work.begin(binding(directory)).unwrap();
                let _ = work.effect(|| Err::<(), String>("interrupted source operation".into()));
                Err(RestoreError::Failed("interrupted native restore".into()))
            })
            .await
            .is_err()
    );
    serde_json::from_value(latest(runtime, session)["snapshot_origin"].clone()).unwrap()
}

fn recovery_request(
    original: SnapshotOrigin,
) -> super::super::snapshot_recovery_types::RecoveryRequest {
    super::super::snapshot_recovery_types::RecoveryRequest {
        confirmation: super::super::SnapshotRecoveryConfirmation {
            journal_id: original.binding.journal_id.clone(),
            action: super::super::SnapshotRecoveryAction::Keep,
            confirmation: "a".repeat(64),
            request_id: "owned-recovery-request".into(),
        },
        original,
        authority: None,
    }
}

async fn finish_recovery(
    owner: SnapshotWorker,
    request: super::super::snapshot_recovery_types::RecoveryRequest,
    count: Arc<AtomicUsize>,
) -> SnapshotCompletion {
    owner
        .run(move |work| {
            let mut native_binding = request.original.binding.clone();
            native_binding.journal_id = "restore_resolution_owned".into();
            native_binding.journal_identity = "resolution_journal_identity".into();
            let origin = work.begin(native_binding).unwrap();
            work.effect(|| Ok(())).unwrap();
            let mut native = SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin.clone()),
                    acknowledgements: count,
                }),
            );
            native.recovery = Some(super::super::SnapshotRecoveryReceipt {
                journal_id: request.confirmation.journal_id,
                resolution_id: origin.binding.journal_id,
                request_id: request.confirmation.request_id,
                action: request.confirmation.action,
                confirmation: request.confirmation.confirmation,
                original: request.original,
                filesystem_complete: true,
                resolution_digest: Some("d".repeat(64)),
                checkout: None,
            });
            Ok(native)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn acknowledgement_cannot_adopt_an_uncommitted_filesystem_receipt() {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let count = Arc::new(AtomicUsize::new(0));
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Acknowledgements(count.clone())),
    );
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    assert!(
        runtime
            .snapshot_restore_lookup(&session, &original.activity_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        runtime
            .snapshot_restore_acknowledge(&session, &original.activity_id)
            .await
            .is_err()
    );
    let request = recovery_request(original);
    let handle = runtime.inner.handle(&session).await.unwrap();
    let state = handle.state.lock().await;
    let owner = runtime
        .snapshot_recovery_worker(&handle, &state, request.clone())
        .await
        .unwrap();
    drop(state);
    let completed = finish_recovery(owner, request.clone(), Arc::new(AtomicUsize::new(0))).await;
    drop(completed);
    assert!(
        runtime
            .snapshot_recovery_acknowledge(&session, &request.confirmation.request_id)
            .await
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(handle.has_location_uncertainty());
    runtime.shutdown().await;
    base.shutdown().await;
}

#[tokio::test]
async fn disposed_acknowledgement_request_retains_actual_worker_resources_through_shutdown() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    let request = recovery_request(original);
    let handle = runtime.inner.handle(&session).await.unwrap();
    let state = handle.state.lock().await;
    let owner = runtime
        .snapshot_recovery_worker(&handle, &state, request.clone())
        .await
        .unwrap();
    drop(state);
    let completed = finish_recovery(owner, request.clone(), Arc::new(AtomicUsize::new(0))).await;
    let origin = completed.worker.scope.snapshot_origin().unwrap().clone();
    let state = handle.state.lock().await;
    let receipt = completed.commit_recovery(&state).unwrap();
    let worker = runtime
        .snapshot_acknowledgement_worker(&handle, &state, receipt, origin)
        .await
        .unwrap();
    drop(state);
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = permits.clone().acquire_owned().await.unwrap();
    let (started, running) = tokio::sync::oneshot::channel();
    let (finish, gate) = std::sync::mpsc::channel();
    let task = tokio::spawn(worker.run(move |work| {
        let _permit = permit;
        work.authorize()?;
        started.send(()).unwrap();
        gate.recv().unwrap();
        assert!(work.authorize().is_err());
        Err("cancelled metadata worker".into())
    }));
    running.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(permits.available_permits(), 0);
    assert!(handle.snapshot_active.load(Ordering::SeqCst));
    let mut shutdown = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.shutdown().await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut shutdown)
            .await
            .is_err()
    );
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(permits.available_permits(), 1);
    assert!(!handle.snapshot_active.load(Ordering::SeqCst));
}

#[tokio::test]
async fn writer_snapshot_fence_rejects_input_even_when_the_callers_precheck_is_stale() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let admitted = runtime
        .admit(&session, Admission::text("held seed", Delivery::Hold))
        .await
        .unwrap();
    let source = session.clone();
    let mut payload: serde_json::Value = runtime.inner.store.read(move |db| {
        let data: String = db.query_row("SELECT data FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq DESC LIMIT 1", [&source, events::ADMITTED], |row| row.get(0))?;
        Ok(serde_json::from_str(&data).unwrap())
    }).unwrap();
    payload["message_id"] = "msg_stale_admission".into();
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    let before = runtime.inner.store.aggregate_seq(&session).unwrap();
    let failed = runtime.inner.store.append(
        &session,
        cyber_store::Expected::Seq(before.unwrap()),
        vec![NewEvent::new(events::ADMITTED, payload)],
    );
    assert!(
        matches!(failed, Err(cyber_store::StoreError::Projector { reason, .. }) if reason == super::super::snapshot_recovery_record::FENCED)
    );
    assert_eq!(runtime.inner.store.aggregate_seq(&session).unwrap(), before);
    let state = runtime.state(&session).await.unwrap();
    assert!(state.input(&admitted.message_id).is_some());
    assert!(state.input("msg_stale_admission").is_none());
    assert_eq!(
        runtime
            .inner
            .store
            .aggregate_seq(&original.activity_id)
            .unwrap(),
        Some(2)
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn ordinary_snapshot_reservation_cannot_race_a_pending_generic_native_owner() {
    let (_temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut generic = activity::Scope::reserve(&runtime.inner, &handle)
        .await
        .unwrap();
    let source = session.clone();
    let events_before: i64 = runtime.inner.store.read(move |db| Ok(db.query_row("SELECT count(*) FROM event WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1", [source], |row| row.get(0))?)).unwrap();
    let state = handle.state.lock().await;
    assert!(runtime.snapshot_worker(&handle, &state).await.is_err());
    drop(state);
    assert!(!handle.snapshot_active.load(Ordering::SeqCst));
    let source = session.clone();
    let events_after: i64 = runtime.inner.store.read(move |db| Ok(db.query_row("SELECT count(*) FROM event WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1", [source], |row| row.get(0))?)).unwrap();
    assert_eq!(
        events_after, events_before,
        "failed admission published native ownership"
    );
    generic.launch().unwrap();
    generic.settle().unwrap();
    drop(generic);
    runtime.shutdown().await;
}

#[tokio::test]
async fn recovery_preserves_closed_admission_and_acknowledges_only_its_exact_native_pair() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    let handle = runtime.inner.handle(&session).await.unwrap();
    legacy_generic_unknown(&runtime, &handle, &session);
    let boundary = runtime.close_subtree_admissions(&session).await.unwrap();
    assert!(runtime.capture_child_admission(&session).is_err());
    let request = recovery_request(original.clone());
    let state = handle.state.lock().await;
    let sequence = state.last_seq;
    let owner = runtime
        .snapshot_recovery_worker(&handle, &state, request.clone())
        .await
        .unwrap();
    assert!(owner.scope.authority().is_err());
    drop(state);
    let reserved = runtime
        .snapshot_recovery_lookup(&session, &request.confirmation.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reserved.state, super::super::SnapshotRecoveryState::Pending);
    assert!(reserved.resolution.is_none() && reserved.receipt.is_none());
    let count = Arc::new(AtomicUsize::new(0));
    let completed = finish_recovery(owner, request.clone(), count.clone()).await;
    let state = handle.state.lock().await;
    completed.commit_recovery(&state).unwrap();
    assert_eq!(state.last_seq, sequence, "recovery moved the transcript");
    drop(state);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(
        handle.location_uncertain.load(Ordering::SeqCst),
        "generic unknown outcome was cleared"
    );
    assert!(
        !handle
            .snapshot_uncertain
            .lock()
            .unwrap()
            .contains(&original.activity_id)
    );
    assert!(runtime.capture_child_admission(&session).is_err());
    let source = session.clone();
    assert_eq!(
        runtime
            .inner
            .store
            .read(move |db| super::super::subtree_reopen::active_scope(db, &source))
            .unwrap(),
        Some(boundary)
    );
    let lookup = runtime
        .snapshot_recovery_confirm(&session, &request.confirmation)
        .await
        .unwrap();
    assert_eq!(
        lookup.state,
        super::super::SnapshotRecoveryState::Acknowledged
    );
    assert!(lookup.receipt.is_some());
    assert_eq!(lookup.journal_acknowledged, None);
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "retry replayed native acknowledgement"
    );
    let mut foreign = request.confirmation;
    foreign.journal_id = "restore_foreign".into();
    assert!(
        runtime
            .snapshot_recovery_confirm(&session, &foreign)
            .await
            .is_err()
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_new_stop_boundary_revokes_recovery_before_native_origin_or_effects() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    let request = recovery_request(original);
    let handle = runtime.inner.handle(&session).await.unwrap();
    let state = handle.state.lock().await;
    let owner = runtime
        .snapshot_recovery_worker(&handle, &state, request.clone())
        .await
        .unwrap();
    drop(state);
    runtime.fence_subtree_admissions(&session).await.unwrap();
    let native_binding = binding(temp.path().canonicalize().unwrap());
    assert!(
        owner
            .run(move |work| {
                assert!(work.authorize().is_err());
                assert!(work.begin(native_binding).is_err());
                assert!(
                    work.effect::<()>(|| panic!("revoked recovery changed source files"))
                        .is_err()
                );
                Err(RestoreError::Failed("recovery revoked".into()))
            })
            .await
            .is_err()
    );
    let lookup = runtime
        .snapshot_recovery_confirm(&session, &request.confirmation)
        .await
        .unwrap();
    assert_eq!(lookup.state, super::super::SnapshotRecoveryState::Unknown);
    assert!(lookup.resolution.is_none() && lookup.receipt.is_none());
    assert!(handle.has_location_uncertainty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn changed_session_rejects_recovery_pair_and_retry_only_looks_up_unknown_request() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let original = failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
    let request = recovery_request(original.clone());
    let handle = runtime.inner.handle(&session).await.unwrap();
    let state = handle.state.lock().await;
    let owner = runtime
        .snapshot_recovery_worker(&handle, &state, request.clone())
        .await
        .unwrap();
    drop(state);
    let count = Arc::new(AtomicUsize::new(0));
    let completed = finish_recovery(owner, request.clone(), count.clone()).await;
    let mut state = handle.state.lock().await;
    runtime
        .inner
        .commit_locked(
            &mut state,
            vec![events::event(
                events::TITLE_GENERATED,
                &events::Titled {
                    title: "changed revision".into(),
                    usage: None,
                    cost: None,
                    model: None,
                },
            )],
        )
        .unwrap();
    assert!(completed.commit_recovery(&state).is_err());
    drop(state);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(
        runtime
            .inner
            .store
            .aggregate_seq(&original.activity_id)
            .unwrap(),
        Some(2)
    );
    let lookup = runtime
        .snapshot_recovery_confirm(&session, &request.confirmation)
        .await
        .unwrap();
    assert_eq!(lookup.state, super::super::SnapshotRecoveryState::Unknown);
    assert!(lookup.receipt.is_none());
    runtime.shutdown().await;
}

#[tokio::test]
async fn late_snapshot_disposal_preserves_durable_acknowledgement_and_unrelated_uncertainty() {
    for (unrelated_uncertainty, other_snapshot) in [(false, false), (true, false), (false, true)] {
        let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let owner = worker(&runtime, &session).await;
        let native_binding = binding(temp.path().canonicalize().unwrap());
        let completed = owner
            .run(move |work| {
                let origin = work.begin(native_binding).unwrap();
                Ok(SnapshotNativeResult::new(
                    vec![],
                    Box::new(Guard {
                        origin: Some(origin),
                        acknowledgements: Arc::new(AtomicUsize::new(0)),
                    }),
                ))
            })
            .await
            .unwrap();
        let id = completed.worker.scope.id().to_owned();
        let mut receipt = latest(&runtime, &session);
        receipt["status"] = "settled".into();
        receipt["phase"] = "snapshot_recovery_committed".into();
        receipt["resolution_receipt"] = serde_json::json!({"request_id":"retained_request"});
        runtime
            .inner
            .store
            .append(
                &id,
                cyber_store::Expected::Seq(1),
                vec![NewEvent::new(activity::CHANGED, receipt.clone())],
            )
            .unwrap();
        let sequence = runtime.inner.store.aggregate_seq(&id).unwrap();
        let notifications = runtime.inner.store.subscribe();
        let handle = runtime.inner.handle(&session).await.unwrap();
        handle
            .location_uncertain
            .store(unrelated_uncertainty, Ordering::SeqCst);
        if other_snapshot {
            handle.mark_snapshot_uncertain("op_other_snapshot");
        }
        drop(completed);
        assert!(notifications.try_recv().is_err());
        assert_eq!(runtime.inner.store.aggregate_seq(&id).unwrap(), sequence);
        assert_eq!(latest(&runtime, &session), receipt);
        assert_eq!(
            handle.has_location_uncertainty(),
            unrelated_uncertainty || other_snapshot
        );
        assert_eq!(
            handle.location_uncertain.load(Ordering::SeqCst),
            unrelated_uncertainty
        );
        assert!(!handle.snapshot_uncertain.lock().unwrap().contains(&id));
        runtime.shutdown().await;
    }
}

#[tokio::test]
async fn late_snapshot_disposal_refuses_changed_native_ownership() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let completed = owner
        .run(move |work| {
            let origin = work.begin(native_binding).unwrap();
            Ok(SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin),
                    acknowledgements: Arc::new(AtomicUsize::new(0)),
                }),
            ))
        })
        .await
        .unwrap();
    let id = completed.worker.scope.id().to_owned();
    let mut foreign = latest(&runtime, &session);
    foreign["snapshot_origin"]["binding"]["journal_id"] = "restore_foreign".into();
    runtime
        .inner
        .store
        .append(
            &id,
            cyber_store::Expected::Seq(1),
            vec![NewEvent::new(activity::CHANGED, foreign.clone())],
        )
        .unwrap();
    let notifications = runtime.inner.store.subscribe();
    drop(completed);
    assert!(notifications.try_recv().is_err());
    assert_eq!(runtime.inner.store.aggregate_seq(&id).unwrap(), Some(2));
    assert_eq!(latest(&runtime, &session), foreign);
    assert!(
        runtime
            .inner
            .handle(&session)
            .await
            .unwrap()
            .has_location_uncertainty()
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn reload_preserves_generic_uncertainty_separately_from_snapshot_owners() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    assert!(
        owner
            .run(move |work| {
                work.begin(native_binding).unwrap();
                Err(RestoreError::Failed("interrupted snapshot".into()))
            })
            .await
            .is_err()
    );
    let handle = runtime.inner.handle(&session).await.unwrap();
    assert!(handle.has_location_uncertainty());
    assert!(!handle.location_uncertain.load(Ordering::SeqCst));
    let snapshot = latest(&runtime, &session);
    let activity_id = snapshot["snapshot_origin"]["activity_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        handle
            .snapshot_uncertain
            .lock()
            .unwrap()
            .contains(&activity_id)
    );
    assert!(
        activity::Scope::reserve(&runtime.inner, &handle)
            .await
            .is_err(),
        "snapshot fence admitted a new generic native scope"
    );
    legacy_generic_unknown(&runtime, &handle, &session);
    assert!(handle.location_uncertain.load(Ordering::SeqCst));
    let reloaded = restart(&runtime, temp.path());
    let restored = reloaded.inner.handle(&session).await.unwrap();
    assert!(restored.location_uncertain.load(Ordering::SeqCst));
    assert!(
        restored
            .snapshot_uncertain
            .lock()
            .unwrap()
            .contains(&activity_id)
    );
    assert!(
        reloaded
            .admit(
                &session,
                Admission::text("must stay fenced", Delivery::Hold)
            )
            .await
            .is_err()
    );
    // Simulate a separate writer's exact durable snapshot acknowledgement. Reload
    // must retain the generic fence even when no snapshot activity remains unknown.
    let mut receipt = snapshot;
    receipt["status"] = "settled".into();
    runtime
        .inner
        .store
        .append(
            &activity_id,
            cyber_store::Expected::Seq(2),
            vec![NewEvent::new(activity::CHANGED, receipt)],
        )
        .unwrap();
    let after_ack = restart(&runtime, temp.path());
    let restored = after_ack.inner.handle(&session).await.unwrap();
    assert!(restored.snapshot_uncertain.lock().unwrap().is_empty());
    assert!(restored.location_uncertain.load(Ordering::SeqCst));
    let state = restored.state.lock().await;
    assert!(after_ack.snapshot_worker(&restored, &state).await.is_err());
    drop(state);
    after_ack.shutdown().await;
    reloaded.shutdown().await;
    runtime.shutdown().await;
}

#[tokio::test]
async fn ordinary_restore_commit_cannot_consume_a_recovery_receipt_or_acknowledge_it() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let acknowledgements = Arc::new(AtomicUsize::new(0));
    let count = acknowledgements.clone();
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let completed = owner
        .run(move |work| {
            let origin = work.begin(native_binding).unwrap();
            let mut result = SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin.clone()),
                    acknowledgements: count,
                }),
            );
            result.recovery = Some(super::super::SnapshotRecoveryReceipt {
                journal_id: "restore_original".into(),
                resolution_id: "restore_resolution".into(),
                request_id: "request".into(),
                action: super::super::SnapshotRecoveryAction::Keep,
                confirmation: "a".repeat(64),
                original: origin,
                filesystem_complete: true,
                resolution_digest: Some("d".repeat(64)),
                checkout: None,
            });
            Ok(result)
        })
        .await
        .unwrap();
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut state = handle.state.lock().await;
    let sequence = state.last_seq;
    assert!(completed.commit(&mut state, vec![]).is_err());
    assert_eq!(state.last_seq, sequence);
    assert_eq!(acknowledgements.load(Ordering::SeqCst), 0);
    assert_eq!(latest(&runtime, &session)["status"], "unknown");
    drop(state);
    runtime.shutdown().await;
}

#[tokio::test]
async fn non_utf8_location_binding_refuses_before_serializing_a_native_origin() {
    use std::os::unix::ffi::OsStringExt;
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let mut owner = worker(&runtime, &session).await;
    // Exercise the serialization boundary even on filesystems that reject these
    // names themselves. This is not a native path-admission acceptance test.
    let directory = temp
        .path()
        .join(std::ffi::OsString::from_vec(b"root_\xff".to_vec()));
    owner.directory = directory.clone();
    let native_binding = binding(directory);
    let result = owner
        .run(move |work| {
            assert!(work.begin(native_binding).unwrap_err().contains("UTF-8"));
            assert!(work.origin().is_none());
            Err(RestoreError::Stale)
        })
        .await;
    assert!(matches!(result, Err(RestoreError::Stale)));
    assert_eq!(latest(&runtime, &session)["status"], "settled");
    runtime.shutdown().await;
}

#[tokio::test]
async fn rejected_origin_append_cannot_authorize_source_effects_even_if_the_adapter_continues() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    runtime
        .inner
        .store
        .set_append_fault(Some(Arc::new(|_, event| {
            event.kind == activity::CHANGED
                && event.data["snapshot_worker"] == true
                && event.data["phase"] == "launching"
                && event.data["status"] == "pending"
        })));
    let result = owner
        .run(move |work| {
            assert!(work.begin(native_binding).is_err());
            assert!(work.origin().is_none());
            assert!(
                work.effect::<()>(|| panic!("unacknowledged origin authorized a source mutation"))
                    .is_err()
            );
            Err(RestoreError::Failed("origin append refused".into()))
        })
        .await;
    assert!(result.is_err());
    runtime.inner.store.set_append_fault(None);
    assert_eq!(latest(&runtime, &session)["status"], "settled");
    assert!(latest(&runtime, &session)["snapshot_origin"].is_null());
    runtime.shutdown().await;
}

#[tokio::test]
async fn snapshot_completion_commits_session_and_exact_origin_before_native_ack() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut unrelated = activity::Scope::reserve(&runtime.inner, &handle)
        .await
        .unwrap();
    let fake = SnapshotOrigin {
        version: 1,
        session_id: session.clone(),
        session_created_id: "untrusted_attempt".into(),
        activity_id: unrelated.id().to_owned(),
        worktree_id: None,
        binding: binding(temp.path().canonicalize().unwrap()),
    };
    assert!(
        unrelated.launch_snapshot(fake).is_err(),
        "generic activity adopted a snapshot origin"
    );
    unrelated.settle().unwrap();
    drop(unrelated);
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let acknowledgements = Arc::new(AtomicUsize::new(0));
    let seen = acknowledgements.clone();
    let completed = owner
        .run(move |work| {
            let origin = work.begin(native_binding.clone()).unwrap();
            assert!(
                work.begin(native_binding).is_err(),
                "origin replacement accepted"
            );
            work.effect(|| Ok(())).unwrap();
            Ok(SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin),
                    acknowledgements: seen,
                }),
            ))
        })
        .await
        .unwrap();
    assert_eq!(acknowledgements.load(Ordering::SeqCst), 0);
    let handle = runtime.inner.handle(&session).await.unwrap();
    assert!(
        activity::Scope::reserve(&runtime.inner, &handle)
            .await
            .is_err()
    );
    let unrelated_id = legacy_generic_unknown(&runtime, &handle, &session);
    let mut state = handle.state.lock().await;
    completed
        .commit(
            &mut state,
            vec![events::event(
                events::TITLE_GENERATED,
                &events::Titled {
                    title: "restored".into(),
                    usage: None,
                    cost: None,
                    model: None,
                },
            )],
        )
        .unwrap();
    assert_eq!(state.info.title, "restored");
    drop(state);
    let record = latest(&runtime, &session);
    assert_eq!(record["status"], "settled");
    assert_eq!(record["phase"], "snapshot_restore_committed");
    assert_eq!(record["snapshot_origin"]["session_id"], session);
    let created = runtime
        .inner
        .store
        .read_events(&session, -1, 1)
        .unwrap()
        .events[0]
        .id
        .clone();
    assert_eq!(record["snapshot_origin"]["session_created_id"], created);
    let activity = record["snapshot_origin"]["activity_id"].as_str().unwrap();
    let receipt = runtime
        .snapshot_restore_lookup(&session, activity)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.origin.activity_id, activity);
    assert_eq!(receipt.session_after - receipt.session_before, 1);
    assert!(
        runtime
            .snapshot_restore_lookup("ses_foreign", activity)
            .await
            .unwrap()
            .is_none()
    );
    let mut changed = receipt.clone();
    changed.events_digest = "0".repeat(64);
    assert!(
        runtime
            .inner
            .store
            .read(move |db| { super::super::snapshot_restore_receipt::verify(db, &changed) })
            .is_err()
    );
    let mut changed = receipt;
    changed.intent_digest = "0".repeat(64);
    assert!(
        runtime
            .inner
            .store
            .read(move |db| { super::super::snapshot_restore_receipt::verify(db, &changed) })
            .is_err()
    );
    assert!(record.get("binary_document").is_none());
    assert_eq!(acknowledgements.load(Ordering::SeqCst), 1);
    let unrelated = runtime
        .inner
        .store
        .read_events(&unrelated_id, -1, 10)
        .unwrap();
    assert_eq!(unrelated.events.last().unwrap().data["status"], "unknown");
    assert!(
        handle.has_location_uncertainty(),
        "snapshot receipt cleared another process's unknown outcome"
    );
    assert!(!handle.snapshot_active.load(Ordering::SeqCst));
    runtime.shutdown().await;
}

#[tokio::test]
async fn disposed_snapshot_request_retains_real_worker_owners_and_stop_remains_available() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let mut retry = Admission::text("retained held input", Delivery::Hold);
    retry.message_id = Some("msg_snapshot_exact_retry".into());
    let admitted = runtime.admit(&session, retry.clone()).await.unwrap();
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = permits.clone().acquire_owned().await.unwrap();
    let (started, running) = tokio::sync::oneshot::channel();
    let (finish, gate) = std::sync::mpsc::channel();
    let task = tokio::spawn(owner.run(move |work| {
        let _permit = permit;
        work.begin(native_binding).unwrap();
        started.send(()).unwrap();
        gate.recv().unwrap();
        assert!(work.authorize().is_err());
        Err(RestoreError::Failed(
            "interrupted worker retained its intent".into(),
        ))
    }));
    running.await.unwrap();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    assert_eq!(permits.available_permits(), 0);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), runtime.state(&session))
            .await
            .unwrap()
            .is_ok()
    );
    assert!(
        runtime
            .admit(&session, Admission::text("must not run", Delivery::Queue))
            .await
            .is_err()
    );
    assert_eq!(
        runtime
            .admit(&session, retry.clone())
            .await
            .unwrap()
            .message_id,
        admitted.message_id
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(3), runtime.interrupt(&session))
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        permits.available_permits(),
        0,
        "interruption released a live worker owner"
    );
    let stopped = tokio::time::timeout(Duration::from_secs(4), runtime.stop_subtree(&session))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.status, super::super::SubtreeStopStatus::Unknown);
    assert!(stopped.persisted);
    assert_eq!(
        permits.available_permits(),
        0,
        "subtree Stop released a live worker owner"
    );
    let shutdown = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.shutdown().await })
    };
    tokio::task::yield_now().await;
    assert!(
        !shutdown.is_finished(),
        "shutdown outlived its worker ownership"
    );
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(permits.available_permits(), 1);
    assert_eq!(latest(&runtime, &session)["status"], "unknown");
    let restarted = restart(&runtime, temp.path());
    assert!(
        restarted
            .admit(&session, Admission::text("still fenced", Delivery::Queue))
            .await
            .is_err()
    );
    restarted.shutdown().await;
}

#[tokio::test]
async fn snapshot_pre_effect_refusal_does_not_manufacture_an_unknown_native_effect() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let wrong = binding(temp.path().join("foreign"));
    let result = owner
        .run(move |work| {
            assert!(
                work.effect::<()>(|| panic!("unbound source effect ran"))
                    .is_err()
            );
            assert!(work.begin(wrong).is_err());
            Err(RestoreError::Stale)
        })
        .await;
    assert!(matches!(result, Err(RestoreError::Stale)));
    let record = latest(&runtime, &session);
    assert_eq!(record["status"], "settled");
    assert!(record.get("snapshot_origin").is_none());
    let handle = runtime.inner.handle(&session).await.unwrap();
    assert!(!handle.snapshot_active.load(Ordering::SeqCst));
    assert!(!handle.has_location_uncertainty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn changed_session_refuses_the_next_source_effect_and_never_acknowledges_it() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let source = temp.path().join("must_not_change");
    std::fs::write(&source, "owner bytes").unwrap();
    let worker_source = source.clone();
    let (started, running) = tokio::sync::oneshot::channel();
    let (finish, gate) = std::sync::mpsc::channel();
    let task = tokio::spawn(owner.run(move |work| {
        work.begin(native_binding).unwrap();
        started.send(()).unwrap();
        gate.recv().unwrap();
        assert!(
            work.effect(|| std::fs::write(worker_source, "wrong").map_err(|e| e.to_string()))
                .is_err()
        );
        Err(RestoreError::Stale)
    }));
    running.await.unwrap();
    runtime
        .rename(&session, "owner changed Session")
        .await
        .unwrap();
    finish.send(()).unwrap();
    assert!(matches!(task.await.unwrap(), Err(RestoreError::Stale)));
    assert_eq!(std::fs::read_to_string(source).unwrap(), "owner bytes");
    assert_eq!(latest(&runtime, &session)["status"], "unknown");
    assert_eq!(
        runtime.state(&session).await.unwrap().info.title,
        "owner changed Session"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn rejected_runtime_commit_keeps_origin_unknown_and_never_acknowledges_the_journal() {
    let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let owner = worker(&runtime, &session).await;
    let native_binding = binding(temp.path().canonicalize().unwrap());
    let acknowledgements = Arc::new(AtomicUsize::new(0));
    let observed = acknowledgements.clone();
    let completed = owner
        .run(move |work| {
            let origin = work.begin(native_binding).unwrap();
            Ok(SnapshotNativeResult::new(
                vec![],
                Box::new(Guard {
                    origin: Some(origin),
                    acknowledgements: observed,
                }),
            ))
        })
        .await
        .unwrap();
    let handle = runtime.inner.handle(&session).await.unwrap();
    let mut state = handle.state.lock().await;
    let sequence = state.last_seq;
    assert!(
        completed
            .commit(
                &mut state,
                vec![NewEvent::new("test.unregistered.1", serde_json::json!({}))]
            )
            .is_err()
    );
    assert_eq!(state.last_seq, sequence);
    assert_eq!(acknowledgements.load(Ordering::SeqCst), 0);
    drop(state);
    assert_eq!(latest(&runtime, &session)["status"], "unknown");
    runtime.shutdown().await;
    let restarted = restart(&runtime, temp.path());
    assert!(
        restarted
            .admit(
                &session,
                Admission::text("unresolved origin must fence direct input", Delivery::Queue)
            )
            .await
            .is_err()
    );
    assert!(
        restarted
            .inner
            .handle(&session)
            .await
            .unwrap()
            .has_location_uncertainty()
    );
    restarted.shutdown().await;
}

struct LostRestoreResources(AtomicUsize);
impl super::super::SnapshotLocationGuard for LostRestoreResources {
    fn verify(&self) -> Result<(), String> {
        Ok(())
    }
    fn bindings(&self) -> Result<Vec<cyber_core::worktrees::CheckoutBinding>, String> {
        if self.0.fetch_add(1, Ordering::SeqCst) < 2 {
            Ok(vec![])
        } else {
            Err("fixture lost the complete retained checkout set".into())
        }
    }
}

#[tokio::test]
async fn ordinary_restore_receipt_binds_native_lifetime_and_rechecks_resources_at_writer() {
    for case in ["valid", "writer", "publication"] {
        let lost = case != "valid";
        let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let resources = Arc::new(std::sync::Mutex::new(None));
        let runtime = runtime_with_snapshots(
            &base,
            base.inner.store.clone(),
            temp.path(),
            Arc::new(Lifetimes {
                inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
                permits: Arc::new(tokio::sync::Semaphore::new(1)),
                foreign: false,
                reject_resources: false,
                resources: resources.clone(),
            }),
        );
        let mut owner = worker(&runtime, &session).await;
        let id = owner.scope.id().to_owned();
        let lifetime = owner.scope.snapshot_lifetime().unwrap().clone();
        if case == "writer" {
            owner._location = Some(Box::new(LostRestoreResources(AtomicUsize::new(0))));
        }
        let native_binding = binding(temp.path().canonicalize().unwrap());
        let acks = Arc::new(AtomicUsize::new(0));
        let seen = acks.clone();
        let completed = owner
            .run(move |work| {
                let origin = work.begin(native_binding).unwrap();
                work.effect(|| Ok(())).unwrap();
                Ok(SnapshotNativeResult::new(
                    vec![],
                    Box::new(Guard {
                        origin: Some(origin),
                        acknowledgements: seen,
                    }),
                ))
            })
            .await
            .unwrap();
        if case == "publication" {
            *resources.lock().unwrap() = None;
        }
        let handle = runtime.inner.handle(&session).await.unwrap();
        let mut state = handle.state.lock().await;
        let before = state.last_seq;
        let result = completed.commit(
            &mut state,
            vec![events::event(
                events::TITLE_GENERATED,
                &events::Titled {
                    title: "checkout bound".into(),
                    usage: None,
                    cost: None,
                    model: None,
                },
            )],
        );
        if lost {
            assert!(result.is_err());
            assert_eq!(state.last_seq, before);
            assert_ne!(state.info.title, "checkout bound");
            assert_eq!(acks.load(Ordering::SeqCst), 0);
            drop(state);
            assert!(
                runtime
                    .snapshot_restore_lookup(&session, &id)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(latest(&runtime, &session)["status"], "unknown");
        } else {
            result.unwrap();
            assert_eq!(state.info.title, "checkout bound");
            drop(state);
            let receipt = runtime
                .snapshot_restore_lookup(&session, &id)
                .await
                .unwrap()
                .unwrap();
            let checkout = receipt.checkout.as_ref().unwrap();
            assert_eq!(checkout.lifetime, lifetime);
            assert_ne!(
                checkout.lifetime.namespace_identity,
                receipt.origin.binding.namespace_identity
            );
            assert_eq!(
                checkout.resources_digest,
                lifetime.checkout_resources_digest(&[]).unwrap()
            );
            let hash = receipt.checkout_acknowledgement_digest().unwrap();
            let mut invalid = receipt.clone();
            invalid.events_digest = "invalid".into();
            assert!(invalid.checkout_acknowledgement_digest().is_err());
            let mut invalid = receipt.clone();
            invalid.session_after = invalid.session_before - 1;
            assert!(invalid.checkout_acknowledgement_digest().is_err());
            let mut changed = receipt.clone();
            changed
                .checkout
                .as_mut()
                .unwrap()
                .lifetime
                .owner
                .session_created_id = "evt_foreign".into();
            assert!(changed.checkout_acknowledgement_digest().is_err());
            let mut changed = receipt.clone();
            changed.checkout.as_mut().unwrap().resources_digest = "f".repeat(64);
            assert_ne!(changed.checkout_acknowledgement_digest().unwrap(), hash);
            assert!(
                runtime
                    .inner
                    .store
                    .read(move |db| super::super::snapshot_restore_receipt::verify(db, &changed))
                    .is_err()
            );
            let mut legacy = receipt;
            legacy.checkout = None;
            let bytes = serde_json::to_value(&legacy).unwrap();
            assert!(bytes.get("checkout").is_none());
            let legacy: SnapshotRestoreReceipt = serde_json::from_value(bytes).unwrap();
            assert!(legacy.checkout_acknowledgement_digest().is_err());
            assert_eq!(acks.load(Ordering::SeqCst), 1);
        }
        runtime.shutdown().await;
    }
}

struct RestoreCheckoutAck {
    stage: &'static str,
    prepared: Arc<AtomicUsize>,
    acknowledged: Arc<AtomicUsize>,
}
impl super::super::SnapshotLocationGuard for RestoreCheckoutAck {
    fn verify(&self) -> Result<(), String> {
        Ok(())
    }
    fn prepare_restore_acknowledgement(
        &mut self,
        receipt: &SnapshotRestoreReceipt,
    ) -> Result<(), String> {
        receipt.checkout_acknowledgement_digest()?;
        self.prepared.fetch_add(1, Ordering::SeqCst);
        if self.stage == "prepare" {
            Err("fixture refused candidate checkout receipt".into())
        } else {
            Ok(())
        }
    }
    fn acknowledge_restore(&mut self, receipt: &SnapshotRestoreReceipt) -> Result<(), String> {
        receipt.checkout_acknowledgement_digest()?;
        self.acknowledged.fetch_add(1, Ordering::SeqCst);
        if self.stage == "publish" {
            Err("fixture failed checkout marker publication".into())
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn ordinary_checkout_acknowledgement_follows_the_consuming_pair_and_keeps_committed_state_on_failure()
 {
    for stage in ["valid", "prepare", "publish"] {
        let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let reconciled = Arc::new(AtomicUsize::new(0));
        let resources = Arc::new(std::sync::Mutex::new(None));
        let runtime = runtime_with_snapshots(
            &base,
            base.inner.store.clone(),
            temp.path(),
            Arc::new(Lifetimes {
                inner: Acknowledgements(reconciled.clone()),
                permits: Arc::new(tokio::sync::Semaphore::new(1)),
                foreign: false,
                reject_resources: false,
                resources: resources.clone(),
            }),
        );
        let mut owner = worker(&runtime, &session).await;
        let id = owner.scope.id().to_owned();
        let prepared = Arc::new(AtomicUsize::new(0));
        let acknowledged = Arc::new(AtomicUsize::new(0));
        owner._location = Some(Box::new(RestoreCheckoutAck {
            stage,
            prepared: prepared.clone(),
            acknowledged: acknowledged.clone(),
        }));
        let binding = binding(temp.path().canonicalize().unwrap());
        let journal_acks = Arc::new(AtomicUsize::new(0));
        let seen = journal_acks.clone();
        let completed = owner
            .run(move |work| {
                let origin = work.begin(binding).unwrap();
                work.effect(|| Ok(())).unwrap();
                Ok(SnapshotNativeResult::new(
                    vec![],
                    Box::new(Guard {
                        origin: Some(origin),
                        acknowledgements: seen,
                    }),
                ))
            })
            .await
            .unwrap();
        assert_eq!(prepared.load(Ordering::SeqCst), 0);
        assert_eq!(acknowledged.load(Ordering::SeqCst), 0);
        let handle = runtime.inner.handle(&session).await.unwrap();
        let mut state = handle.state.lock().await;
        let before = state.last_seq;
        let result = completed.commit(
            &mut state,
            vec![events::event(
                events::TITLE_GENERATED,
                &events::Titled {
                    title: "durable checkout".into(),
                    usage: None,
                    cost: None,
                    model: None,
                },
            )],
        );
        assert_eq!(prepared.load(Ordering::SeqCst), 1);
        if stage == "prepare" {
            assert!(result.is_err());
            assert_eq!(state.last_seq, before);
            assert_eq!(acknowledged.load(Ordering::SeqCst), 0);
            assert_eq!(journal_acks.load(Ordering::SeqCst), 0);
            drop(state);
            assert!(
                runtime
                    .snapshot_restore_lookup(&session, &id)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(latest(&runtime, &session)["status"], "unknown");
        } else {
            if stage == "publish" {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("runtime acknowledgement committed")
                );
            } else {
                result.unwrap();
            }
            assert_eq!(state.info.title, "durable checkout");
            assert!(state.last_seq > before);
            assert_eq!(acknowledged.load(Ordering::SeqCst), 1);
            assert_eq!(
                journal_acks.load(Ordering::SeqCst),
                usize::from(stage == "valid")
            );
            drop(state);
            assert!(
                runtime
                    .snapshot_restore_lookup(&session, &id)
                    .await
                    .unwrap()
                    .unwrap()
                    .checkout
                    .is_some()
            );
            assert_eq!(latest(&runtime, &session)["status"], "settled");
            if stage == "publish" {
                let before = runtime
                    .snapshot_restore_lookup(&session, &id)
                    .await
                    .unwrap()
                    .unwrap();
                *resources.lock().unwrap() = None;
                assert!(
                    runtime
                        .snapshot_restore_acknowledge(&session, &id)
                        .await
                        .is_err()
                );
                assert_eq!(reconciled.load(Ordering::SeqCst), 0);
                assert!(!handle.snapshot_active.load(Ordering::SeqCst));
                *resources.lock().unwrap() = Some(vec![]);
                let reconciled_receipt = runtime
                    .snapshot_restore_acknowledge(&session, &id)
                    .await
                    .unwrap();
                assert_eq!(reconciled_receipt, before);
                runtime
                    .snapshot_restore_acknowledge(&session, &id)
                    .await
                    .unwrap();
                assert_eq!(reconciled.load(Ordering::SeqCst), 2);
                assert_eq!(
                    journal_acks.load(Ordering::SeqCst),
                    0,
                    "metadata retry replayed the original worker"
                );
                assert_eq!(
                    runtime.state(&session).await.unwrap().info.title,
                    "durable checkout"
                );
            }
        }
        runtime.shutdown().await;
    }
}

#[tokio::test]
async fn managed_completion_without_a_journal_receipt_keeps_pre_effect_recovery_evidence() {
    for managed in [false, true] {
        let (temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let handle = runtime.inner.handle(&session).await.unwrap();
        let created = runtime
            .inner
            .store
            .read_events(&session, -1, 1)
            .unwrap()
            .events[0]
            .id
            .clone();
        let id = cyber_core::ids::new_id("op");
        // Writer-boundary fixture: no native source worker or checkout is admitted.
        let lifetime = super::super::SnapshotLifetimeBinding {
            version: 2,
            owner: super::super::SnapshotLifetimeRequest {
                activity_id: id.clone(),
                session_id: session.clone(),
                session_created_id: created,
                directory: temp.path().canonicalize().unwrap(),
                worktree_id: managed.then(|| "wt_fixture".into()),
            },
            root_identity: "fixture-root".into(),
            namespace_identity: "fixture-namespace".into(),
            directory_identity: "fixture-directory".into(),
            file_identity: "fixture-file".into(),
            source_phase: Some("phase_fixture".into()),
        };
        let mut scope = activity::Scope::reserve_snapshot_owned(
            &runtime.inner,
            session.clone(),
            id,
            Some(lifetime),
        )
        .await
        .unwrap();
        let mut state = handle.state.lock().await;
        let before = state.last_seq;
        let result = scope.commit_snapshot(
            &mut state,
            before,
            vec![events::event(
                events::TITLE_GENERATED,
                &events::Titled {
                    title: "no source effects".into(),
                    usage: None,
                    cost: None,
                    model: None,
                },
            )],
            None,
            Box::new(BindingLease {
                bindings: vec![],
                settlements: Arc::new(AtomicUsize::new(0)),
            }),
        );
        if managed {
            assert!(result.is_err());
            assert_eq!(state.last_seq, before);
        } else {
            result.unwrap();
            assert_eq!(state.info.title, "no source effects");
        }
        drop(state);
        drop(scope);
        let record = latest(&runtime, &session);
        assert_eq!(
            record["status"],
            if managed { "unknown" } else { "settled" }
        );
        assert!(record["snapshot_origin"].is_null());
        assert_eq!(record["snapshot_source_admitted"], false);
        assert!(record["snapshot_restore_receipt"].is_null());
        runtime.shutdown().await;
    }
}

#[tokio::test]
async fn resolution_retains_original_and_distinct_resolution_lifetimes_through_native_work() {
    for changed in [false, true] {
        let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let permits = Arc::new(tokio::sync::Semaphore::new(2));
        let resources = Arc::new(std::sync::Mutex::new(None));
        let runtime = runtime_with_snapshots(
            &base,
            base.inner.store.clone(),
            temp.path(),
            Arc::new(Lifetimes {
                inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
                permits: permits.clone(),
                foreign: false,
                reject_resources: false,
                resources: resources.clone(),
            }),
        );
        let original =
            failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
        assert_eq!(permits.available_permits(), 2);
        let request = recovery_request(original.clone());
        let handle = runtime.inner.handle(&session).await.unwrap();
        let state = handle.state.lock().await;
        let owner = runtime
            .snapshot_recovery_worker(&handle, &state, request.clone())
            .await
            .unwrap();
        assert_eq!(owner._original.as_ref().unwrap().origin, original);
        assert_ne!(owner.scope.id(), original.activity_id);
        assert_eq!(
            permits.available_permits(),
            0,
            "both owners must remain held"
        );
        drop(state);
        if changed {
            *resources.lock().unwrap() = None;
            let effects = Arc::new(AtomicUsize::new(0));
            let seen = effects.clone();
            let result = owner
                .run(move |work| {
                    work.authorize().map_err(RestoreError::Failed)?;
                    seen.fetch_add(1, Ordering::SeqCst);
                    Err(RestoreError::Failed("unexpected dispatch".into()))
                })
                .await;
            assert!(result.is_err());
            assert_eq!(effects.load(Ordering::SeqCst), 0);
        } else {
            let completed = finish_recovery(owner, request, Arc::new(AtomicUsize::new(0))).await;
            assert_eq!(permits.available_permits(), 0);
            // No consuming pair: retaining owners alone cannot acknowledge either.
            drop(completed);
        }
        assert_eq!(permits.available_permits(), 2);
        let events = runtime
            .inner
            .store
            .read_events(&original.activity_id, -1, 10)
            .unwrap();
        assert_eq!(events.events.last().unwrap().data["status"], "unknown");
        runtime.shutdown().await;
    }
}

struct NativeDisposal {
    permits: Arc<tokio::sync::Semaphore>,
    disposed: Arc<std::sync::atomic::AtomicBool>,
}
impl SnapshotNativeGuard for NativeDisposal {
    fn verify(&self, origin: Option<&SnapshotOrigin>) -> Result<(), String> {
        if origin.is_none() {
            Ok(())
        } else {
            Err("unexpected origin".into())
        }
    }
    fn acknowledge(&mut self, _: Option<&SnapshotOrigin>) -> Result<(), String> {
        Ok(())
    }
}
impl Drop for NativeDisposal {
    fn drop(&mut self) {
        assert_eq!(
            self.permits.available_permits(),
            0,
            "native cleanup outlived lifetime ownership"
        );
        self.disposed.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn native_result_cleanup_precedes_worker_lifetime_exit() {
    let (temp, base, session, _, _) = super::super::binary_documents_tests::fixture().await;
    let permits = Arc::new(tokio::sync::Semaphore::new(1));
    let runtime = runtime_with_snapshots(
        &base,
        base.inner.store.clone(),
        temp.path(),
        Arc::new(Lifetimes {
            inner: Acknowledgements(Arc::new(AtomicUsize::new(0))),
            permits: permits.clone(),
            foreign: false,
            reject_resources: false,
            resources: Default::default(),
        }),
    );
    let owner = worker(&runtime, &session).await;
    let disposed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let pin = NativeDisposal {
        permits: permits.clone(),
        disposed: disposed.clone(),
    };
    let completed = owner
        .run(move |_| Ok(SnapshotNativeResult::new(vec![], Box::new(pin))))
        .await
        .unwrap();
    assert_eq!(permits.available_permits(), 0);
    drop(completed);
    assert!(disposed.load(Ordering::SeqCst));
    assert_eq!(permits.available_permits(), 1);
    runtime.shutdown().await;
}

struct RecoveryRoleAck {
    role: super::super::SnapshotRecoveryCheckoutRole,
    fail: &'static str,
    order: Arc<std::sync::Mutex<Vec<String>>>,
}
impl super::super::SnapshotLocationGuard for RecoveryRoleAck {
    fn verify(&self) -> Result<(), String> {
        Ok(())
    }
    fn prepare_recovery_acknowledgement(
        &mut self,
        receipt: &super::super::SnapshotRecoveryReceipt,
        role: super::super::SnapshotRecoveryCheckoutRole,
    ) -> Result<(), String> {
        assert_eq!(role, self.role);
        receipt.checkout_acknowledgement_digest()?;
        self.order.lock().unwrap().push(format!("prepare {role:?}"));
        if self.fail == "candidate" && role == super::super::SnapshotRecoveryCheckoutRole::Original
        {
            Err("fixture rejected original candidate".into())
        } else {
            Ok(())
        }
    }
    fn acknowledge_recovery(
        &mut self,
        receipt: &super::super::SnapshotRecoveryReceipt,
        role: super::super::SnapshotRecoveryCheckoutRole,
    ) -> Result<(), String> {
        assert_eq!(role, self.role);
        receipt.checkout_acknowledgement_digest()?;
        self.order.lock().unwrap().push(format!("publish {role:?}"));
        if self.fail == "publication"
            && role == super::super::SnapshotRecoveryCheckoutRole::Original
        {
            Err("fixture interrupted original publication".into())
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn recovery_checkout_pair_binds_both_roles_and_reconciles_without_replaying_source() {
    use super::super::SnapshotRecoveryCheckoutRole as Role;
    fn lost_reply(_: &rusqlite::hooks::Wal, _: i32) -> rusqlite::Result<()> {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("recovery checkout pair reply lost".into()),
        ))
    }
    for fail in ["valid", "candidate", "publication", "reply"] {
        let (temp, base, _, _, _) = super::super::binary_documents_tests::fixture().await;
        let store = cyber_store::Store::open(cyber_store::StoreOptions::new(
            cyber_core::paths::DatabaseLocation::File(temp.path().join("checkout-pair.db")),
            Runtime::registry(),
        ))
        .unwrap();
        let permits = Arc::new(tokio::sync::Semaphore::new(2));
        let resources = Arc::new(std::sync::Mutex::new(None));
        let metadata = Arc::new(AtomicUsize::new(0));
        let runtime = runtime_with_snapshots(
            &base,
            Arc::new(store),
            temp.path(),
            Arc::new(Lifetimes {
                inner: Acknowledgements(metadata.clone()),
                permits: permits.clone(),
                foreign: false,
                reject_resources: false,
                resources: resources.clone(),
            }),
        );
        let session = runtime
            .create_session(super::super::CreateSession {
                directory: temp.path().to_str().unwrap().into(),
                model: "test/main".into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .id;
        let original =
            failed_snapshot(&runtime, &session, temp.path().canonicalize().unwrap()).await;
        let request = recovery_request(original.clone());
        let handle = runtime.inner.handle(&session).await.unwrap();
        let state = handle.state.lock().await;
        let mut owner = runtime
            .snapshot_recovery_worker(&handle, &state, request.clone())
            .await
            .unwrap();
        drop(state);
        let resolution_activity = owner.scope.id().to_owned();
        let order = Arc::new(std::sync::Mutex::new(vec![]));
        owner._location = Some(Box::new(RecoveryRoleAck {
            role: Role::Resolution,
            fail,
            order: order.clone(),
        }));
        owner._original.as_mut().unwrap().claims = Some(Box::new(RecoveryRoleAck {
            role: Role::Original,
            fail,
            order: order.clone(),
        }));
        let journal_acks = Arc::new(AtomicUsize::new(0));
        let completed = finish_recovery(owner, request.clone(), journal_acks.clone()).await;
        if fail == "reply" {
            runtime
                .inner
                .store
                .transaction(|tx| {
                    tx.wal_hook(Some(lost_reply));
                    Ok(())
                })
                .unwrap();
        }
        let state = handle.state.lock().await;
        let result = completed.commit_recovery(&state);
        drop(state);
        assert_eq!(permits.available_permits(), 2);
        let lookup = runtime
            .snapshot_recovery_lookup(&session, &request.confirmation.request_id)
            .await
            .unwrap()
            .unwrap();
        if fail == "candidate" {
            assert!(result.is_err());
            assert!(lookup.receipt.is_none());
            assert_eq!(
                *order.lock().unwrap(),
                ["prepare Resolution", "prepare Original"]
            );
            assert_eq!(journal_acks.load(Ordering::SeqCst), 0);
        } else {
            if fail == "reply" {
                assert!(result.is_err());
            } else if fail == "publication" {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("runtime acknowledgement committed")
                );
            } else {
                result.unwrap();
            }
            let receipt = lookup.receipt.unwrap();
            let checkout = receipt.checkout.as_ref().unwrap();
            assert_eq!(
                checkout
                    .original
                    .as_ref()
                    .unwrap()
                    .lifetime
                    .owner
                    .activity_id,
                original.activity_id
            );
            assert_eq!(
                checkout.resolution.lifetime.owner.activity_id,
                resolution_activity
            );
            assert_eq!(checkout.resolution_origin.activity_id, resolution_activity);
            let mut expected_order = vec!["prepare Resolution", "prepare Original"];
            if fail != "reply" {
                expected_order.extend(["publish Resolution", "publish Original"]);
            }
            assert_eq!(*order.lock().unwrap(), expected_order);
            let hash = receipt.checkout_acknowledgement_digest().unwrap();
            let mut changed = receipt.clone();
            changed
                .checkout
                .as_mut()
                .unwrap()
                .original
                .as_mut()
                .unwrap()
                .resources_digest = "f".repeat(64);
            assert_ne!(changed.checkout_acknowledgement_digest().unwrap(), hash);
            assert!(
                runtime
                    .inner
                    .store
                    .read(
                        move |db| super::super::snapshot_acknowledgement::verify_committed(
                            db, &changed
                        )
                    )
                    .is_err()
            );
            if fail == "publication" || fail == "reply" {
                *resources.lock().unwrap() = None;
                assert!(
                    runtime
                        .snapshot_recovery_acknowledge(&session, &request.confirmation.request_id)
                        .await
                        .is_err()
                );
                assert_eq!(metadata.load(Ordering::SeqCst), 0);
                *resources.lock().unwrap() = Some(vec![]);
                for _ in 0..2 {
                    let reconciled = runtime
                        .snapshot_recovery_acknowledge(&session, &request.confirmation.request_id)
                        .await
                        .unwrap();
                    assert_eq!(reconciled.receipt.as_ref(), Some(&receipt));
                    assert_eq!(reconciled.journal_acknowledged, Some(true));
                }
                assert_eq!(metadata.load(Ordering::SeqCst), 2);
                assert_eq!(journal_acks.load(Ordering::SeqCst), 0);
            }
            if fail == "reply" {
                runtime.shutdown().await;
                let reopened = cyber_store::Store::open(cyber_store::StoreOptions::new(
                    cyber_core::paths::DatabaseLocation::File(temp.path().join("checkout-pair.db")),
                    Runtime::registry(),
                ))
                .unwrap();
                let reloaded = runtime_with_snapshots(
                    &base,
                    Arc::new(reopened),
                    temp.path(),
                    runtime.inner.options.snapshots.clone(),
                );
                let observed = reloaded
                    .snapshot_recovery_acknowledge(&session, &request.confirmation.request_id)
                    .await
                    .unwrap();
                assert_eq!(observed.receipt.as_ref(), Some(&receipt));
                assert_eq!(metadata.load(Ordering::SeqCst), 3);
                assert_eq!(journal_acks.load(Ordering::SeqCst), 0);
                reloaded.shutdown().await;
            }
            let mut legacy = receipt;
            legacy.checkout = None;
            assert!(
                serde_json::to_value(&legacy)
                    .unwrap()
                    .get("checkout")
                    .is_none()
            );
            assert!(legacy.checkout_acknowledgement_digest().is_err());
        }
        runtime.shutdown().await;
    }
}
