//! Background handoff through real built-in dispatch and durable runtime ownership.
mod support;
use cyber_server::runtime::{JobStatus, PermissionReply, ToolHost, ToolOutcome};
use serde_json::{Value, json};
use support::flow::{Flow, call, text};
use tokio_util::sync::CancellationToken;

async fn spawn(flow: &Flow, parent: &str, input: Value) -> Value {
    let mut inv = flow.f.invocation("default", "agent", input);
    inv.session_id = parent.into();
    let output = flow.f.host.execute(inv, CancellationToken::new()).await;
    let text = support::ok(output);
    serde_json::from_str(&text).expect(&text)
}
async fn terminal(flow: &Flow, id: &str) -> cyber_server::runtime::Job {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let job = flow.runtime.job(id).unwrap();
            if job.status != JobStatus::Running && job.notified {
                return job;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("job did not finish")
}

#[tokio::test]
async fn background_returns_before_completion_and_survives_parent_interrupt() {
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":".env"})),
            text("private child findings"),
            text("notice handled"),
        ],
        true,
    );
    flow.f.write(".env", "secret intermediate content");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let ack = spawn(
        &flow,
        &parent,
        json!({"prompt":"inspect","background":true,"name":"inspection"}),
    )
    .await;
    assert_eq!(ack["state"], "running");
    assert_eq!(ack["name"], "inspection");
    let id = ack["job_id"].as_str().unwrap();
    let request = flow.pending(&parent).await;
    assert!(
        request
            .origin
            .as_ref()
            .unwrap()
            .title
            .contains("inspection")
    );
    assert_eq!(flow.runtime.job(id).unwrap().status, JobStatus::Running);
    flow.runtime.interrupt(&parent).await.unwrap();
    assert!(flow.runtime.is_running(ack["id"].as_str().unwrap()));
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    let job = terminal(&flow, id).await;
    assert_eq!(job.status, JobStatus::Completed);
    assert_eq!(
        job.result.as_ref().unwrap()["text"],
        "private child findings"
    );
    assert!(job.tokens > 0);
    assert!(job.unpriced);
    flow.settle(&parent).await;
    let state = flow.runtime.state(&parent).await.unwrap();
    assert_eq!(state.inbox.len(), 1);
    let notice = serde_json::to_string(&state.inbox[0].parts).unwrap();
    assert!(notice.contains("inspection"));
    assert!(!notice.contains("secret intermediate content"));
    flow.runtime.recover_jobs().await.unwrap();
    assert_eq!(flow.runtime.state(&parent).await.unwrap().inbox.len(), 1);
}

#[tokio::test]
async fn task_stop_is_scoped_idempotent_and_has_a_golden() {
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":".env"})),
            text("cancel notice handled"),
        ],
        true,
    );
    flow.f.write(".env", "secret");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let ack = spawn(
        &flow,
        &parent,
        json!({"prompt":"inspect","background":true}),
    )
    .await;
    flow.pending(&parent).await;
    let other = flow.session("default").await;
    let mut stop = flow
        .f
        .invocation("default", "task_stop", json!({"job_id":ack["job_id"]}));
    stop.session_id = other;
    assert!(matches!(
        flow.f
            .host
            .execute(stop.clone(), CancellationToken::new())
            .await,
        ToolOutcome::Failed(_)
    ));
    stop.session_id = parent.clone();
    let output = support::ok(
        flow.f
            .host
            .execute(stop.clone(), CancellationToken::new())
            .await,
    );
    assert!(!flow.runtime.is_running(ack["id"].as_str().unwrap()));
    assert!(flow.runtime.pending_requests(Some(&parent)).is_empty());
    let mut normalized: Value = serde_json::from_str(&output).unwrap();
    normalized["job_id"] = json!("<job>");
    support::golden(&flow.f, "task_stop", &normalized.to_string());
    assert_eq!(
        support::ok(flow.f.host.execute(stop, CancellationToken::new()).await),
        output
    );
}

