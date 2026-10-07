//! Profile starting Modes and step ceilings reach actual runtime admission.
mod support;

use cyber_server::runtime::*;
use serde_json::json;
use support::flow::{Flow, call, text};

async fn session(flow: &Flow, mode: Option<&str>) -> String {
    flow.runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            title: Some("Profile limits".into()),
            mode: mode.map(str::to_owned),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn configured_agent_plan_mode_starts_the_session_read_only() {
    let flow = Flow::new(
        vec![
            call(
                "write",
                "write",
                json!({"path":"result.txt","content":"changed"}),
            ),
            text("done"),
        ],
        false,
    );
    flow.f
        .set_config(json!({"agents":{"build":{"permission_mode":"plan"}}}));
    let id = session(&flow, None).await;
    flow.prompt(&id, "write result.txt").await;
    flow.settle(&id).await;
    assert_eq!(flow.runtime.state(&id).await.unwrap().info.mode, "plan");
    assert!(!flow.f.repo.join("result.txt").exists());
    assert!(
        flow.output(&id, "write")
            .await
            .contains("Plan mode is read-only")
    );
}

#[tokio::test]
async fn profile_step_limit_disables_tools_on_the_last_allowed_turn() {
    let flow = Flow::new(
        vec![
            call("r0", "read", json!({"path":"data.txt"})),
            call("r1", "read", json!({"path":"data.txt"})),
            call("r2", "read", json!({"path":"data.txt"})),
            text("should not reach this"),
        ],
        false,
    );
    flow.f.write("data.txt", "content");
    flow.f.set_config(json!({"agents":{"build":{"steps":3}}}));
    let id = session(&flow, Some("default")).await;
    flow.prompt(&id, "inspect").await;
    flow.settle(&id).await;
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 3);
    assert!(!requests[1].tools.is_empty());
    assert!(requests[2].tools.is_empty());
    assert_eq!(
        flow.output(&id, "r2").await,
        "Tools are disabled after the maximum agent steps"
    );
}

#[tokio::test]
async fn repaired_profile_mode_is_resolved_before_the_pending_prompt_runs() {
    let flow = Flow::new(
        vec![
            call(
                "write",
                "write",
                json!({"path":"result.txt","content":"changed"}),
            ),
            text("done"),
        ],
        false,
    );
    flow.f.set_config(json!({"agents":{"build":{"steps":0}}}));
    let id = session(&flow, None).await;
    flow.prompt(&id, "write result.txt").await;
    flow.settle(&id).await;
    assert_eq!(
        flow.runtime.state(&id).await.unwrap().inbox[0].status,
        InputStatus::Pending
    );
    flow.f
        .set_config(json!({"agents":{"build":{"permission_mode":"plan"}}}));
    flow.runtime.wake(&id).await.unwrap();
    flow.settle(&id).await;
    assert_eq!(flow.runtime.state(&id).await.unwrap().info.mode, "plan");
    assert!(!flow.f.repo.join("result.txt").exists());
}

#[tokio::test]
async fn explicit_mode_wins_and_profile_changes_do_not_rewrite_the_starting_choice() {
    let flow = Flow::new(vec![text("one"), text("two")], false);
    flow.f
        .set_config(json!({"agents":{"build":{"permission_mode":"plan"}}}));
    let explicit = session(&flow, Some("default")).await;
    let inherited = session(&flow, None).await;
    flow.f
        .set_config(json!({"agents":{"build":{"permission_mode":"bypass"}}}));
    for id in [&explicit, &inherited] {
        flow.prompt(id, "continue").await;
        flow.settle(id).await;
    }
    assert_eq!(
        flow.runtime.state(&explicit).await.unwrap().info.mode,
        "default"
    );
    assert_eq!(
        flow.runtime.state(&inherited).await.unwrap().info.mode,
        "plan"
    );
}

