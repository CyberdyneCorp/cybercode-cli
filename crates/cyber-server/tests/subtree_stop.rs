mod support;
use cyber_server::runtime::*;
use cyber_store::{Expected, NewEvent};
use serde_json::json;
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
async fn job(h: &Harness, parent: &str, child: &str, name: &str) -> Job {
    h.runtime
        .start_child_job(
            parent,
            child,
            name.into(),
            name.into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn subtree_stop_settles_nested_jobs_and_preserves_unrelated_work_and_input() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let nested = child(&h, &root).await;
    let leaf = child(&h, &nested).await;
    let other = h.session().await;
    let other_child = child(&h, &other).await;
    let first = job(&h, &root, &nested, "first").await;
    let second = job(&h, &nested, &leaf, "second").await;
    let unrelated = job(&h, &other, &other_child, "unrelated").await;
    let mut input = Admission::text("kept", Delivery::Queue);
    input.resume = false;
    h.runtime.admit(&root, input).await.unwrap();
    let before = h.runtime.state(&root).await.unwrap().inbox.len();
    let report = h.runtime.stop_subtree(&root).await.unwrap();
    assert_eq!(
        report.status,
        SubtreeStopStatus::Acknowledged,
        "{:?}",
        report.problems
    );
    assert!(report.persisted);
    assert_eq!(
        h.runtime.job(&first.id).unwrap().status,
        JobStatus::Cancelled
    );
    assert_eq!(
        h.runtime.job(&second.id).unwrap().status,
        JobStatus::Cancelled
    );
    assert_eq!(
        h.runtime.job(&unrelated.id).unwrap().status,
        JobStatus::Running
    );
    assert_eq!(h.runtime.state(&root).await.unwrap().inbox.len(), before);
    assert!(matches!(
        h.restart().capture_child_admission(&leaf),
        Err(RuntimeError::Conflict(_))
    ));
}

#[tokio::test]
async fn target_scope_stops_parent_owned_job_without_stopping_sibling() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let sibling = child(&h, &parent).await;
    let owned = job(&h, &parent, &target, "owned").await;
    let other = job(&h, &parent, &sibling, "other").await;
    let report = h.runtime.stop_subtree(&target).await.unwrap();
    assert_eq!(
        report.status,
        SubtreeStopStatus::Acknowledged,
        "{:?}",
        report.problems
    );
    assert_eq!(
        h.runtime.job(&owned.id).unwrap().status,
        JobStatus::Cancelled
    );
    assert_eq!(h.runtime.job(&other.id).unwrap().status, JobStatus::Running);
    assert!(h.runtime.capture_child_admission(&parent).is_ok());
}

#[tokio::test]
async fn held_result_owner_prevents_false_acknowledgement_and_late_claims() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let target = child(&h, &parent).await;
    let owner = h.runtime.claim_child_execution(&parent, &target).unwrap();
    let report = h.runtime.stop_subtree(&target).await.unwrap();
    assert_eq!(report.status, SubtreeStopStatus::Unknown);
    assert!(report.persisted);
    assert!(h.runtime.claim_child_execution(&parent, &target).is_err());
    drop(owner);
    let retry = h.runtime.stop_subtree(&target).await.unwrap();
    assert_eq!(
        retry.status,
        SubtreeStopStatus::Acknowledged,
        "{:?}",
        retry.problems
    );
    assert_eq!(retry.scope_id, report.scope_id);
    assert!(h.restart().capture_child_admission(&target).is_err());
}

#[tokio::test]
async fn stop_receipt_cannot_claim_a_different_scope_owner() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    h.runtime.close_subtree_admissions(&root).await.unwrap();
    let seq = h.store.aggregate_seq(&root).unwrap().unwrap();
    assert!(
        h.store
            .append(
                &root,
                Expected::Seq(seq),
                vec![NewEvent::new(
                    "session.subtree.stopped.1",
                    json!({"session_id":root,"scope_id":"foreign",
        "status":"acknowledged","problems":[],"persisted":true})
                )]
            )
            .is_err()
    );
    assert_eq!(h.store.aggregate_seq(&root).unwrap().unwrap(), seq);
}

#[tokio::test]
async fn another_runtime_cannot_acknowledge_a_job_without_its_live_owner() {
    let h = Harness::new(Setup::default());
    let root = h.session().await;
    let nested = child(&h, &root).await;
    let owned = job(&h, &root, &nested, "foreign-owner").await;
    let other_runtime = h.restart();
    let report = other_runtime.stop_subtree(&root).await.unwrap();
    assert_eq!(report.status, SubtreeStopStatus::Unknown);
    assert!(report.persisted);
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains(&owned.id))
    );
    assert_eq!(h.runtime.job(&owned.id).unwrap().status, JobStatus::Running);
    assert!(h.restart().capture_child_admission(&nested).is_err());
    other_runtime.shutdown().await;
}
