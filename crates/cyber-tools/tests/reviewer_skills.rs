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
