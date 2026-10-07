//! Resume attempt and prompt admission commit together, with schema-preserving replay.
mod support;
use cyber_server::runtime::{Admission, CreateSession, Delivery, StructuredSchema};
use serde_json::json;
use support::{Harness, Setup, text, tools};

#[tokio::test]
async fn a_pending_resume_clears_terminal_output_atomically_and_survives_restart() {
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![
                tools(&[("first", "return_result", "1")]),
                tools(&[("second", "return_result", "2")]),
            ],
        )],
        ..Default::default()
    });
    let parent = h.session().await;
    let id = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            subagent_name: Some("inspection".into()),
            output_schema: Some(StructuredSchema::new(json!({"type":"integer"})).unwrap()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    h.runtime
        .admit(&id, Admission::text("initial", Delivery::Queue))
        .await
        .unwrap();
    h.runtime.wait_idle(&id).await;
    assert_eq!(h.state(&id).await.structured_result(), Some(&json!(1)));
    let owner = h.runtime.claim_child_execution(&parent, &id).unwrap();
    let mut admission = Admission::text("follow up", Delivery::Queue);
    admission.resume = false;
    admission.message_id = Some("msg_followup".into());
    let receipt = h
        .runtime
        .resume_child(&owner, admission.clone(), "inspection".into(), None)
        .await
        .unwrap();
    let seq = h.state(&id).await.last_seq;
    assert!(h.state(&id).await.structured_result().is_none());
    assert_eq!(
        h.runtime
            .resume_child(&owner, admission, "inspection".into(), None)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(h.state(&id).await.last_seq, seq);
    h.runtime.shutdown().await;
    drop(owner);
    let restarted = h.restart();
    let state = restarted.state(&id).await.unwrap();
    assert!(state.structured_result().is_none());
    assert_eq!(
        state.output_schema().unwrap().schema(),
        &json!({"type":"integer"})
    );
    assert_eq!(
        restarted
            .resolve_subagent(&parent, "inspection")
            .await
            .unwrap()
            .id,
        id
    );
    restarted.wake(&id).await.unwrap();
    restarted.wait_idle(&id).await;
    assert_eq!(
        restarted.state(&id).await.unwrap().structured_result(),
        Some(&json!(2))
    );
    assert_eq!(h.models.requests("test/main").len(), 2);
}

#[tokio::test]
async fn background_attempt_reports_only_new_priced_usage() {
    use cyber_server::runtime::JobAttempt;
    let h = Harness::new(Setup {
        scripts: vec![(
            "test/main",
            vec![text("first"), text("second"), text("notice handled")],
        )],
        ..Default::default()
    });
    let parent = h.session().await;
    let id = h
        .runtime
        .create_session(CreateSession {
            directory: h.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.clone()),
            subagent_name: Some("inspection".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    h.runtime
        .admit(&id, Admission::text("initial", Delivery::Queue))
        .await
        .unwrap();
    h.runtime.wait_idle(&id).await;
    let state = h.state(&id).await;
    let baseline = h.runtime.job_usage(&id).unwrap();
    let owner = h.runtime.claim_child_execution(&parent, &id).unwrap();
    h.runtime
        .resume_child(
            &owner,
            Admission::text("follow up", Delivery::Queue),
            "inspection".into(),
            None,
        )
        .await
        .unwrap();
    let (weak, child) = (h.runtime.downgrade(), id.clone());
    let job = h
        .runtime
        .start_child_job_attempt(
            &parent,
            &id,
            JobAttempt {
                name: "inspection".into(),
                description: "Continue project inspection".into(),
                usage: baseline,
            },
            None,
            Box::pin(async move {
                weak.wait_idle(&child)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(json!({"text":"second"}))
            }),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if h.runtime.job(&job.id).unwrap().notified {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let completed = h.runtime.job(&job.id).unwrap();
    assert_eq!(completed.tokens, 110);
    assert!((completed.cost - state.totals.cost).abs() < 1e-12);
    assert!(!completed.unpriced);
    drop(owner);
}