#[tokio::test]
async fn structured_background_results_are_delivered_in_handback() {
    let value = json!({"findings":["parser is iterative"]});
    let flow = Flow::new(
        vec![
            call("result", "return_result", value.clone()),
            text("notice handled"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let ack=spawn(&flow,&parent,json!({"prompt":"inspect","background":true,"output_schema":{"type":"object","required":["findings"]}})).await;
    let job = terminal(&flow, ack["job_id"].as_str().unwrap()).await;
    assert_eq!(job.status, JobStatus::Completed);
    assert_eq!(job.result.unwrap()["result"], value);
    flow.settle(&parent).await;
    assert_eq!(flow.runtime.state(&parent).await.unwrap().inbox.len(), 1);
}

#[tokio::test]
async fn deleting_parent_cancels_background_children_without_handback() {
    let flow = Flow::new(vec![call("read", "read", json!({"path":".env"}))], true);
    flow.f.write(".env", "secret");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let ack = spawn(
        &flow,
        &parent,
        json!({"prompt":"inspect","background":true}),
    )
    .await;
    flow.pending(&parent).await;
    flow.runtime.delete(&parent).await.unwrap();
    assert!(flow.runtime.jobs(None).unwrap().is_empty());
    assert!(!flow.runtime.is_running(ack["id"].as_str().unwrap()));
    assert!(
        flow.runtime
            .state(ack["id"].as_str().unwrap())
            .await
            .is_err()
    );
    assert_eq!(flow.main.requests().len(), 1);
}

#[tokio::test]
async fn profile_background_default_allows_an_explicit_foreground_override() {
    for background in [None, Some(false)] {
        let flow = Flow::new(vec![text("child done"), text("notice handled")], false);
        flow.f.set_config(
            json!({"permissions":{"agent":"allow"},"agents":{"general":{"background":true}}}),
        );
        let parent = flow.session("default").await;
        let mut input = json!({"prompt":"inspect"});
        if let Some(background) = background {
            input["background"] = json!(background);
        }
        let result = spawn(&flow, &parent, input).await;
        if background.is_none() {
            terminal(&flow, result["job_id"].as_str().unwrap()).await;
        } else {
            assert_eq!(result["text"], "child done");
            assert!(flow.runtime.jobs(None).unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn busy_parent_keeps_handback_queued_until_its_safe_boundary() {
    let flow = Flow::new(
        vec![
            call("child-read", "read", json!({"path":".env"})),
            call("parent-read", "read", json!({"path":"pause.txt"})),
            text("child findings"),
            text("parent finishes current work"),
            text("parent handles handback"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f.write("pause.txt", "pause");
    flow.f
        .set_config(json!({"permissions":{"agent":"allow","read":{"pause.txt":"ask"}}}));
    let parent = flow.session("default").await;
    let ack = spawn(
        &flow,
        &parent,
        json!({"prompt":"inspect","background":true}),
    )
    .await;
    let child_request = flow.pending(&parent).await;
    flow.prompt(&parent, "parent work").await;
    let parent_request = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Some(request) = flow
                .runtime
                .pending_requests(Some(&parent))
                .into_iter()
                .find(|request| request.session_id == parent)
            {
                break request;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    flow.runtime
        .reply_permission(&child_request.id, PermissionReply::Once)
        .await
        .unwrap();
    terminal(&flow, ack["job_id"].as_str().unwrap()).await;
    assert!(flow.runtime.is_running(&parent));
    let state = flow.runtime.state(&parent).await.unwrap();
    assert_eq!(
        state
            .inbox
            .iter()
            .filter(|row| row.status == cyber_server::runtime::InputStatus::Pending)
            .count(),
        1
    );
    assert_eq!(flow.main.requests().len(), 3);
    flow.runtime
        .reply_permission(&parent_request.id, PermissionReply::Once)
        .await
        .unwrap();
    flow.settle(&parent).await;
    assert_eq!(flow.main.requests().len(), 5);
}

#[tokio::test]
async fn background_handoff_keeps_its_pool_permit_until_child_settlement() {
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":".env"})),
            text("cancel notice handled"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f
        .set_config(json!({"permissions":{"agent":"allow"},"agents":{"max_concurrent":1}}));
    let parent = flow.session("default").await;
    let ack = spawn(
        &flow,
        &parent,
        json!({"prompt":"inspect","background":true}),
    )
    .await;
    flow.pending(&parent).await;
    let mut inv = flow
        .f
        .invocation("default", "agent", json!({"prompt":"queued child"}));
    inv.session_id = parent.clone();
    let mut queued = Box::pin(flow.f.host.execute(inv, CancellationToken::new()));
    assert!(futures::poll!(&mut queued).is_pending());
    flow.f.set_config(json!({"permissions":{"agent":"allow"},"agents":{"max_concurrent":1,"general":{"disabled":true}}}));
    flow.runtime
        .cancel_job(ack["job_id"].as_str().unwrap())
        .await
        .unwrap();
    assert!(support::failed(queued.await).contains("Unknown agent"));
    assert_eq!(
        flow.runtime
            .list(&cyber_server::runtime::ListFilter {
                parent: Some(parent),
                ..Default::default()
            })
            .unwrap()
            .sessions
            .len(),
        1
    );
}

#[tokio::test]
async fn structured_background_mismatch_uses_one_retry_and_reports_failure() {
    let flow = Flow::new(
        vec![
            text("first plain answer"),
            text("second plain answer"),
            text("failure notice handled"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let ack = spawn(
        &flow,
        &parent,
        json!({"prompt":"inspect","background":true,"output_schema":{"type":"object"}}),
    )
    .await;
    let job = terminal(&flow, ack["job_id"].as_str().unwrap()).await;
    assert_eq!(job.status, JobStatus::Error);
    assert!(job.error.unwrap().contains("SchemaMismatch"));
    assert_eq!(
        flow.runtime
            .state(ack["id"].as_str().unwrap())
            .await
            .unwrap()
            .inbox
            .len(),
        2
    );
    flow.settle(&parent).await;
    assert_eq!(flow.main.requests().len(), 3);
}

#[tokio::test]
async fn an_active_name_collision_is_refused_before_creating_another_child() {
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":".env"})),
            text("cancel notice handled"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let ack = spawn(
        &flow,
        &parent,
        json!({"prompt":"inspect","background":true,"name":"scan"}),
    )
    .await;
    flow.pending(&parent).await;
    let mut inv = flow.f.invocation(
        "default",
        "agent",
        json!({"prompt":"inspect again","background":true,"name":"scan"}),
    );
    inv.session_id = parent.clone();
    assert!(
        support::failed(flow.f.host.execute(inv, CancellationToken::new()).await)
            .contains("name already active")
    );
    assert_eq!(
        flow.runtime
            .list(&cyber_server::runtime::ListFilter {
                parent: Some(parent.clone()),
                ..Default::default()
            })
            .unwrap()
            .sessions
            .len(),
        1
    );
    flow.runtime
        .cancel_job(ack["job_id"].as_str().unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn gate_queued_background_creation_rechecks_permission_denies() {
    let flow = Flow::new(vec![], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let reservation = flow.runtime.reserve_child_job(&parent).await.unwrap();
    let mut inv = flow.f.invocation(
        "default",
        "agent",
        json!({"prompt":"inspect","background":true}),
    );
    inv.session_id = parent.clone();
    let mut queued = Box::pin(flow.f.host.execute(inv, CancellationToken::new()));
    assert!(futures::poll!(&mut queued).is_pending());
    flow.f
        .set_config(json!({"permissions":{"agent":{"general":"deny"}}}));
    drop(reservation);
    assert!(support::failed(queued.await).contains("Permission denied"));
    assert!(flow.runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert!(
        flow.runtime
            .list(&cyber_server::runtime::ListFilter {
                parent: Some(parent),
                ..Default::default()
            })
            .unwrap()
            .sessions
            .is_empty()
    );
}

#[tokio::test]
async fn default_background_names_are_unique_across_completed_tasks() {
    let flow = Flow::new(
        vec![
            text("first findings"),
            text("first notice handled"),
            text("second findings"),
            text("second notice handled"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    for expected in ["general", "general-2"] {
        let ack = spawn(
            &flow,
            &parent,
            json!({"prompt":"inspect","background":true}),
        )
        .await;
        assert_eq!(ack["name"], expected);
        terminal(&flow, ack["job_id"].as_str().unwrap()).await;
        flow.settle(&parent).await;
    }
}
