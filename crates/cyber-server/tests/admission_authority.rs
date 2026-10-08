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
