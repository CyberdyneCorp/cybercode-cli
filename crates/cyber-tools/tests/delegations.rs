//! Durable admission uses the real host's pre-effect launch boundary.
mod support;
use cyber_server::runtime::{Delegation, DelegationStatus, JobStatus, UserSubtask};
use serde_json::json;
use support::flow::{Flow, call, text};
fn request(prompt: &str) -> UserSubtask {
    UserSubtask {
        admission_id: None,
        prompt: prompt.into(),
        agent: Some("general".into()),
        attachments: vec![],
        max_steps: Some(2),
    }
}
async fn settled(flow: &Flow, parent: &str, id: &str) -> Delegation {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let data = flow.runtime.delegation(parent, id).unwrap().unwrap();
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
async fn queued_identity_replays_and_cancellation_preserves_an_existing_child() {
    let flow = Flow::new(vec![call("read", "read", json!({"path":".env"}))], true);
    flow.f.write(".env", "private");
    flow.f.set_config(json!({"agents":{"max_concurrent":1}}));
    let parent = flow.session("default").await;
    let first = flow
        .runtime
        .subtask(&parent, "existing child")
        .await
        .unwrap();
    flow.pending(&parent).await;
    let id = "op_queued";
    let started = flow
        .runtime
        .start_delegation(&parent, id, request("queued child"))
        .await
        .unwrap();
    assert_eq!(started.status, DelegationStatus::Pending);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        flow.runtime
            .start_delegation(&parent, id, request("queued child"))
            .await
            .unwrap()
            .status,
        DelegationStatus::Pending
    );
    assert!(
        flow.runtime
            .start_delegation(&parent, id, request("changed child"))
            .await
            .unwrap_err()
            .to_string()
            .contains("different input")
    );
    let other = flow.session("default").await;
    assert!(flow.runtime.delegation(&other, id).is_err());
    flow.runtime.cancel_delegation(&parent, id).await.unwrap();
    let done = settled(&flow, &parent, id).await;
    assert_eq!(done.status, DelegationStatus::Cancelled);
    assert!(done.job_id.is_none());
    assert_eq!(flow.runtime.jobs(Some(&parent)).unwrap().len(), 1);
    assert_eq!(
        flow.runtime.job(&first.id).unwrap().status,
        JobStatus::Running
    );
    assert!(flow.runtime.is_running(&first.child_id));
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        3
    );
    assert_eq!(
        flow.runtime
            .start_delegation(&parent, id, request("queued child"))
            .await
            .unwrap()
            .status,
        DelegationStatus::Cancelled
    );
    flow.runtime.cancel_job(&first.id).await.unwrap();
    flow.runtime.shutdown().await;
}
#[tokio::test]
async fn cancellation_tombstone_prevents_a_delayed_post_from_creating_work() {
    let flow = Flow::new(vec![], false);
    let parent = flow.session("dont-ask").await;
    let id = "op_delayed";
    assert_eq!(
        flow.runtime
            .cancel_delegation(&parent, id)
            .await
            .unwrap()
            .status,
        DelegationStatus::Cancelled
    );
    assert_eq!(
        flow.runtime
            .start_delegation(&parent, id, request("delayed packet"))
            .await
            .unwrap()
            .status,
        DelegationStatus::Cancelled
    );
    assert!(
        flow.runtime
            .start_delegation(&parent, id, request("different delayed packet"))
            .await
            .unwrap_err()
            .to_string()
            .contains("different input")
    );
    assert!(flow.runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert!(flow.main.requests().is_empty());
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        1
    );
}
#[tokio::test]
async fn lost_admission_response_replays_one_recorded_job_and_stop_is_scoped() {
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":".env"})),
            text("notice handled"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    let parent = flow.session("default").await;
    let id = "op_once";
    flow.runtime
        .start_delegation(&parent, id, request("inspect project"))
        .await
        .unwrap();
    let record = settled(&flow, &parent, id).await;
    assert_eq!(record.status, DelegationStatus::Admitted);
    let job_id = record.job_id.unwrap();
    flow.pending(&parent).await;
    assert_eq!(
        flow.runtime
            .start_delegation(&parent, id, request("inspect project"))
            .await
            .unwrap()
            .job_id
            .as_deref(),
        Some(job_id.as_str())
    );
    assert_eq!(flow.runtime.jobs(Some(&parent)).unwrap().len(), 1);
    flow.runtime.cancel_delegation(&parent, id).await.unwrap();
    assert_eq!(
        flow.runtime.job(&job_id).unwrap().status,
        JobStatus::Cancelled
    );
    flow.runtime.shutdown().await;
}
