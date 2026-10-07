//! Explicit user subtask dispatch through the runtime and authenticated HTTP API.
mod support;
use cyber_server::runtime::{CreateSession, JobStatus, PermissionReply};
use serde_json::json;
use support::flow::{Flow, call, text};

#[tokio::test]
async fn typed_user_delegation_preserves_images_and_caps_requested_steps() {
    use cyber_llm::Content;
    use cyber_server::runtime::UserSubtask;
    let flow = Flow::new(vec![text("child answer"), text("notice handled")], false);
    flow.f.set_config(json!({"agents":{"general":{"steps":2}}}));
    let parent = flow.session("dont-ask").await;
    let attachments = vec![
        Content::Text {
            text: "attachment".into(),
        },
        Content::Image {
            media_type: "image/png".into(),
            data: "aW1hZ2U=".into(),
        },
    ];
    let job = flow
        .runtime
        .subtask_request(
            &parent,
            UserSubtask {
                prompt: "inspect attachments".into(),
                agent: Some("general".into()),
                attachments: attachments.clone(),
                max_steps: Some(10),
            },
        )
        .await
        .unwrap();
    wait_notice(&flow, &job.id).await;
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.max_steps, Some(2));
    let requests = flow.main.requests();
    let message = requests[0]
        .messages
        .iter()
        .find(|message| {
            message
                .content
                .iter()
                .any(|part| matches!(part, Content::Image { .. }))
        })
        .unwrap();
    assert_eq!(&message.content[1..], attachments.as_slice());
    assert_eq!(
        flow.runtime.job(&job.id).unwrap().status,
        JobStatus::Completed
    );
}

#[tokio::test]
async fn one_step_user_ceiling_removes_tools_and_zero_creates_no_child() {
    use cyber_server::runtime::UserSubtask;
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":"public.txt"})),
            text("child answer"),
            text("notice handled"),
        ],
        false,
    );
    flow.f.write("public.txt", "public");
    let parent = flow.session("dont-ask").await;
    let request = |steps| UserSubtask {
        prompt: "one step".into(),
        agent: Some("general".into()),
        attachments: vec![],
        max_steps: Some(steps),
    };
    assert!(
        flow.runtime
            .subtask_request(&parent, request(0))
            .await
            .is_err()
    );
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        1
    );
    let job = flow
        .runtime
        .subtask_request(&parent, request(1))
        .await
        .unwrap();
    wait_notice(&flow, &job.id).await;
    assert!(!flow.main.requests()[0].tools.is_empty());
    assert!(flow.main.requests()[1].tools.is_empty());
    assert_eq!(
        flow.runtime
            .state(&job.child_id)
            .await
            .unwrap()
            .info
            .max_steps,
        Some(1)
    );
}

#[tokio::test]
async fn user_subtask_forks_without_spawn_approval_and_preserves_child_approvals() {
    let flow = Flow::new(
        vec![
            text("parent answer"),
            call("read", "read", json!({"path":".env"})),
            text("parent continues"),
            text("child findings"),
            text("notice handled"),
        ],
        true,
    );
    flow.f.write(".env", "private output");
    let parent = flow.session("default").await;
    flow.prompt(&parent, "original objective").await;
    flow.settle(&parent).await;
    let original = flow.runtime.state(&parent).await.unwrap();
    let job = flow
        .runtime
        .subtask(&parent, "try another approach")
        .await
        .unwrap();
    assert_eq!(job.kind, "subagent");
    assert_eq!(job.session_id, parent);
    let request = flow.pending(&parent).await;
    assert_eq!(request.session_id, job.child_id);
    assert!(
        matches!(&request.kind,cyber_server::runtime::PendingKind::Permission(ask) if ask.action == "read")
    );
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.agent, original.info.agent);
    assert_eq!(child.info.model, original.info.model);
    assert_eq!(
        child.task.objective.as_ref().unwrap().text,
        "original objective"
    );
    assert_eq!(
        flow.runtime.state(&parent).await.unwrap().entries,
        original.entries
    );
    flow.prompt(&parent, "continue the primary task").await;
    flow.settle(&parent).await;
    assert!(flow.runtime.is_running(&job.child_id));
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    wait_notice(&flow, &job.id).await;
    let done = flow.runtime.job(&job.id).unwrap();
    assert_eq!(done.status, JobStatus::Completed);
    assert_eq!(done.result.unwrap()["text"], "child findings");
    assert_eq!(done.tokens, 215);
    assert!(format!("{:?}", flow.main.requests()[1].messages).contains("parent answer"));
}

