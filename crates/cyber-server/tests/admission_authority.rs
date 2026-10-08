mod support;

use cyber_server::runtime::*;
use cyber_store::{Expected, NewEvent};
use serde_json::json;
use support::{Harness, Setup};

async fn child(
    h: &Harness,
    parent: &str,
    authority: Option<AdmissionAuthority>,
) -> Result<SessionInfo, RuntimeError> {
    h.runtime
        .create_session(CreateSession {
            parent_id: Some(parent.into()),
            admission_authority: authority,
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
}

#[tokio::test]
async fn source_interrupt_refuses_old_ticket_but_allows_fresh_launch() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let ticket = h.runtime.capture_child_admission(&parent).unwrap();
    h.runtime.interrupt(&parent).await.unwrap();
    assert!(matches!(
        child(&h, &parent, Some(ticket)).await,
        Err(RuntimeError::Conflict(_))
    ));
    child(&h, &parent, None).await.unwrap();
}

#[tokio::test]
async fn ancestor_drain_interrupt_preserves_background_authority_but_scope_fence_refuses_it() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let nested = child(&h, &parent, None).await.unwrap();
    let ticket = h.runtime.capture_child_admission(&nested.id).unwrap();
    h.runtime.interrupt(&parent).await.unwrap();
    ticket.verify(&h.runtime, &nested.id).unwrap();
    h.runtime.fence_subtree_admissions(&parent).await.unwrap();
    assert!(matches!(
        child(&h, &nested.id, Some(ticket)).await,
        Err(RuntimeError::Conflict(_))
    ));
    child(&h, &nested.id, None).await.unwrap();
}

#[tokio::test]
async fn tickets_refuse_other_runtime_and_source() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let other = h.session().await;
    let ticket = h.runtime.capture_child_admission(&parent).unwrap();
    assert!(ticket.verify(&h.runtime, &other).is_err());
    assert!(ticket.verify(&h.restart(), &parent).is_err());
}

#[tokio::test]
async fn writer_refuses_stale_bindings_without_child_history_or_projection() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let created = child(&h, &parent, None).await.unwrap();
    // Replay the already prepared creation payload after a committed cancellation boundary.
    let mut payload = h.store.read_events(&created.id, -1, 1).unwrap().events[0]
        .data
        .clone();
    let id = cyber_core::ids::new_id("ses");
    payload["info"]["id"] = json!(id);
    h.runtime.interrupt(&parent).await.unwrap();
    assert!(
        h.store
            .append(
                &id,
                Expected::Seq(-1),
                vec![NewEvent::new("session.created.1", payload)]
            )
            .is_err()
    );
    assert!(h.store.read_events(&id, -1, 10).unwrap().events.is_empty());
    assert!(matches!(
        h.runtime.state(&id).await,
        Err(RuntimeError::SessionNotFound(_))
    ));
}

#[tokio::test]
async fn deleted_and_recreated_source_cannot_revive_old_authority() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let authority = h.runtime.capture_child_admission(&parent).unwrap();
    h.runtime.delete(&parent).await.unwrap();
    h.runtime
        .create_session(CreateSession {
            id: Some(parent.clone()),
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(matches!(
        authority.verify(&h.runtime, &parent),
        Err(RuntimeError::Conflict(_))
    ));
}

#[tokio::test]
async fn capture_refuses_missing_source_and_cyclic_durable_ancestry() {
    let h = Harness::new(Setup::default());
    let missing = cyber_core::ids::new_id("ses");
    assert!(matches!(
        h.runtime.capture_child_admission(&missing),
        Err(RuntimeError::SessionNotFound(_))
    ));
    let parent = h.session().await;
    let id = parent.clone();
    h.store
        .transaction(move |tx| {
            tx.execute("UPDATE session SET parent_id=id WHERE id=?1", [&id])?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        h.runtime.capture_child_admission(&parent),
        Err(RuntimeError::Corrupt(_))
    ));
}

struct DelayedLocation {
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl ToolHost for DelayedLocation {
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        Vec::new()
    }
    fn execute(
        &self,
        _: Invocation,
        _: tokio_util::sync::CancellationToken,
    ) -> futures::future::BoxFuture<'_, ToolOutcome> {
        Box::pin(async { ToolOutcome::Failed("Unexpected tool execution".into()) })
    }
    fn claim_location<'a>(
        &'a self,
        info: &'a SessionInfo,
        _: bool,
        _: tokio_util::sync::CancellationToken,
    ) -> futures::future::BoxFuture<'a, Result<LocationLease, String>> {
        Box::pin(async move {
            if info.parent_id.is_some() {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(LocationLease::unmanaged())
        })
    }
}

