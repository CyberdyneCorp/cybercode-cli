//! SubagentStart/SubagentStop hooks around every real child launch and handback.
#![cfg(unix)]
mod support;
use cyber_core::hooks::HookOutcome;
use cyber_llm::ErrorKind;
use cyber_llm::adapters::ScriptStep;
use cyber_server::runtime::{
    DelegationPhase, DelegationStatus, HookExecutionRecord, JobStatus, LiveEvent, NoSnapshots,
    ToolHost, UserSubtask,
};
use serde_json::{Value, json};
use std::sync::Arc;
use support::flow::{Flow, call, text};
use tokio_util::sync::CancellationToken;

fn hooks(f: &Flow, hooks: Value) {
    f.f.global_hooks(hooks);
}

fn command(output: &str) -> Value {
    json!({"type":"command","command":format!("cat >/dev/null; printf '%s' '{}'",output.replace('\'',"'\\''")),"id":"guard"})
}

fn capture(file: &str) -> Value {
    json!({"type":"command","command":format!("{{ cat; echo; }} >> {file}"),"id":"capture"})
}

fn lines(f: &Flow, file: &str) -> Vec<Value> {
    std::fs::read_to_string(f.f.repo.join(file))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn receipts(f: &Flow, session: &str, event: &str) -> Vec<HookExecutionRecord> {
    f.runtime
        .hook_executions(session, 50)
        .unwrap()
        .into_iter()
        .filter(|receipt| receipt.event == event)
        .collect()
}

fn children(f: &Flow) -> i64 {
    f.f.store
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM session WHERE parent_id IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .unwrap()
}

fn git(directory: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn git_flow(script: Vec<Vec<ScriptStep>>) -> Flow {
    let f = support::Fixture::native_git();
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.name", "Hook test"],
        vec!["config", "user.email", "test@example.invalid"],
    ] {
        git(&f.repo, &args);
    }
    f.write("tracked.txt", "original\n");
    git(&f.repo, &["add", "tracked.txt"]);
    git(&f.repo, &["commit", "--quiet", "-m", "initial"]);
    Flow::with(f, script, false, Arc::new(NoSnapshots))
}

fn spawn(description: &str) -> Vec<ScriptStep> {
    call(
        "spawn",
        "agent",
        json!({"prompt":"inspect the parser","agent":"general","description":description}),
    )
}

async fn background(flow: &Flow, parent: &str, input: Value) -> Value {
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
            if job.status != JobStatus::Running {
                return job;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("job did not finish")
}

#[tokio::test]
async fn start_deny_refuses_before_any_child_effect_even_in_bypass() {
    let flow = git_flow(vec![
        call(
            "spawn",
            "agent",
            json!({"prompt":"inspect","agent":"general","description":"Inspect the parser","isolation":"worktree"}),
        ),
        text("parent done"),
    ]);
    flow.f
        .set_config(json!({"permissions":{"agent":"allow","worktree":"allow"}}));
    hooks(
        &flow,
        json!({"SubagentStart":[{"matcher":"general","hooks":[command(r#"{"decision":"deny","reason":"no children"}"#)]}]}),
    );
    let parent = flow.session("bypass").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let output = flow.output(&parent, "spawn").await;
    assert!(
        output.contains("blocked by hook guard: no children"),
        "{output}"
    );
    assert_eq!(children(&flow), 0);
    assert!(flow.runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert_eq!(git(&flow.f.repo, &["worktree", "list"]).lines().count(), 1);
    // Only the parent's two Turns reached the provider.
    assert_eq!(flow.main.requests().len(), 2);
    let started = receipts(&flow, &parent, "SubagentStart");
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].outcome, Some(HookOutcome::Blocked));
}

#[tokio::test]
async fn start_matcher_selects_by_agent_and_payload_carries_child_identity() {
    let flow = Flow::new(
        vec![
            spawn("Inspect the parser"),
            text("child answer"),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    hooks(
        &flow,
        json!({"SubagentStart":[
            {"matcher":"explore","hooks":[command(r#"{"decision":"deny","reason":"no explore"}"#)]},
            {"hooks":[capture("start.jsonl")]}
        ]}),
    );
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let output = flow.output(&parent, "spawn").await;
    let result: Value = serde_json::from_str(&output).expect(&output);
    assert_eq!(result["text"], "child answer");
    let start = lines(&flow, "start.jsonl");
    assert_eq!(start.len(), 1);
    let start = &start[0];
    assert_eq!(start["event"], "SubagentStart");
    assert_eq!(start["session_id"], parent);
    assert_eq!(start["agent_type"], "general");
    assert_eq!(start["child_session_id"], result["id"]);
    assert_eq!(start["background"], false);
    assert_eq!(start["fork"], false);
    assert_eq!(start["isolation"], "none");
    assert_eq!(start["resume"], false);
    assert_eq!(start["user_requested"], false);
    assert_eq!(start["description"], "Inspect the parser");
    assert!(start["skill"].is_null());
    assert_eq!(receipts(&flow, &parent, "SubagentStart").len(), 1);
}

#[tokio::test]
async fn user_delegation_is_blocked_before_launch_and_settles_failed() {
    let flow = Flow::new(vec![], false);
    hooks(
        &flow,
        json!({"SubagentStart":[{"matcher":"general","hooks":[command(r#"{"decision":"deny","reason":"review frozen"}"#)]}]}),
    );
    let parent = flow.session("default").await;
    let id = "op_hooked";
    flow.runtime
        .start_delegation(
            &parent,
            id,
            UserSubtask {
                skill_command: None,
                admission_id: None,
                prompt: "review".into(),
                agent: Some("general".into()),
                attachments: vec![],
                max_steps: None,
                hook_context: None,
            },
        )
        .await
        .unwrap();
    let done = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let data = flow.runtime.delegation(&parent, id).unwrap().unwrap();
            if !matches!(
                data.status,
                DelegationStatus::Pending | DelegationStatus::Cancelling
            ) {
                return data;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(done.status, DelegationStatus::Failed);
    assert_eq!(done.phase, DelegationPhase::Reserved);
    assert!(done.job_id.is_none());
    assert!(
        done.error
            .as_deref()
            .unwrap_or_default()
            .contains("blocked by hook guard: review frozen"),
        "{:?}",
        done.error
    );
    assert_eq!(children(&flow), 0);
    assert!(flow.main.requests().is_empty());
    let started = receipts(&flow, &parent, "SubagentStart");
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].outcome, Some(HookOutcome::Blocked));
}

#[tokio::test]
async fn stop_fires_once_for_foreground_completion_without_result_text() {
    let flow = Flow::new(
        vec![
            spawn("Inspect the parser"),
            text("private child answer"),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    hooks(
        &flow,
        json!({"SubagentStop":[{"matcher":"general","hooks":[capture("stop.jsonl")]}]}),
    );
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let output = flow.output(&parent, "spawn").await;
    let result: Value = serde_json::from_str(&output).expect(&output);
    let stop = lines(&flow, "stop.jsonl");
    assert_eq!(stop.len(), 1);
    assert_eq!(stop[0]["event"], "SubagentStop");
    assert_eq!(stop[0]["session_id"], parent);
    assert_eq!(stop[0]["child_session_id"], result["id"]);
    assert_eq!(stop[0]["agent_type"], "general");
    assert_eq!(stop[0]["subagent_name"], "general");
    assert_eq!(stop[0]["status"], "completed");
    assert!(stop[0]["error"].is_null());
    assert!(!stop[0].to_string().contains("private child answer"));
    assert_eq!(receipts(&flow, &parent, "SubagentStop").len(), 1);
}

#[tokio::test]
async fn stop_fires_for_background_child_before_job_end() {
    let flow = Flow::new(vec![text("background answer")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    hooks(
        &flow,
        json!({"SubagentStop":[{"hooks":[capture("stop.jsonl")]}]}),
    );
    let parent = flow.session("default").await;
    let ack = background(
        &flow,
        &parent,
        json!({"prompt":"inspect","background":true,"name":"inspection","description":"Inspect in the background"}),
    )
    .await;
    let job = terminal(&flow, ack["job_id"].as_str().unwrap()).await;
    assert_eq!(job.status, JobStatus::Completed);
    // The handler settled inside the job's work, so its record exists at job end.
    let stop = lines(&flow, "stop.jsonl");
    assert_eq!(stop.len(), 1);
    assert_eq!(stop[0]["status"], "completed");
    assert_eq!(stop[0]["child_session_id"], ack["id"]);
    assert_eq!(stop[0]["subagent_name"], "inspection");
    let receipt = receipts(&flow, &parent, "SubagentStop");
    assert_eq!(receipt.len(), 1);
    assert!(receipt[0].started_ms <= job.ended_ms.unwrap());
}

#[tokio::test]
async fn stop_reports_error_for_a_failing_child() {
    let flow = Flow::new(
        vec![
            spawn("Inspect the parser"),
            vec![ScriptStep::Error {
                error: ErrorKind::InvalidRequest,
                message: "child provider refused".into(),
            }],
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    hooks(
        &flow,
        json!({"SubagentStop":[{"hooks":[capture("stop.jsonl")]}]}),
    );
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let stop = lines(&flow, "stop.jsonl");
    assert_eq!(stop.len(), 1);
    assert_eq!(stop[0]["status"], "error");
    assert!(stop[0]["error"].as_str().unwrap().contains("Subagent"));
}

#[tokio::test]
async fn interrupted_child_emits_no_stop() {
    let flow = Flow::new(
        vec![
            spawn("Inspect the parser"),
            call("read", "read", json!({"path":".env"})),
            text("never"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    hooks(
        &flow,
        json!({"SubagentStop":[{"hooks":[capture("stop.jsonl")]}]}),
    );
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.pending(&parent).await;
    flow.runtime.interrupt(&parent).await.unwrap();
    flow.settle(&parent).await;
    assert!(lines(&flow, "stop.jsonl").is_empty());
    assert!(receipts(&flow, &parent, "SubagentStop").is_empty());
}

#[tokio::test]
async fn stop_decisions_are_ignored_and_only_fail_closed_unavailability_fails_handback() {
    for (handler, expected_ok) in [
        (command(r#"{"decision":"deny","reason":"too late"}"#), true),
        (
            json!({"type":"command","command":"cat >/dev/null; printf '{bad'","id":"guard","fail_closed":true}),
            false,
        ),
        (
            json!({"type":"command","command":"cat >/dev/null; printf '{bad'","id":"guard"}),
            true,
        ),
    ] {
        let flow = Flow::new(
            vec![
                spawn("Inspect the parser"),
                text("child answer"),
                text("parent done"),
            ],
            false,
        );
        flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
        hooks(&flow, json!({"SubagentStop":[{"hooks":[handler.clone()]}]}));
        let mut live = flow.runtime.subscribe();
        let parent = flow.session("default").await;
        flow.prompt(&parent, "delegate").await;
        flow.settle(&parent).await;
        let output = flow.output(&parent, "spawn").await;
        if expected_ok {
            let result: Value = serde_json::from_str(&output).expect(&output);
            assert_eq!(result["text"], "child answer", "{handler}");
        } else {
            assert!(output.contains("hook guard unavailable"), "{output}");
        }
        let mut noticed = false;
        while let Ok(event) = live.try_recv() {
            noticed |= matches!(event, LiveEvent::HookNotice { .. });
        }
        assert!(noticed, "{handler}");
        assert_eq!(receipts(&flow, &parent, "SubagentStop").len(), 1);
    }
}
