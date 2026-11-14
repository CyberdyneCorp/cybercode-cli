//! Durable input bindings and real request/lease lifetimes in shared Sessions.
mod support;
use cyber_server::runtime::{
    Admission, Delivery, InputClient, PermissionAsk, PermissionReply, Runtime,
};
use std::{sync::Arc, time::Duration};
use support::*;
fn setup() -> Harness {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("c1", "shell", "{}")]),
                text("first done"),
                tools(&[("c2", "shell", "{}")]),
                text("second done"),
            ],
        )],
        ..Default::default()
    });
    h.tools.set("shell", Behavior::Ask("same resource".into()));
    h
}
async fn attach(h: &Harness, session: &str) -> String {
    h.runtime
        .add_input_client(session, &h.repo.display().to_string())
        .await
        .unwrap()
        .client_id
}
fn bound(text: &str, client: Option<&str>, delivery: Delivery) -> Admission {
    let mut input = Admission::text(text, delivery);
    input.input_client = Some(InputClient {
        client_id: client.map(str::to_owned),
    });
    input
}
async fn pending(
    runtime: &Runtime,
    session: &str,
    count: usize,
) -> Vec<cyber_server::runtime::PendingRequest> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let requests = runtime.pending_requests(Some(session));
            if requests.len() == count {
                break requests;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("request count did not settle")
}
#[tokio::test]
async fn releasing_one_input_client_does_not_borrow_another_client_or_browser() {
    let h = setup();
    let session = h.session().await;
    let directory = h.repo.display().to_string();
    let first = attach(&h, &session).await;
    let second = attach(&h, &session).await;
    let _browser = h
        .runtime
        .add_interactive_client(&session, &directory)
        .await
        .unwrap();
    let _stream = h
        .runtime
        .attach_client(cyber_server::runtime::presence::Watching::All);
    let a = h
        .runtime
        .admit(&session, bound("first", Some(&first), Delivery::Queue))
        .await
        .unwrap();
    let old = pending(&h.runtime, &session, 1).await.remove(0);
    let invocation = h.tools.executed.lock().unwrap()[0].clone();
    assert!(invocation.asker.attended());
    let b = h
        .runtime
        .admit(&session, bound("second", Some(&second), Delivery::Queue))
        .await
        .unwrap();
    h.runtime
        .release_input_client(&session, &directory, &first)
        .await
        .unwrap();
    assert!(
        !invocation.asker.attended(),
        "borrowed another client's presence"
    );
    let next = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(next) = h
                .runtime
                .pending_requests(Some(&session))
                .into_iter()
                .find(|request| request.id != old.id)
            {
                break next;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(next.input_message_ids, vec![b.message_id]);
    assert_ne!(a.message_id, next.input_message_ids[0]);
    assert_eq!(
        h.state(&session).await.calls["c1"].output.as_deref(),
        Some("Denied: no approver")
    );
    assert!(
        h.runtime
            .reply_permission(&old.id, PermissionReply::Once)
            .await
            .is_err()
    );
    h.runtime
        .reply_permission(&next.id, PermissionReply::Once)
        .await
        .unwrap();
    h.settle(&session).await;
    h.runtime.shutdown().await;
}
#[tokio::test]
async fn explicit_unattended_binding_never_borrows_session_presence() {
    let h = setup();
    let session = h.session().await;
    h.runtime
        .add_interactive_client(&session, &h.repo.display().to_string())
        .await
        .unwrap();
    h.runtime
        .admit(&session, bound("unattended", None, Delivery::Queue))
        .await
        .unwrap();
    h.settle(&session).await;
    assert!(h.runtime.pending_requests(Some(&session)).is_empty());
    assert_eq!(
        h.state(&session).await.calls["c1"].output.as_deref(),
        Some("Denied: no approver")
    );
    h.runtime.shutdown().await;
}
#[tokio::test]
async fn input_binding_retry_is_immutable_and_commit_failure_admits_nothing() {
    let h = setup();
    let session = h.session().await;
    let directory = h.repo.display().to_string();
    let first = attach(&h, &session).await;
    let second = attach(&h, &session).await;
    let mut input = bound("held", Some(&first), Delivery::Hold);
    input.message_id = Some("msg_scoped".into());
    h.store.set_append_fault(Some(Arc::new(|_, event| {
        event.kind == "session.input.client_bound.1"
    })));
    assert!(h.runtime.admit(&session, input.clone()).await.is_err());
    assert!(h.state(&session).await.input("msg_scoped").is_none());
    h.store.set_append_fault(None);
    let receipt = h.runtime.admit(&session, input.clone()).await.unwrap();
    h.runtime
        .release_input_client(&session, &directory, &first)
        .await
        .unwrap();
    assert_eq!(
        h.runtime
            .admit(&session, input.clone())
            .await
            .unwrap()
            .admitted_seq,
        receipt.admitted_seq
    );
    input.input_client = Some(InputClient {
        client_id: Some(second),
    });
    assert!(
        h.runtime.admit(&session, input).await.is_err(),
        "rebound an existing admission"
    );
    let records = h
        .store
        .read_events(&session, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .filter(|event| event.kind == "session.input.client_bound.1")
        .count();
    assert_eq!(records, 1);
    let other = h.session().await;
    assert!(
        h.runtime
            .admit(&other, bound("foreign", Some(&first), Delivery::Hold))
            .await
            .is_err()
    );
    h.runtime.shutdown().await;
}
#[tokio::test]
async fn restart_retains_binding_without_recreating_live_authority() {
    let h = setup();
    let session = h.session().await;
    let directory = h.repo.display().to_string();
    let first = attach(&h, &session).await;
    let input = h
        .runtime
        .admit(&session, bound("restart", Some(&first), Delivery::Hold))
        .await
        .unwrap();
    let restarted = h.restart();
    restarted
        .add_interactive_client(&session, &directory)
        .await
        .unwrap();
    restarted
        .add_input_client(&session, &directory)
        .await
        .unwrap();
    restarted
        .release(&session, &input.message_id, Delivery::Queue)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), restarted.wait_idle(&session))
        .await
        .unwrap();
    assert!(restarted.pending_requests(Some(&session)).is_empty());
    assert_eq!(
        restarted.state(&session).await.unwrap().calls["c1"]
            .output
            .as_deref(),
        Some("Denied: no approver")
    );
    restarted.shutdown().await;
    h.runtime.shutdown().await;
}
fn ask() -> PermissionAsk {
    PermissionAsk {
        action: "bash".into(),
        resources: vec!["same resource".into()],
        always_patterns: vec!["same *".into()],
        metadata: serde_json::json!({}),
    }
}
#[tokio::test]
async fn permission_reply_cascades_stay_with_the_input_client() {
    for reply in [
        PermissionReply::Always,
        PermissionReply::Reject {
            message: Some("decline own request".into()),
        },
    ] {
        let h = Harness::new(Setup {
            scripts: vec![(
                "test/main",
                vec![
                    tools(&[("c1", "clock", "{}")]),
                    text("first"),
                    tools(&[("c2", "clock", "{}")]),
                    text("second"),
                ],
            )],
            ..Default::default()
        });
        let session = h.session().await;
        let directory = h.repo.display().to_string();
        let first = attach(&h, &session).await;
        let second = attach(&h, &session).await;
        h.runtime
            .admit(&session, bound("first", Some(&first), Delivery::Queue))
            .await
            .unwrap();
        h.settle(&session).await;
        h.runtime
            .admit(&session, bound("second", Some(&second), Delivery::Queue))
            .await
            .unwrap();
        h.settle(&session).await;
        let calls = h.tools.executed.lock().unwrap().clone();
        let (a, b) = (calls[0].asker.clone(), calls[1].asker.clone());
        let first_request = tokio::spawn(async move { a.permission(ask()).await });
        let old = pending(&h.runtime, &session, 1).await.remove(0);
        let other = tokio::spawn(async move { b.permission(ask()).await });
        let requests = pending(&h.runtime, &session, 2).await;
        let other_id = requests
            .iter()
            .find(|request| request.id != old.id)
            .unwrap()
            .id
            .clone();
        h.runtime
            .reply_permission(&old.id, reply.clone())
            .await
            .unwrap();
        assert_eq!(first_request.await.unwrap(), reply);
        assert_eq!(
            h.runtime.pending_requests(Some(&session))[0].id,
            other_id,
            "reply crossed input clients"
        );
        assert!(!other.is_finished());
        h.runtime
            .release_input_client(&session, &directory, &second)
            .await
            .unwrap();
        assert_eq!(other.await.unwrap(), PermissionReply::Unattended);
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn wrong_reply_type_preserves_the_original_owned_request() {
    let h = setup();
    let session = h.session().await;
    let client = attach(&h, &session).await;
    h.runtime
        .admit(&session, bound("first", Some(&client), Delivery::Queue))
        .await
        .unwrap();
    let request = pending(&h.runtime, &session, 1).await.remove(0);
    assert!(
        h.runtime
            .answer_question(&request.id, cyber_server::runtime::QuestionReply::Dismissed)
            .await
            .is_err()
    );
    assert_eq!(h.runtime.pending_requests(Some(&session))[0].id, request.id);
    h.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    h.settle(&session).await;
    h.runtime.shutdown().await;
}

/// A background subagent Job dispatched from the root's pending `c1` call, carrying
/// that call's complete input cohort.
async fn subagent(h: &Harness, root: &str) -> String {
    let invocation = h.tools.executed.lock().unwrap()[0].clone();
    let reservation = h
        .runtime
        .reserve_child_job_for_call(root, &invocation)
        .await
        .unwrap();
    let child = child_session(h, root).await;
    let job = h
        .runtime
        .start_child_job(
            root,
            &child,
            "scan".into(),
            "Inspect sources".into(),
            Some(reservation),
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    assert!(job.input_origin.is_some());
    child
}
async fn child_session(h: &Harness, parent: &str) -> String {
    h.runtime
        .create_session(cyber_server::runtime::CreateSession {
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
async fn child_ask(runtime: &Runtime, child: &str) -> tokio::task::JoinHandle<PermissionReply> {
    let asker = runtime.operation_asker(child).await.unwrap();
    tokio::spawn(async move { asker.permission(ask()).await })
}
async fn owned(
    runtime: &Runtime,
    child: &str,
    count: usize,
) -> Vec<cyber_server::runtime::PendingRequest> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let requests: Vec<_> = runtime
                .pending_requests(Some(child))
                .into_iter()
                .filter(|request| request.session_id == child)
                .collect();
            if requests.len() == count {
                break requests;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("descendant request count did not settle")
}

#[tokio::test]
async fn a_subagent_request_carries_the_parents_input_cohort() {
    let h = setup();
    let root = h.session().await;
    let client = attach(&h, &root).await;
    let input = h
        .runtime
        .admit(&root, bound("delegate", Some(&client), Delivery::Queue))
        .await
        .unwrap();
    pending(&h.runtime, &root, 1).await;
    let child = subagent(&h, &root).await;
    let grandchild_job = {
        let reservation = h.runtime.reserve_child_job(&child).await.unwrap();
        let grandchild = child_session(&h, &child).await;
        h.runtime
            .start_child_job(
                &child,
                &grandchild,
                "nested".into(),
                "Nested".into(),
                Some(reservation),
                Box::pin(futures::future::pending()),
            )
            .await
            .unwrap();
        grandchild
    };
    let asked = child_ask(&h.runtime, &child).await;
    let request = owned(&h.runtime, &child, 1).await.remove(0);
    assert_eq!(request.root_session_id.as_deref(), Some(root.as_str()));
    assert_eq!(
        request.root_input_message_ids,
        vec![input.message_id.clone()]
    );
    assert!(
        h.runtime
            .pending_requests(Some(&root))
            .iter()
            .any(|routed| routed.id == request.id
                && routed.root_input_message_ids == request.root_input_message_ids),
        "the root's view lacks the cohort"
    );
    let durable = h
        .store
        .read_events(&child, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .find(|event| event.kind == "permission.asked.1")
        .unwrap();
    assert_eq!(durable.data["root_session_id"], root.as_str());
    assert_eq!(
        durable.data["root_input_message_ids"],
        serde_json::json!([input.message_id])
    );
    h.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    assert_eq!(asked.await.unwrap(), PermissionReply::Once);
    // The nested link has no input origin of its own, so the grandchild is unproven.
    let nested = child_ask(&h.runtime, &grandchild_job).await;
    let unproven = owned(&h.runtime, &grandchild_job, 1).await.remove(0);
    assert!(unproven.root_session_id.is_none() && unproven.root_input_message_ids.is_empty());
    h.runtime.shutdown().await;
    drop(nested);
}

#[tokio::test]
async fn a_descendant_request_waits_only_for_its_bound_input_client() {
    let h = setup();
    let root = h.session().await;
    let directory = h.repo.display().to_string();
    let first = attach(&h, &root).await;
    let second = attach(&h, &root).await;
    let _browser = h
        .runtime
        .add_interactive_client(&root, &directory)
        .await
        .unwrap();
    h.runtime
        .admit(&root, bound("delegate", Some(&first), Delivery::Queue))
        .await
        .unwrap();
    pending(&h.runtime, &root, 1).await;
    let child = subagent(&h, &root).await;
    let asked = child_ask(&h.runtime, &child).await;
    let request = owned(&h.runtime, &child, 1).await.remove(0);
    h.runtime
        .release_input_client(&root, &directory, &second)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!asked.is_finished(), "another client's release settled it");
    h.runtime
        .release_input_client(&root, &directory, &first)
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), asked)
            .await
            .unwrap()
            .unwrap(),
        PermissionReply::Unattended,
        "borrowed the browser's or another client's attendance"
    );
    assert!(
        h.runtime
            .reply_permission(&request.id, PermissionReply::Once)
            .await
            .is_err()
    );
    let later = child_ask(&h.runtime, &child).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), later)
            .await
            .unwrap()
            .unwrap(),
        PermissionReply::Unattended,
        "an expired cohort reopened through other attendance"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn mixed_or_detached_descendant_requests_grant_no_ownership() {
    let h = setup();
    let root = h.session().await;
    let directory = h.repo.display().to_string();
    let client = attach(&h, &root).await;
    let _browser = h
        .runtime
        .add_interactive_client(&root, &directory)
        .await
        .unwrap();
    h.runtime
        .admit(&root, bound("delegate", Some(&client), Delivery::Queue))
        .await
        .unwrap();
    pending(&h.runtime, &root, 1).await;
    let mixed = subagent(&h, &root).await;
    let detached_reservation = h.runtime.reserve_child_job(&root).await.unwrap();
    h.runtime
        .start_child_job(
            &root,
            &mixed,
            "again".into(),
            "Without provenance".into(),
            Some(detached_reservation),
            Box::pin(futures::future::pending()),
        )
        .await
        .unwrap();
    let detached = child_session(&h, &root).await;
    let mut asks = Vec::new();
    for child in [&mixed, &detached] {
        asks.push(child_ask(&h.runtime, child).await);
        let request = owned(&h.runtime, child, 1).await.remove(0);
        assert!(
            request.root_session_id.is_none() && request.root_input_message_ids.is_empty(),
            "{child} was stamped with an unproven cohort"
        );
    }
    h.runtime
        .release_input_client(&root, &directory, &client)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        asks.iter().all(|ask| !ask.is_finished()),
        "an unowned request followed the input client"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn descendant_cohort_survives_restart_as_provenance_not_attendance() {
    let h = setup();
    let root = h.session().await;
    let directory = h.repo.display().to_string();
    let client = attach(&h, &root).await;
    let input = h
        .runtime
        .admit(&root, bound("delegate", Some(&client), Delivery::Queue))
        .await
        .unwrap();
    pending(&h.runtime, &root, 1).await;
    let child = subagent(&h, &root).await;
    let restarted = h.restart();
    restarted
        .add_interactive_client(&root, &directory)
        .await
        .unwrap();
    restarted.add_input_client(&root, &directory).await.unwrap();
    let job = restarted
        .jobs(Some(&root))
        .unwrap()
        .into_iter()
        .find(|job| job.child_id == child)
        .unwrap();
    assert_eq!(
        job.input_origin.unwrap().input_message_ids,
        vec![input.message_id]
    );
    let asked = child_ask(&restarted, &child).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), asked)
            .await
            .unwrap()
            .unwrap(),
        PermissionReply::Unattended,
        "restart recreated the bound client's authority"
    );
    assert!(restarted.pending_requests(Some(&child)).is_empty());
    restarted.shutdown().await;
    h.runtime.shutdown().await;
}
