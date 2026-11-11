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