async fn wait_notice(flow: &Flow, id: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !flow.runtime.job(id).unwrap().notified {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn blank_prompts_and_deny_rules_fail_before_child_creation() {
    let flow = Flow::new(vec![], false);
    let parent = flow.session("bypass").await;
    assert!(
        flow.runtime
            .subtask(&parent, "  ")
            .await
            .unwrap_err()
            .to_string()
            .contains("must not be empty")
    );
    flow.f.set_config(json!({"permissions":{"agent":"deny"},"agents":{"build":{"permissions":{"agent":"allow"}}}}));
    assert!(
        flow.runtime
            .subtask(&parent, "do some work")
            .await
            .unwrap_err()
            .to_string()
            .contains("Permission denied")
    );
    assert!(flow.runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        1
    );
    assert!(flow.main.requests().is_empty());
}

#[tokio::test]
async fn ancestor_denies_and_profile_tool_restrictions_cannot_be_bypassed() {
    let flow = Flow::new(vec![], false);
    let parent = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("bypass".into()),
            rules: Some(json!({"agent":"deny"})),
            ..Default::default()
        })
        .await
        .unwrap();
    let child = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent.id.clone()),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        flow.runtime
            .subtask(&child.id, "nested child task")
            .await
            .unwrap_err()
            .to_string()
            .contains("Permission denied")
    );
    flow.f
        .set_config(json!({"agents":{"build":{"tools":{"deny":["agent"]}}}}));
    assert!(
        flow.runtime
            .subtask(&parent.id, "another child task")
            .await
            .unwrap_err()
            .to_string()
            .contains("unavailable for agent")
    );
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        2
    );
    assert!(flow.main.requests().is_empty());
}

#[tokio::test]
async fn a_cancelled_queued_subtask_creates_no_child_and_shutdown_does_not_deadlock() {
    let flow = Flow::new(vec![call("read", "read", json!({"path":".env"}))], true);
    flow.f.write(".env", "private");
    flow.f.set_config(json!({"agents":{"max_concurrent":1}}));
    let parent = flow.session("default").await;
    let first = flow
        .runtime
        .subtask(&parent, "first child task")
        .await
        .unwrap();
    flow.pending(&parent).await;
    let runtime = flow.runtime.clone();
    let id = parent.clone();
    let queued = tokio::spawn(async move { runtime.subtask(&id, "queued child task").await });
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !queued.is_finished(),
        "queued subtask ended without a permit"
    );
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        2
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), flow.runtime.shutdown())
        .await
        .unwrap();
    assert!(queued.await.unwrap().is_err());
    assert_eq!(
        flow.runtime.job(&first.id).unwrap().status,
        JobStatus::Interrupted
    );
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        2
    );
    assert!(flow.runtime.pending_requests(None).is_empty());
}

struct Catalog;
impl cyber_server::http::Services for Catalog {
    fn models(
        &self,
        _: &std::path::Path,
    ) -> futures::future::BoxFuture<'_, Result<Vec<cyber_server::http::ModelInfo>, String>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn default_model(&self, _: &std::path::Path) -> Option<String> {
        Some("test/main".into())
    }
    fn agents(&self, _: &std::path::Path) -> Vec<cyber_server::http::AgentInfo> {
        vec![]
    }
    fn tools(&self, _: &cyber_server::runtime::TurnContext) -> Vec<cyber_server::runtime::ToolDef> {
        vec![]
    }
    fn commands(&self, _: &std::path::Path) -> Vec<cyber_server::http::CommandInfo> {
        vec![]
    }
    fn find_files(&self, _: &std::path::Path, _: &str, _: usize) -> Vec<String> {
        vec![]
    }
    fn expand_command(&self, _: &std::path::Path, _: &str, _: &str) -> Option<String> {
        None
    }
}

