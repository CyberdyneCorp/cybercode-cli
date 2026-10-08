mod support;
use cyber_server::runtime::*;
use cyber_store::{Expected, NewEvent};
use serde_json::{Value, json};
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
fn input() -> Admission {
    let mut input = Admission::text("resume", Delivery::Queue);
    input.resume = false;
    input
}

#[test]
#[ignore = "invoked by cross-process ownership test"]
fn ownership_worker() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("CYBER_OWNERSHIP_TEST_DIR").expect("worker directory"),
    );
    let parent = std::env::var("CYBER_OWNERSHIP_TEST_PARENT").unwrap();
    let target = std::env::var("CYBER_OWNERSHIP_TEST_TARGET").unwrap();
    let busy = std::env::var("CYBER_OWNERSHIP_TEST_BUSY").unwrap() == "true";
    let store = std::sync::Arc::new(support::open_store(&directory.join("cyber.db")));
    let runtime = support::runtime(
        &store,
        &support::models(vec![], 200_000, &[]),
        &support::Tools::new(),
        &directory,
        Default::default(),
        None,
    );
    let claim = runtime.claim_child_execution(&parent, &target);
    assert_eq!(claim.is_err(), busy);
}

#[tokio::test]
async fn independent_process_writer_respects_live_owner_and_acknowledged_release() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let owner = h.runtime.claim_child_execution(&parent, &target).unwrap();
    let run = |busy: bool| {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "ownership_worker", "--ignored", "--nocapture"])
            .env("CYBER_OWNERSHIP_TEST_DIR", h.dir.path())
            .env("CYBER_OWNERSHIP_TEST_PARENT", &parent)
            .env("CYBER_OWNERSHIP_TEST_TARGET", &target)
            .env("CYBER_OWNERSHIP_TEST_BUSY", busy.to_string())
            .status()
            .unwrap()
    };
    assert!(run(true).success());
    drop(owner);
    assert!(run(false).success());
}

#[tokio::test]
async fn child_result_owner_is_exclusive_across_runtimes_until_released() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let owner = h.runtime.claim_child_execution(&parent, &target).unwrap();
    let other = h.restart();
    assert!(other.claim_child_execution(&parent, &target).is_err());
    drop(owner);
    assert!(other.claim_child_execution(&parent, &target).is_ok());
}

#[tokio::test]
async fn foreign_parent_cannot_claim_another_parents_child() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let foreign = h.session().await;
    let target = child(&h, &foreign).await;
    assert!(h.runtime.claim_child_execution(&parent, &target).is_err());
    assert!(h.runtime.claim_child_execution(&foreign, &target).is_ok());
}

#[tokio::test]
async fn original_child_target_fence_refuses_resume_without_new_input() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let owner = h.runtime.claim_child_execution(&parent, &target).unwrap();
    h.runtime.fence_subtree_admissions(&target).await.unwrap();
    assert!(matches!(
        h.runtime
            .resume_child(&owner, input(), "child".into(), None)
            .await,
        Err(RuntimeError::Conflict(_))
    ));
    assert!(h.runtime.state(&target).await.unwrap().inbox.is_empty());
}

#[tokio::test]
async fn foreign_target_result_owner_prevents_false_subtree_acknowledgement() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let owner = h.runtime.claim_child_execution(&parent, &target).unwrap();
    let other = h.restart();
    let report = other.stop_subtree(&target).await.unwrap();
    assert_eq!(report.status, SubtreeStopStatus::Unknown);
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("Child result owner"))
    );
    assert!(h.runtime.capture_child_admission(&parent).is_ok());
    drop(owner);
    assert_eq!(
        other.stop_subtree(&target).await.unwrap().status,
        SubtreeStopStatus::Acknowledged
    );
}

// Native Location guard regression.
#[derive(Default)]
struct LocationHost {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl ToolHost for LocationHost {
    fn definitions(&self, _: &TurnContext) -> Vec<ToolDef> {
        vec![]
    }
    fn execute(
        &self,
        _: Invocation,
        _: tokio_util::sync::CancellationToken,
    ) -> futures::future::BoxFuture<'_, ToolOutcome> {
        Box::pin(async { ToolOutcome::Aborted })
    }
    fn claim_location<'a>(
        &'a self,
        _: &'a SessionInfo,
        _: bool,
        _: tokio_util::sync::CancellationToken,
    ) -> futures::future::BoxFuture<'a, Result<LocationLease, String>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(LocationLease::unmanaged())
        })
    }
}

#[tokio::test]
async fn setup_keeps_original_target_authority_while_location_admission_waits() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let host = Arc::new(LocationHost::default());
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
    let authority = runtime.capture_child_admission(&parent).unwrap();
    let executed = Arc::new(AtomicBool::new(false));
    let work = {
        let runtime = runtime.clone();
        let target = target.clone();
        let executed = executed.clone();
        tokio::spawn(async move {
            let cancel = tokio_util::sync::CancellationToken::new();
            authority
                .run(
                    cancel.clone(),
                    runtime.own_session_worktree_setup(&target, cancel, async {
                        executed.store(true, Ordering::SeqCst);
                        Ok(())
                    }),
                )
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(1), host.entered.notified())
        .await
        .unwrap();
    runtime.fence_subtree_admissions(&target).await.unwrap();
    host.release.notify_one();
    assert!(work.await.unwrap().is_err());
    assert!(!executed.load(Ordering::SeqCst));
    runtime.shutdown().await;
}

// The following cases exercise the new callback capability and private ledger.
#[tokio::test]
async fn child_sourced_continuation_survives_parent_interrupt_but_not_subtree_fence() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let owner = h
        .runtime
        .claim_child_continuation(&parent, &target)
        .unwrap();
    h.runtime.interrupt(&parent).await.unwrap();
    h.runtime
        .resume_child(&owner, input(), "child".into(), None)
        .await
        .unwrap();
    assert_eq!(h.runtime.state(&target).await.unwrap().inbox.len(), 1);
    h.runtime.fence_subtree_admissions(&parent).await.unwrap();
    assert!(matches!(
        h.runtime
            .resume_child(&owner, input(), "child".into(), None)
            .await,
        Err(RuntimeError::Conflict(_))
    ));
    assert_eq!(h.runtime.state(&target).await.unwrap().inbox.len(), 1);
}

