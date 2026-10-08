mod support;
use cyber_server::runtime::*;
use cyber_store::{Expected, NewEvent};
use futures::future::BoxFuture;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use support::{Harness, Setup};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

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
async fn reopen(
    runtime: &Runtime,
    report: &SubtreeStopReport,
) -> Result<SubtreeReopenReport, RuntimeError> {
    runtime
        .reopen_subtree(
            &report.session_id,
            &report.scope_id,
            report.receipt_id.as_deref().unwrap(),
        )
        .await
}

#[tokio::test]
async fn matched_reopening_preserves_input_revokes_old_authority_and_survives_restart() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let nested = child(&h, &root).await;
    let old = h.runtime.capture_child_admission(&nested).unwrap();
    let mut input = Admission::text("kept", Delivery::Queue);
    input.resume = false;
    h.runtime.admit(&root, input).await.unwrap();
    let before = h.state(&root).await.inbox;
    let report = h.runtime.stop_subtree(&root).await.unwrap();
    let result = reopen(&h.runtime, &report).await.unwrap();
    assert_eq!(result.stop_receipt_id, report.receipt_id.unwrap());
    assert_eq!(result.scope_id, report.scope_id);
    assert!(!result.reopen_receipt_id.is_empty());
    assert_eq!(h.state(&root).await.inbox, before);
    assert!(!h.runtime.is_running(&root));
    assert!(h.runtime.capture_child_admission(&nested).is_ok());
    assert!(old.verify(&h.runtime, &nested).is_err());
    let restarted = h.restart();
    assert!(restarted.capture_child_admission(&nested).is_ok());
    assert!(!restarted.is_running(&root));
}

#[tokio::test]
async fn unknown_and_superseded_stop_reviews_cannot_reopen() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let nested = child(&h, &root).await;
    let owner = h.runtime.claim_child_execution(&root, &nested).unwrap();
    let unknown = h.runtime.stop_subtree(&root).await.unwrap();
    assert_eq!(unknown.status, SubtreeStopStatus::Unknown);
    assert!(reopen(&h.runtime, &unknown).await.is_err());
    drop(owner);
    let settled = h.runtime.stop_subtree(&root).await.unwrap();
    assert_eq!(settled.status, SubtreeStopStatus::Acknowledged);
    assert!(reopen(&h.runtime, &unknown).await.is_err());
    let newer = h.runtime.stop_subtree(&root).await.unwrap();
    assert!(reopen(&h.runtime, &settled).await.is_err());
    assert!(h.runtime.capture_child_admission(&root).is_err());
    reopen(&h.runtime, &newer).await.unwrap();
}

#[tokio::test]
async fn reopening_keeps_nested_closures_and_old_reviews_cannot_clear_a_new_scope() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let nested = child(&h, &root).await;
    let nested_stop = h.runtime.stop_subtree(&nested).await.unwrap();
    let root_stop = h.runtime.stop_subtree(&root).await.unwrap();
    reopen(&h.runtime, &root_stop).await.unwrap();
    assert!(h.runtime.capture_child_admission(&root).is_ok());
    assert!(h.runtime.capture_child_admission(&nested).is_err());
    reopen(&h.runtime, &nested_stop).await.unwrap();
    assert!(h.runtime.capture_child_admission(&nested).is_ok());
    let again = h.runtime.stop_subtree(&root).await.unwrap();
    assert_ne!(again.scope_id, root_stop.scope_id);
    assert!(reopen(&h.runtime, &root_stop).await.is_err());
    assert!(h.runtime.capture_child_admission(&root).is_err());
    reopen(&h.runtime, &again).await.unwrap();
}

