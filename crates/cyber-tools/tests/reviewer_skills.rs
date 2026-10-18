//! Named fork agent capture and immutable reviewer filesystem authority.
mod support;
use cyber_server::runtime::{CreateSession, Delivery, JobStatus, NoSnapshots};
use serde_json::json;
use std::sync::Arc;
use support::flow::{Flow, call, text};

#[tokio::test]
async fn reviewer_refuses_file_and_shell_mutations_even_in_bypass_with_allow_rules() {
    use cyber_server::runtime::ToolHost;
    use tokio_util::sync::CancellationToken;
    let fixture = support::Fixture::new();
    fixture.write("notes.txt", "review evidence");
    fixture.set_config(json!({"permissions":"allow","agents":{"reviewer":{"permission_mode":"bypass","permissions":"allow","tools":{"allow":["*"]}}}}));
    for mode in ["bypass", "plan", "default"] {
        for (tool, input) in [
            ("write", json!({"path":"blocked.txt","content":"blocked"})),
            (
                "write",
                json!({"path":".cyber/plans/ses_test.md","content":"blocked plan"}),
            ),
            ("bash", json!({"command":"printf blocked > shell.txt"})),
        ] {
            let mut invocation = fixture.invocation(mode, tool, input);
            invocation.agent = "reviewer".into();
            let output = support::failed(
                fixture
                    .host
                    .execute(invocation, CancellationToken::new())
                    .await,
            );
            assert!(
                output.contains("denied")
                    || output.contains("Denied")
                    || output.contains("read-only"),
                "{output}"
            );
        }
        let mut invocation = fixture.invocation(mode, "read", json!({"path":"notes.txt"}));
        invocation.agent = "reviewer".into();
        assert!(
            support::ok(
                fixture
                    .host
                    .execute(invocation, CancellationToken::new())
                    .await
            )
            .contains("review evidence")
        );
    }
    assert!(!fixture.repo.join("blocked.txt").exists());
    assert!(!fixture.repo.join("shell.txt").exists());
    assert!(!fixture.repo.join(".cyber/plans/ses_test.md").exists());
}

