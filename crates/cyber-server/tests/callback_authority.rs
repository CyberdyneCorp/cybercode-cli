mod support;
use cyber_server::runtime::*;
use cyber_store::{Expected, NewEvent};
use serde_json::json;
use support::{Harness, Setup};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn unrelated_callback_capture_cannot_escape_original_source_revocation() {
    let h = Harness::new(Setup::default());
    let source = h.session().await;
    let unrelated = h.session().await;
    let original = h.runtime.capture_child_admission(&source).unwrap();
    let captured = original
        .run(CancellationToken::new(), async {
            h.runtime.capture_child_admission(&unrelated).unwrap()
        })
        .await;
    h.runtime.fence_subtree_admissions(&source).await.unwrap();
    assert!(captured.verify(&h.runtime, &unrelated).is_err());
    assert!(h.runtime.capture_child_admission(&unrelated).is_ok());
}

#[tokio::test]
async fn unrelated_callback_writer_retains_original_source_guard() {
    let h = Harness::new(Setup::default());
    let source = h.session().await;
    let unrelated = h.session().await;
    let original = h.runtime.capture_child_admission(&source).unwrap();
    let created = original
        .run(CancellationToken::new(), async {
            h.runtime
                .create_session(CreateSession {
                    parent_id: Some(unrelated.clone()),
                    directory: h.repo.display().to_string(),
                    model: "test/main".into(),
                    ..Default::default()
                })
                .await
                .unwrap()
        })
        .await;
    let mut data = h.store.read_events(&created.id, -1, 1).unwrap().events[0]
        .data
        .clone();
    let delayed = cyber_core::ids::new_id("ses");
    data["info"]["id"] = json!(delayed);
    h.runtime.fence_subtree_admissions(&source).await.unwrap();
    assert!(
        h.store
            .append(
                &delayed,
                Expected::Seq(-1),
                vec![NewEvent::new("session.created.1", data)]
            )
            .is_err()
    );
    assert!(
        h.store
            .read_events(&delayed, -1, 1)
            .unwrap()
            .events
            .is_empty()
    );
}

#[tokio::test]
async fn unrelated_idle_receipt_preserves_origin_after_terminal_binding_removal() {
    let h = Harness::new(Setup::default());
    let source = h.session().await;
    let unrelated = h.session().await;
    let original = h.runtime.capture_child_admission(&source).unwrap();
    let created = original
        .run(CancellationToken::new(), async {
            h.runtime
                .create_session(CreateSession {
                    parent_id: Some(unrelated.clone()),
                    directory: h.repo.display().to_string(),
                    model: "test/main".into(),
                    ..Default::default()
                })
                .await
                .unwrap()
        })
        .await;
    let bindings =
        h.store.read_events(&created.id, -1, 1).unwrap().events[0].data["admission_bindings"]
            .clone();
    let operation = cyber_core::ids::new_id("op");
    h.store
        .append(
            &operation,
            Expected::Seq(-1),
            vec![NewEvent::new(
                "native.activity.changed.1",
                json!({"session_id":unrelated,"status":"pending",
            "phase":"reserved","admission_bindings":bindings}),
            )],
        )
        .unwrap();
    h.store
        .append(
            &operation,
            Expected::Seq(0),
            vec![NewEvent::new(
                "native.activity.changed.1",
                json!({"session_id":unrelated,"status":"settled",
            "phase":"launching"}),
            )],
        )
        .unwrap();
    let report = h.runtime.stop_subtree(&source).await.unwrap();
    assert_eq!(report.status, SubtreeStopStatus::Acknowledged);
    h.store
        .append(
            &operation,
            Expected::Seq(1),
            vec![NewEvent::new(
                "native.activity.changed.1",
                json!({"session_id":unrelated,"status":"unknown",
            "phase":"launching"}),
            )],
        )
        .unwrap();
    assert!(
        h.runtime
            .reopen_subtree(
                &source,
                &report.scope_id,
                report.receipt_id.as_deref().unwrap()
            )
            .await
            .is_err()
    );
    assert_eq!(
        h.runtime.stop_subtree(&source).await.unwrap().status,
        SubtreeStopStatus::Unknown
    );
}