#[tokio::test]
async fn ancestor_callback_capture_retains_original_child_scope() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let authority = h.runtime.capture_child_admission(&target).unwrap();
    let ancestor = authority
        .clone()
        .run(tokio_util::sync::CancellationToken::new(), async {
            h.runtime.capture_child_admission(&parent).unwrap()
        })
        .await;
    h.runtime.fence_subtree_admissions(&target).await.unwrap();
    assert!(ancestor.verify(&h.runtime, &parent).is_err());
    assert!(
        authority
            .run(tokio_util::sync::CancellationToken::new(), async {
                h.runtime.capture_child_admission(&parent)
            })
            .await
            .is_err()
    );
    assert!(h.runtime.capture_child_admission(&parent).is_ok());
}

fn held(h: &Harness) -> (String, Value) {
    h.store.read(|db| {
        Ok(db.query_row("SELECT aggregate_id,data FROM event WHERE type='child.execution.changed.1' AND json_extract(data,'$.status')='held' ORDER BY rowid DESC LIMIT 1", [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?)
    }).map(|(id, data)| (id, serde_json::from_str(&data).unwrap())).unwrap()
}

#[tokio::test]
async fn target_guards_survive_callback_capture_and_writer_commit() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let owner = h.runtime.claim_child_execution(&parent, &target).unwrap();
    let sibling = owner
        .run(tokio_util::sync::CancellationToken::new(), async {
            child(&h, &parent).await
        })
        .await;
    let mut created = h.store.read_events(&sibling, -1, 1).unwrap().events[0]
        .data
        .clone();
    assert!(
        !created["admission_bindings"][0]["guards"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let late = cyber_core::ids::new_id("ses");
    created["info"]["id"] = json!(late);
    h.runtime.fence_subtree_admissions(&target).await.unwrap();
    assert!(
        h.store
            .append(
                &late,
                Expected::Seq(-1),
                vec![NewEvent::new("session.created.1", created)]
            )
            .is_err()
    );
    assert!(h.store.read_events(&late, -1, 1).unwrap().events.is_empty());
}

#[tokio::test]
async fn released_owner_revokes_retained_callback_authority() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let owner = h.runtime.claim_child_execution(&parent, &target).unwrap();
    let authority = owner
        .run(tokio_util::sync::CancellationToken::new(), async {
            h.runtime.capture_child_admission(&parent).unwrap()
        })
        .await;
    let sibling = owner
        .run(tokio_util::sync::CancellationToken::new(), async {
            child(&h, &parent).await
        })
        .await;
    let mut created = h.store.read_events(&sibling, -1, 1).unwrap().events[0]
        .data
        .clone();
    let late = cyber_core::ids::new_id("ses");
    created["info"]["id"] = json!(late);
    drop(owner);
    assert!(authority.verify(&h.runtime, &parent).is_err());
    assert!(h.runtime.capture_child_admission(&parent).is_ok());
    assert!(
        h.store
            .append(
                &late,
                Expected::Seq(-1),
                vec![NewEvent::new("session.created.1", created)]
            )
            .is_err()
    );
}

#[tokio::test]
async fn writer_refuses_closed_target_even_with_other_valid_target_guards() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let sibling = child(&h, &parent).await;
    let _owner = h.runtime.claim_child_execution(&parent, &sibling).unwrap();
    let (_, mut record) = held(&h);
    h.runtime.close_subtree_admissions(&target).await.unwrap();
    record["child_id"] = json!(target);
    let operation = cyber_core::ids::new_id("op");
    assert!(
        h.store
            .append(
                &operation,
                Expected::Seq(-1),
                vec![NewEvent::new("child.execution.changed.1", record)]
            )
            .is_err()
    );
    assert!(
        h.store
            .read_events(&operation, -1, 1)
            .unwrap()
            .events
            .is_empty()
    );
}

#[tokio::test]
async fn release_is_bound_to_original_owner_and_survives_source_closure() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let owner = h.runtime.claim_child_execution(&parent, &target).unwrap();
    let (operation, mut record) = held(&h);
    record["status"] = json!("released");
    record.as_object_mut().unwrap().remove("admission_bindings");
    record["child_id"] = json!("foreign");
    assert!(
        h.store
            .append(
                &operation,
                Expected::Seq(0),
                vec![NewEvent::new("child.execution.changed.1", record)]
            )
            .is_err()
    );
    h.runtime.close_subtree_admissions(&parent).await.unwrap();
    drop(owner);
    let events = h.store.read_events(&operation, -1, 10).unwrap().events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].data["status"], "released");
}