#[tokio::test]
async fn descendants_cannot_widen_a_reviewer_even_after_its_mode_is_bypass() {
    let flow = Flow::new(
        vec![
            call(
                "write",
                "write",
                json!({"path":".cyber/plans/ses_review_child.md","content":"blocked"}),
            ),
            call(
                "shell",
                "bash",
                json!({"command":"printf blocked > shell.txt"}),
            ),
            text("readonly summary"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":"allow"}));
    let parent = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            agent: Some("reviewer".into()),
            mode: Some("bypass".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    let child = flow
        .runtime
        .create_session(CreateSession {
            id: Some("ses_review_child".into()),
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent),
            agent: Some("general".into()),
            mode: Some("bypass".into()),
            rules: Some(json!("allow")),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    flow.prompt(&child, "check mutations").await;
    flow.settle(&child).await;
    let state = flow.runtime.state(&child).await.unwrap();
    assert_eq!(
        state.calls["write"].status,
        cyber_server::runtime::CallStatus::Error
    );
    assert_eq!(
        state.calls["shell"].status,
        cyber_server::runtime::CallStatus::Error
    );
    assert!(
        !flow
            .f
            .repo
            .join(".cyber/plans/ses_review_child.md")
            .exists()
    );
    assert!(!flow.f.repo.join("shell.txt").exists());
    flow.runtime.shutdown().await;
}

async fn completed(flow: &Flow, parent: &str) -> cyber_server::runtime::Job {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(job) = flow.runtime.jobs(Some(parent)).unwrap().into_iter().next()
                && job.status != JobStatus::Running
            {
                break job;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn user_fork_keeps_its_captured_reviewer_after_the_source_agent_changes() {
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![text("parent review summary")],
        false,
        Arc::new(NoSnapshots),
        vec![(
            "test/review",
            vec![
                call(
                    "write",
                    "write",
                    json!({"path":"review.txt","content":"blocked"}),
                ),
                text("structured findings"),
            ],
        )],
    );
    flow.f.set_config(json!({"permissions":"allow","agents":{"reviewer":{"permission_mode":"bypass","permissions":"allow","tools":{"allow":["*"]}}}}));
    let source = "---\nname: audit\ndescription: Audit\ncontext: fork\nagent: reviewer\nmodel: test/review\nallowed-tools: ['write:*']\n---\nPRIVATE REVIEW BODY\n";
    flow.f.write(".cyber/skills/audit/SKILL.md", source);
    let parent = flow.session("bypass").await;
    let info = flow.runtime.state(&parent).await.unwrap().info;
    let turn = cyber_server::runtime::TurnContext {
        session_id: parent.clone(),
        directory: info.directory,
        agent: info.agent,
        mode: info.mode,
        prefers_apply_patch: false,
        rules: info.rules,
        worktree: Default::default(),
    };
    let plan = flow
        .f
        .host
        .command_plan(&turn, "audit", "")
        .await
        .unwrap()
        .unwrap();
    let mut admission = plan.admission(Some("msg_reviewer_capture".into()), Delivery::Hold);
    admission.resume = false;
    flow.runtime
        .admit_user(&parent, admission.clone())
        .await
        .unwrap();
    flow.f.write(
        ".cyber/skills/audit/SKILL.md",
        &source.replace("agent: reviewer", "agent: general"),
    );
    let changed = flow
        .f
        .host
        .command_plan(&turn, "audit", "")
        .await
        .unwrap()
        .unwrap()
        .admission(Some("msg_reviewer_capture".into()), Delivery::Hold);
    assert!(flow.runtime.admit_user(&parent, changed).await.is_err());
    flow.runtime
        .release(&parent, "msg_reviewer_capture", Delivery::Queue)
        .await
        .unwrap();
    let job = completed(&flow, &parent).await;
    assert_eq!(job.status, JobStatus::Completed);
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.agent, "reviewer");
    assert_eq!(child.info.mode, "bypass");
    assert_eq!(
        child.calls["write"].status,
        cyber_server::runtime::CallStatus::Error
    );
    assert!(!flow.f.repo.join("review.txt").exists());
    flow.settle(&parent).await;
    assert!(
        !format!("{:?}", flow.runtime.state(&parent).await.unwrap().entries)
            .contains("PRIVATE REVIEW BODY")
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn model_loaded_fork_selects_the_declared_reviewer() {
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![
            call("skill", "skill", json!({"name":"audit"})),
            text("parent continues"),
            text("parent summary"),
        ],
        false,
        Arc::new(NoSnapshots),
        vec![(
            "test/review",
            vec![
                call("read", "read", json!({"path":"notes.txt"})),
                text("findings"),
            ],
        )],
    );
    flow.f.set_config(json!({"permissions":"allow"}));
    flow.f.write("notes.txt", "review evidence");
    flow.f.write(".cyber/skills/audit/SKILL.md","---\nname: audit\ndescription: Audit\ncontext: fork\nagent: reviewer\nmodel: test/review\n---\nReview notes\n");
    let parent = flow.session("bypass").await;
    flow.prompt(&parent, "audit").await;
    flow.settle(&parent).await;
    let job = completed(&flow, &parent).await;
    assert_eq!(job.status, JobStatus::Completed);
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.agent, "reviewer");
    assert_eq!(
        child.calls["read"].status,
        cyber_server::runtime::CallStatus::Ok
    );
    assert_eq!(
        flow.runtime.state(&parent).await.unwrap().info.agent,
        "build"
    );
    flow.runtime.shutdown().await;
}

/// Runs a user-invoked `context: fork` skill after a settled parent Turn and
/// asserts the child received the parent's projected history (agents-subagents
/// "Forked subagents"; skills-commands "Path-triggered and forked skills").
/// Without a declared `model:` the child runs on the parent's model, even under
/// a named agent: forks inherit the command's effective model (m1-3 design,
/// "Captured user skill command admission").
async fn assert_user_fork_inherits_parent_history(
    agent: Option<&str>,
    expected_agent: &str,
    model: Option<&'static str>,
) {
    let flow = user_fork_flow(agent, model);
    let parent = flow.session("default").await;
    flow.prompt(&parent, "original objective").await;
    flow.settle(&parent).await;
    let source = flow.runtime.state(&parent).await.unwrap();
    let job = run_user_fork(&flow, &parent).await;
    assert_eq!(job.status, JobStatus::Completed);
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    let child_model = model.unwrap_or("test/main");
    assert_forked_child(&child, &source, expected_agent, child_model);
    let request = forked_request(&flow, child_model);
    assert!(
        request.contains("original objective") && request.contains("parent answer"),
        "forked child request lacks inherited parent messages: {request}"
    );
    flow.settle(&parent).await;
    let parent_entries = format!("{:?}", flow.runtime.state(&parent).await.unwrap().entries);
    assert!(!parent_entries.contains("PRIVATE FORK BODY"));
    assert!(parent_entries.contains("audit summary"), "{parent_entries}");
    flow.runtime.shutdown().await;
}

/// The child is a fork of `source`: its own copy of the parent's history,
/// running as `agent` on `model`.
fn assert_forked_child(
    child: &cyber_server::runtime::SessionState,
    source: &cyber_server::runtime::SessionState,
    agent: &str,
    model: &str,
) {
    assert_eq!(
        child.info.parent_id.as_deref(),
        Some(source.info.id.as_str())
    );
    assert_eq!(child.info.agent, agent);
    assert_eq!(child.info.model, model);
    let entries = format!("{:?}", child.entries);
    assert!(
        entries.contains("original objective") && entries.contains("parent answer"),
        "forked child did not inherit parent history: {entries}"
    );
    assert_ne!(child.entries[0].id(), source.entries[0].id());
}

/// The parent answers on `test/main`; the child answers on its declared model,
/// or on the parent's when the skill declares none.
fn user_fork_flow(agent: Option<&str>, model: Option<&'static str>) -> Flow {
    let (main, extra) = match model {
        Some(model) => (
            vec![text("parent answer"), text("handback handled")],
            vec![(model, vec![text("audit summary")])],
        ),
        None => (
            vec![
                text("parent answer"),
                text("audit summary"),
                text("handback handled"),
            ],
            Vec::new(),
        ),
    };
    let flow = Flow::with_models(
        support::Fixture::new(),
        main,
        false,
        Arc::new(NoSnapshots),
        extra,
    );
    flow.f.set_config(json!({"permissions":"allow"}));
    let agent_line = agent.map_or_else(String::new, |name| format!("agent: {name}\n"));
    let model_line = model.map_or_else(String::new, |model| format!("model: {model}\n"));
    flow.f.write(
        ".cyber/skills/audit/SKILL.md",
        &format!(
            "---\nname: audit\ndescription: Audit\ncontext: fork\n{agent_line}{model_line}---\nPRIVATE FORK BODY\n"
        ),
    );
    flow
}

/// Admits `/audit` as a user command and waits for its forked child Job.
async fn run_user_fork(flow: &Flow, parent: &str) -> cyber_server::runtime::Job {
    admit_user_fork(flow, parent).await;
    completed(flow, parent).await
}

/// Admits and releases `/audit` as the user command `msg_fork_history`.
async fn admit_user_fork(flow: &Flow, parent: &str) {
    let source = flow.runtime.state(parent).await.unwrap();
    let turn = cyber_server::runtime::TurnContext {
        session_id: parent.into(),
        directory: source.info.directory.clone(),
        agent: source.info.agent.clone(),
        mode: source.info.mode.clone(),
        prefers_apply_patch: false,
        rules: source.info.rules.clone(),
        worktree: Default::default(),
    };
    let plan = flow
        .f
        .host
        .command_plan(&turn, "audit", "")
        .await
        .unwrap()
        .unwrap();
    let mut admission = plan.admission(Some("msg_fork_history".into()), Delivery::Hold);
    admission.resume = false;
    flow.runtime.admit_user(parent, admission).await.unwrap();
    flow.runtime
        .release(parent, "msg_fork_history", Delivery::Queue)
        .await
        .unwrap();
}

/// The forked child's provider request, found by its private skill body.
fn forked_request(flow: &Flow, model: &str) -> String {
    flow.requests(model)
        .iter()
        .map(|request| format!("{:?}", request.messages))
        .find(|messages| messages.contains("PRIVATE FORK BODY"))
        .unwrap_or_else(|| panic!("no forked child request on {model}"))
}

#[tokio::test]
async fn user_forked_skill_inherits_parent_history() {
    assert_user_fork_inherits_parent_history(None, "build", Some("test/review")).await;
}

#[tokio::test]
async fn user_forked_skill_with_named_agent_inherits_parent_history() {
    assert_user_fork_inherits_parent_history(Some("reviewer"), "reviewer", Some("test/review"))
        .await;
}

#[tokio::test]
async fn user_forked_skill_with_named_agent_and_no_model_inherits_parent_model() {
    assert_user_fork_inherits_parent_history(Some("reviewer"), "reviewer", None).await;
}

/// A global command hook that answers every event with `output`.
#[cfg(unix)]
fn hook(output: &str) -> serde_json::Value {
    json!({"type":"command","command":format!("cat >/dev/null; printf '%s' '{output}'"),"id":"guard"})
}

/// The reviewer fork of `/audit` (declared `agent: reviewer`, `model: test/review`).
#[cfg(unix)]
fn reviewer_fork_flow(reviewer: Vec<Vec<cyber_llm::adapters::ScriptStep>>) -> Flow {
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![text("handback handled"), text("parent idle")],
        false,
        Arc::new(NoSnapshots),
        vec![("test/review", reviewer)],
    );
    flow.f.set_config(json!({"permissions":"allow"}));
    flow.f.write(
        ".cyber/skills/audit/SKILL.md",
        "---\nname: audit\ndescription: Audit\ncontext: fork\nagent: reviewer\nmodel: test/review\n---\nPRIVATE FORK BODY\n",
    );
    flow
}

#[cfg(unix)]
fn receipts(
    flow: &Flow,
    session: &str,
    event: &str,
) -> Vec<cyber_server::runtime::HookExecutionRecord> {
    flow.runtime
        .hook_executions(session, 50)
        .unwrap()
        .into_iter()
        .filter(|receipt| receipt.event == event)
        .collect()
}

/// The reviewer's own tool calls run through the invoking Location's
/// PreToolUse hooks; bypass Mode does not skip a hook deny.
#[cfg(unix)]
#[tokio::test]
async fn pre_tool_use_deny_blocks_a_user_forked_reviewers_read_even_in_bypass() {
    let flow = reviewer_fork_flow(vec![
        call("read", "read", json!({"path":"notes.txt"})),
        text("findings without evidence"),
    ]);
    flow.f.write("notes.txt", "PRIVATE NOTE CONTENT");
    flow.f.global_hooks(json!({"PreToolUse":[{"matcher":"read","hooks":[hook(r#"{"decision":"deny","reason":"no reads"}"#)]}]}));
    let parent = flow.session("bypass").await;
    let job = run_user_fork(&flow, &parent).await;
    assert_eq!(job.status, JobStatus::Completed);
    let child = flow.runtime.state(&job.child_id).await.unwrap();
    assert_eq!(child.info.agent, "reviewer");
    assert_eq!(
        child.calls["read"].status,
        cyber_server::runtime::CallStatus::Error
    );
    let requests = flow.requests("test/review");
    assert_eq!(requests.len(), 2);
    let followup = format!("{:?}", requests[1].messages);
    assert!(
        followup.contains("blocked by hook guard: no reads"),
        "{followup}"
    );
    assert!(!followup.contains("PRIVATE NOTE CONTENT"), "{followup}");
    let blocked = receipts(&flow, &job.child_id, "PreToolUse");
    assert_eq!(blocked.len(), 1);
    assert_eq!(
        blocked[0].outcome,
        Some(cyber_core::hooks::HookOutcome::Blocked)
    );
    flow.settle(&parent).await;
    flow.runtime.shutdown().await;
}

/// SubagentStart sees the reviewer launch of a user fork and refuses it before
/// any child Session or Job exists.
#[cfg(unix)]
#[tokio::test]
async fn subagent_start_deny_refuses_a_user_forked_reviewer_before_launch() {
    let flow = reviewer_fork_flow(vec![text("never reviewed")]);
    flow.f.global_hooks(json!({"SubagentStart":[{"matcher":"reviewer","hooks":[hook(r#"{"decision":"deny","reason":"reviews frozen"}"#)]}]}));
    let parent = flow.session("bypass").await;
    admit_user_fork(&flow, &parent).await;
    let id =
        cyber_server::runtime::Runtime::skill_command_delegation_id(&parent, "msg_fork_history");
    let delegation = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(data) = flow.runtime.delegation(&parent, &id).unwrap()
                && data.status == cyber_server::runtime::DelegationStatus::Failed
            {
                break data;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(delegation.job_id.is_none());
    assert!(
        delegation
            .error
            .as_deref()
            .is_some_and(|error| error.contains("blocked by hook guard: reviews frozen")),
        "{:?}",
        delegation.error
    );
    flow.settle(&parent).await;
    let children: i64 = flow
        .f
        .store
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM session WHERE parent_id IS NOT NULL",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(children, 0);
    assert!(flow.runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert!(flow.requests("test/review").is_empty());
    let started = receipts(&flow, &parent, "SubagentStart");
    assert_eq!(started.len(), 1);
    assert_eq!(
        started[0].outcome,
        Some(cyber_core::hooks::HookOutcome::Blocked)
    );
    flow.runtime.shutdown().await;
}
