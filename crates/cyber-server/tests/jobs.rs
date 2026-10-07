//! Durable process-local task recovery and idempotent notification.
mod support;
use cyber_server::runtime::{Admission, CreateSession, Delivery, JobStatus};
use serde_json::json;
use support::{Harness, Setup, text};

async fn child(h: &Harness, parent: &str) -> String {
    h.runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.into()),
            title: Some("Background child".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn shutdown_settles_ownership_and_restart_delivers_one_interrupted_notice() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("notice handled")])],
        ..Default::default()
    });
    let parent = h.session().await;
    let child = child(&h, &parent).await;
    let job = h
        .runtime
        .start_child_job(
            &parent,
            &child,
            "scan".into(),
            "Inspect project sources".into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), h.runtime.shutdown())
        .await
        .unwrap();
    let stopped = h.runtime.job(&job.id).unwrap();
    assert_eq!(stopped.status, JobStatus::Interrupted);
    assert!(!stopped.notified);
    assert!(h.state(&parent).await.inbox.is_empty());
    let restarted = h.restart();
    restarted.recover_jobs().await.unwrap();
    restarted.wait_idle(&parent).await;
    restarted.recover_jobs().await.unwrap();
    assert_eq!(restarted.state(&parent).await.unwrap().inbox.len(), 1);
    assert_eq!(h.models.requests("test/main").len(), 1);
    assert!(restarted.job(&job.id).unwrap().notified);
}

#[tokio::test]
async fn a_notice_admitted_before_notification_settlement_is_not_duplicated() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("notice handled")])],
        ..Default::default()
    });
    let parent = h.session().await;
    let child = child(&h, &parent).await;
    let job = h
        .runtime
        .start_child_job(
            &parent,
            &child,
            "scan".into(),
            "Inspect project sources".into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    h.runtime.shutdown().await;
    let first = h.restart();
    first.recover_jobs().await.unwrap();
    first.wait_idle(&parent).await;
    let mut notice = first.job(&job.id).unwrap();
    // Reconstruct the durable window after admission but before the acknowledgement.
    notice.notified = false;
    first.shutdown().await;
    h.store
        .append(
            &parent,
            cyber_store::Expected::Any,
            vec![cyber_store::NewEvent::new(
                "job.ended.1",
                serde_json::to_value(notice).unwrap(),
            )],
        )
        .unwrap();
    let second = h.restart();
    second.recover_jobs().await.unwrap();
    second.wait_idle(&parent).await;
    assert_eq!(second.state(&parent).await.unwrap().inbox.len(), 1);
    assert_eq!(h.models.requests("test/main").len(), 1);
}

#[tokio::test]
async fn duplicate_active_names_do_not_register_a_second_owner() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("cancel notice handled")])],
        ..Default::default()
    });
    let parent = h.session().await;
    let first = child(&h, &parent).await;
    let second = child(&h, &parent).await;
    let job = h
        .runtime
        .start_child_job(
            &parent,
            &first,
            "scan".into(),
            "Inspect project sources".into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    assert!(
        h.runtime
            .start_child_job(
                &parent,
                &second,
                "scan".into(),
                "Inspect project sources".into(),
                None,
                Box::pin(async { Ok(json!({})) })
            )
            .await
            .is_err()
    );
    assert_eq!(h.runtime.jobs(Some(&parent)).unwrap().len(), 1);
    let cancelled = h.runtime.cancel_job(&job.id).await.unwrap();
    assert_eq!(cancelled.status, JobStatus::Cancelled);
}