#[tokio::test]
async fn same_value_explicit_switch_settles_an_unavailable_starting_default_once() {
    let flow = Flow::new(vec![text("done")], false);
    flow.f.set_config(json!({"agents":{"build":{"steps":0}}}));
    let id = session(&flow, None).await;
    flow.runtime.switch_mode(&id, "default").await.unwrap();
    flow.runtime.switch_mode(&id, "default").await.unwrap();
    flow.f
        .set_config(json!({"agents":{"build":{"permission_mode":"plan"}}}));
    flow.prompt(&id, "continue").await;
    flow.settle(&id).await;
    let rows = flow.f.store.read_events(&id, -1, 500).unwrap().events;
    assert_eq!(
        rows.iter()
            .filter(|r| r.kind == "session.mode.switched.1")
            .count(),
        1
    );
    assert_eq!(SessionState::replay(&rows).unwrap().info.mode, "default");
}

#[tokio::test]
async fn replay_and_fork_retain_a_pending_starting_default_until_repair() {
    let flow = Flow::new(vec![text("one"), text("two")], false);
    flow.f.set_config(json!({"agents":{"build":{"steps":0}}}));
    let id = session(&flow, None).await;
    let fork = flow.runtime.fork(&id, None).await.unwrap();
    let rows = flow.f.store.read_events(&fork.id, -1, 500).unwrap().events;
    assert_eq!(rows[0].data["mode_default_pending"], true);
    assert_eq!(SessionState::replay(&rows).unwrap().info.mode, "default");
    flow.f
        .set_config(json!({"agents":{"build":{"permission_mode":"plan"}}}));
    for current in [&id, &fork.id] {
        flow.prompt(current, "continue").await;
        flow.settle(current).await;
        let rows = flow.f.store.read_events(current, -1, 500).unwrap().events;
        assert_eq!(SessionState::replay(&rows).unwrap().info.mode, "plan");
    }
}

#[tokio::test]
async fn promoting_new_input_resets_the_profile_step_limit() {
    let flow = Flow::new(vec![text("one"), text("two")], false);
    flow.f.set_config(json!({"agents":{"build":{"steps":1}}}));
    let id = session(&flow, Some("default")).await;
    for prompt in ["first", "second"] {
        flow.prompt(&id, prompt).await;
        flow.settle(&id).await;
        assert_eq!(flow.runtime.state(&id).await.unwrap().steps_since_input, 1);
    }
    let requests = flow.main.requests();
    assert!(
        requests
            .iter()
            .all(|r| r.tools.is_empty() && r.tools_disabled)
    );
}

#[tokio::test]
async fn session_budgets_remain_stricter_than_a_larger_profile_limit() {
    let flow = Flow::new(
        vec![
            call("r0", "read", json!({"path":"data.txt"})),
            text("summary"),
        ],
        false,
    );
    flow.f.write("data.txt", "content");
    flow.f.set_config(json!({"agents":{"build":{"steps":100}}}));
    let id = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("default".into()),
            max_steps: Some(1),
            title: Some("Limited".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    flow.prompt(&id, "inspect").await;
    flow.settle(&id).await;
    let requests = flow.main.requests();
    assert!(!requests[0].tools_disabled);
    assert!(requests[1].tools_disabled && requests[1].tools.is_empty());
}

#[tokio::test]
async fn general_subagents_default_to_a_summary_on_the_fiftieth_turn() {
    let script = (0..50)
        .map(|i| {
            call(
                &format!("r{i}"),
                "read",
                json!({"path":"data.txt", "offset":i + 1, "limit":1}),
            )
        })
        .collect();
    let flow = Flow::new(script, false);
    flow.f.write("data.txt", &"content\n".repeat(50));
    let parent = session(&flow, Some("default")).await;
    let id = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: Some(parent),
            agent: Some("general".into()),
            mode: Some("default".into()),
            max_steps: Some(100),
            title: Some("Child".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    flow.prompt(&id, "inspect").await;
    flow.settle(&id).await;
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 50);
    assert!(!requests[48].tools_disabled);
    assert!(requests[49].tools_disabled && requests[49].tools.is_empty());
    assert_eq!(
        flow.output(&id, "r49").await,
        "Tools are disabled after the maximum agent steps"
    );
}
