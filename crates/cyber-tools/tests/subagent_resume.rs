//! Resume reuses a direct child with real inference and preserved history.
mod support;
use cyber_server::runtime::{ToolHost, ToolOutcome};
use serde_json::{Value, json};
use support::flow::{Flow, call, text};
use tokio_util::sync::CancellationToken;

async fn invoke(flow: &Flow, parent: &str, input: Value) -> ToolOutcome {
    let mut inv = flow.f.invocation("default", "agent", input);
    inv.session_id = parent.into();
    flow.f.host.execute(inv, CancellationToken::new()).await
}

fn output(outcome: ToolOutcome) -> String {
    match outcome {
        ToolOutcome::Ok(output) => {
            assert!(
                serde_json::from_str::<Value>(&output)
                    .unwrap()
                    .get("result")
                    .is_none(),
                "typed result lost"
            );
            output
        }
        ToolOutcome::Structured { output, value } => {
            assert_eq!(
                serde_json::from_str::<Value>(&output).unwrap()["result"],
                value
            );
            output
        }
        other => panic!("expected a result, got {other:?}"),
    }
}

#[tokio::test]
async fn resume_by_name_and_id_reuses_the_child_and_its_history() {
    let flow = Flow::new(vec![text("first"), text("second"), text("third")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let first: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"initial","name":"inspection"}),
        )
        .await,
    ))
    .unwrap();
    let id = first["id"].as_str().unwrap();
    for (resume, expected) in [("inspection", "second"), (id, "third")] {
        let output = output(
            invoke(
                &flow,
                &parent,
                json!({"prompt":"follow up","resume":resume}),
            )
            .await,
        );
        let result: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(result["id"], id);
        assert_eq!(result["name"], "inspection");
        assert_eq!(result["text"], expected);
    }
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        2
    );
    assert_eq!(flow.main.requests().len(), 3);
    assert!(format!("{:?}", flow.main.requests()[1].messages).contains("first"));
}

#[tokio::test]
async fn resume_is_direct_parent_scoped_and_permission_gated() {
    let flow = Flow::new(vec![text("first")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let first: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"initial","name":"inspection"}),
        )
        .await,
    ))
    .unwrap();
    let other = flow.session("default").await;
    for reference in ["missing", first["id"].as_str().unwrap(), "inspection"] {
        assert!(
            support::failed(
                invoke(
                    &flow,
                    &other,
                    json!({"prompt":"follow up","resume":reference})
                )
                .await
            )
            .contains("Subagent not found")
        );
    }
    flow.f.set_config(json!({"permissions":{"agent":"deny"}}));
    assert!(
        support::failed(
            invoke(
                &flow,
                &parent,
                json!({"prompt":"follow up","resume":"inspection"})
            )
            .await
        )
        .contains("Permission denied")
    );
    assert_eq!(flow.main.requests().len(), 1);
}

#[tokio::test]
async fn resume_uses_the_existing_profile_and_refuses_identity_changes() {
    let flow = Flow::new(vec![text("first"), text("second")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let first: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"initial","agent":"explore"}),
        )
        .await,
    ))
    .unwrap();
    let second: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"follow up","resume":"explore"}),
        )
        .await,
    ))
    .unwrap();
    assert_eq!(second["id"], first["id"]);
    assert_eq!(
        flow.runtime
            .state(first["id"].as_str().unwrap())
            .await
            .unwrap()
            .info
            .agent,
        "explore"
    );
    for patch in [
        json!({"agent":"general"}),
        json!({"name":"renamed"}),
        json!({"model":"other/main"}),
    ] {
        let mut input = json!({"prompt":"follow up","resume":"explore"});
        input
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(support::failed(invoke(&flow, &parent, input).await).contains("cannot change"));
    }
    assert_eq!(flow.main.requests().len(), 2);
}

