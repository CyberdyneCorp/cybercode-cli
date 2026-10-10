mod support;
use cyber_server::runtime::{Delivery, NoSnapshots};
use serde_json::json;
use std::sync::Arc;
use support::flow::{Flow, call, text};

#[tokio::test]
async fn skill_command_model_applies_without_replacing_the_session_model() {
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![text("base answer")],
        false,
        Arc::new(NoSnapshots),
        vec![(
            "other/skill",
            vec![
                call("read", "read", json!({"path":"release.txt"})),
                text("skill answer"),
            ],
        )],
    );
    flow.f.write(".cyber/skills/release/SKILL.md", "---\nname: release\ndescription: Release changes\nmodel: other/skill\n---\nRelease $ARGUMENTS\n");
    flow.f.write("release.txt", "release data");
    let id = flow.session("default").await;
    let info = flow.runtime.state(&id).await.unwrap().info;
    let turn = cyber_server::runtime::TurnContext {
        session_id: id.clone(),
        directory: info.directory,
        agent: info.agent,
        mode: info.mode,
        prefers_apply_patch: false,
        rules: info.rules,
    };
    let plan = flow
        .f
        .host
        .command_plan(&turn, "release", "now")
        .await
        .unwrap()
        .unwrap();
    flow.runtime
        .admit_user(&id, plan.admission(None, Delivery::Queue))
        .await
        .unwrap();
    flow.settle(&id).await;
    assert!(
        format!("{:?}", flow.runtime.state(&id).await.unwrap().entries).contains("skill answer")
    );
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().info.model,
        "test/main"
    );
    let epoch = flow.runtime.state(&id).await.unwrap().epoch.unwrap();
    assert_eq!(epoch.provider, "other");
    assert_eq!(epoch.number, 1);
    assert!(flow.main.requests().is_empty());
    flow.runtime.resume(&id).await.unwrap();
    flow.settle(&id).await;
    assert!(
        format!("{:?}", flow.runtime.state(&id).await.unwrap().entries).contains("base answer")
    );
    assert_eq!(flow.main.requests().len(), 1);
    flow.runtime.shutdown().await;
}

async fn turn(flow: &Flow, id: &str) -> cyber_server::runtime::TurnContext {
    let info = flow.runtime.state(id).await.unwrap().info;
    cyber_server::runtime::TurnContext {
        session_id: id.into(),
        directory: info.directory,
        agent: info.agent,
        mode: info.mode,
        prefers_apply_patch: false,
        rules: info.rules,
    }
}