#[tokio::test]
async fn orphaned_sweep_and_unknown_idle_evidence_refuse_an_earlier_acknowledgement() {
    for pending_sweep in [false, true] {
        let h = Harness::new(Setup::default());
        let root = h.session().await;
        let report = h.runtime.stop_subtree(&root).await.unwrap();
        if pending_sweep {
            h.store
                .append(
                    &root,
                    Expected::Any,
                    vec![NewEvent::new(
                        "session.subtree.stopping.1",
                        json!({"scope_id":report.scope_id,"sweep_id":"op_orphan"}),
                    )],
                )
                .unwrap();
        } else {
            h.store
                .append(
                    "op_idle",
                    Expected::Seq(-1),
                    vec![NewEvent::new(
                        "native.activity.changed.1",
                        json!({"session_id":root,"status":"unknown","phase":"launching"}),
                    )],
                )
                .unwrap();
        }
        assert!(reopen(&h.restart(), &report).await.is_err());
        assert!(h.restart().capture_child_admission(&root).is_err());
    }
}

#[derive(Default)]
struct Gate {
    armed: AtomicBool,
    ready: Notify,
    release: Notify,
    fail_proof: AtomicBool,
}
struct Host {
    tools: Arc<support::Tools>,
    gate: Arc<Gate>,
}
impl ToolHost for Host {
    fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        self.tools.definitions(turn)
    }
    fn execute(&self, call: Invocation, cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        self.tools.execute(call, cancel)
    }
    fn claim_location<'a>(
        &'a self,
        _: &'a SessionInfo,
        _: bool,
        _: CancellationToken,
    ) -> BoxFuture<'a, Result<LocationLease, String>> {
        Box::pin(async move {
            if self.gate.armed.swap(false, Ordering::SeqCst) {
                self.gate.ready.notify_one();
                self.gate.release.notified().await;
                if self.gate.fail_proof.load(Ordering::SeqCst) {
                    return Err("native recovery required".into());
                }
            }
            Ok(LocationLease::unmanaged())
        })
    }
}
fn gated(h: &Harness) -> (Runtime, Arc<Gate>) {
    let gate = Arc::new(Gate::default());
    let runtime = Runtime::new(RuntimeOptions {
        store: Arc::clone(&h.store),
        resolver: Arc::clone(&h.models) as Arc<dyn ModelResolver>,
        tools: Arc::new(Host {
            tools: Arc::clone(&h.tools),
            gate: Arc::clone(&gate),
        }),
        global_config_dir: h.dir.path().join("global"),
        shell: "zsh".into(),
        claude_compat: true,
        compaction: CompactionConfig::default(),
        retry: cyber_llm::RetryPolicy::default(),
        max_steps: None,
        today: None,
        interactive: false,
        snapshots: Arc::new(NoSnapshots),
    });
    (runtime, gate)
}

#[tokio::test]
async fn writer_refuses_changed_descendant_evidence_after_native_location_wait() {
    for operation_change in [false, true] {
        let h = Harness::new(Setup::default());
        let root = h.session().await;
        let nested = child(&h, &root).await;
        let (runtime, gate) = gated(&h);
        let report = runtime.stop_subtree(&root).await.unwrap();
        gate.armed.store(true, Ordering::SeqCst);
        let owned = runtime.clone();
        let review = report.clone();
        let task = tokio::spawn(async move { reopen(&owned, &review).await });
        tokio::time::timeout(Duration::from_secs(2), gate.ready.notified())
            .await
            .unwrap();
        if operation_change {
            h.store
                .append(
                    "op_new_receipt",
                    Expected::Seq(-1),
                    vec![NewEvent::new(
                        "native.activity.changed.1",
                        json!({"session_id":root,"status":"settled","phase":"reserved"}),
                    )],
                )
                .unwrap();
        } else {
            h.store
                .append(
                    &nested,
                    Expected::Any,
                    vec![NewEvent::new("session.admission.fenced.1", json!({}))],
                )
                .unwrap();
        }
        gate.release.notify_one();
        assert!(matches!(
            task.await.unwrap(),
            Err(RuntimeError::Conflict(_))
        ));
        assert!(h.restart().capture_child_admission(&root).is_err());
    }
}