#[tokio::test]
async fn idle_child_with_an_unsettled_execution_owner_is_busy() {
    let flow = Flow::new(vec![text("first")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let first: Value = serde_json::from_str(&output(
        invoke(&flow, &parent, json!({"prompt":"initial"})).await,
    ))
    .unwrap();
    let id = first["id"].as_str().unwrap();
    let owner = flow.runtime.claim_child_execution(&parent, id).unwrap();
    assert!(!flow.runtime.is_running(id));
    assert!(
        support::failed(invoke(&flow, &parent, json!({"prompt":"follow up","resume":id})).await)
            .contains("Subagent busy")
    );
    assert_eq!(flow.main.requests().len(), 1);
    drop(owner);
}

#[tokio::test]
async fn structured_resume_returns_a_new_value_with_one_correction() {
    let flow = Flow::new(
        vec![
            call("first", "return_result", json!(1)),
            text("missing"),
            call("second", "return_result", json!(2)),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let first: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"initial","output_schema":{"type":"integer"}}),
        )
        .await,
    ))
    .unwrap();
    assert_eq!(first["result"], 1);
    let second: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"follow up","resume":first["id"]}),
        )
        .await,
    ))
    .unwrap();
    assert_eq!(second["id"], first["id"]);
    assert_eq!(second["result"], 2);
    assert_eq!(flow.main.requests().len(), 3);
}

async fn job_done(flow: &Flow, job: &str, parent: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if flow.runtime.job(job).unwrap().notified {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    flow.runtime.wait_idle(parent).await;
}

#[tokio::test]
async fn background_resume_creates_a_new_job_and_exactly_one_handback_per_attempt() {
    let flow = Flow::new(
        vec![
            text("first"),
            text("notice one"),
            text("second"),
            text("notice two"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let first: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"initial","background":true}),
        )
        .await,
    ))
    .unwrap();
    job_done(&flow, first["job_id"].as_str().unwrap(), &parent).await;
    let second: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"follow up","resume":"general","background":true}),
        )
        .await,
    ))
    .unwrap();
    assert_eq!(second["id"], first["id"]);
    assert_eq!(second["name"], first["name"]);
    assert_ne!(second["job_id"], first["job_id"]);
    job_done(&flow, second["job_id"].as_str().unwrap(), &parent).await;
    let jobs = flow.runtime.jobs(Some(&parent)).unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].tokens, 110);
    assert_eq!(jobs[1].tokens, 110);
    assert_eq!(
        flow.runtime
            .job(first["job_id"].as_str().unwrap())
            .unwrap()
            .result
            .as_ref()
            .unwrap()["text"],
        "first"
    );
    assert_eq!(
        flow.runtime
            .job(second["job_id"].as_str().unwrap())
            .unwrap()
            .result
            .as_ref()
            .unwrap()["text"],
        "second"
    );
    assert_eq!(flow.runtime.state(&parent).await.unwrap().inbox.len(), 2);
    flow.runtime.recover_jobs().await.unwrap();
    assert_eq!(flow.runtime.state(&parent).await.unwrap().inbox.len(), 2);
}

#[tokio::test]
async fn another_resume_cannot_interrupt_a_foreground_owner_waiting_for_approval() {
    let flow = std::sync::Arc::new(Flow::new(
        vec![
            text("first"),
            call("read", "read", json!({"path":".env"})),
            text("second"),
        ],
        true,
    ));
    flow.f.write(".env", "private data");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let first: Value = serde_json::from_str(&output(
        invoke(&flow, &parent, json!({"prompt":"initial"})).await,
    ))
    .unwrap();
    let id = first["id"].as_str().unwrap().to_string();
    let (owned, caller, reference) = (flow.clone(), parent.clone(), id.clone());
    let active = tokio::spawn(async move {
        invoke(
            &owned,
            &caller,
            json!({"prompt":"follow up","resume":reference}),
        )
        .await
    });
    let request = flow.pending(&parent).await;
    assert!(
        support::failed(invoke(&flow, &parent, json!({"prompt":"race","resume":id})).await)
            .contains("Subagent busy")
    );
    assert!(flow.runtime.is_running(&id));
    assert_eq!(flow.main.requests().len(), 2);
    flow.runtime
        .reply_permission(&request.id, cyber_server::runtime::PermissionReply::Once)
        .await
        .unwrap();
    let result: Value = serde_json::from_str(&output(active.await.unwrap())).unwrap();
    assert_eq!(result["id"], id);
    assert_eq!(result["text"], "second");
    assert_eq!(flow.main.requests().len(), 3);
}