#[tokio::test]
async fn user_only_skill_grants_expire_after_the_loading_turn() {
    use cyber_server::runtime::CallStatus;
    let flow = Flow::new(
        vec![
            call(
                "first",
                "write",
                json!({"path":"release.txt","content":"release"}),
            ),
            call(
                "later",
                "write",
                json!({"path":"later.txt","content":"later"}),
            ),
            text("done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"edit":"ask"}}));
    flow.f.write(".cyber/skills/release/SKILL.md", "---\nname: release\ndescription: Release changes\ndisable-model-invocation: true\nallowed-tools: ['write:*']\n---\nRelease carefully\n");
    let id = flow.session("default").await;
    let frame = turn(&flow, &id).await;
    let plan = flow
        .f
        .host
        .command_plan(&frame, "release", "")
        .await
        .unwrap()
        .unwrap();
    flow.runtime
        .admit_user(&id, plan.admission(None, Delivery::Queue))
        .await
        .unwrap();
    flow.settle(&id).await;
    let state = flow.runtime.state(&id).await.unwrap();
    assert_eq!(state.calls["first"].status, CallStatus::Ok);
    assert_eq!(state.calls["later"].status, CallStatus::Error);
    assert_eq!(
        std::fs::read_to_string(flow.f.repo.join("release.txt")).unwrap(),
        "release"
    );
    assert!(!flow.f.repo.join("later.txt").exists());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn captured_command_declarations_survive_replay_and_bind_retry_identity() {
    let mut flow = Flow::new(Vec::new(), false);
    let path = ".cyber/skills/release/SKILL.md";
    flow.f.write(path,"---\nname: release\ndescription: Release changes\nmodel: test/main\nallowed-tools: ['write:*']\n---\nRelease carefully\n");
    let id = flow.session("default").await;
    let frame = turn(&flow, &id).await;
    let plan = flow
        .f
        .host
        .command_plan(&frame, "release", "")
        .await
        .unwrap()
        .unwrap();
    let mut admission = plan.admission(Some("msg_skill_command".into()), Delivery::Hold);
    admission.resume = false;
    let first = flow
        .runtime
        .admit_user(&id, admission.clone())
        .await
        .unwrap();
    assert_eq!(
        flow.runtime
            .admit_user(&id, admission.clone())
            .await
            .unwrap(),
        first
    );
    flow.f.write(path,"---\nname: release\ndescription: Release changes\nmodel: test/other\ndisallowed-tools: ['write:*']\n---\nRelease carefully\n");
    flow.restart_default_runtime().await;
    let state = flow.runtime.state(&id).await.unwrap();
    let captured = state.inbox[0].skill_command.as_ref().unwrap();
    assert_eq!(captured.model.as_deref(), Some("test/main"));
    assert_eq!(captured.activation.allowed_tools, ["write:*"]);
    assert!(captured.activation.disallowed_tools.is_empty());
    assert_eq!(
        flow.runtime.admit_user(&id, admission).await.unwrap(),
        first
    );
    let plan = flow
        .f
        .host
        .command_plan(&frame, "release", "")
        .await
        .unwrap()
        .unwrap();
    let mut changed = plan.admission(Some("msg_skill_command".into()), Delivery::Hold);
    changed.resume = false;
    assert!(matches!(
        flow.runtime.admit_user(&id, changed).await,
        Err(cyber_server::runtime::RuntimeError::PromptConflict(_))
    ));
    assert!(flow.main.requests().is_empty());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn explicit_skill_denial_refuses_user_command_before_admission() {
    let flow = Flow::new(Vec::new(), false);
    flow.f
        .set_config(json!({"permissions":{"skill":{"release":"deny"}}}));
    flow.f.write(".cyber/skills/release/SKILL.md","---\nname: release\ndescription: Release changes\nallowed-tools: ['write:*']\n---\nRelease carefully\n");
    let id = flow.session("bypass").await;
    let frame = turn(&flow, &id).await;
    let before = flow.runtime.state(&id).await.unwrap().last_seq;
    let error = flow
        .f
        .host
        .command_plan(&frame, "release", "")
        .await
        .unwrap_err();
    assert!(error.contains("Permission denied"), "{error}");
    assert_eq!(flow.runtime.state(&id).await.unwrap().last_seq, before);
    assert!(flow.main.requests().is_empty());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn model_loaded_inline_skill_does_not_change_the_callers_model() {
    let flow = Flow::with_models(
        support::Fixture::new(),
        vec![
            call("skill", "skill", json!({"name":"release"})),
            text("base answer"),
        ],
        false,
        Arc::new(NoSnapshots),
        vec![("test/skill", vec![text("wrong model")])],
    );
    flow.f.write(".cyber/skills/release/SKILL.md","---\nname: release\ndescription: Release changes\nmodel: test/skill\n---\nRelease carefully\n");
    let id = flow.session("bypass").await;
    flow.prompt(&id, "load release").await;
    flow.settle(&id).await;
    let state = flow.runtime.state(&id).await.unwrap();
    assert!(format!("{:?}", state.entries).contains("base answer"));
    assert_eq!(state.info.model, "test/main");
    assert_eq!(flow.main.requests().len(), 2);
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn a_fork_inherits_the_commands_effective_model() {
    let flow = Flow::with_models(
        support::Fixture::new(),
        Vec::new(),
        false,
        Arc::new(NoSnapshots),
        vec![(
            "test/skill",
            vec![
                call(
                    "fork",
                    "agent",
                    json!({"prompt":"inspect independently","fork":true}),
                ),
                text("child findings"),
                text("parent answer"),
            ],
        )],
    );
    flow.f.write(".cyber/skills/release/SKILL.md","---\nname: release\ndescription: Release changes\nmodel: test/skill\nallowed-tools: [agent]\n---\nInspect independently\n");
    let id = flow.session("default").await;
    let frame = turn(&flow, &id).await;
    let plan = flow
        .f
        .host
        .command_plan(&frame, "release", "")
        .await
        .unwrap()
        .unwrap();
    flow.runtime
        .admit_user(&id, plan.admission(None, Delivery::Queue))
        .await
        .unwrap();
    flow.settle(&id).await;
    let result: serde_json::Value = serde_json::from_str(&flow.output(&id, "fork").await).unwrap();
    let child = flow
        .runtime
        .state(result["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(child.info.model, "test/skill");
    assert_eq!(result["text"], "child findings");
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().info.model,
        "test/main"
    );
    assert!(flow.main.requests().is_empty());
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn unavailable_command_model_preserves_pending_input_without_using_the_base_model() {
    let flow = Flow::new(vec![text("base must not run")], false);
    flow.f.write(".cyber/skills/release/SKILL.md", "---\nname: release\ndescription: Release changes\nmodel: test/unavailable\n---\nRelease changes\n");
    let id = flow.session("default").await;
    let plan = flow
        .f
        .host
        .command_plan(&turn(&flow, &id).await, "release", "")
        .await
        .unwrap()
        .unwrap();
    flow.runtime
        .admit_user(&id, plan.admission(None, Delivery::Queue))
        .await
        .unwrap();
    flow.settle(&id).await;
    let state = flow.runtime.state(&id).await.unwrap();
    assert_eq!(state.pending(Delivery::Queue).count(), 1);
    assert!(state.entries.is_empty());
    assert!(state.epoch.is_none());
    assert_eq!(state.info.model, "test/main");
    assert!(flow.main.requests().is_empty());
    flow.runtime.resume(&id).await.unwrap();
    flow.settle(&id).await;
    assert_eq!(
        flow.runtime
            .state(&id)
            .await
            .unwrap()
            .pending(Delivery::Queue)
            .count(),
        1
    );
    assert!(flow.main.requests().is_empty());
    flow.runtime.shutdown().await;
}
