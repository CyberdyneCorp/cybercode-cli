//! Receipt recovery is grounded in real child Jobs and historical operation authority.
mod support;
use cyber_server::runtime::{DelegationStatus, Job, JobStatus, NoSnapshots, UserSubtask};
use cyber_store::{Expected, NewEvent};
use serde_json::{Value, json};
use std::sync::Arc;
use support::flow::{Flow, text};

fn request() -> UserSubtask {
    UserSubtask {
        skill_command: None,
        admission_id: None,
        prompt: "same child task".into(),
        agent: Some("general".into()),
        attachments: vec![],
        max_steps: Some(2),
    }
}

fn fixture() -> Flow {
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![text("first summary"), text("second summary")],
        false,
        Arc::new(NoSnapshots),
        vec![(
            "test/child",
            vec![text("first child"), text("second child")],
        )],
    );
    flow.f
        .set_config(json!({"agents":{"general":{"model":"test/child"}}}));
    flow
}

async fn launched(flow: &Flow, parent: &str, id: &str) -> (Job, Value) {
    flow.runtime
        .start_delegation(parent, id, request())
        .await
        .unwrap();
    let job = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(data) = flow.runtime.delegation(parent, id).unwrap()
                && data.status == DelegationStatus::Admitted
            {
                let job = flow.runtime.job(data.job_id.as_deref().unwrap()).unwrap();
                if job.status == JobStatus::Completed {
                    break job;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    flow.settle(parent).await;
    let data = flow
        .f
        .store
        .read_events(id, -1, 20)
        .unwrap()
        .events
        .into_iter()
        .find(|event| {
            event.data["data"]["status"] == "pending" && event.data["data"]["phase"] == "launching"
        })
        .unwrap()
        .data;
    (job, data)
}

fn missing_receipt(flow: &Flow, id: &str, data: Value) {
    let seq = flow.f.store.aggregate_seq(id).unwrap().unwrap();
    // Restore the launching receipt to model a crash before the host's final acknowledgement.
    flow.f
        .store
        .append(
            id,
            Expected::Seq(seq),
            vec![NewEvent::new("delegation.changed.1", data)],
        )
        .unwrap();
}

#[tokio::test]
async fn lost_launch_receipt_links_only_its_bound_job_without_dispatch() {
    let mut flow = fixture();
    let parent = flow.session("bypass").await;
    let (job, receipt) = launched(&flow, &parent, "op_receipt_first").await;
    let (other, _) = launched(&flow, &parent, "op_receipt_second").await;
    missing_receipt(&flow, "op_receipt_first", receipt);
    flow.restart_default_runtime().await;
    assert_eq!(
        flow.runtime
            .delegation(&parent, "op_receipt_first")
            .unwrap()
            .unwrap()
            .status,
        DelegationStatus::Unknown
    );
    let requests = flow.main.requests().len();
    let recovered = flow
        .runtime
        .reconcile_delegation(&parent, "op_receipt_first")
        .await
        .unwrap();
    assert_eq!(recovered.status, DelegationStatus::Admitted);
    assert_eq!(recovered.job_id.as_deref(), Some(job.id.as_str()));
    assert_ne!(recovered.job_id.as_deref(), Some(other.id.as_str()));
    assert_eq!(
        flow.runtime.job(&job.id).unwrap().status,
        JobStatus::Completed
    );
    assert_eq!(
        flow.runtime
            .reconcile_delegation(&parent, "op_receipt_first")
            .await
            .unwrap()
            .job_id,
        recovered.job_id
    );
    assert_eq!(
        flow.runtime
            .start_delegation(&parent, "op_receipt_first", request())
            .await
            .unwrap()
            .job_id,
        recovered.job_id
    );
    assert_eq!(flow.runtime.jobs(Some(&parent)).unwrap().len(), 2);
    assert_eq!(flow.requests("test/child").len(), 2);
    assert_eq!(flow.main.requests().len(), requests);
    let stranger = flow.session("bypass").await;
    assert!(
        flow.runtime
            .reconcile_delegation(&stranger, "op_receipt_first")
            .await
            .is_err()
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn missing_origin_evidence_remains_unknown() {
    let mut flow = fixture();
    let parent = flow.session("bypass").await;
    let (_, mut receipt) = launched(&flow, &parent, "op_receipt_unproven").await;
    receipt
        .as_object_mut()
        .unwrap()
        .remove("admission_bindings");
    missing_receipt(&flow, "op_receipt_unproven", receipt);
    flow.restart_default_runtime().await;
    let seq = flow.f.store.aggregate_seq("op_receipt_unproven").unwrap();
    assert_eq!(
        flow.runtime
            .reconcile_delegation(&parent, "op_receipt_unproven")
            .await
            .unwrap()
            .status,
        DelegationStatus::Unknown
    );
    assert_eq!(
        flow.f.store.aggregate_seq("op_receipt_unproven").unwrap(),
        seq
    );
    assert_eq!(flow.runtime.jobs(Some(&parent)).unwrap().len(), 1);
    assert_eq!(flow.requests("test/child").len(), 1);
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn ambiguous_job_evidence_does_not_rewrite_the_receipt() {
    let mut flow = fixture();
    let parent = flow.session("bypass").await;
    let (_, receipt) = launched(&flow, &parent, "op_receipt_ambiguous").await;
    let mut started = flow
        .f
        .store
        .read_events(&parent, -1, 100)
        .unwrap()
        .events
        .into_iter()
        .find(|event| event.kind == "job.started.1")
        .unwrap()
        .data;
    started["id"] = "job_duplicate_evidence".into();
    started["admission_bindings"] = receipt["admission_bindings"].clone();
    started["admission_bindings"][0]["operations"] = json!(["op_receipt_ambiguous"]);
    missing_receipt(&flow, "op_receipt_ambiguous", receipt);

    flow.f
        .store
        .append(
            &parent,
            Expected::Any,
            vec![NewEvent::new("job.started.1", started)],
        )
        .unwrap();
    flow.restart_default_runtime().await;
    let seq = flow.f.store.aggregate_seq("op_receipt_ambiguous").unwrap();
    let outcome = flow
        .runtime
        .reconcile_delegation(&parent, "op_receipt_ambiguous")
        .await
        .unwrap();
    assert_eq!(outcome.status, DelegationStatus::Unknown);
    assert!(outcome.job_id.is_none());
    assert_eq!(
        flow.f.store.aggregate_seq("op_receipt_ambiguous").unwrap(),
        seq
    );
    assert_eq!(flow.requests("test/child").len(), 1);
    flow.runtime.shutdown().await;
}