#[tokio::test]
async fn resume_can_replace_the_schema_and_keeps_the_full_typed_result() {
    let flow = Flow::new(
        vec![
            call("first", "return_result", json!(1)),
            call("second", "return_result", json!(null)),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let first: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"initial","output_schema":{"type":"integer"}}),
        )
        .await,
    ))
    .unwrap();
    let outcome = invoke(
        &flow,
        &parent,
        json!({"prompt":"follow up","resume":first["id"],"output_schema":{"type":"null"}}),
    )
    .await;
    match outcome {
        ToolOutcome::Structured { value, .. } => assert_eq!(value, json!(null)),
        other => panic!("typed result lost: {other:?}"),
    }
    assert_eq!(flow.main.requests().len(), 2);
    assert!(flow.main.requests()[1].tools.iter().any(|tool| tool.name == "return_result" && tool.input_schema == json!({"type":"null"})));
}

#[tokio::test]
async fn background_structured_resume_retains_the_schema_and_typed_handback() {
    let flow = Flow::new(
        vec![
            call("first", "return_result", json!(1)),
            text("notice one"),
            call("second", "return_result", json!(2)),
            text("notice two"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let first: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"initial","background":true,"output_schema":{"type":"integer"}}),
        )
        .await,
    ))
    .unwrap();
    job_done(&flow, first["job_id"].as_str().unwrap(), &parent).await;
    let second: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"follow up","resume":"general","background":true}),
        )
        .await,
    ))
    .unwrap();
    job_done(&flow, second["job_id"].as_str().unwrap(), &parent).await;
    assert_eq!(second["id"], first["id"]);
    assert_eq!(
        flow.runtime
            .job(second["job_id"].as_str().unwrap())
            .unwrap()
            .result
            .unwrap()["result"],
        2
    );
    assert_eq!(flow.main.requests().len(), 4);
}

#[tokio::test]
async fn failed_resume_preparation_cannot_return_the_previous_answer() {
    let flow = Flow::new(vec![text("previous answer")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let first: Value = serde_json::from_str(&output(
        invoke(&flow, &parent, json!({"prompt":"initial"})).await,
    ))
    .unwrap();
    flow.f.set_config(
        json!({"permissions":{"agent":"allow"},"agents":{"general":{"model":"test/missing"}}}),
    );
    let result = invoke(
        &flow,
        &parent,
        json!({"prompt":"follow up","resume":first["id"]}),
    )
    .await;
    assert!(support::failed(result).contains("prompt was not promoted"));
    assert!(
        flow.runtime
            .state(first["id"].as_str().unwrap())
            .await
            .unwrap()
            .pending(cyber_server::runtime::Delivery::Queue)
            .next()
            .is_some()
    );
    assert_eq!(flow.main.requests().len(), 1);
}

#[tokio::test]
async fn failed_background_resume_preparation_has_no_stale_result() {
    let flow = Flow::new(vec![text("previous answer"), text("notice handled")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let first: Value = serde_json::from_str(&output(
        invoke(&flow, &parent, json!({"prompt":"initial"})).await,
    ))
    .unwrap();
    flow.f.set_config(
        json!({"permissions":{"agent":"allow"},"agents":{"general":{"model":"test/missing"}}}),
    );
    let ack: Value = serde_json::from_str(&output(
        invoke(
            &flow,
            &parent,
            json!({"prompt":"follow up","resume":first["id"],"background":true}),
        )
        .await,
    ))
    .unwrap();
    job_done(&flow, ack["job_id"].as_str().unwrap(), &parent).await;
    let job = flow.runtime.job(ack["job_id"].as_str().unwrap()).unwrap();
    assert_eq!(job.status, cyber_server::runtime::JobStatus::Error);
    assert!(job.result.is_none());
    assert!(job.error.unwrap().contains("prompt was not promoted"));
    assert_eq!(job.tokens, 0);
    assert!(
        flow.runtime
            .state(first["id"].as_str().unwrap())
            .await
            .unwrap()
            .pending(cyber_server::runtime::Delivery::Queue)
            .next()
            .is_some()
    );
    assert_eq!(flow.main.requests().len(), 2);
}