#[tokio::test]
async fn a_panicked_owner_records_failure_and_does_not_leave_cancellation_waiting() {
    let h = Harness::new(Setup {
        scripts: vec![("test/main", vec![text("failure notice handled")])],
        ..Default::default()
    });
    let parent = h.session().await;
    let child = child(&h, &parent).await;
    let job = h
        .runtime
        .start_child_job(
            &parent,
            &child,
            "scan".into(),
            "Inspect project sources".into(),
            None,
            Box::pin(async { panic!("injected owner failure") }),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if h.runtime.job(&job.id).unwrap().status != JobStatus::Running {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let settled = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        h.runtime.cancel_job(&job.id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(settled.status, JobStatus::Error);
    assert!(settled.error.unwrap().contains("internal error"));
}

#[tokio::test]
async fn background_admission_fences_creation_and_registration_against_deletion() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let reservation = h.runtime.reserve_child_job(&parent).await.unwrap();
    let mut deletion = Box::pin(h.runtime.delete(&parent));
    assert!(futures::poll!(&mut deletion).is_pending());
    let child = child(&h, &parent).await;
    h.runtime
        .start_child_job(
            &parent,
            &child,
            "scan".into(),
            "Inspect project sources".into(),
            Some(reservation),
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), deletion)
        .await
        .unwrap()
        .unwrap();
    assert!(h.runtime.jobs(None).unwrap().is_empty());
    assert!(h.runtime.state(&child).await.is_err());
    assert!(h.runtime.reserve_child_job(&parent).await.is_err());
}

#[tokio::test]
async fn job_pages_are_bounded_and_preserve_every_record_across_cursors() {
    let h = Harness::new(Setup::default());
    let parent = h.session().await;
    let child = child(&h, &parent).await;
    let job = h
        .runtime
        .start_child_job(
            &parent,
            &child,
            "scan".into(),
            "Inspect project sources".into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    h.runtime.shutdown().await;
    let mut events = Vec::new();
    for number in 1..=3 {
        let mut record = job.clone();
        record.id = format!("job_seed_{number}");
        record.name = format!("scan-{number}");
        events.push(cyber_store::NewEvent::new(
            "job.started.1",
            serde_json::to_value(&record).unwrap(),
        ));
        record.status = JobStatus::Completed;
        record.notified = true;
        events.push(cyber_store::NewEvent::new(
            "job.ended.1",
            serde_json::to_value(&record).unwrap(),
        ));
    }
    h.store
        .append(&parent, cyber_store::Expected::Any, events)
        .unwrap();
    let (first, cursor) = h.runtime.jobs_page(Some(&parent), 2, None).unwrap();
    assert_eq!(first.len(), 2);
    let (second, end) = h
        .runtime
        .jobs_page(Some(&parent), 2, cursor.as_deref())
        .unwrap();
    assert_eq!(second.len(), 2);
    assert!(end.is_none());
    let ids = first
        .into_iter()
        .chain(second)
        .map(|job| job.id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 4);
    assert!(h.runtime.jobs_page(Some(&parent), 201, None).is_err());
}

#[tokio::test]
async fn dropping_an_owner_recovers_a_still_running_job_without_redispatch() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                text("partial child work"),
                text("interruption notice handled"),
            ],
        )],
        ..Default::default()
    });
    let parent = h.session().await;
    let child = child(&h, &parent).await;
    h.runtime
        .admit(&child, Admission::text("inspect", Delivery::Queue))
        .await
        .unwrap();
    h.runtime.wait_idle(&child).await;
    let job = h
        .runtime
        .start_child_job(
            &parent,
            &child,
            "scan".into(),
            "Inspect project sources".into(),
            None,
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    let weak = h.runtime.downgrade();
    let restarted = h.restart();
    drop(h.runtime);
    assert!(weak.upgrade().is_none());
    assert_eq!(restarted.job(&job.id).unwrap().status, JobStatus::Running);
    restarted.recover_jobs().await.unwrap();
    restarted.wait_idle(&parent).await;
    let restored = restarted.job(&job.id).unwrap();
    assert_eq!(restored.status, JobStatus::Interrupted);
    assert!(restored.notified);
    assert_eq!(restored.tokens, 110);
    assert!(restored.cost > 0.0);
    assert!(!restored.unpriced);
    assert!(restored.error.unwrap().contains("not redispatched"));
    restarted.recover_jobs().await.unwrap();
    assert_eq!(restarted.state(&parent).await.unwrap().inbox.len(), 1);
    assert_eq!(h.models.requests("test/main").len(), 2);
}