#[tokio::test]
async fn cancellation_during_location_claim_returns_conflict_without_creating_child() {
    use std::sync::Arc;
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let host = Arc::new(DelayedLocation {
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
        interactive: true,
        snapshots: Arc::new(NoSnapshots),
    });
    let id = cyber_core::ids::new_id("ses");
    let task_runtime = runtime.clone();
    let request = CreateSession {
        id: Some(id.clone()),
        parent_id: Some(parent.clone()),
        directory: h.repo.display().to_string(),
        model: "test/main".into(),
        ..Default::default()
    };
    let task = tokio::spawn(async move { task_runtime.create_session(request).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), host.started.notified())
        .await
        .unwrap();
    runtime.interrupt(&parent).await.unwrap();
    host.release.notify_one();
    assert!(matches!(
        task.await.unwrap(),
        Err(RuntimeError::Conflict(_))
    ));
    assert!(h.store.read_events(&id, -1, 10).unwrap().events.is_empty());
    assert!(matches!(
        runtime.state(&id).await,
        Err(RuntimeError::SessionNotFound(_))
    ));
}

#[tokio::test]
async fn nested_callback_authority_retains_source_boundary_until_handoff() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let authority = h.runtime.capture_child_admission(&parent).unwrap();
    let nested = authority
        .run(tokio_util::sync::CancellationToken::new(), async {
            let nested = child(&h, &parent, None).await.unwrap();
            let ticket = h.runtime.capture_child_admission(&nested.id).unwrap();
            ticket.verify(&h.runtime, &nested.id).unwrap();
            h.runtime.interrupt(&parent).await.unwrap();
            assert!(matches!(
                ticket.verify(&h.runtime, &nested.id),
                Err(RuntimeError::Conflict(_))
            ));
            assert!(matches!(
                child(&h, &nested.id, None).await,
                Err(RuntimeError::Conflict(_))
            ));
            nested
        })
        .await;
    // A subsequent independent child action has no inherited callback ownership.
    h.runtime
        .capture_child_admission(&nested.id)
        .unwrap()
        .verify(&h.runtime, &nested.id)
        .unwrap();
    child(&h, &nested.id, None).await.unwrap();
}

#[tokio::test]
async fn writer_refuses_scoped_input_after_source_cancellation_without_inbox_changes() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let nested = child(&h, &parent, None).await.unwrap();
    let authority = h.runtime.capture_child_admission(&parent).unwrap();
    authority
        .run(tokio_util::sync::CancellationToken::new(), async {
            let mut admission = Admission::text("first", Delivery::Hold);
            admission.resume = false;
            h.runtime.admit(&nested.id, admission).await.unwrap();
        })
        .await;
    let events = h.store.read_events(&nested.id, -1, 10).unwrap().events;
    let mut payload = events
        .iter()
        .find(|event| event.kind == "session.prompt.admitted.1")
        .unwrap()
        .data
        .clone();
    payload["message_id"] = json!(cyber_core::ids::new_id("msg"));
    h.runtime.interrupt(&parent).await.unwrap();
    let seq = h.store.aggregate_seq(&nested.id).unwrap().unwrap();
    assert!(
        h.store
            .append(
                &nested.id,
                Expected::Seq(seq),
                vec![NewEvent::new("session.prompt.admitted.1", payload)]
            )
            .is_err()
    );
    assert_eq!(h.store.aggregate_seq(&nested.id).unwrap().unwrap(), seq);
    assert_eq!(h.runtime.state(&nested.id).await.unwrap().inbox.len(), 1);
}

