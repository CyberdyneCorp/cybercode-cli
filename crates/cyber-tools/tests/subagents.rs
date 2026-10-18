//! Foreground child execution through actual built-in dispatch and durable Sessions.
mod support;

use serde_json::{Value, json};
use support::flow::{Flow, call, text};

#[tokio::test]
async fn foreground_child_returns_only_its_final_answer_and_durable_identity() {
    let flow = Flow::new(
        vec![
            call(
                "spawn",
                "agent",
                json!({"prompt":"inspect the parser", "agent":"general", "description":"Inspect the parser"}),
            ),
            call("read", "read", json!({"path":"data.txt"})),
            text("parser findings"),
            text("parent done"),
        ],
        false,
    );
    flow.f.write("data.txt", "private intermediate output");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let output = flow.output(&parent, "spawn").await;
    let result: Value = serde_json::from_str(&output).expect(&output);
    let child = flow
        .runtime
        .state(result["id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(child.info.parent_id.as_deref(), Some(parent.as_str()));
    assert_eq!(child.info.title, "Inspect the parser (@general)");
    assert_eq!(result["text"], "parser findings");
    let mut normalized = result.clone();
    normalized["id"] = json!("<session>");
    support::golden(&flow.f, "agent", &normalized.to_string());
    assert!(!output.contains("private intermediate"));
    assert_eq!(flow.main.requests().len(), 4);
    assert!(!flow.runtime.is_running(&child.info.id));
}

#[tokio::test]
async fn denied_or_unknown_agents_do_not_create_children() {
    for (agent, permissions, expected) in [
        (
            "general",
            json!({"agent":{"general":"deny"}}),
            "Permission denied",
        ),
        (
            "missing",
            json!({"agent":"allow"}),
            "Unknown agent \"missing\". Available:",
        ),
        ("build", json!({"agent":"allow"}), "cannot run a subagent"),
        (
            "title",
            json!({"agent":"allow"}),
            "Unknown agent \"title\". Available:",
        ),
    ] {
        let flow = Flow::new(
            vec![
                call("spawn", "agent", json!({"prompt":"inspect", "agent":agent})),
                text("done"),
            ],
            false,
        );
        flow.f.set_config(json!({"permissions":permissions}));
        let parent = flow.session("bypass").await;
        flow.prompt(&parent, "delegate").await;
        flow.settle(&parent).await;
        let output = flow.output(&parent, "spawn").await;
        assert!(output.contains(expected), "{output}");
        let count = flow
            .f
            .store
            .read(|conn| {
                Ok(conn.query_row(
                    "SELECT count(*) FROM session WHERE parent_id IS NOT NULL",
                    [],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn depth_limit_refuses_before_child_creation() {
    let flow = Flow::new(
        vec![
            call("spawn", "agent", json!({"prompt":"inspect"})),
            text("done"),
        ],
        false,
    );
    flow.f
        .set_config(json!({"permissions":{"agent":"allow"}, "agents":{"max_depth":0}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    assert!(
        flow.output(&parent, "spawn")
            .await
            .contains("Subagent depth limit reached (0)")
    );
}

#[tokio::test]
async fn oversized_final_answer_has_a_utf8_preview_and_full_managed_artifact() {
    let answer = "é".repeat(20_000);
    let flow = Flow::new(
        vec![
            call("spawn", "agent", json!({"prompt":"inspect"})),
            text(&answer),
            text("done"),
        ],
        false,
    );
    flow.f
        .set_config(json!({"permissions":{"agent":"allow"},"agents":{"result_max_bytes":16383}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let result: Value = serde_json::from_str(&flow.output(&parent, "spawn").await).unwrap();
    assert_eq!(result["text"].as_str().unwrap().len(), 16382);
    assert_eq!(
        std::fs::read_to_string(result["output_file"].as_str().unwrap()).unwrap(),
        answer
    );
}

#[tokio::test]
async fn parent_interrupt_stops_a_child_waiting_for_permission() {
    let flow = Flow::new(
        vec![
            call("spawn", "agent", json!({"prompt":"read secrets"})),
            call("read", "read", json!({"path":".env"})),
            text("must not run"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    let request = flow.pending(&parent).await;
    let child = request.session_id.clone();
    assert_ne!(child, parent);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        flow.runtime.interrupt(&parent),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!flow.runtime.is_running(&parent));
    assert!(!flow.runtime.is_running(&child));
    assert_eq!(flow.main.requests().len(), 2);
    assert_eq!(
        flow.runtime
            .state(&child)
            .await
            .unwrap()
            .info
            .parent_id
            .as_deref(),
        Some(parent.as_str())
    );
}

#[tokio::test]
async fn dropping_a_tool_future_stops_its_child() {
    use cyber_server::runtime::ToolHost;
    use tokio_util::sync::CancellationToken;
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":".env"})),
            text("must not run"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    let mut inv = flow
        .f
        .invocation("default", "agent", json!({"prompt":"read secrets"}));
    inv.session_id = parent.clone();
    let host = flow.f.host.clone();
    let task = tokio::spawn(async move { host.execute(inv, CancellationToken::new()).await });
    let request = flow.pending(&parent).await;
    let child = request.session_id.clone();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        flow.runtime.wait_idle(&child),
    )
    .await
    .unwrap();
    assert!(!flow.runtime.is_running(&child));
    assert_eq!(flow.main.requests().len(), 1);
}

#[tokio::test]
async fn a_profile_cannot_start_a_child_in_bypass_under_default() {
    let flow = Flow::new(
        vec![
            call("spawn", "agent", json!({"prompt":"inspect"})),
            text("findings"),
            text("done"),
        ],
        false,
    );
    flow.f.set_config(
        json!({"permissions":{"agent":"allow"},"agents":{"general":{"permission_mode":"bypass"}}}),
    );
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let result: Value = serde_json::from_str(&flow.output(&parent, "spawn").await).unwrap();
    assert_eq!(
        flow.runtime
            .state(result["id"].as_str().unwrap())
            .await
            .unwrap()
            .info
            .mode,
        "default"
    );
}

#[tokio::test]
async fn concurrency_waits_fifo_without_creating_queued_children() {
    use cyber_server::runtime::{PermissionReply, ToolHost};
    use tokio_util::sync::CancellationToken;
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":".env"})),
            text("first"),
            call("read", "read", json!({"path":".env"})),
            text("second"),
            call("read", "read", json!({"path":".env"})),
            text("third"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f
        .set_config(json!({"permissions":{"agent":"allow"},"agents":{"max_concurrent":1}}));
    let parent = flow.session("default").await;
    let invocation = |description: &str| {
        let mut inv = flow.f.invocation(
            "default",
            "agent",
            json!({"prompt":"inspect", "description":description}),
        );
        inv.session_id = parent.clone();
        inv
    };
    let host = flow.f.host.clone();
    let first_inv = invocation("First queued child task");
    let first =
        tokio::spawn(async move { host.execute(first_inv, CancellationToken::new()).await });
    let request = flow.pending(&parent).await;
    let host = flow.f.host.clone();
    let second_inv = invocation("Second queued child task");
    let mut second =
        Box::pin(async move { host.execute(second_inv, CancellationToken::new()).await });
    assert!(futures::poll!(&mut second).is_pending());
    let host = flow.f.host.clone();
    let third_inv = invocation("Third queued child task");
    let mut third =
        Box::pin(async move { host.execute(third_inv, CancellationToken::new()).await });
    assert!(futures::poll!(&mut third).is_pending());
    let count = flow
        .f
        .store
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM session WHERE parent_id IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 1);
    let second = tokio::spawn(second);
    let third = tokio::spawn(third);
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    support::ok(first.await.unwrap());
    let request = flow.pending(&parent).await;
    assert_eq!(
        flow.runtime
            .state(&request.session_id)
            .await
            .unwrap()
            .info
            .title,
        "Second queued child task (@general)"
    );
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    support::ok(second.await.unwrap());
    let request = flow.pending(&parent).await;
    assert_eq!(
        flow.runtime
            .state(&request.session_id)
            .await
            .unwrap()
            .info
            .title,
        "Third queued child task (@general)"
    );
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    support::ok(third.await.unwrap());
    assert_eq!(flow.main.requests().len(), 6);
}

#[tokio::test]
async fn unsupported_isolation_fails_before_creating_sessions() {
    let input = json!({"prompt":"inspect","isolation":"remote"});
    let flow = Flow::new(vec![call("spawn", "agent", input), text("done")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    assert!(flow.output(&parent, "spawn").await.contains("isolation"));
    let count = flow
        .f
        .store
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM session WHERE parent_id IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn a_queued_call_rechecks_disabled_profiles_before_creation() {
    use cyber_server::runtime::{PermissionReply, ToolHost, ToolOutcome};
    use tokio_util::sync::CancellationToken;
    let flow = Flow::new(
        vec![call("read", "read", json!({"path":".env"})), text("done")],
        true,
    );
    flow.f.write(".env", "private");
    flow.f
        .set_config(json!({"permissions":{"agent":"allow"},"agents":{"max_concurrent":1}}));
    let parent = flow.session("default").await;
    let mut inv = flow.f.invocation(
        "default",
        "agent",
        json!({"prompt":"inspect", "model":"test/main"}),
    );
    inv.session_id = parent.clone();
    let host = flow.f.host.clone();
    let first_inv = inv.clone();
    let first =
        tokio::spawn(async move { host.execute(first_inv, CancellationToken::new()).await });
    let request = flow.pending(&parent).await;
    let host = flow.f.host.clone();
    let mut second = Box::pin(async move { host.execute(inv, CancellationToken::new()).await });
    assert!(futures::poll!(&mut second).is_pending());
    flow.f.set_config(json!({"permissions":{"agent":"allow"},"agents":{"max_concurrent":1,"general":{"disabled":true}}}));
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    assert!(matches!(first.await.unwrap(), ToolOutcome::Failed(_)));
    let result = support::failed(second.await);
    assert!(result.contains("Unknown agent \"general\""), "{result}");
    let count = flow
        .f
        .store
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM session WHERE parent_id IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn queued_child_launch_is_refused_after_source_interrupt() {
    use cyber_server::runtime::{PermissionReply, ToolHost};
    use tokio_util::sync::CancellationToken;
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":".env"})),
            text("done"),
            text("late child"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f
        .set_config(json!({"permissions":{"agent":"allow"},"agents":{"max_concurrent":1}}));
    let parent = flow.session("default").await;
    let mut inv = flow.f.invocation(
        "default",
        "agent",
        json!({"prompt":"inspect", "model":"test/main"}),
    );
    inv.session_id = parent.clone();
    let host = flow.f.host.clone();
    let first_inv = inv.clone();
    let first =
        tokio::spawn(async move { host.execute(first_inv, CancellationToken::new()).await });
    let request = flow.pending(&parent).await;
    let host = flow.f.host.clone();
    let mut second = Box::pin(async move { host.execute(inv, CancellationToken::new()).await });
    assert!(futures::poll!(&mut second).is_pending());
    flow.runtime.interrupt(&parent).await.unwrap();
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    support::ok(first.await.unwrap());
    let result = support::failed(second.await);
    assert!(result.contains("fenced"), "{result}");
    let count = flow
        .f
        .store
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM session WHERE parent_id IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, 1);
}

/// The `agent` description each Turn sends to the model.
fn agent_description(request: &cyber_llm::LlmRequest) -> String {
    request
        .tools
        .iter()
        .find(|tool| tool.name == "agent")
        .map(|tool| tool.description.clone())
        .expect("agent tool offered")
}

#[tokio::test]
async fn agent_tool_description_lists_permitted_subagents_per_turn() {
    let flow = Flow::new(vec![text("first"), text("second")], false);
    flow.f.set_config(json!({
        "permissions":{"agent":{"general":"deny"}},
        "agents":{
            "zeta":{"description":"Zeta   helper\nfor tests","mode":"subagent"},
            "quiet":{"description":"Hidden helper","mode":"subagent","hidden":true},
            "lead":{"description":"Primary only","mode":"primary"}
        }
    }));
    let parent = flow.session("default").await;
    flow.prompt(&parent, "first").await;
    flow.settle(&parent).await;
    flow.f.set_config(json!({
        "permissions":{"agent":{"general":"deny"}},
        "agents":{"alpha":{"description":"Loaded later","mode":"all"}}
    }));
    flow.prompt(&parent, "second").await;
    flow.settle(&parent).await;
    let requests = flow.main.requests();
    let first = agent_description(&requests[0]);
    let catalogue = first.split_once("\n\n").unwrap().1;
    assert_eq!(
        catalogue,
        "Available agents:\n\
         - explore: Explore code and answer questions without changing files\n\
         - reviewer: Review changes and report structured findings without modifying files\n\
         - zeta: Zeta helper for tests"
    );
    let second = agent_description(&requests[1]);
    assert!(
        second.contains("Available agents:\n- alpha: Loaded later\n- explore:"),
        "{second}"
    );
    assert!(!second.contains("- zeta:") && !second.contains("- general:"));
}

/// Every `session.tool.progress` the runtime publishes, as `<call>:<status>`.
fn record_progress(flow: &Flow) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut live = flow.runtime.subscribe();
    let sink = seen.clone();
    tokio::spawn(async move {
        while let Ok(event) = live.recv().await {
            if let cyber_server::runtime::LiveEvent::ToolProgress {
                session_id,
                call_id,
                status,
            } = event
            {
                sink.lock()
                    .unwrap()
                    .push(format!("{session_id}/{call_id}:{status}"));
            }
        }
    });
    seen
}

async fn wait_for(seen: &std::sync::Mutex<Vec<String>>, entry: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !seen.lock().unwrap().iter().any(|seen| seen == entry) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {entry} progress"));
}

#[tokio::test]
async fn queued_foreground_spawn_reports_queued_progress() {
    use cyber_server::runtime::{PermissionReply, ToolHost};
    use tokio_util::sync::CancellationToken;
    let flow = Flow::new(
        vec![
            call("read", "read", json!({"path":".env"})),
            text("first"),
            call("read", "read", json!({"path":".env"})),
            text("second"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f
        .set_config(json!({"permissions":{"agent":"allow"},"agents":{"max_concurrent":1}}));
    let parent = flow.session("default").await;
    let invocation = |call_id: &str| {
        let mut inv = flow.f.invocation(
            "default",
            "agent",
            json!({"prompt":"inspect", "description":"Inspect one private file"}),
        );
        inv.session_id = parent.clone();
        inv.call_id = call_id.into();
        inv
    };
    let seen = record_progress(&flow);
    let host = flow.f.host.clone();
    let first_inv = invocation("first");
    let first =
        tokio::spawn(async move { host.execute(first_inv, CancellationToken::new()).await });
    let request = flow.pending(&parent).await;
    let host = flow.f.host.clone();
    let second_inv = invocation("second");
    let second =
        tokio::spawn(async move { host.execute(second_inv, CancellationToken::new()).await });
    wait_for(&seen, &format!("{parent}/second:queued")).await;
    assert_eq!(*seen.lock().unwrap(), [format!("{parent}/second:queued")]);
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    support::ok(first.await.unwrap());
    wait_for(&seen, &format!("{parent}/second:running")).await;
    let request = flow.pending(&parent).await;
    flow.runtime
        .reply_permission(&request.id, PermissionReply::Once)
        .await
        .unwrap();
    support::ok(second.await.unwrap());
    assert_eq!(
        *seen.lock().unwrap(),
        [
            format!("{parent}/second:queued"),
            format!("{parent}/second:running")
        ]
    );
}
