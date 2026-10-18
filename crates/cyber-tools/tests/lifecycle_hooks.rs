//! Runtime-owned lifecycle hooks: Stop continuation, StopFailure observation,
//! UserPromptSubmit/SessionStart context admission and JobEnded observation.
#![cfg(unix)]
mod support;
use cyber_core::hooks::HookOutcome;
use cyber_llm::Content;
use cyber_llm::ErrorKind;
use cyber_llm::adapters::ScriptStep;
use cyber_server::runtime::{
    Admission, CreateSession, Delivery, HookExecutionRecord, JobStatus, LiveEvent, PermissionReply,
    ToolHost,
};
use serde_json::{Value, json};
use support::flow::{Flow, call, text};
use tokio::sync::broadcast::Receiver;

const LIMIT: &str = "stop hook continuation limit reached";

fn hooks(f: &Flow, hooks: Value) {
    f.f.global_hooks(hooks);
}

fn capture(file: &str) -> Value {
    json!({"type":"command","command":format!("{{ cat; echo; }} >> {file}"),"id":"capture"})
}

fn command(output: &str) -> Value {
    json!({"type":"command","command":format!("cat >/dev/null; printf '%s' '{}'",output.replace('\'',"'\\''")),"id":"guard"})
}

/// Blocks the first Stop it sees, then allows every later one.
fn block_once() -> Value {
    json!({"type":"command","id":"guard","command":
        "cat >/dev/null; if [ -f stopped ]; then exit 0; fi; touch stopped; printf '%s' '{\"decision\":\"block\",\"reason\":\"tests failing\"}'"})
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

fn drain_events(live: &mut Receiver<LiveEvent>) -> Vec<LiveEvent> {
    let mut events = Vec::new();
    while let Ok(event) = live.try_recv() {
        events.push(event);
    }
    events
}

fn notices(events: &[LiveEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            LiveEvent::HookNotice { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn errors(events: &[LiveEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            LiveEvent::Error { kind, message, .. } => Some((kind.clone(), message.clone())),
            _ => None,
        })
        .collect()
}

fn active(stops: &[Value]) -> Vec<bool> {
    stops
        .iter()
        .map(|stop| {
            stop["stop_hook_active"]
                .as_bool()
                .expect("stop_hook_active")
        })
        .collect()
}

#[tokio::test]
async fn blocking_stop_admits_the_reason_and_continues_once() {
    let flow = Flow::new(vec![text("first answer"), text("second answer")], false);
    hooks(
        &flow,
        json!({"Stop":[{"hooks":[block_once()]},{"hooks":[capture("stop.jsonl")]}]}),
    );
    let session = flow.session("default").await;
    flow.prompt(&session, "work").await;
    flow.settle(&session).await;
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 2);
    let second = format!("{:?}", requests[1].messages);
    assert!(second.contains("guard: tests failing"), "{second}");
    assert!(
        !format!("{:?}", requests[0].messages).contains("tests failing"),
        "the reason is admitted only after the first Stop"
    );
    let stops = lines(&flow, "stop.jsonl");
    assert_eq!(active(&stops), [false, true]);
    assert_eq!(stops[0]["event"], "Stop");
    assert_eq!(stops[0]["session_id"], session);
    assert!(!flow.runtime.is_running(&session));
    let state = flow.runtime.state(&session).await.unwrap();
    let sources: Vec<&str> = state.inbox.iter().map(|row| row.source.as_str()).collect();
    assert_eq!(sources, ["user", "hook"]);
    let receipts = receipts(&flow, &session, "Stop");
    assert_eq!(receipts.len(), 4);
    assert!(
        receipts
            .iter()
            .any(|receipt| receipt.outcome == Some(HookOutcome::Blocked))
    );
}

#[tokio::test]
async fn continuation_cap_stops_with_a_warning_and_a_new_prompt_resets_it() {
    let flow = Flow::new(
        (0..6).map(|turn| text(&format!("answer {turn}"))).collect(),
        false,
    );
    hooks(&flow, always_block(2));
    let mut live = flow.runtime.subscribe();
    let session = flow.session("default").await;
    flow.prompt(&session, "work").await;
    flow.settle(&session).await;
    assert_eq!(flow.main.requests().len(), 3);
    assert_eq!(active(&lines(&flow, "stop.jsonl")), [false, true, true]);
    let events = drain_events(&mut live);
    assert_eq!(
        notices(&events)
            .iter()
            .filter(|message| message.as_str() == LIMIT)
            .count(),
        1,
        "{:?}",
        notices(&events)
    );
    assert!(errors(&events).is_empty(), "{:?}", errors(&events));

    flow.prompt(&session, "again").await;
    flow.settle(&session).await;
    assert_eq!(flow.main.requests().len(), 6);
    assert_eq!(
        active(&lines(&flow, "stop.jsonl")),
        [false, true, true, false, true, true]
    );
}

fn always_block(max: u32) -> Value {
    json!({
        "max_stop_continuations": max,
        "Stop":[
            {"hooks":[command(r#"{"decision":"block","reason":"keep going"}"#)]},
            {"hooks":[capture("stop.jsonl")]}
        ]
    })
}

#[tokio::test]
async fn stop_continuations_never_exceed_the_step_limit() {
    let flow = Flow::new(
        (0..8).map(|turn| text(&format!("answer {turn}"))).collect(),
        false,
    );
    hooks(&flow, always_block(5));
    let mut live = flow.runtime.subscribe();
    let session = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("default".into()),
            max_steps: Some(2),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    flow.prompt(&session, "work").await;
    flow.settle(&session).await;
    // Two budgeted steps, then the tool-less step-limit Turn ends the Drain without Stop.
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 3);
    assert!(!requests[1].tools_disabled);
    assert!(requests[2].tools_disabled);
    assert_eq!(active(&lines(&flow, "stop.jsonl")), [false, true]);
    let events = drain_events(&mut live);
    assert!(!notices(&events).contains(&LIMIT.to_string()));
    assert!(errors(&events).is_empty(), "{:?}", errors(&events));
}

#[tokio::test]
async fn a_public_admission_claiming_hook_resets_the_stop_count() {
    let mut script = vec![
        text("answer 0"),
        call("read", "read", json!({"path":".env"})),
    ];
    script.extend((1..6).map(|turn| text(&format!("answer {turn}"))));
    let flow = Flow::new(script, true);
    flow.f.write(".env", "private");
    hooks(&flow, always_block(2));
    let mut live = flow.runtime.subscribe();
    let session = flow.session("default").await;
    flow.prompt(&session, "work").await;
    // The first continuation Turn waits on a permission; admit claimed-hook input then.
    let pending = flow.pending(&session).await;
    let mut admission = Admission::text("public input", Delivery::Queue);
    admission.source = "hook".into();
    flow.runtime.admit_user(&session, admission).await.unwrap();
    flow.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    flow.settle(&session).await;
    assert_eq!(
        active(&lines(&flow, "stop.jsonl")),
        [false, false, true, true]
    );
    // The tool result Turn, then the admitted input's Turn, then two continuations.
    assert_eq!(flow.main.requests().len(), 6);
    let limits = notices(&drain_events(&mut live))
        .into_iter()
        .filter(|message| message == LIMIT)
        .count();
    assert_eq!(limits, 1);
}

#[tokio::test]
async fn interrupted_drains_emit_no_stop() {
    let flow = Flow::new(
        vec![call("read", "read", json!({"path":".env"})), text("never")],
        true,
    );
    flow.f.write(".env", "private");
    hooks(
        &flow,
        json!({"Stop":[{"hooks":[capture("stop.jsonl")]}],"StopFailure":[{"hooks":[capture("failure.jsonl")]}]}),
    );
    let session = flow.session("default").await;
    flow.prompt(&session, "read the secrets").await;
    flow.pending(&session).await;
    flow.runtime.interrupt(&session).await.unwrap();
    flow.settle(&session).await;
    assert!(lines(&flow, "stop.jsonl").is_empty());
    assert!(lines(&flow, "failure.jsonl").is_empty());
    assert!(receipts(&flow, &session, "Stop").is_empty());
}

#[tokio::test]
async fn child_drains_run_stop_but_structured_children_do_not() {
    let schema =
        json!({"type":"object","required":["findings"],"properties":{"findings":{"type":"array"}}});
    let flow = Flow::new(
        vec![
            call(
                "plain",
                "agent",
                json!({"prompt":"inspect","agent":"general"}),
            ),
            text("child answer"),
            call(
                "typed",
                "agent",
                json!({"prompt":"inspect","agent":"general","output_schema":schema}),
            ),
            text("text before the result"),
            call("result", "return_result", json!({"findings":[]})),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    hooks(
        &flow,
        json!({"Stop":[{"hooks":[capture("stop.jsonl"),command(r#"{"decision":"block","reason":"never for typed children"}"#)]}],
               "max_stop_continuations": 0}),
    );
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let plain: Value = serde_json::from_str(&flow.output(&parent, "plain").await).unwrap();
    let typed: Value = serde_json::from_str(&flow.output(&parent, "typed").await).unwrap();
    assert_eq!(typed["result"], json!({"findings":[]}));
    let sessions: Vec<Value> = lines(&flow, "stop.jsonl")
        .into_iter()
        .map(|stop| stop["session_id"].clone())
        .collect();
    assert_eq!(sessions, [plain["id"].clone(), json!(parent)]);
    assert!(!sessions.contains(&typed["id"]));
    assert_eq!(flow.main.requests().len(), 6);
}

#[tokio::test]
async fn provider_failure_runs_stop_failure_without_stop_or_changing_the_error() {
    let flow = Flow::new(
        vec![vec![ScriptStep::Error {
            error: ErrorKind::InvalidRequest,
            message: "provider refused".into(),
        }]],
        false,
    );
    hooks(
        &flow,
        json!({"Stop":[{"hooks":[capture("stop.jsonl")]}],
               "StopFailure":[{"matcher":"invalid_request","hooks":[capture("failure.jsonl")]}]}),
    );
    let mut live = flow.runtime.subscribe();
    let session = flow.session("default").await;
    flow.prompt(&session, "work").await;
    flow.settle(&session).await;
    let published = errors(&drain_events(&mut live));
    assert_eq!(published.len(), 1, "{published:?}");
    assert_eq!(published[0].0, "invalid_request");
    assert!(published[0].1.contains("provider refused"), "{published:?}");
    assert!(lines(&flow, "stop.jsonl").is_empty());
    let failures = lines(&flow, "failure.jsonl");
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0]["event"], "StopFailure");
    assert_eq!(failures[0]["session_id"], session);
    assert_eq!(failures[0]["error_kind"], "invalid_request");
    assert_eq!(failures[0]["error"], published[0].1);
    assert_eq!(receipts(&flow, &session, "StopFailure").len(), 1);
}

#[tokio::test]
async fn unavailable_fail_closed_stop_surfaces_a_drain_error() {
    for fail_closed in [true, false] {
        let flow = Flow::new(vec![text("answer"), text("never")], false);
        hooks(
            &flow,
            json!({"Stop":[{"hooks":[{"type":"command","id":"guard","fail_closed":fail_closed,
                "command":"cat >/dev/null; printf '{bad'"}]}]}),
        );
        let mut live = flow.runtime.subscribe();
        let session = flow.session("default").await;
        flow.prompt(&session, "work").await;
        flow.settle(&session).await;
        let events = drain_events(&mut live);
        let errors = errors(&events);
        assert_eq!(flow.main.requests().len(), 1, "fail_closed={fail_closed}");
        assert!(!notices(&events).is_empty(), "fail_closed={fail_closed}");
        if fail_closed {
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert_eq!(errors[0].0, "hook");
            assert!(errors[0].1.contains("hook guard unavailable"), "{errors:?}");
        } else {
            assert!(errors.is_empty(), "{errors:?}");
        }
        assert_eq!(receipts(&flow, &session, "Stop").len(), 1);
    }
}

#[tokio::test]
async fn block_on_a_non_stop_lifecycle_event_is_ignored() {
    let flow = Flow::new(
        vec![
            vec![ScriptStep::Error {
                error: ErrorKind::InvalidRequest,
                message: "provider refused".into(),
            }],
            text("never"),
        ],
        false,
    );
    hooks(
        &flow,
        json!({"StopFailure":[{"hooks":[command(r#"{"decision":"block","reason":"retry it"}"#)]},
                              {"hooks":[command(r#"{"decision":"deny","reason":"no"}"#)]}]}),
    );
    let mut live = flow.runtime.subscribe();
    let session = flow.session("default").await;
    flow.prompt(&session, "work").await;
    flow.settle(&session).await;
    assert_eq!(flow.main.requests().len(), 1);
    let published = errors(&drain_events(&mut live));
    assert_eq!(published.len(), 1, "{published:?}");
    assert!(published[0].1.contains("provider refused"));
    let state = flow.runtime.state(&session).await.unwrap();
    assert!(
        !format!("{:?}", state.inbox).contains("retry it"),
        "a StopFailure block admits nothing"
    );
    assert_eq!(receipts(&flow, &session, "StopFailure").len(), 2);
}

fn context(id: &str, text: &str) -> Value {
    json!({"type":"command","id":id,"command":format!(
        "cat >/dev/null; printf '%s' '{}'",
        json!({"additional_context":text}).to_string().replace('\'',"'\\''")
    )})
}

/// Denies any prompt that carries a fake API key.
fn key_guard() -> Value {
    json!({"type":"command","id":"guard","command":
        "if grep -q 'sk-live-'; then printf '%s' '{\"decision\":\"deny\",\"reason\":\"prompt contains an API key\"}'; fi"})
}

fn contents(request: &cyber_llm::LlmRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .map(|message| format!("{:?}", message.content))
        .collect()
}

#[tokio::test]
async fn user_prompt_submit_deny_rejects_the_prompt_before_admission() {
    let flow = Flow::new(vec![text("never")], false);
    hooks(
        &flow,
        json!({"UserPromptSubmit":[{"hooks":[capture("prompt.jsonl"),key_guard()]}]}),
    );
    let session = flow.session("default").await;
    let error = flow
        .runtime
        .admit_user(
            &session,
            Admission::text("deploy with sk-live-0123456789abcdef", Delivery::Queue),
        )
        .await
        .expect_err("a denied prompt is not admitted");
    assert!(
        error
            .to_string()
            .contains("blocked by hook guard: prompt contains an API key"),
        "{error}"
    );
    let state = flow.runtime.state(&session).await.unwrap();
    assert!(state.inbox.is_empty(), "{:?}", state.inbox);
    assert!(state.entries.is_empty(), "{:?}", state.entries);
    assert!(!flow.runtime.is_running(&session));
    assert!(flow.main.requests().is_empty());
    let receipts = receipts(&flow, &session, "UserPromptSubmit");
    assert_eq!(receipts.len(), 2);
    assert!(
        receipts
            .iter()
            .any(|receipt| receipt.outcome == Some(HookOutcome::Blocked))
    );
    let payload = lines(&flow, "prompt.jsonl");
    assert_eq!(payload.len(), 1);
    assert_eq!(payload[0]["event"], "UserPromptSubmit");
    assert_eq!(payload[0]["session_id"], session);
    assert_eq!(payload[0]["prompt"], "deploy with sk-live-0123456789abcdef");
    assert_eq!(payload[0]["source"], "user");
}

#[tokio::test]
async fn user_prompt_submit_deny_also_guards_queued_prompt_edits() {
    let flow = Flow::new(Vec::new(), false);
    hooks(&flow, json!({"UserPromptSubmit":[{"hooks":[key_guard()]}]}));
    let session = flow.session("default").await;
    let held = flow
        .runtime
        .admit_user(&session, Admission::text("deploy", Delivery::Hold))
        .await
        .unwrap()
        .message_id;
    let error = flow
        .runtime
        .edit_input(
            &session,
            &held,
            Some(vec![Content::Text {
                text: "deploy with sk-live-0123456789abcdef".into(),
            }]),
            None,
        )
        .await
        .expect_err("a denied edit is not applied");
    assert!(
        error.to_string().contains("blocked by hook guard"),
        "{error}"
    );
    let state = flow.runtime.state(&session).await.unwrap();
    assert_eq!(
        format!("{:?}", state.inbox[0].parts),
        format!(
            "{:?}",
            vec![Content::Text {
                text: "deploy".into()
            }]
        )
    );
}

#[tokio::test]
async fn user_prompt_context_is_admitted_immediately_before_the_prompt() {
    let flow = Flow::new(vec![text("answer")], false);
    hooks(
        &flow,
        json!({"UserPromptSubmit":[{"hooks":[context("ticket","Ticket ABC-1: the parser drops comments")]}]}),
    );
    let mut live = flow.runtime.subscribe();
    let session = flow.session("default").await;
    flow.prompt(&session, "fix the ticket").await;
    flow.settle(&session).await;
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 1);
    let messages = contents(&requests[0]);
    let at = messages
        .iter()
        .position(|message| message.contains("Ticket ABC-1"))
        .expect("context reaches the model");
    assert!(messages[at].contains("ticket"), "tagged: {}", messages[at]);
    assert!(messages[at + 1].contains("fix the ticket"), "{messages:?}");
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.contains("Ticket ABC-1"))
            .count(),
        1
    );
    assert!(
        !notices(&drain_events(&mut live))
            .iter()
            .any(|notice| notice.contains("admission remains unavailable"))
    );
}

#[tokio::test]
async fn session_start_context_is_admitted_once_at_the_first_turn() {
    let mut flow = Flow::new(vec![text("one"), text("two"), text("three")], false);
    let config = json!({"SessionStart":[{"hooks":[capture("start.jsonl"),context("conventions","Repo convention: tabs")]}]});
    hooks(&flow, config.clone());
    let session = flow.session("default").await;
    flow.prompt(&session, "first").await;
    flow.settle(&session).await;
    flow.prompt(&session, "second").await;
    flow.settle(&session).await;
    flow.restart_default_runtime().await;
    hooks(&flow, config);
    flow.prompt(&session, "third").await;
    flow.settle(&session).await;
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        let messages = contents(request);
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.contains("Repo convention: tabs"))
                .count(),
            1,
            "{messages:?}"
        );
    }
    let first = contents(&requests[0]);
    let at = first
        .iter()
        .position(|message| message.contains("Repo convention: tabs"))
        .unwrap();
    assert!(first[at].contains("conventions"), "tagged: {}", first[at]);
    assert!(first[at + 1].contains("first"), "{first:?}");
    let starts = lines(&flow, "start.jsonl");
    assert_eq!(starts.len(), 1, "{starts:?}");
    assert_eq!(starts[0]["event"], "SessionStart");
    assert_eq!(starts[0]["session_id"], session);
    assert_eq!(starts[0]["source"], "startup");
}

#[tokio::test]
async fn child_and_hook_admissions_do_not_run_user_prompt_submit() {
    let flow = Flow::new(
        vec![
            call(
                "plain",
                "agent",
                json!({"prompt":"inspect","agent":"general"}),
            ),
            text("child answer"),
            text("child again"),
            text("parent done"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    hooks(
        &flow,
        json!({"UserPromptSubmit":[{"hooks":[capture("prompt.jsonl")]}],
               "SessionStart":[{"hooks":[capture("start.jsonl")]}],
               "Stop":[{"hooks":[block_once()]}]}),
    );
    let parent = flow.session("default").await;
    flow.prompt(&parent, "delegate").await;
    flow.settle(&parent).await;
    let child: Value = serde_json::from_str(&flow.output(&parent, "plain").await).unwrap();
    let child = child["id"].as_str().unwrap().to_owned();
    let state = flow.runtime.state(&child).await.unwrap();
    let sources: Vec<&str> = state.inbox.iter().map(|row| row.source.as_str()).collect();
    assert_eq!(sources, ["session", "hook"], "the child continued once");
    let prompts = lines(&flow, "prompt.jsonl");
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert_eq!(prompts[0]["session_id"], parent);
    assert_eq!(prompts[0]["prompt"], "delegate");
    let starts: Vec<Value> = lines(&flow, "start.jsonl")
        .into_iter()
        .map(|start| start["session_id"].clone())
        .collect();
    assert_eq!(starts, [json!(parent)]);
}

#[tokio::test]
async fn job_ended_observes_a_settled_background_job() {
    let flow = Flow::new(vec![text("background answer"), text("parent ack")], false);
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    hooks(
        &flow,
        json!({"JobEnded":[{"hooks":[capture("job.jsonl"),
            {"type":"command","id":"broken","fail_closed":true,"command":"cat >/dev/null; printf '{bad'"}]}]}),
    );
    let parent = flow.session("default").await;
    let mut inv = flow.f.invocation(
        "default",
        "agent",
        json!({"prompt":"inspect","background":true,"name":"inspection"}),
    );
    inv.session_id = parent.clone();
    let ack: Value = serde_json::from_str(&support::ok(
        flow.f
            .host
            .execute(inv, tokio_util::sync::CancellationToken::new())
            .await,
    ))
    .unwrap();
    let id = ack["job_id"].as_str().unwrap().to_owned();
    let ended = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let done = receipts(&flow, &parent, "JobEnded");
            if done.len() == 2 && done.iter().all(|receipt| receipt.outcome.is_some()) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(ended.is_ok(), "JobEnded did not run");
    let job = flow.runtime.job(&id).unwrap();
    assert_eq!(job.status, JobStatus::Completed);
    assert!(job.notified);
    let events = lines(&flow, "job.jsonl");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["event"], "JobEnded");
    assert_eq!(events[0]["session_id"], parent);
    assert_eq!(events[0]["job"]["id"], json!(id));
    assert_eq!(events[0]["job"]["status"], "completed");
    assert_eq!(events[0]["job"]["child_id"], ack["id"]);
    assert!(events[0].as_object().unwrap().contains_key("exit_code"));
    assert!(events[0]["exit_code"].is_null());
    for receipt in receipts(&flow, &parent, "JobEnded") {
        assert!(receipt.started_ms >= job.ended_ms.unwrap());
    }
    flow.settle(&parent).await;
    let state = flow.runtime.state(&parent).await.unwrap();
    assert!(
        state
            .inbox
            .iter()
            .any(|row| row.message_id == format!("msg_handback_{id}")),
        "{:?}",
        state.inbox
    );
}

#[tokio::test]
async fn unavailable_fail_closed_user_prompt_handler_rejects_the_prompt() {
    for fail_closed in [true, false] {
        let flow = Flow::new(vec![text("answer")], false);
        hooks(
            &flow,
            json!({"UserPromptSubmit":[{"hooks":[{"type":"command","id":"guard","fail_closed":fail_closed,
                "command":"cat >/dev/null; printf '{bad'"}]}]}),
        );
        let session = flow.session("default").await;
        let admitted = flow
            .runtime
            .admit_user(&session, Admission::text("work", Delivery::Queue))
            .await;
        flow.settle(&session).await;
        let state = flow.runtime.state(&session).await.unwrap();
        if fail_closed {
            assert!(admitted.is_err(), "fail_closed={fail_closed}");
            assert!(state.inbox.is_empty());
            assert!(flow.main.requests().is_empty());
        } else {
            assert!(admitted.is_ok(), "{admitted:?}");
            assert_eq!(flow.main.requests().len(), 1);
        }
    }
}

fn subtask(prompt: &str) -> cyber_server::runtime::UserSubtask {
    cyber_server::runtime::UserSubtask {
        skill_command: None,
        admission_id: None,
        prompt: prompt.into(),
        agent: Some("general".into()),
        attachments: Vec::new(),
        max_steps: None,
        hook_context: None,
    }
}

async fn terminal(flow: &Flow, job: &str) -> cyber_server::runtime::Job {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let job = flow.runtime.job(job).unwrap();
            if job.status != JobStatus::Running {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Job did not settle")
}

async fn turn(flow: &Flow, session: &str) -> cyber_server::runtime::TurnContext {
    let info = flow.runtime.state(session).await.unwrap().info;
    cyber_server::runtime::TurnContext {
        session_id: session.into(),
        directory: info.directory,
        agent: info.agent,
        mode: info.mode,
        prefers_apply_patch: false,
        rules: info.rules,
        worktree: Default::default(),
    }
}

/// Admit the `audit` forked skill as a public user command.
async fn fork_command(flow: &Flow, session: &str, message: &str) {
    flow.f.write(
        ".cyber/skills/audit/SKILL.md",
        "---\nname: audit\ndescription: Audit changes\ncontext: fork\n---\nPRIVATE AUDIT BODY\n",
    );
    let plan = flow
        .f
        .host
        .command_plan(&turn(flow, session).await, "audit", "")
        .await
        .unwrap()
        .unwrap();
    flow.runtime
        .admit_user(
            session,
            plan.admission(Some(message.into()), Delivery::Queue),
        )
        .await
        .unwrap();
}

async fn only_job(flow: &Flow, session: &str) -> cyber_server::runtime::Job {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(job) = flow.runtime.jobs(Some(session)).unwrap().into_iter().next() {
                return job;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("no Job started")
}

/// Index of the message carrying `needle`, asserting it appears exactly once.
fn once(messages: &[String], needle: &str) -> usize {
    let hits: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter_map(|(at, message)| message.contains(needle).then_some(at))
        .collect();
    assert_eq!(hits.len(), 1, "{needle} in {messages:?}");
    hits[0]
}

#[tokio::test]
async fn public_delegations_run_user_prompt_submit_before_any_record() {
    let flow = Flow::new(vec![text("never")], false);
    hooks(
        &flow,
        json!({"UserPromptSubmit":[{"hooks":[capture("prompt.jsonl"),key_guard()]}]}),
    );
    let parent = flow.session("default").await;
    let secret = "look at sk-live-0123456789abcdef";
    let delegated = flow
        .runtime
        .start_delegation(&parent, "op_secret", subtask(secret))
        .await
        .expect_err("a denied delegation is refused");
    assert!(
        delegated
            .to_string()
            .contains("blocked by hook guard: prompt contains an API key"),
        "{delegated}"
    );
    assert!(
        flow.runtime
            .delegation(&parent, "op_secret")
            .unwrap()
            .is_none()
    );
    let subtask_error = flow
        .runtime
        .subtask_request(&parent, subtask(secret))
        .await
        .expect_err("a denied subtask is refused");
    assert!(
        subtask_error.to_string().contains("blocked by hook guard"),
        "{subtask_error}"
    );
    assert!(flow.runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert!(flow.main.requests().is_empty());
    let prompts = lines(&flow, "prompt.jsonl");
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    for prompt in &prompts {
        assert_eq!(prompt["session_id"], parent);
        assert_eq!(prompt["prompt"], secret);
        assert_eq!(prompt["source"], "user");
    }
}

/// A delegated or forked prompt runs no Turn of its own to bound, so a
/// UserPromptSubmit `continue: false` refuses it instead of being dropped.
#[tokio::test]
async fn user_prompt_submit_continue_false_refuses_prompts_without_a_turn() {
    let flow = Flow::new(vec![text("never")], false);
    hooks(
        &flow,
        json!({"UserPromptSubmit":[{"hooks":[command(r#"{"continue":false,"stop_reason":"frozen"}"#)]}]}),
    );
    let parent = flow.session("default").await;
    let refused = flow
        .runtime
        .start_delegation(&parent, "op_frozen", subtask("inspect"))
        .await
        .expect_err("the delegation is refused");
    assert!(
        refused.to_string().contains("[hook guard] frozen"),
        "{refused}"
    );
    assert!(
        flow.runtime
            .delegation(&parent, "op_frozen")
            .unwrap()
            .is_none()
    );
    flow.f.write(
        ".cyber/skills/audit/SKILL.md",
        "---\nname: audit\ndescription: Audit changes\ncontext: fork\n---\nPRIVATE AUDIT BODY\n",
    );
    let plan = flow
        .f
        .host
        .command_plan(&turn(&flow, &parent).await, "audit", "")
        .await
        .unwrap()
        .unwrap();
    let forked = flow
        .runtime
        .admit_user(&parent, plan.admission(None, Delivery::Queue))
        .await
        .expect_err("the forked skill is refused");
    assert!(
        forked.to_string().contains("[hook guard] frozen"),
        "{forked}"
    );
    assert!(flow.runtime.jobs(Some(&parent)).unwrap().is_empty());
    assert!(flow.main.requests().is_empty());
}

#[tokio::test]
async fn delegation_context_reaches_the_child_before_its_prompt_once() {
    let flow = Flow::new(vec![text("child answer"), text("parent ack")], false);
    hooks(
        &flow,
        json!({"UserPromptSubmit":[{"hooks":[capture("prompt.jsonl"),context("ticket","Ticket ABC-1: the parser drops comments")]}]}),
    );
    let parent = flow.session("default").await;
    flow.runtime
        .start_delegation(&parent, "op_ticket", subtask("inspect the parser"))
        .await
        .unwrap();
    let job = only_job(&flow, &parent).await;
    assert_eq!(terminal(&flow, &job.id).await.status, JobStatus::Completed);
    // An exact retry returns the recorded delegation without another hook run.
    flow.runtime
        .start_delegation(&parent, "op_ticket", subtask("inspect the parser"))
        .await
        .unwrap();
    flow.settle(&parent).await;
    assert_eq!(lines(&flow, "prompt.jsonl").len(), 1);
    let child = flow
        .main
        .requests()
        .into_iter()
        .map(|request| contents(&request))
        .find(|messages| messages.iter().any(|m| m.contains("inspect the parser")))
        .expect("child request");
    let at = once(&child, "Ticket ABC-1");
    assert!(child[at].contains("[hook ticket]"), "{}", child[at]);
    assert!(child[at + 1].contains("inspect the parser"), "{child:?}");
    let parent_state = format!("{:?}", flow.runtime.state(&parent).await.unwrap().entries);
    assert!(!parent_state.contains("Ticket ABC-1"), "{parent_state}");
}

#[tokio::test]
async fn forked_skill_command_commits_its_prompt_context_with_the_forwarding() {
    let mut flow = Flow::new(vec![text("audit summary"), text("parent ack")], false);
    let config = json!({"UserPromptSubmit":[{"hooks":[context("ticket","Ticket ABC-1: audit the parser")]}]});
    hooks(&flow, config.clone());
    let parent = flow.session("default").await;
    fork_command(&flow, &parent, "msg_fork").await;
    let job = only_job(&flow, &parent).await;
    assert_eq!(terminal(&flow, &job.id).await.status, JobStatus::Completed);
    flow.settle(&parent).await;
    let requests: Vec<Vec<String>> = flow.main.requests().iter().map(contents).collect();
    assert_eq!(requests.len(), 2);
    let at = once(&requests[0], "Ticket ABC-1");
    assert!(requests[0][at].contains("[hook ticket]"));
    assert!(
        requests[0][at + 1].contains("PRIVATE AUDIT BODY"),
        "{:?}",
        requests[0]
    );
    once(&requests[1], "Ticket ABC-1");
    let entries = format!("{:?}", flow.runtime.state(&parent).await.unwrap().entries);
    assert!(!entries.contains("PRIVATE AUDIT BODY"));
    // Replay neither loses nor re-admits the forwarded row's context.
    flow.restart_default_runtime().await;
    hooks(&flow, config);
    let replayed = format!("{:?}", flow.runtime.state(&parent).await.unwrap().entries);
    assert_eq!(replayed.matches("Ticket ABC-1").count(), 1, "{replayed}");
}

#[tokio::test]
async fn session_start_precedes_a_forked_first_input_and_survives_a_crash() {
    let mut flow = Flow::new(
        vec![
            call("secret", "read", json!({"path":".env"})),
            text("parent ack"),
        ],
        true,
    );
    flow.f.write(".env", "private");
    let config = json!({
        "SessionStart":[{"hooks":[capture("order.jsonl"),context("conventions","Repo convention: tabs")]}],
        "SubagentStart":[{"hooks":[capture("order.jsonl")]}],
        "JobEnded":[{"hooks":[capture("job.jsonl")]}]
    });
    hooks(&flow, config.clone());
    let parent = flow.session("default").await;
    fork_command(&flow, &parent, "msg_fork").await;
    flow.pending(&parent).await;
    let job = only_job(&flow, &parent).await;
    let order: Vec<Value> = lines(&flow, "order.jsonl")
        .into_iter()
        .map(|line| line["event"].clone())
        .collect();
    assert_eq!(order, ["SessionStart", "SubagentStart"]);
    // SessionStart committed, but the parent never opened a Turn before the crash.
    let _crashed = flow.crash_default_runtime();
    hooks(&flow, config);
    flow.runtime.recover_jobs().await.unwrap();
    assert_eq!(
        flow.runtime.job(&job.id).unwrap().status,
        JobStatus::Interrupted
    );
    flow.settle(&parent).await;
    let starts = lines(&flow, "order.jsonl")
        .into_iter()
        .filter(|line| line["event"] == "SessionStart")
        .count();
    assert_eq!(starts, 1, "SessionStart ran again after the crash");
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 2);
    once(&contents(&requests[1]), "Repo convention: tabs");
    let ended = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let ended = lines(&flow, "job.jsonl");
            if !ended.is_empty() {
                return ended;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("JobEnded did not run for the recovered Job");
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0]["job"]["id"], json!(job.id));
    assert_eq!(ended[0]["job"]["status"], "interrupted");
}

#[tokio::test]
async fn forked_sessions_start_with_resume_and_fail_closed_start_blocks_the_turn() {
    let flow = Flow::new(vec![text("one"), text("two")], false);
    hooks(
        &flow,
        json!({"SessionStart":[{"hooks":[capture("start.jsonl")]}]}),
    );
    let session = flow.session("default").await;
    flow.prompt(&session, "first").await;
    flow.settle(&session).await;
    let fork = flow.runtime.fork(&session, None).await.unwrap().id;
    flow.prompt(&fork, "second").await;
    flow.settle(&fork).await;
    let starts = lines(&flow, "start.jsonl");
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_eq!(starts[0]["source"], "startup");
    assert_eq!(starts[1]["session_id"], fork);
    assert_eq!(starts[1]["source"], "resume");

    let blocked = Flow::new(vec![text("never")], false);
    hooks(
        &blocked,
        json!({"SessionStart":[{"hooks":[{"type":"command","id":"guard","fail_closed":true,
            "command":"cat >/dev/null; printf '{bad'"}]}]}),
    );
    let mut live = blocked.runtime.subscribe();
    let session = blocked.session("default").await;
    blocked.prompt(&session, "work").await;
    blocked.settle(&session).await;
    let published = errors(&drain_events(&mut live));
    assert_eq!(published.len(), 1, "{published:?}");
    assert_eq!(published[0].0, "hook");
    assert!(blocked.main.requests().is_empty());
    let state = blocked.runtime.state(&session).await.unwrap();
    assert!(state.entries.is_empty(), "{:?}", state.entries);
}

#[tokio::test]
async fn job_ended_observes_a_cancelled_background_job() {
    let flow = Flow::new(vec![call("secret", "read", json!({"path":".env"}))], true);
    flow.f.write(".env", "private");
    hooks(
        &flow,
        json!({"JobEnded":[{"hooks":[capture("job.jsonl")]}]}),
    );
    let parent = flow.session("default").await;
    let job = flow.runtime.subtask(&parent, "inspect").await.unwrap();
    flow.pending(&parent).await;
    flow.runtime.cancel_job(&job.id).await.unwrap();
    assert_eq!(terminal(&flow, &job.id).await.status, JobStatus::Cancelled);
    let ended = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let ended = lines(&flow, "job.jsonl");
            if !ended.is_empty() {
                return ended;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("JobEnded did not run for the cancelled Job");
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0]["job"]["status"], "cancelled");
}

#[tokio::test]
async fn user_prompt_submit_ignores_the_claimed_source_and_exact_retries() {
    let flow = Flow::new(vec![text("answer")], false);
    hooks(
        &flow,
        json!({"UserPromptSubmit":[{"hooks":[capture("prompt.jsonl"),key_guard()]}]}),
    );
    let session = flow.session("default").await;
    for source in ["hook", "session"] {
        let mut admission = Admission::text("use sk-live-0123456789abcdef", Delivery::Hold);
        admission.source = source.into();
        let error = flow
            .runtime
            .admit_user(&session, admission)
            .await
            .expect_err("a spoofed source is still checked");
        assert!(
            error.to_string().contains("blocked by hook guard"),
            "{error}"
        );
    }
    let mut admission = Admission::text("hold this", Delivery::Hold);
    admission.message_id = Some("msg_retry".into());
    let first = flow
        .runtime
        .admit_user(&session, admission.clone())
        .await
        .unwrap();
    let retry = flow.runtime.admit_user(&session, admission).await.unwrap();
    assert_eq!(first.admitted_seq, retry.admitted_seq);
    let sources: Vec<Value> = lines(&flow, "prompt.jsonl")
        .into_iter()
        .map(|line| line["source"].clone())
        .collect();
    assert_eq!(sources, ["hook", "session", "user"]);
    assert!(flow.runtime.state(&session).await.unwrap().inbox.len() == 1);
}

#[tokio::test]
async fn removed_rows_never_admit_their_context() {
    let flow = Flow::new(vec![text("answer")], false);
    hooks(
        &flow,
        json!({"UserPromptSubmit":[{"hooks":[{"type":"command","id":"ticket","command":
            "if grep -q withdrawn; then printf '%s' '{\"additional_context\":\"Withdrawn ticket context\"}'; fi"}]}]}),
    );
    let session = flow.session("default").await;
    let held = flow
        .runtime
        .admit_user(
            &session,
            Admission::text("withdrawn request", Delivery::Hold),
        )
        .await
        .unwrap()
        .message_id;
    flow.runtime.remove_input(&session, &held).await.unwrap();
    flow.prompt(&session, "real request").await;
    flow.settle(&session).await;
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        !contents(&requests[0])
            .iter()
            .any(|message| message.contains("Withdrawn ticket context")),
        "{:?}",
        contents(&requests[0])
    );
    let entries = format!("{:?}", flow.runtime.state(&session).await.unwrap().entries);
    assert!(!entries.contains("Withdrawn ticket context"));
}

#[tokio::test]
async fn continuing_an_idle_child_runs_user_prompt_submit_on_the_child() {
    let flow = Flow::new(
        vec![
            call(
                "plain",
                "agent",
                json!({"prompt":"inspect","agent":"general"}),
            ),
            text("child answer"),
            text("parent done"),
            text("child follow-up"),
        ],
        false,
    );
    flow.f.set_config(json!({"permissions":{"agent":"allow"}}));
    hooks(
        &flow,
        json!({"UserPromptSubmit":[{"hooks":[capture("prompt.jsonl"),context("ticket","Ticket ABC-1")]}]}),
    );
    let parent = flow.session("default").await;
    flow.runtime
        .admit_user(&parent, Admission::text("delegate", Delivery::Queue))
        .await
        .unwrap();
    flow.settle(&parent).await;
    let child: Value = serde_json::from_str(&flow.output(&parent, "plain").await).unwrap();
    let child = child["id"].as_str().unwrap().to_owned();
    assert!(!flow.runtime.is_running(&child));
    flow.runtime
        .admit_user(
            &child,
            Admission::text("follow up on the parser", Delivery::Queue),
        )
        .await
        .unwrap();
    flow.settle(&child).await;
    let prompts: Vec<Value> = lines(&flow, "prompt.jsonl")
        .into_iter()
        .map(|line| line["session_id"].clone())
        .collect();
    assert_eq!(prompts, [json!(parent), json!(child)]);
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 4);
    let last = contents(&requests[3]);
    let at = last
        .iter()
        .rposition(|message| message.contains("Ticket ABC-1"))
        .expect("context reaches the continued child");
    assert!(last[at + 1].contains("follow up on the parser"), "{last:?}");
}

fn system_texts(state: &cyber_server::runtime::SessionState) -> Vec<String> {
    state
        .entries
        .iter()
        .filter_map(|entry| match entry {
            cyber_server::runtime::Entry::System { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn stop_continue_false_admits_no_continuation_and_ends_with_the_stop_reason() {
    let flow = Flow::new(vec![text("first answer"), text("never")], false);
    let mut stop = command(
        r#"{"decision":"block","reason":"keep going","continue":false,"stop_reason":"policy stop"}"#,
    );
    stop["fail_closed"] = json!(true);
    hooks(&flow, json!({"Stop":[{"hooks":[stop]}]}));
    let mut live = flow.runtime.subscribe();
    let session = flow.session("default").await;
    flow.prompt(&session, "work").await;
    flow.settle(&session).await;
    assert_eq!(flow.main.requests().len(), 1);
    let events = drain_events(&mut live);
    assert!(errors(&events).is_empty(), "{:?}", errors(&events));
    assert!(notices(&events).contains(&"policy stop".to_string()));
    let state = flow.runtime.state(&session).await.unwrap();
    let sources: Vec<&str> = state.inbox.iter().map(|row| row.source.as_str()).collect();
    assert_eq!(sources, ["user"], "no Stop continuation was admitted");
    assert_eq!(system_texts(&state), ["[hook guard] policy stop"]);
    assert!(!flow.runtime.is_running(&session));
    assert_eq!(receipts(&flow, &session, "Stop").len(), 1);
}

#[tokio::test]
async fn session_start_continue_false_admits_context_then_stops_after_the_turn() {
    let flow = Flow::new(
        vec![
            call("call_read", "read", json!({"path":"a.txt"})),
            text("never"),
        ],
        false,
    );
    flow.f.write("a.txt", "safe");
    let mut start = command(r#"{"additional_context":"conventions","continue":false}"#);
    start["id"] = json!("start");
    hooks(&flow, json!({"SessionStart":[{"hooks":[start]}]}));
    let session = flow.session("default").await;
    flow.prompt(&session, "work").await;
    flow.settle(&session).await;
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 1, "the Drain stops after the first Turn");
    assert!(format!("{:?}", requests[0].messages).contains("[hook start] conventions"));
    let state = flow.runtime.state(&session).await.unwrap();
    assert_eq!(
        state.calls["call_read"].status,
        cyber_server::runtime::CallStatus::Ok
    );
    assert!(
        system_texts(&state)
            .iter()
            .any(|text| text == "[hook start] continue: false"),
        "{:?}",
        system_texts(&state)
    );
    // A later prompt is not stopped by the consumed SessionStart request.
    flow.prompt(&session, "again").await;
    flow.settle(&session).await;
    assert_eq!(flow.main.requests().len(), 2);
}

#[tokio::test]
async fn user_prompt_submit_continue_false_stops_after_the_turn() {
    let flow = Flow::new(
        vec![
            call("call_read", "read", json!({"path":"a.txt"})),
            text("never"),
        ],
        false,
    );
    flow.f.write("a.txt", "safe");
    // Only the prompt that names the ticket asks to stop.
    let guard = json!({"type":"command","id":"guard","fail_closed":true,"command":
        "if grep -q TICKET-7; then printf '%s' '{\"continue\":false,\"stop_reason\":\"ticket closed\",\"additional_context\":\"read only\"}'; fi"});
    hooks(&flow, json!({"UserPromptSubmit":[{"hooks":[guard]}]}));
    let mut live = flow.runtime.subscribe();
    let session = flow.session("default").await;
    flow.prompt(&session, "work on TICKET-7").await;
    flow.settle(&session).await;
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 1, "the Drain stops after the prompt's Turn");
    assert!(format!("{:?}", requests[0].messages).contains("[hook guard] read only"));
    let state = flow.runtime.state(&session).await.unwrap();
    assert_eq!(
        state.calls["call_read"].status,
        cyber_server::runtime::CallStatus::Ok,
        "the rest of the Turn ran"
    );
    assert_eq!(
        system_texts(&state),
        ["[hook guard] read only", "[hook guard] ticket closed"]
    );
    let events = drain_events(&mut live);
    assert!(errors(&events).is_empty(), "{:?}", errors(&events));
    assert!(
        notices(&events).contains(&"ticket closed".to_string()),
        "{:?}",
        notices(&events)
    );
    assert!(!flow.runtime.is_running(&session));
    // The consumed stop does not end the next prompt's Drain.
    flow.prompt(&session, "again").await;
    flow.settle(&session).await;
    assert_eq!(flow.main.requests().len(), 2);
}

fn flow_with(
    script: Vec<Vec<ScriptStep>>,
    interactive: bool,
    extra: Vec<(&'static str, support::flow::Scripts)>,
) -> Flow {
    Flow::with_models(
        support::Fixture::new(),
        script,
        interactive,
        std::sync::Arc::new(cyber_server::runtime::NoSnapshots),
        extra,
    )
}

#[tokio::test]
async fn pre_model_switch_deny_refuses_a_user_switch_and_automatic_switches_are_observed() {
    let flow = flow_with(
        vec![text("main answer")],
        false,
        vec![("test/other", vec![text("other answer")])],
    );
    flow.f
        .set_config(json!({"agents":{"build":{"model":"test/other"}}}));
    // Denies a switch to the frozen model and every automatic switch.
    let guard = json!({"type":"command","id":"guard","command":
        "if grep -q -e 'test/frozen' -e '\"automatic\":true'; then printf '%s' '{\"decision\":\"deny\",\"reason\":\"frozen model\"}'; fi"});
    hooks(
        &flow,
        json!({"PreModelSwitch":[{"hooks":[guard]},{"hooks":[capture("switch.jsonl")]}],
               "PostModelSwitch":[{"hooks":[capture("switch.jsonl")]}]}),
    );
    let session = flow
        .runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            model_is_default: true,
            mode: Some("default".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    assert_eq!(
        flow.runtime.state(&session).await.unwrap().info.model,
        "test/other"
    );
    let error = flow
        .runtime
        .switch_model(&session, "test/frozen")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("frozen model"), "{error}");
    assert_eq!(
        flow.runtime.state(&session).await.unwrap().info.model,
        "test/other",
        "a denied switch records nothing"
    );
    // The profile's default model changes: the next boundary switches automatically
    // even though PreModelSwitch denies it.
    flow.f
        .set_config(json!({"agents":{"build":{"model":"test/main"}}}));
    flow.prompt(&session, "work").await;
    flow.settle(&session).await;
    assert_eq!(flow.main.requests().len(), 1);
    assert_eq!(
        flow.runtime.state(&session).await.unwrap().info.model,
        "test/main"
    );
    let seen: Vec<(String, String, bool)> = lines(&flow, "switch.jsonl")
        .iter()
        .map(|event| {
            assert_eq!(event["session_id"], session);
            (
                event["event"].as_str().unwrap().to_owned(),
                event["to"].as_str().unwrap().to_owned(),
                event["automatic"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("PreModelSwitch".into(), "test/frozen".into(), false),
            ("PreModelSwitch".into(), "test/main".into(), true),
            ("PostModelSwitch".into(), "test/main".into(), true),
        ]
    );
}

#[tokio::test]
async fn pre_compact_context_reaches_the_summary_instructions_without_a_message() {
    let flow = flow_with(
        vec![text("first answer")],
        false,
        vec![("test/summary", vec![text("the summary")])],
    );
    hooks(
        &flow,
        json!({"PreCompact":[{"hooks":[context("ticket","Keep ticket CYB-42 in the summary")]},
                             {"hooks":[capture("compact.jsonl")]}],
               "PostCompact":[{"hooks":[capture("compact.jsonl")]}]}),
    );
    let session = flow.session("default").await;
    flow.prompt(&session, &"Explain the build. ".repeat(3000))
        .await;
    flow.settle(&session).await;
    flow.runtime
        .compact(&session, Some("focus on tests".into()))
        .await
        .unwrap();
    let summary = flow.requests("test/summary");
    assert_eq!(summary.len(), 1);
    let prompt = format!("{:?}", summary[0].messages);
    assert!(prompt.contains("focus on tests"), "{prompt}");
    assert!(
        prompt.contains("[hook ticket] Keep ticket CYB-42 in the summary"),
        "{prompt}"
    );
    let state = flow.runtime.state(&session).await.unwrap();
    assert!(state.compacted.is_some());
    assert!(
        !system_texts(&state)
            .iter()
            .any(|text| text.contains("CYB-42")),
        "PreCompact context is not admitted as a message"
    );
    assert!(!format!("{:?}", state.inbox).contains("CYB-42"));
    let events = lines(&flow, "compact.jsonl");
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["event"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["PreCompact", "PostCompact"]);
    assert_eq!(events[0]["trigger"], "manual");
    assert_eq!(events[0]["instructions"], "focus on tests");
    assert_eq!(events[1]["trigger"], "manual");
}

#[tokio::test]
async fn session_end_failure_never_prevents_deletion_and_is_noticed() {
    let flow = Flow::new(vec![text("answer")], false);
    hooks(
        &flow,
        json!({"SessionEnd":[{"hooks":[capture("end.jsonl"),
            {"type":"command","id":"broken","fail_closed":true,"command":"cat >/dev/null; printf '{bad'"}]}]}),
    );
    let mut live = flow.runtime.subscribe();
    let session = flow.session("default").await;
    flow.prompt(&session, "work").await;
    flow.settle(&session).await;
    flow.runtime.archive(&session, true).await.unwrap();
    assert!(flow.runtime.state(&session).await.unwrap().info.archived);
    flow.runtime.delete(&session).await.unwrap();
    assert!(flow.runtime.state(&session).await.is_err());
    let ended = lines(&flow, "end.jsonl");
    let reasons: Vec<&str> = ended
        .iter()
        .map(|event| event["reason"].as_str().unwrap())
        .collect();
    assert_eq!(reasons, ["archive", "delete"]);
    assert!(ended.iter().all(|event| event["session_id"] == session));
    assert!(
        !notices(&drain_events(&mut live)).is_empty(),
        "the unavailable handler leaves a notice"
    );
}

/// One model step that calls `read` twice.
fn two_reads() -> Vec<ScriptStep> {
    let mut steps = call("read_a", "read", json!({"path":"a.txt"}));
    let mut second = call("read_b", "read", json!({"path":"b.txt"}));
    steps.truncate(1);
    second.truncate(1);
    steps.extend(second);
    steps.extend(call("unused", "read", json!({})).into_iter().skip(1));
    steps
}

#[tokio::test]
async fn post_tool_batch_fires_once_per_turn_after_every_group_settles() {
    let flow = Flow::new(
        vec![
            two_reads(),
            text("both read"),
            call("secret", "read", json!({"path":".env"})),
            text("never"),
        ],
        true,
    );
    flow.f.write("a.txt", "a");
    flow.f.write("b.txt", "b");
    flow.f.write(".env", "private");
    hooks(
        &flow,
        json!({"PostToolUse":[{"hooks":[capture("tools.jsonl")]}],
               "PostToolBatch":[{"hooks":[capture("tools.jsonl")]}]}),
    );
    let session = flow.session("default").await;
    flow.prompt(&session, "read both").await;
    flow.settle(&session).await;
    let events = lines(&flow, "tools.jsonl");
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["event"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["PostToolUse", "PostToolUse", "PostToolBatch"]);
    let batch = &events[2];
    let mut calls: Vec<(String, String)> = batch["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| {
            (
                call["call_id"].as_str().unwrap().to_owned(),
                call["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    calls.sort();
    assert_eq!(
        calls,
        [
            ("read_a".to_owned(), "ok".to_owned()),
            ("read_b".to_owned(), "ok".to_owned())
        ]
    );
    // An interrupted Turn still reports its settled batch once, observed in a
    // runtime-owned task so the interrupt never waits for it.
    flow.prompt(&session, "read the secrets").await;
    flow.pending(&session).await;
    flow.runtime.interrupt(&session).await.unwrap();
    flow.settle(&session).await;
    let batches = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let batches: Vec<Value> = lines(&flow, "tools.jsonl")
                .into_iter()
                .filter(|event| event["event"] == "PostToolBatch")
                .collect();
            if batches.len() >= 2 && receipts(&flow, &session, "PostToolBatch").len() == 2 {
                return batches;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the interrupted batch is observed");
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[1]["calls"][0]["call_id"], "secret");
    assert_eq!(batches[1]["calls"][0]["status"], "interrupted");
    assert_eq!(receipts(&flow, &session, "PostToolBatch").len(), 2);
}