#[tokio::test]
async fn disposed_or_refused_native_proof_cannot_reopen_admission() {
    for dispose in [false, true] {
        let h = Harness::new(Setup::default());
        let root = h.session().await;
        let (runtime, gate) = gated(&h);
        let report = runtime.stop_subtree(&root).await.unwrap();
        gate.armed.store(true, Ordering::SeqCst);
        let owned = runtime.clone();
        let task = tokio::spawn(async move { reopen(&owned, &report).await });
        tokio::time::timeout(Duration::from_secs(2), gate.ready.notified())
            .await
            .unwrap();
        if dispose {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            gate.fail_proof.store(true, Ordering::SeqCst);
            gate.release.notify_one();
            assert!(task.await.unwrap().is_err());
        }
        assert!(h.restart().capture_child_admission(&root).is_err());
    }
}

#[tokio::test]
async fn reopening_refuses_projection_identity_corruption() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let report = h.runtime.stop_subtree(&root).await.unwrap();
    let id = root.clone();
    h.store
        .transaction(move |tx| {
            tx.execute("UPDATE session SET directory='replaced' WHERE id=?1", [id])?;
            Ok(())
        })
        .unwrap();
    assert!(reopen(&h.runtime, &report).await.is_err());
    assert!(h.restart().capture_child_admission(&root).is_err());
}

#[tokio::test]
async fn reopening_after_owned_job_settlement_does_not_restart_the_job() {
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
    let stopped = h.runtime.stop_subtree(&root).await.unwrap();
    assert_eq!(stopped.status, SubtreeStopStatus::Acknowledged);
    reopen(&h.runtime, &stopped).await.unwrap();
    assert_eq!(h.runtime.job(&job.id).unwrap().status, JobStatus::Cancelled);
    assert!(!h.runtime.is_running(&nested));
    assert!(h.runtime.capture_child_admission(&nested).is_ok());
}

#[tokio::test]
async fn reopening_refuses_gaps_in_acknowledged_session_history() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let mut input = Admission::text("acknowledged input", Delivery::Queue);
    input.resume = false;
    h.runtime.admit(&root, input).await.unwrap();
    let stopped = h.runtime.stop_subtree(&root).await.unwrap();
    let id = root.clone();
    h.store
        .transaction(move |tx| {
            tx.execute(
                "DELETE FROM event WHERE aggregate_id=?1 AND type='session.prompt.admitted.1'",
                [id],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(reopen(&h.runtime, &stopped).await.is_err());
    assert!(h.restart().capture_child_admission(&root).is_err());
}

#[tokio::test]
async fn reopened_scope_still_refuses_persisted_old_writer_authority_after_restart() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let nested = child(&h, &root).await;
    let owner = h.runtime.claim_child_execution(&root, &nested).unwrap();
    let target = nested.clone();
    let data: String = h.store.read(move |db| {
        Ok(db.query_row("SELECT data FROM event WHERE type='child.execution.changed.1'
            AND json_extract(data,'$.child_id')=?1 AND json_extract(data,'$.status')='held' LIMIT 1",
            [target], |row| row.get(0))?)
    }).unwrap();
    let record: serde_json::Value = serde_json::from_str(&data).unwrap();
    drop(owner);
    let stopped = h.runtime.stop_subtree(&root).await.unwrap();
    reopen(&h.runtime, &stopped).await.unwrap();
    let restarted = h.restart();
    assert!(restarted.capture_child_admission(&root).is_ok());
    let error = h
        .store
        .append(
            "op_stale",
            Expected::Seq(-1),
            vec![NewEvent::new(
                "native.activity.changed.1",
                json!({"session_id":root,"status":"pending","phase":"reserved",
            "admission_bindings":record["admission_bindings"]}),
            )],
        )
        .unwrap_err();
    assert!(error.to_string().contains("fenced"), "{error}");
    restarted
        .create_session(CreateSession {
            parent_id: Some(root),
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
}