#[tokio::test]
async fn authenticated_http_subtask_returns_a_job_and_replays_its_idempotency_key() {
    use cyber_server::http::{self, AppState, HttpOptions};
    use std::sync::Arc;
    let flow = Flow::new(vec![call("read", "read", json!({"path":".env"}))], true);
    flow.f.write(".env", "private");
    let parent = flow.session("default").await;
    let state = AppState {
        service: None,
        runtime: flow.runtime.clone(),
        remote_tools: Arc::default(),
        store: flow.f.store.clone(),
        services: Arc::new(Catalog),
        options: Arc::new(HttpOptions {
            version: "test".into(),
            password: Some("secret".into()),
            cors_origins: vec![],
            default_directory: flow.f.repo.clone(),
            features: vec![],
        }),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let closing = tokio_util::sync::CancellationToken::new();
    let _on_drop = closing.clone().drop_guard();
    let server = tokio::spawn(http::serve_tcp(
        http::router(state),
        listener,
        closing.clone().cancelled_owned(),
    ));
    let client = reqwest::Client::new();
    let url = format!("http://{address}/api/v1/sessions/{parent}/subtask");
    assert_eq!(
        client
            .post(&url)
            .json(&json!({"prompt":"child task"}))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .post(&url)
            .basic_auth("cyber", Some("secret"))
            .json(&json!({"prompt":" "}))
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::BAD_REQUEST
    );
    let body = json!({"prompt":"try another approach", "agent":"explore", "max_steps":2, "attachments":[{"type":"image","media_type":"image/png","data":"aW1hZ2U="}]});
    let response = client
        .post(&url)
        .basic_auth("cyber", Some("secret"))
        .header("Idempotency-Key", "subtask-test")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let result: serde_json::Value = response.json().await.unwrap();
    let request = flow.pending(&parent).await;
    assert_eq!(result["data"]["child_id"], request.session_id);
    assert_eq!(
        flow.runtime
            .state(&request.session_id)
            .await
            .unwrap()
            .info
            .agent,
        "explore"
    );
    let child = flow.runtime.state(&request.session_id).await.unwrap();
    assert_eq!(child.info.max_steps, Some(2));
    assert!(flow.main.requests()[0].messages.iter().any(|message| {
        message.content.iter().any(
            |part| matches!(part, cyber_llm::Content::Image { data, .. } if data == "aW1hZ2U="),
        )
    }));
    let mut changed = body.clone();
    changed["max_steps"] = json!(3);
    let conflict = client
        .post(&url)
        .basic_auth("cyber", Some("secret"))
        .header("Idempotency-Key", "subtask-test")
        .json(&changed)
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), reqwest::StatusCode::CONFLICT);
    let replay = client
        .post(&url)
        .basic_auth("cyber", Some("secret"))
        .header("Idempotency-Key", "subtask-test")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), reqwest::StatusCode::ACCEPTED);
    assert_eq!(replay.headers()["idempotent-replayed"], "true");
    assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), result);
    assert_eq!(flow.runtime.jobs(Some(&parent)).unwrap().len(), 1);
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        2
    );
    flow.runtime
        .cancel_job(result["data"]["id"].as_str().unwrap())
        .await
        .unwrap();
    closing.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_subtask_during_a_parent_turn_keeps_its_pinned_agent_and_mode() {
    let flow = Flow::new(
        vec![
            call("parent-read", "read", json!({"path":".env"})),
            text("child findings"),
            text("parent finishes"),
            text("notice handled"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    let parent = flow.session("default").await;
    flow.prompt(&parent, "original objective").await;
    let pending = flow.pending(&parent).await;
    flow.runtime.switch_agent(&parent, "general").await.unwrap();
    flow.runtime.switch_mode(&parent, "bypass").await.unwrap();
    let job = flow
        .runtime
        .subtask(&parent, "try another approach")
        .await
        .unwrap();
    wait_notice(&flow, &job.id).await;
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.agent, "build");
    assert_eq!(child.info.mode, "default");
    let original = flow.runtime.state(&parent).await.unwrap();
    assert_eq!(
        original.calls["parent-read"].status,
        cyber_server::runtime::CallStatus::Dispatched
    );
    assert!(
        flow.runtime
            .pending_requests(Some(&parent))
            .iter()
            .any(|request| request.id == pending.id)
    );
    let copied = child.calls.values().next().unwrap();
    assert_ne!(copied.call_id, "parent-read");
    assert!(copied.status.is_settled());
    assert!(
        copied
            .output
            .as_ref()
            .unwrap()
            .contains("source call was still active")
    );
    flow.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    flow.settle(&parent).await;
    assert_eq!(flow.main.requests().len(), 4);
    let settled = flow.runtime.state(&parent).await.unwrap();
    assert!(
        matches!(settled.entries.last(), Some(cyber_server::runtime::Entry::Assistant(answer)) if answer.error.is_none() && answer.text == "notice handled")
    );
}

#[tokio::test]
async fn explicit_user_subtasks_keep_each_permission_mode_on_the_child() {
    for mode in [
        "default",
        "accept-edits",
        "plan",
        "auto",
        "dont-ask",
        "bypass",
    ] {
        let flow = Flow::new(vec![text("child findings"), text("notice handled")], false);
        let parent = flow.session(mode).await;
        let job = flow
            .runtime
            .subtask(&parent, "try another approach")
            .await
            .unwrap_or_else(|error| panic!("{mode}: {error}"));
        wait_notice(&flow, &job.id).await;
        let child = flow.runtime.state(&job.child_id).await.unwrap();
        assert_eq!(child.info.mode, mode);
        assert_eq!(
            flow.runtime.job(&job.id).unwrap().status,
            JobStatus::Completed
        );
        flow.f.set_config(json!({"permissions":{"agent":"deny"}}));
        assert!(
            flow.runtime
                .subtask(&parent, "another child task")
                .await
                .unwrap_err()
                .to_string()
                .contains("Permission denied")
        );
    }
}

#[tokio::test]
async fn explicit_spawn_approval_does_not_approve_a_child_write_in_plan_mode() {
    let flow = Flow::new(
        vec![
            call(
                "write",
                "write",
                json!({"path":"new.txt","content":"unapproved"}),
            ),
            text("child reports refusal"),
            text("notice handled"),
        ],
        false,
    );
    let parent = flow.session("plan").await;
    let job = flow
        .runtime
        .subtask(&parent, "try another approach")
        .await
        .unwrap();
    wait_notice(&flow, &job.id).await;
    assert!(!flow.f.repo.join("new.txt").exists());
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(
        child.calls["write"].status,
        cyber_server::runtime::CallStatus::Error
    );
    assert_eq!(child.info.mode, "plan");
}

#[tokio::test]
async fn queued_subtask_rechecks_profile_tool_restrictions_before_creation() {
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":".env"})),
            text("notice handled"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f.set_config(json!({"agents":{"max_concurrent":1}}));
    let parent = flow.session("default").await;
    let first = flow
        .runtime
        .subtask(&parent, "first child task")
        .await
        .unwrap();
    flow.pending(&parent).await;
    let runtime = flow.runtime.clone();
    let id = parent.clone();
    let queued = tokio::spawn(async move { runtime.subtask(&id, "queued child task").await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(!queued.is_finished());
    flow.f
        .set_config(json!({"agents":{"max_concurrent":1,"build":{"tools":{"deny":["agent"]}}}}));
    flow.runtime.cancel_job(&first.id).await.unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), queued)
        .await
        .unwrap()
        .unwrap();
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("unavailable for agent")
    );
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        2
    );
    assert_eq!(flow.runtime.jobs(Some(&parent)).unwrap().len(), 1);
    flow.runtime.wait_idle(&parent).await;
}

#[tokio::test]
async fn named_user_delegation_is_fresh_and_keeps_child_approvals_and_handback() {
    let flow = Flow::new(
        vec![
            text("old parent answer"),
            call("read", "read", json!({"path":".env"})),
            text("target findings"),
            text("notice handled"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    let parent = flow.session("default").await;
    flow.prompt(&parent, "original parent objective").await;
    flow.settle(&parent).await;
    let previous = flow.runtime.state(&parent).await.unwrap().entries;
    let job = flow
        .runtime
        .subtask_with_agent(&parent, "where is retry logic?", Some("explore".into()))
        .await
        .unwrap();
    let request = flow.pending(&parent).await;
    assert_eq!(request.session_id, job.child_id);
    assert!(
        matches!(&request.kind, cyber_server::runtime::PendingKind::Permission(ask) if ask.action == "read")
    );
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.agent, "explore");
    assert_eq!(child.info.mode, "default");
    let history = serde_json::to_string(&child.entries).unwrap();
    assert!(
        !history.contains("original parent objective") && !history.contains("old parent answer")
    );
    assert_eq!(flow.runtime.state(&parent).await.unwrap().entries, previous);
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    wait_notice(&flow, &job.id).await;
    let completed = flow.runtime.job(&job.id).unwrap();
    assert_eq!(completed.status, JobStatus::Completed);
    assert_eq!(completed.result.unwrap()["text"], "target findings");
}

#[tokio::test]
async fn explicit_targets_refuse_unknown_hidden_primary_and_denied_agents_without_children() {
    let flow = Flow::new(vec![], false);
    let parent = flow.session("default").await;
    flow.f
        .set_config(json!({"agents":{"secret":{"hidden":true},"primary":{"mode":"primary"}}}));
    for name in ["missing", "secret", "primary", "build", "evaluator"] {
        assert!(
            flow.runtime
                .subtask_with_agent(&parent, "do work", Some(name.into()))
                .await
                .is_err()
        );
    }
    flow.f
        .set_config(json!({"permissions":{"agent":{"explore":"deny"}}}));
    let error = flow
        .runtime
        .subtask_with_agent(&parent, "do work", Some("explore".into()))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Permission denied"), "{error}");
    assert!(flow.runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert_eq!(
        flow.runtime
            .list(&Default::default())
            .unwrap()
            .sessions
            .len(),
        1
    );
    assert!(flow.main.requests().is_empty());
}

#[tokio::test]
async fn named_delegation_keeps_all_six_modes_and_refuses_plan_child_writes() {
    for mode in [
        "default",
        "plan",
        "accept-edits",
        "auto",
        "dont-ask",
        "bypass",
    ] {
        let flow = Flow::new(vec![text("child answer"), text("notice handled")], false);
        flow.f
            .set_config(json!({"agents":{"general":{"permission_mode":"bypass"}}}));
        let parent = flow.session(mode).await;
        let job = flow
            .runtime
            .subtask_with_agent(&parent, "inspect", Some("general".into()))
            .await
            .unwrap();
        wait_notice(&flow, &job.id).await;
        assert_eq!(
            flow.runtime.state(&job.child_id).await.unwrap().info.mode,
            mode
        );
    }
    let flow = Flow::new(
        vec![
            call(
                "write",
                "write",
                json!({"path":"forbidden.txt","content":"write"}),
            ),
            text("write refused"),
            text("notice handled"),
        ],
        false,
    );
    let parent = flow.session("plan").await;
    let job = flow
        .runtime
        .subtask_with_agent(&parent, "try a write", Some("general".into()))
        .await
        .unwrap();
    wait_notice(&flow, &job.id).await;
    assert!(!flow.f.repo.join("forbidden.txt").exists());
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert!(
        child
            .calls
            .values()
            .any(|call| matches!(&call.status, cyber_server::runtime::CallStatus::Error))
    );
}
