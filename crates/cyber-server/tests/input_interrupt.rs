//! Actual runtime control: shared Session ownership and retained retry identity.
mod support;
use cyber_server::runtime::{
    Admission, Delivery, InputInterrupt, InputInterruptProblem, InputInterruptStatus, InputStatus,
    PermissionReply, ProcessJob, Runtime,
};
use std::time::Duration;
use support::*;
use tokio_util::sync::CancellationToken;

fn request(id: &str, inputs: &[&str]) -> InputInterrupt {
    InputInterrupt {
        request_id: id.into(),
        input_message_ids: inputs.iter().map(|id| (*id).into()).collect(),
    }
}
async fn receipt(
    runtime: &Runtime,
    session: &str,
    id: &str,
) -> cyber_server::runtime::InputInterruptReceipt {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let receipt = runtime
                .input_interrupt_receipt(session, id)
                .unwrap()
                .unwrap();
            if receipt.status != InputInterruptStatus::Requested {
                return receipt;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("input interruption did not settle")
}
async fn pending(h: &Harness, session: &str, call: &str) -> cyber_server::runtime::PendingRequest {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(request) = h
                .runtime
                .pending_requests(Some(session))
                .into_iter()
                .find(|request| request.call_id == call)
            {
                return request;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("owned question/permission did not appear")
}
fn setup() -> Harness {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c1", "shell", "{}")]),
                tools(&[("c2", "shell", "{}")]),
                text("done"),
            ],
        )],
        ..Default::default()
    });
    h.tools.set("shell", Behavior::Ask("test".into()));
    h
}
#[tokio::test]
async fn queued_interruption_preserves_active_foreign_request_and_retry_never_retargets() {
    let h = setup();
    let session = h.session().await;
    let first = h
        .runtime
        .admit(&session, Admission::text("first", Delivery::Queue))
        .await
        .unwrap();
    let active = pending(&h, &session, "c1").await;
    let second = h
        .runtime
        .admit(&session, Admission::text("second", Delivery::Queue))
        .await
        .unwrap();
    let stop = request("stop-second", &[&second.message_id]);
    h.runtime
        .interrupt_inputs(&session, stop.clone())
        .await
        .unwrap();
    let settled = receipt(&h.runtime, &session, "stop-second").await;
    assert_eq!(settled.status, InputInterruptStatus::Acknowledged);
    assert!(!settled.drain_selected);
    assert_eq!(settled.refused, 1);
    assert_eq!(
        h.state(&session)
            .await
            .input(&second.message_id)
            .unwrap()
            .status,
        InputStatus::Refused
    );
    assert_eq!(h.runtime.pending_requests(Some(&session))[0].id, active.id);
    h.runtime
        .reply_permission(&active.id, PermissionReply::Once)
        .await
        .unwrap();
    let replacement = pending(&h, &session, "c2").await;
    let before = h.state(&session).await.last_seq;
    let retried = h.runtime.interrupt_inputs(&session, stop).await.unwrap();
    assert_eq!(retried.status, InputInterruptStatus::Acknowledged);
    assert_eq!(h.state(&session).await.last_seq, before);
    assert_eq!(
        h.runtime.pending_requests(Some(&session))[0].id,
        replacement.id
    );
    assert!(
        h.runtime
            .interrupt_inputs(&session, request("stop-second", &[&first.message_id]))
            .await
            .is_err()
    );
    h.runtime.interrupt(&session).await.unwrap();
    h.runtime.shutdown().await;
}
#[tokio::test]
async fn active_interruption_resumes_other_input_and_does_not_wait_for_its_request() {
    let h = setup();
    let session = h.session().await;
    let first = h
        .runtime
        .admit(&session, Admission::text("first", Delivery::Queue))
        .await
        .unwrap();
    pending(&h, &session, "c1").await;
    let second = h
        .runtime
        .admit(&session, Admission::text("second", Delivery::Queue))
        .await
        .unwrap();
    h.runtime
        .interrupt_inputs(&session, request("stop-first", &[&first.message_id]))
        .await
        .unwrap();
    let settled = receipt(&h.runtime, &session, "stop-first").await;
    assert_eq!(
        settled.status,
        InputInterruptStatus::Acknowledged,
        "{settled:?}"
    );
    assert!(settled.drain_selected);
    let other = pending(&h, &session, "c2").await;
    assert_eq!(other.input_message_ids, vec![second.message_id]);
    assert!(h.runtime.is_running(&session));
    h.runtime.interrupt(&session).await.unwrap();
    h.runtime.shutdown().await;
}
#[tokio::test]
async fn mixed_cohort_does_not_authorize_cancelling_the_shared_drain() {
    let h = setup();
    let session = h.session().await;
    let mut admission = Admission::text("first", Delivery::Steer);
    admission.resume = false;
    let first = h.runtime.admit(&session, admission).await.unwrap();
    let second = h
        .runtime
        .admit(&session, Admission::text("second", Delivery::Steer))
        .await
        .unwrap();
    let active = pending(&h, &session, "c1").await;
    assert_eq!(
        active.input_message_ids,
        vec![first.message_id.clone(), second.message_id]
    );
    h.runtime
        .interrupt_inputs(&session, request("stop-first", &[&first.message_id]))
        .await
        .unwrap();
    let settled = receipt(&h.runtime, &session, "stop-first").await;
    assert_eq!(settled.status, InputInterruptStatus::Unknown);
    assert_eq!(settled.problems, vec![InputInterruptProblem::MixedDrain]);
    assert!(!settled.drain_selected);
    assert_eq!(h.runtime.pending_requests(Some(&session))[0].id, active.id);
    h.runtime.interrupt(&session).await.unwrap();
    h.runtime.shutdown().await;
}
fn process(h: &Harness, session: &str, notify: bool) -> ProcessJob {
    ProcessJob {
        session_id: session.into(),
        kind: "bash".into(),
        command: "test-owned-job".into(),
        description: "fixture".into(),
        notify,
        output_dir: h.dir.path().join("jobs"),
        max_jobs: 200,
    }
}
#[tokio::test]
async fn cancels_only_proven_process_job_and_keeps_detached_job_alive() {
    let h = setup();
    let session = h.session().await;
    let first = h
        .runtime
        .admit(&session, Admission::text("first", Delivery::Queue))
        .await
        .unwrap();
    pending(&h, &session, "c1").await;
    let invocation = h.tools.executed.lock().unwrap()[0].clone();
    let reservation = h
        .runtime
        .reserve_process_job_for_call(process(&h, &session, true), &invocation)
        .await
        .unwrap();
    let owned = h
        .runtime
        .start_process_job(
            reservation,
            Box::new(|io| {
                Box::pin(async move {
                    io.stop.cancelled().await;
                    Ok(None)
                })
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        owned.input_origin.as_ref().unwrap().input_message_ids,
        vec![first.message_id.clone()]
    );
    let detached = h
        .runtime
        .reserve_process_job(process(&h, &session, false))
        .await
        .unwrap();
    let detached = h
        .runtime
        .start_process_job(
            detached,
            Box::new(|io| {
                Box::pin(async move {
                    io.stop.cancelled().await;
                    Ok(None)
                })
            }),
        )
        .await
        .unwrap();
    h.runtime
        .interrupt_inputs(&session, request("stop-first", &[&first.message_id]))
        .await
        .unwrap();
    let settled = receipt(&h.runtime, &session, "stop-first").await;
    assert_eq!(
        settled.status,
        InputInterruptStatus::Acknowledged,
        "{settled:?}"
    );
    assert_eq!(settled.jobs_cancelled, 1);
    assert_eq!(
        h.runtime.job(&owned.id).unwrap().status,
        cyber_server::runtime::JobStatus::Cancelled
    );
    assert_eq!(
        h.runtime.job(&detached.id).unwrap().status,
        cyber_server::runtime::JobStatus::Running
    );
    assert!(
        h.state(&session)
            .await
            .input(&format!("msg_handback_{}", owned.id))
            .is_none()
    );
    h.runtime.cancel_job_quietly(&detached.id).await.unwrap();
    h.runtime.shutdown().await;
}
#[tokio::test]
async fn late_handback_is_refused_after_intent_and_cannot_restart_interrupted_input() {
    let h = setup();
    let session = h.session().await;
    let first = h
        .runtime
        .admit(&session, Admission::text("first", Delivery::Queue))
        .await
        .unwrap();
    pending(&h, &session, "c1").await;
    let invocation = h.tools.executed.lock().unwrap()[0].clone();
    let reservation = h
        .runtime
        .reserve_process_job_for_call(process(&h, &session, true), &invocation)
        .await
        .unwrap();
    let finish = CancellationToken::new();
    let gate = finish.clone();
    let job = h
        .runtime
        .start_process_job(
            reservation,
            Box::new(move |_| {
                Box::pin(async move {
                    gate.cancelled().await;
                    Ok(Some(0))
                })
            }),
        )
        .await
        .unwrap();
    // Hold publication admission so cleanup remains observable after requester
    // disposal, while this already registered Job completes on its own.
    let barrier = h
        .runtime
        .reserve_process_job(process(&h, &session, false))
        .await
        .unwrap();
    h.runtime
        .interrupt_inputs(&session, request("late-stop", &[&first.message_id]))
        .await
        .unwrap();
    finish.cancel();
    let handback = format!("msg_handback_{}", job.id);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if h.state(&session).await.input(&handback).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("late handback was not durably admitted");
    assert_eq!(
        h.runtime
            .input_interrupt_receipt(&session, "late-stop")
            .unwrap()
            .unwrap()
            .status,
        InputInterruptStatus::Requested
    );
    drop(barrier);
    let settled = receipt(&h.runtime, &session, "late-stop").await;
    assert_eq!(
        settled.status,
        InputInterruptStatus::Acknowledged,
        "{settled:?}"
    );
    h.settle(&session).await;
    assert_eq!(
        h.state(&session).await.input(&handback).unwrap().status,
        InputStatus::Refused
    );
    assert!(
        h.store
            .read_events(&session, -1, 500)
            .unwrap()
            .events
            .iter()
            .filter(|event| event.kind == "session.step.started.1")
            .all(|event| !event.data["input_message_ids"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(&handback))))
    );
    h.runtime.shutdown().await;
}
#[tokio::test]
async fn interrupted_receipt_from_old_runtime_requires_reconciliation_without_signalling_new_work()
{
    let h = setup();
    let session = h.session().await;
    let first = h
        .runtime
        .admit(&session, Admission::text("first", Delivery::Queue))
        .await
        .unwrap();
    pending(&h, &session, "c1").await;
    h.runtime
        .interrupt_inputs(&session, request("original-stop", &[&first.message_id]))
        .await
        .unwrap();
    assert_eq!(
        receipt(&h.runtime, &session, "original-stop").await.status,
        InputInterruptStatus::Acknowledged
    );
    h.runtime.shutdown().await;
    h.store.append(&session,cyber_store::Expected::Any,vec![cyber_store::NewEvent::new("session.input.interruption.1",serde_json::json!({
        "request_id":"lost-stop","input_message_ids":[first.message_id],"generation":"old-runtime",
        "status":"requested","drain_selected":true,"refused":0,"jobs_cancelled":0,"problems":[]
    }))]).unwrap();
    let restarted = h.restart();
    let replacement = restarted
        .admit(&session, Admission::text("replacement", Delivery::Queue))
        .await
        .unwrap();
    let active = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(active) = restarted
                .pending_requests(Some(&session))
                .into_iter()
                .find(|request| request.call_id == "c2")
            {
                break active;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(active.input_message_ids, vec![replacement.message_id]);
    let observed = restarted
        .input_interrupt_receipt(&session, "lost-stop")
        .unwrap()
        .unwrap();
    assert_eq!(observed.status, InputInterruptStatus::Unknown);
    assert_eq!(
        observed.problems,
        vec![InputInterruptProblem::ReconciliationRequired]
    );
    let before = h.store.read_events(&session, -1, 500).unwrap().events.len();
    let retry = restarted
        .interrupt_inputs(&session, request("lost-stop", &[&first.message_id]))
        .await
        .unwrap();
    assert_eq!(retry.status, InputInterruptStatus::Unknown);
    assert_eq!(
        h.store.read_events(&session, -1, 500).unwrap().events.len(),
        before
    );
    assert_eq!(restarted.pending_requests(Some(&session))[0].id, active.id);
    restarted.interrupt(&session).await.unwrap();
    restarted.shutdown().await;
}

#[tokio::test]
async fn rejected_intent_commit_does_not_signal_or_refuse_any_input() {
    let h = setup();
    let session = h.session().await;
    let first = h
        .runtime
        .admit(&session, Admission::text("first", Delivery::Queue))
        .await
        .unwrap();
    let active = pending(&h, &session, "c1").await;
    let second = h
        .runtime
        .admit(&session, Admission::text("second", Delivery::Queue))
        .await
        .unwrap();
    let before = h.state(&session).await.last_seq;
    h.store
        .set_append_fault(Some(std::sync::Arc::new(|_, event| {
            event.kind == "session.input.interruption.1"
        })));
    assert!(
        h.runtime
            .interrupt_inputs(
                &session,
                request("refused-stop", &[&first.message_id, &second.message_id])
            )
            .await
            .is_err()
    );
    assert!(
        h.runtime
            .input_interrupt_receipt(&session, "refused-stop")
            .unwrap()
            .is_none()
    );
    assert_eq!(h.state(&session).await.last_seq, before);
    assert_eq!(
        h.state(&session)
            .await
            .input(&second.message_id)
            .unwrap()
            .status,
        InputStatus::Pending
    );
    assert_eq!(h.runtime.pending_requests(Some(&session))[0].id, active.id);
    h.store.set_append_fault(None);
    h.runtime
        .interrupt_inputs(
            &session,
            request("accepted-stop", &[&first.message_id, &second.message_id]),
        )
        .await
        .unwrap();
    assert_eq!(
        receipt(&h.runtime, &session, "accepted-stop").await.status,
        InputInterruptStatus::Acknowledged
    );
    h.runtime.shutdown().await;
}
#[tokio::test]
async fn failed_completion_write_reports_unknown_and_retry_does_not_capture_foreign_work() {
    let h = setup();
    let session = h.session().await;
    h.runtime
        .admit(&session, Admission::text("first", Delivery::Queue))
        .await
        .unwrap();
    let active = pending(&h, &session, "c1").await;
    let second = h
        .runtime
        .admit(&session, Admission::text("second", Delivery::Queue))
        .await
        .unwrap();
    h.store
        .set_append_fault(Some(std::sync::Arc::new(|_, event| {
            event.kind == "session.input.interruption.1" && event.data["status"] != "requested"
        })));
    let stop = request("completion-lost", &[&second.message_id]);
    h.runtime
        .interrupt_inputs(&session, stop.clone())
        .await
        .unwrap();
    let observed = receipt(&h.runtime, &session, "completion-lost").await;
    assert_eq!(observed.status, InputInterruptStatus::Unknown);
    assert!(
        observed
            .problems
            .contains(&InputInterruptProblem::ReconciliationRequired)
    );
    h.store.set_append_fault(None);
    let before = h.state(&session).await.last_seq;
    assert_eq!(
        h.runtime
            .interrupt_inputs(&session, stop)
            .await
            .unwrap()
            .status,
        InputInterruptStatus::Unknown
    );
    assert_eq!(h.state(&session).await.last_seq, before);
    assert_eq!(h.runtime.pending_requests(Some(&session))[0].id, active.id);
    h.runtime
        .interrupt_inputs(
            &session,
            request("cleanup", &[&active.input_message_ids[0]]),
        )
        .await
        .unwrap();
    receipt(&h.runtime, &session, "cleanup").await;
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn requester_disposal_retains_cleanup_through_shutdown_without_recursive_admission() {
    let h = setup();
    let session = h.session().await;
    let first = h
        .runtime
        .admit(&session, Admission::text("first", Delivery::Queue))
        .await
        .unwrap();
    pending(&h, &session, "c1").await;
    let barrier = h
        .runtime
        .reserve_process_job(process(&h, &session, false))
        .await
        .unwrap();
    // The response is dropped; cleanup remains runtime-owned behind publication.
    h.runtime
        .interrupt_inputs(&session, request("retained-stop", &[&first.message_id]))
        .await
        .unwrap();
    let runtime = h.runtime.clone();
    let shutdown = tokio::spawn(async move { runtime.shutdown().await });
    h.runtime.shutting_down().await;
    assert!(!shutdown.is_finished());
    assert_eq!(
        h.runtime
            .input_interrupt_receipt(&session, "retained-stop")
            .unwrap()
            .unwrap()
            .status,
        InputInterruptStatus::Requested
    );
    drop(barrier);
    tokio::time::timeout(Duration::from_secs(10), shutdown)
        .await
        .unwrap()
        .unwrap();
    let settled = receipt(&h.runtime, &session, "retained-stop").await;
    assert_eq!(
        settled.status,
        InputInterruptStatus::Acknowledged,
        "{settled:?}"
    );
}