#[tokio::test]
async fn writer_refuses_job_handoff_after_source_fence_but_terminal_settlement_remains_writable() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let nested = child(&h, &parent, None).await.unwrap();
    let job = h
        .runtime
        .start_child_job(
            &parent,
            &nested.id,
            "original".into(),
            "original".into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    let events = h.store.read_events(&parent, -1, 20).unwrap().events;
    let mut payload = events
        .iter()
        .find(|event| event.kind == "job.started.1")
        .unwrap()
        .data
        .clone();
    let id = cyber_core::ids::new_id("job");
    payload["id"] = json!(id);
    h.runtime.fence_subtree_admissions(&parent).await.unwrap();
    let seq = h.store.aggregate_seq(&parent).unwrap().unwrap();
    assert!(
        h.store
            .append(
                &parent,
                Expected::Seq(seq),
                vec![NewEvent::new("job.started.1", payload)]
            )
            .is_err()
    );
    assert_eq!(h.store.aggregate_seq(&parent).unwrap().unwrap(), seq);
    assert!(h.runtime.job(&id).is_err());
    assert_eq!(h.runtime.job(&job.id).unwrap().status, JobStatus::Running);
    assert_eq!(
        h.runtime.cancel_job(&job.id).await.unwrap().status,
        JobStatus::Cancelled
    );
}

#[tokio::test]
async fn writer_rolls_back_resumed_attempt_and_input_after_source_interruption() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let nested = child(&h, &parent, None).await.unwrap();
    let owner = h
        .runtime
        .claim_child_execution(&parent, &nested.id)
        .unwrap();
    let mut admission = Admission::text("held", Delivery::Hold);
    admission.resume = false;
    h.runtime
        .resume_child(&owner, admission, "named".into(), None)
        .await
        .unwrap();
    drop(owner);
    let original = h.store.read_events(&nested.id, -1, 20).unwrap().events;
    let resume = original
        .iter()
        .find(|event| event.kind == "session.subagent.resumed.1")
        .unwrap()
        .data
        .clone();
    let mut input = original
        .iter()
        .find(|event| event.kind == "session.prompt.admitted.1")
        .unwrap()
        .data
        .clone();
    input["message_id"] = json!(cyber_core::ids::new_id("msg"));
    h.runtime.interrupt(&parent).await.unwrap();
    let seq = h.store.aggregate_seq(&nested.id).unwrap().unwrap();
    assert!(
        h.store
            .append(
                &nested.id,
                Expected::Seq(seq),
                vec![
                    NewEvent::new("session.subagent.resumed.1", resume),
                    NewEvent::new("session.prompt.admitted.1", input)
                ]
            )
            .is_err()
    );
    assert_eq!(h.store.aggregate_seq(&nested.id).unwrap().unwrap(), seq);
    assert_eq!(
        h.store
            .read_events(&nested.id, -1, 20)
            .unwrap()
            .events
            .len(),
        original.len()
    );
    assert_eq!(h.runtime.state(&nested.id).await.unwrap().inbox.len(), 1);
}

#[tokio::test]
async fn fenced_callback_refuses_before_committing_a_staged_conversation_revert() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![support::text("answer")])],
        ..Default::default()
    });
    let parent = h.session().await;
    let nested = child(&h, &parent, None).await.unwrap();
    let receipt = h
        .runtime
        .admit(&nested.id, Admission::text("initial", Delivery::Queue))
        .await
        .unwrap();
    h.runtime.wait_idle(&nested.id).await;
    h.runtime
        .revert_stage(&nested.id, &receipt.message_id, RevertTarget::Conversation)
        .await
        .unwrap();
    let before = h.runtime.state(&nested.id).await.unwrap();
    let authority = h.runtime.capture_child_admission(&parent).unwrap();
    authority
        .run(tokio_util::sync::CancellationToken::new(), async {
            h.runtime.interrupt(&parent).await.unwrap();
            let mut admission = Admission::text("late", Delivery::Hold);
            admission.resume = false;
            assert!(matches!(
                h.runtime.admit(&nested.id, admission).await,
                Err(RuntimeError::Conflict(_))
            ));
        })
        .await;
    let after = h.runtime.state(&nested.id).await.unwrap();
    assert_eq!(
        after.last_seq, before.last_seq,
        "Fenced callback must not commit staged conversation changes"
    );
    assert!(after.revert.is_some());
    assert_eq!(after.entries.len(), before.entries.len());
}
