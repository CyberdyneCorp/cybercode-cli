//! Command hooks execute around real built-in calls, before permission evaluation.
#![cfg(unix)]
mod support;
use cyber_core::config::{self, LoadRequest};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_server::runtime::{CallStatus, HookExecutionStatus, LiveEvent};
use serde_json::{Value, json};
use support::flow::{Flow, call, text};

fn hooks(f: &Flow, hooks: Value) {
    let env = std::collections::HashMap::from([(
        "CYBER_HOME".into(),
        f.f.dir.path().join("hook-config").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, f.f.dir.path());
    paths.ensure().unwrap();
    std::fs::write(
        paths.config.join("cyber.jsonc"),
        json!({"hooks":hooks}).to_string(),
    )
    .unwrap();
    let home = f.f.dir.path().to_path_buf();
    let trust = TrustStore::new(paths.trust_file());
    f.f.host
        .attach_hook_config(
            std::sync::Arc::new(move |location| {
                config::load(&LoadRequest {
                    location,
                    paths: &paths,
                    env: &env,
                    home: &home,
                    profile: None,
                    overrides: &[],
                    flags: json!({}),
                })
                .map_err(|error| error.to_string())
            }),
            trust,
        )
        .unwrap();
}
fn command(output: &str) -> Value {
    json!({"type":"command","command":format!("cat >/dev/null; printf '%s' '{}'",output.replace('\'',"'\\''")),"id":"guard"})
}

#[tokio::test]
async fn command_hook_denies_write_even_in_bypass_mode() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"unsafe"}),
            ),
            text("done"),
        ],
        false,
    );
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[command(r#"{"decision":"deny","reason":"policy"}"#)]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert!(!f.f.repo.join("a.txt").exists());
    assert!(
        f.output(&session, "call_write")
            .await
            .contains("blocked by hook guard: policy")
    );
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].status, HookExecutionStatus::Completed);
    assert_eq!(receipts[0].call_id.as_deref(), Some("call_write"));
    assert_eq!(receipts[0].tool_name.as_deref(), Some("write"));
    assert!(receipts[0].io.is_none());
}

#[tokio::test]
async fn rewrites_feed_later_conditions_and_are_schema_validated_before_effects() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"original.txt","content":"old"}),
            ),
            text("done"),
        ],
        false,
    );
    let first = command(r#"{"updated_input":{"path":"rewritten.txt","content":"safe"}}"#);
    let mut second = command(r#"{"decision":"deny","reason":"chained"}"#);
    second["if"] = json!({"field":"tool_input.path","matches":"^rewritten.txt$"});
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[first,second]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert!(f.output(&session, "call_write").await.contains("chained"));
    assert!(!f.f.repo.join("original.txt").exists());
    assert!(!f.f.repo.join("rewritten.txt").exists());
    assert_eq!(f.runtime.hook_executions(&session, 10).unwrap().len(), 2);
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"original.txt","content":"old"}),
            ),
            text("done"),
        ],
        false,
    );
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[command(r#"{"updated_input":{"path":42}}"#)]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert_eq!(
        f.output(&session, "call_write").await,
        "hook produced invalid tool input"
    );
    assert!(!f.f.repo.join("original.txt").exists());
}

#[tokio::test]
async fn hook_allow_skips_ask_and_success_events_receive_final_input() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"original.txt","content":"old"}),
            ),
            text("done"),
        ],
        false,
    );
    hooks(
        &f,
        json!({
            "PreToolUse":[{"matcher":"write","hooks":[command(r#"{"decision":"allow","updated_input":{"path":"final.txt","content":"safe"}}"#)]}],
            "PostToolUse":[{"matcher":"write","hooks":[{"type":"command","command":"cat > post.json","id":"post"}]}]
        }),
    );
    let session = f.session("default").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("final.txt")).unwrap(),
        "safe"
    );
    assert!(!f.f.repo.join("original.txt").exists());
    let post: Value =
        serde_json::from_slice(&std::fs::read(f.f.repo.join("post.json")).unwrap()).unwrap();
    assert_eq!(post["tool_input"]["path"], "final.txt");
    assert_eq!(post["session_id"], session);
    assert_eq!(post["call_id"], "call_write");
    assert!(post["tool_output"].is_string());
    assert!(post["duration_ms"].is_u64());
    assert_eq!(f.runtime.hook_executions(&session, 10).unwrap().len(), 2);
}

#[tokio::test]
async fn hook_allow_cannot_override_permission_deny_and_failure_event_is_emitted() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"protected.txt","content":"unsafe"}),
            ),
            text("done"),
        ],
        false,
    );
    f.f.set_config(json!({"permissions":{"edit":{"protected.txt":"deny","*":"ask"}}}));
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[command(r#"{"decision":"allow"}"#)]}],"PostToolUseFailure":[{"matcher":"write","hooks":[{"type":"command","command":"cat > failure.json"}]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert!(!f.f.repo.join("protected.txt").exists());
    let state = f.runtime.state(&session).await.unwrap();
    assert_eq!(state.calls["call_write"].status, CallStatus::Error);
    let failure: Value =
        serde_json::from_slice(&std::fs::read(f.f.repo.join("failure.json")).unwrap()).unwrap();
    assert_eq!(failure["event"], "PostToolUseFailure");
    assert!(failure["tool_output"].as_str().unwrap().contains("denied"));
}

#[tokio::test]
async fn command_errors_are_nonblocking_with_live_diagnostics_and_no_raw_receipt_io() {
    let f = Flow::new(
        vec![
            call("call_read", "read", json!({"path":"a.txt"})),
            text("done"),
        ],
        false,
    );
    f.f.write("a.txt", "safe");
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"read","hooks":[{"type":"command","command":"printf private-diagnostic >&2; exit 1","id":"broken"}]}]}),
    );
    let mut live = f.runtime.subscribe();
    let session = f.session("default").await;
    f.prompt(&session, "read").await;
    f.settle(&session).await;
    assert!(f.output(&session, "call_read").await.contains("safe"));
    let mut noticed = false;
    while let Ok(event) = live.try_recv() {
        if let LiveEvent::HookNotice { message, .. } = event {
            noticed |= message.contains("private-diagnostic");
        }
    }
    assert!(noticed);
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert!(
        !serde_json::to_string(&receipts)
            .unwrap()
            .contains("private-diagnostic")
    );
}

#[tokio::test]
async fn project_command_requires_individual_approval_at_actual_dispatch() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"safe"}),
            ),
            text("done"),
            call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"unsafe"}),
            ),
            text("done"),
        ],
        false,
    );
    let env = std::collections::HashMap::from([(
        "CYBER_HOME".into(),
        f.f.dir.path().join("hook-config").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, f.f.dir.path());
    paths.ensure().unwrap();
    std::fs::write(f.f.repo.join("cyber.jsonc"),json!({"hooks":{"PreToolUse":[{"matcher":"write","hooks":[command(r#"{"decision":"deny","reason":"project guard"}"#)]}]},"sandbox":{"network":"off"}}).to_string()).unwrap();
    let trust_path = paths.trust_file();
    let trust = TrustStore::new(&trust_path);
    let home = f.f.dir.path().to_path_buf();
    let load = std::sync::Arc::new(move |location: &std::path::Path| {
        config::load(&LoadRequest {
            location,
            paths: &paths,
            env: &env,
            home: &home,
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .map_err(|error| error.to_string())
    });
    let resolved = load(&f.f.repo).unwrap();
    trust
        .approve(
            &resolved.trust.checkout_root,
            resolved.trust.digest.as_ref().unwrap(),
        )
        .unwrap();
    let approved = load(&f.f.repo).unwrap();
    let digest = cyber_core::hooks::HookCatalog::from_config(&approved)
        .unwrap()
        .definitions[0]
        .digest
        .clone();
    f.f.host
        .attach_hook_config(load, TrustStore::new(trust_path))
        .unwrap();
    let first = f.session("bypass").await;
    f.prompt(&first, "write").await;
    f.settle(&first).await;
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("a.txt")).unwrap(),
        "safe"
    );
    let skipped = f.runtime.hook_executions(&first, 10).unwrap();
    assert_eq!(skipped.len(), 1, "{skipped:#?}");
    assert_eq!(
        skipped[0].outcome,
        Some(cyber_core::hooks::HookOutcome::Skipped)
    );
    assert_eq!(
        skipped[0].decision.as_ref().unwrap().reason.as_deref(),
        Some("untrusted")
    );
    trust.approve_hook(&f.f.repo, &digest).unwrap();
    let second = f.session("bypass").await;
    f.prompt(&second, "write").await;
    f.settle(&second).await;
    if cyber_sandbox::available() {
        assert!(
            f.output(&second, "call_write")
                .await
                .contains("project guard")
        );
    }
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("a.txt")).unwrap(),
        "safe"
    );
}

#[tokio::test]
async fn absolute_file_target_matches_relative_paths_and_duplicate_command_runs_once() {
    let fixture = support::Fixture::new();
    fixture.write("a.txt", "safe");
    let target = fixture.repo.join("a.txt");
    let f = Flow::with(
        fixture,
        vec![
            call("call_read", "read", json!({"path":target})),
            text("done"),
        ],
        false,
        std::sync::Arc::new(cyber_server::runtime::NoSnapshots),
    );
    let handler = json!({"type":"command","command":"printf x >> counter"});
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"read","paths":["a.txt"],"hooks":[handler.clone(),handler]}]}),
    );
    let session = f.session("default").await;
    f.prompt(&session, "read").await;
    f.settle(&session).await;
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("counter")).unwrap(),
        "x"
    );
    assert_eq!(f.runtime.hook_executions(&session, 10).unwrap().len(), 1);
}

#[tokio::test]
async fn hook_ask_requires_user_reply_even_in_bypass_mode() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"safe"}),
            ),
            text("done"),
        ],
        true,
    );
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[command(r#"{"decision":"ask"}"#)]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    let pending = f.pending(&session).await;
    assert!(!f.f.repo.join("a.txt").exists());
    f.runtime
        .reply_permission(&pending.id, cyber_server::runtime::PermissionReply::Once)
        .await
        .unwrap();
    f.settle(&session).await;
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("a.txt")).unwrap(),
        "safe"
    );
}

#[tokio::test]
async fn async_definition_does_not_deduplicate_a_synchronous_guard() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"unsafe"}),
            ),
            text("done"),
        ],
        false,
    );
    let guard = command(r#"{"decision":"deny","reason":"supported guard"}"#);
    let mut asynchronous = guard.clone();
    asynchronous["async"] = json!(true);
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[asynchronous,guard]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert!(!f.f.repo.join("a.txt").exists());
    assert!(
        f.output(&session, "call_write")
            .await
            .contains("supported guard")
    );
    // Both ran: the background run's deny is recorded but ignored.
    let receipts = wait_receipts(&f, &session, 2).await;
    assert!(receipts.iter().all(|record| {
        record.status == HookExecutionStatus::Completed
            && record.outcome == Some(cyber_core::hooks::HookOutcome::Blocked)
    }));
}

#[tokio::test]
async fn once_command_runs_once_per_session_and_messages_stay_user_visible() {
    let f = Flow::new(
        vec![
            call(
                "call_one",
                "write",
                json!({"path":"one.txt","content":"one"}),
            ),
            call(
                "call_two",
                "write",
                json!({"path":"two.txt","content":"two"}),
            ),
            text("done"),
        ],
        false,
    );
    let handler = json!({"type":"command","command":"cat >/dev/null; printf x >> once-count; printf '{}'","id":"once-guard","once":true,"status_message":"Checking once","system_message":"Checked once"});
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[handler]}]}),
    );
    let mut live = f.runtime.subscribe();
    let session = f.session("bypass").await;
    f.prompt(&session, "write twice").await;
    f.settle(&session).await;
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("once-count")).unwrap(),
        "x"
    );
    assert_eq!(f.runtime.hook_executions(&session, 10).unwrap().len(), 1);
    assert!(f.f.repo.join("one.txt").exists() && f.f.repo.join("two.txt").exists());
    let mut notices = Vec::new();
    while let Ok(event) = live.try_recv() {
        if let LiveEvent::HookNotice { message, .. } = event {
            notices.push(message);
        }
    }
    assert_eq!(notices, ["Checking once", "Checked once"]);
    let state = f.runtime.state(&session).await.unwrap();
    assert!(
        !serde_json::to_string(&state)
            .unwrap()
            .contains("Checking once")
    );
}

#[tokio::test]
async fn permission_request_command_answers_only_the_current_ask_without_rewriting_input() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"docs/a.txt","content":"original"}),
            ),
            text("done"),
        ],
        false,
    );
    f.f.set_config(json!({"permissions":{"edit":"ask"}}));
    std::fs::create_dir(f.f.repo.join("docs")).unwrap();
    let mut allow =
        command(r#"{"decision":"allow","updated_input":{"path":"wrong.txt","content":"changed"}}"#);
    allow["command"] = json!(
        "cat > permission.json; printf '%s' '{\"decision\":\"allow\",\"updated_input\":{\"path\":\"wrong.txt\",\"content\":\"changed\"}}'"
    );
    hooks(
        &f,
        json!({"PermissionRequest":[{"matcher":"write","paths":["docs/**"],"hooks":[allow]}]}),
    );
    let session = f.session("default").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("docs/a.txt")).unwrap(),
        "original"
    );
    assert!(!f.f.repo.join("wrong.txt").exists());
    assert!(f.runtime.pending_requests(Some(&session)).is_empty());
    let input: Value =
        serde_json::from_slice(&std::fs::read(f.f.repo.join("permission.json")).unwrap()).unwrap();
    assert_eq!(input["event"], "PermissionRequest");
    assert_eq!(input["permission"]["action"], "edit");
    assert_eq!(input["tool_input"]["path"], "docs/a.txt");
    assert_eq!(input["session_id"], session);
    let saved: i64 =
        f.f.store
            .read(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM permission_saved", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .unwrap();
    assert_eq!(saved, 0, "hook allow must not become a saved approval");
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(receipts.len(), 1);
    assert!(receipts[0].io.is_none());
    assert!(
        receipts[0]
            .decision
            .as_ref()
            .unwrap()
            .updated_input
            .is_none()
    );
}

#[tokio::test]
async fn permission_request_deny_prevents_user_prompt_and_effect_even_for_explicit_hook_ask() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"unsafe"}),
            ),
            text("done"),
        ],
        true,
    );
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[command(r#"{"decision":"ask"}"#)]}],"PermissionRequest":[{"matcher":"write","hooks":[command(r#"{"decision":"deny","reason":"request guard"}"#)]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert!(!f.f.repo.join("a.txt").exists());
    assert!(
        f.output(&session, "call_write")
            .await
            .contains("request guard")
    );
    assert!(f.runtime.pending_requests(Some(&session)).is_empty());
    assert_eq!(f.runtime.hook_executions(&session, 10).unwrap().len(), 2);
}

#[tokio::test]
async fn permission_request_allow_cannot_override_hard_denials_and_is_not_run_for_allowed_calls() {
    for mode in ["default", "plan", "bypass"] {
        let f = Flow::new(
            vec![
                call(
                    "call_write",
                    "write",
                    json!({"path":"a.txt","content":"safe"}),
                ),
                text("done"),
            ],
            false,
        );
        if mode == "default" {
            f.f.set_config(json!({"permissions":{"edit":"deny"}}));
        }
        hooks(
            &f,
            json!({"PermissionRequest":[{"matcher":"write","hooks":[{"type":"command","command":"cat >/dev/null; printf x > unexpected-hook; printf '{\"decision\":\"allow\"}'"}]}]}),
        );
        let session = f.session(mode).await;
        f.prompt(&session, "write").await;
        f.settle(&session).await;
        assert_eq!(f.f.repo.join("a.txt").exists(), mode == "bypass");
        assert!(!f.f.repo.join("unexpected-hook").exists());
        assert!(f.runtime.hook_executions(&session, 10).unwrap().is_empty());
    }
}

#[tokio::test]
async fn explicit_pre_hook_ask_cannot_bypass_auto_always_block_through_request_allow() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"unsafe"}),
            ),
            text("done"),
        ],
        false,
    );
    f.f.set_config(json!({"permissions":{"auto_mode":{"rules":{"always_block":[{"action":"edit","resource":"*"}]}}}}));
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[command(r#"{"decision":"ask"}"#)]}],"PermissionRequest":[{"matcher":"write","hooks":[command(r#"{"decision":"allow"}"#)]}]}),
    );
    let session = f.session("auto").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert!(!f.f.repo.join("a.txt").exists());
    assert!(
        f.output(&session, "call_write")
            .await
            .contains("Blocked by auto mode")
    );
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].event, "PreToolUse");
}

async fn wait_hook_condition(f: &Flow, session: &str, condition: impl Fn() -> bool) {
    let ready = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await;
    if ready.is_err() {
        f.runtime.interrupt(session).await.unwrap();
        f.settle(session).await;
        panic!("hook concurrency barrier was not reached");
    }
}
fn waiting_command(id: &str) -> Value {
    json!({"type":"command","id":id,"timeout":10,"command":format!("cat >/dev/null; printf x >> count-{id}; touch started-{id}; while [ ! -f release ] && [ ! -f release-{id} ]; do sleep 0.02; done; printf '{{}}'")})
}

#[tokio::test]
async fn post_commands_overlap_up_to_configured_limit_and_deduplicate_at_execution() {
    let f = Flow::new(
        vec![
            call("call_read", "read", json!({"path":"a.txt"})),
            text("done"),
        ],
        false,
    );
    f.f.write("a.txt", "safe");
    let a = waiting_command("a");
    hooks(
        &f,
        json!({"concurrency":2,"PostToolUse":[{"matcher":"read","hooks":[a.clone(),waiting_command("b"),{"type":"command","command":"cat >/dev/null; touch started-c; printf '{}'"},a]}]}),
    );
    let session = f.session("default").await;
    f.prompt(&session, "read").await;
    wait_hook_condition(&f, &session, || {
        f.f.repo.join("started-a").exists() && f.f.repo.join("started-b").exists()
    })
    .await;
    assert!(
        !f.f.repo.join("started-c").exists(),
        "third command exceeded the pool limit"
    );
    assert_eq!(f.runtime.hook_executions(&session, 10).unwrap().len(), 2);
    f.f.write("release-b", "ready");
    wait_hook_condition(&f, &session, || f.f.repo.join("started-c").exists()).await;
    assert!(
        f.runtime
            .hook_executions(&session, 10)
            .unwrap()
            .iter()
            .any(|record| record.hook_id == "a" && record.status == HookExecutionStatus::Running),
        "a completed out-of-order result must free its slot before the first handler finishes"
    );
    f.f.write("release", "ready");
    f.settle(&session).await;
    assert!(f.f.repo.join("started-c").exists());
    assert_eq!(f.f.read("count-a"), "x");
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(receipts.len(), 3);
    assert!(
        receipts
            .iter()
            .all(|record| record.outcome == Some(cyber_core::hooks::HookOutcome::Ok))
    );
}

#[tokio::test]
async fn concurrent_permission_decisions_merge_in_declared_order_after_reverse_completion() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"unsafe"}),
            ),
            text("done"),
        ],
        false,
    );
    let first = json!({"type":"command","id":"first","timeout":10,"command":"cat >/dev/null; touch started-first; while [ ! -f release ]; do sleep 0.02; done; printf '{\"decision\":\"deny\",\"reason\":\"first declared\"}'"});
    let second = json!({"type":"command","id":"second","command":"cat >/dev/null; printf '{\"decision\":\"deny\",\"reason\":\"second declared\"}'"});
    hooks(
        &f,
        json!({"concurrency":2,"PermissionRequest":[{"matcher":"write","hooks":[first,second]}]}),
    );
    let session = f.session("default").await;
    f.prompt(&session, "write").await;
    wait_hook_condition(&f, &session, || {
        let records = f.runtime.hook_executions(&session, 10).unwrap();
        records
            .iter()
            .any(|r| r.hook_id == "second" && r.status == HookExecutionStatus::Completed)
            && records
                .iter()
                .any(|r| r.hook_id == "first" && r.status == HookExecutionStatus::Running)
    })
    .await;
    f.f.write("release", "ready");
    f.settle(&session).await;
    assert!(
        f.output(&session, "call_write")
            .await
            .contains("second: second declared")
    );
    assert!(!f.f.repo.join("a.txt").exists());
    assert!(f.runtime.pending_requests(Some(&session)).is_empty());
}

#[tokio::test]
async fn interrupted_concurrent_hooks_settle_launched_owners_without_launching_queued_commands() {
    let f = Flow::new(
        vec![
            call("call_read", "read", json!({"path":"a.txt"})),
            text("done"),
        ],
        false,
    );
    f.f.write("a.txt", "safe");
    hooks(
        &f,
        json!({"concurrency":2,"PostToolUse":[{"matcher":"read","hooks":[waiting_command("a"),waiting_command("b"),{"type":"command","command":"cat >/dev/null; touch queued-effect"}]}]}),
    );
    let session = f.session("default").await;
    f.prompt(&session, "read").await;
    wait_hook_condition(&f, &session, || {
        f.f.repo.join("started-a").exists() && f.f.repo.join("started-b").exists()
    })
    .await;
    f.runtime.interrupt(&session).await.unwrap();
    f.settle(&session).await;
    assert!(!f.f.repo.join("queued-effect").exists());
    let records = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.status == HookExecutionStatus::Completed
                && record.acknowledged == Some(true)
                && record.must_stop)
    );
}

#[tokio::test]
async fn fail_closed_parallel_admission_cancels_siblings_and_retains_original_error() {
    let f = Flow::new(
        vec![
            call("call_read", "read", json!({"path":"a.txt"})),
            text("done"),
        ],
        false,
    );
    f.f.write("a.txt", "safe");
    let refusal = json!({"type":"command","id":"refusal","timeout":10,"fail_closed":true,"command":"cat >/dev/null; while [ ! -f started-a ]; do sleep 0.02; done; printf '{not json'"});
    hooks(
        &f,
        json!({"concurrency":2,"PostToolUse":[{"matcher":"read","hooks":[waiting_command("a"),refusal,{"type":"command","command":"cat >/dev/null; touch queued-effect"}]}]}),
    );
    let session = f.session("default").await;
    f.prompt(&session, "read").await;
    f.settle(&session).await;
    assert!(
        f.output(&session, "call_read")
            .await
            .contains("post-hook failed: hook refusal unavailable")
    );
    assert!(!f.f.repo.join("queued-effect").exists());
    let records = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.status == HookExecutionStatus::Completed
                && record.acknowledged == Some(true))
    );
    assert!(
        records
            .iter()
            .any(|record| record.hook_id == "a" && record.must_stop)
    );
}

#[tokio::test]
async fn http_policy_decision_denies_real_builtin_write_before_effects() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/policy", listener.local_addr().unwrap());
    let served = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
        assert!(socket.read(&mut bytes).await.unwrap() > 0);
        let body = r#"{"decision":"deny","reason":"HTTP policy"}"#;
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"unsafe"}),
            ),
            text("done"),
        ],
        false,
    );
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[{"type":"http","url":url,"id":"policy"}]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    served.await.unwrap();
    assert!(!f.f.repo.join("a.txt").exists());
    assert!(
        f.output(&session, "call_write")
            .await
            .contains("policy: HTTP policy")
    );
    assert_eq!(
        f.runtime.hook_executions(&session, 10).unwrap()[0].outcome,
        Some(cyber_core::hooks::HookOutcome::Blocked)
    );
}

/// A local HTTP endpoint for one hook request: it answers with `response`, or holds
/// the connection open without answering when `response` is None.
fn http_hook_endpoint(response: Option<&'static str>) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/policy", listener.local_addr().unwrap());
    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
    let served = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 8192];
        assert!(socket.read(&mut bytes).await.unwrap() > 0);
        let Some(response) = response else {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            return;
        };
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    (url, served)
}

#[tokio::test]
async fn exit_two_pre_tool_command_blocks_with_stderr_reason() {
    let f = Flow::new(
        vec![
            call(
                "call_bash",
                "bash",
                json!({"command":"printf ran > prod.txt"}),
            ),
            text("done"),
        ],
        false,
    );
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"bash","hooks":[{"type":"command","id":"prod","command":"cat >/dev/null; printf 'rm on prod paths is forbidden' >&2; exit 2"}]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "run it").await;
    f.settle(&session).await;
    assert!(!f.f.repo.join("prod.txt").exists(), "the call never ran");
    let output = f.output(&session, "call_bash").await;
    assert!(output.contains("rm on prod paths is forbidden"), "{output}");
    let state = f.runtime.state(&session).await.unwrap();
    assert_ne!(state.calls["call_bash"].status, CallStatus::Ok);
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(
        receipts[0].outcome,
        Some(cyber_core::hooks::HookOutcome::Blocked)
    );
    // The model received the reason as the tool error and the Turn went on.
    let second = serde_json::to_string(&f.main.requests()[1].messages).unwrap();
    assert!(second.contains("rm on prod paths is forbidden"), "{second}");
}

#[tokio::test]
async fn post_tool_command_exit_one_warns_with_hook_id_and_continues() {
    let f = Flow::new(
        vec![
            call("call_read", "read", json!({"path":"a.txt"})),
            text("done"),
        ],
        false,
    );
    f.f.write("a.txt", "safe");
    hooks(
        &f,
        json!({"PostToolUse":[{"matcher":"read","hooks":[{"type":"command","id":"audit","command":"cat >/dev/null; printf 'audit log offline' >&2; exit 1"}]}]}),
    );
    let mut live = f.runtime.subscribe();
    let session = f.session("default").await;
    f.prompt(&session, "read").await;
    f.settle(&session).await;
    let state = f.runtime.state(&session).await.unwrap();
    assert_eq!(state.calls["call_read"].status, CallStatus::Ok);
    assert_eq!(f.main.requests().len(), 2, "the Turn continued");
    let warned = hook_notices(&mut live)
        .into_iter()
        .any(|(_, message)| message.contains("hook audit exited with code 1: audit log offline"));
    assert!(warned, "a warning names the hook and its stderr");
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(
        receipts[0].outcome,
        Some(cyber_core::hooks::HookOutcome::Error)
    );
}

#[tokio::test]
async fn http_fail_closed_timeout_blocks_call_with_hook_unavailable() {
    let (url, served) = http_hook_endpoint(None);
    let f = write_flow("a.txt");
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[{"type":"http","url":url,"id":"policy","timeout":1,"fail_closed":true}]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    f.settle_within(&session, std::time::Duration::from_secs(20))
        .await;
    served.abort();
    assert!(
        !f.f.repo.join("a.txt").exists(),
        "the guarded write is blocked"
    );
    let output = f.output(&session, "call_write").await;
    assert!(output.contains("hook policy unavailable"), "{output}");
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(
        receipts[0].outcome,
        Some(cyber_core::hooks::HookOutcome::Timeout)
    );
}

#[tokio::test]
async fn http_non_2xx_is_nonblocking_on_a_real_call() {
    let body = r#"{"decision":"deny","reason":"ignored on error"}"#;
    let response: &'static str = Box::leak(
        format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_boxed_str(),
    );
    let (url, served) = http_hook_endpoint(Some(response));
    let f = write_flow("a.txt");
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[{"type":"http","url":url,"id":"policy"}]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    served.await.unwrap();
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("a.txt")).unwrap(),
        "original",
        "a non-2xx answer never blocks without fail_closed"
    );
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(
        receipts[0].outcome,
        Some(cyber_core::hooks::HookOutcome::Error)
    );
}

#[tokio::test]
async fn rewrite_is_visible_as_hook_notice_in_transcript() {
    let f = Flow::new(
        vec![
            call(
                "call_bash",
                "bash",
                json!({"command":"printf original > ran.txt"}),
            ),
            text("done"),
        ],
        false,
    );
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"bash","hooks":[handler("ci", r#"{"updated_input":{"command":"printf rewritten > ran.txt"}}"#)]}]}),
    );
    let mut live = f.runtime.subscribe();
    let session = f.session("bypass").await;
    f.prompt(&session, "run it").await;
    f.settle(&session).await;
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("ran.txt")).unwrap(),
        "rewritten"
    );
    let rewrites: Vec<String> =
        f.f.store
            .read_events(&session, -1, 200)
            .unwrap()
            .events
            .into_iter()
            .filter(|e| e.kind == "session.system.added.1" && e.data["reason"] == "hook_rewrite")
            .map(|e| e.data["text"].as_str().unwrap().to_owned())
            .collect();
    assert_eq!(rewrites.len(), 1, "one durable rewrite record");
    let state = f.runtime.state(&session).await.unwrap();
    assert_eq!(system_texts(&state), rewrites, "the transcript shows it");
    assert!(rewrites[0].contains("bash"), "{}", rewrites[0]);
    assert!(
        rewrites[0].contains("printf rewritten > ran.txt"),
        "{}",
        rewrites[0]
    );
    // The model sees the rewrite with the result of the call it requested.
    let second = serde_json::to_string(&f.main.requests()[1].messages).unwrap();
    assert!(second.contains("printf rewritten"), "{second}");
    assert!(hook_notices(&mut live).contains(&("ci".into(), "hook rewrote tool input".into())));
}

/// Regression: a rewrite that injects a credential shows the model and the transcript a
/// redacted notice, while the call itself runs with the credential.
#[tokio::test]
async fn rewrite_notice_redacts_injected_credentials() {
    let token = "ghp_abcdefghijklmnop0123456789";
    let f = Flow::new(
        vec![
            call("call_bash", "bash", json!({"command":"printf x > ran.txt"})),
            text("done"),
        ],
        false,
    );
    let rewritten = format!("printf {token} > ran.txt");
    let answer = json!({"updated_input":{"command":rewritten}}).to_string();
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"bash","hooks":[handler("ci", &answer)]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "run it").await;
    f.settle(&session).await;
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("ran.txt")).unwrap(),
        token
    );
    let rewrites: Vec<String> =
        f.f.store
            .read_events(&session, -1, 200)
            .unwrap()
            .events
            .into_iter()
            .filter(|e| e.kind == "session.system.added.1" && e.data["reason"] == "hook_rewrite")
            .map(|e| e.data["text"].as_str().unwrap().to_owned())
            .collect();
    assert_eq!(rewrites.len(), 1);
    assert!(!rewrites[0].contains(token), "{}", rewrites[0]);
    assert!(rewrites[0].contains("[REDACTED]"), "{}", rewrites[0]);
    let second = serde_json::to_string(&f.main.requests()[1].messages).unwrap();
    assert!(!second.contains(token), "{second}");
}

#[tokio::test]
async fn memory_pre_hook_denial_preserves_normal_receipt_and_has_no_storage_effects() {
    let content =
        "---\nname: coding-policy\ndescription: Coding policy\ntype: reference\n---\nFact\n";
    let f = Flow::new(
        vec![
            call(
                "remember",
                "memory",
                json!({"operation":"write","scope":"global","content":content}),
            ),
            text("done"),
        ],
        false,
    );
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"memory","hooks":[command(r#"{"decision":"deny","reason":"memory policy"}"#)]}]}),
    );
    let session = f.session("bypass").await;
    f.prompt(&session, "remember").await;
    f.settle(&session).await;
    assert!(
        f.output(&session, "remember")
            .await
            .contains("blocked by hook guard: memory policy")
    );
    assert!(!f.f.dir.path().join("memory").exists());
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].status, HookExecutionStatus::Completed);
    assert_eq!(receipts[0].tool_name.as_deref(), Some("memory"));
}

fn handler(id: &str, output: &str) -> Value {
    let mut handler = command(output);
    handler["id"] = json!(id);
    handler
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

fn hook_notices(live: &mut tokio::sync::broadcast::Receiver<LiveEvent>) -> Vec<(String, String)> {
    let mut notices = Vec::new();
    while let Ok(event) = live.try_recv() {
        if let LiveEvent::HookNotice {
            hook_id, message, ..
        } = event
        {
            notices.push((hook_id, message));
        }
    }
    notices
}

#[tokio::test]
async fn post_tool_context_reaches_the_next_request_in_declared_order() {
    let f = Flow::new(
        vec![
            call("call_read", "read", json!({"path":"a.txt"})),
            text("done"),
        ],
        false,
    );
    f.f.write("a.txt", "safe");
    let mut second = handler("second", r#"{"additional_context":"two"}"#);
    second["fail_closed"] = json!(true);
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"read","hooks":[handler("pre", r#"{"additional_context":"zero"}"#)]}],
            "PostToolUse":[{"matcher":"read","hooks":[handler("first", r#"{"additional_context":"one"}"#),second]}]}),
    );
    let mut live = f.runtime.subscribe();
    let session = f.session("default").await;
    f.prompt(&session, "read").await;
    f.settle(&session).await;
    // A fail_closed handler that returns context is no longer refused.
    assert!(f.output(&session, "call_read").await.contains("safe"));
    let requests = f.main.requests();
    assert_eq!(requests.len(), 2);
    let expected = "[hook pre] zero\n[hook first] one\n[hook second] two";
    assert!(!format!("{:?}", requests[0].messages).contains("zero"));
    let state = f.runtime.state(&session).await.unwrap();
    assert_eq!(system_texts(&state), [expected]);
    let second = serde_json::to_string(&requests[1].messages).unwrap();
    assert_eq!(second.matches("[hook first] one").count(), 1, "{second}");
    assert!(
        hook_notices(&mut live)
            .iter()
            .all(|(_, message)| !message.contains("admission"))
    );
}

#[tokio::test]
async fn continue_false_sticks_over_a_later_continue_true_and_stops_after_the_turn() {
    let f = Flow::new(
        vec![
            call("call_read", "read", json!({"path":"a.txt"})),
            text("never requested"),
        ],
        false,
    );
    f.f.write("a.txt", "safe");
    let mut stop = handler(
        "guard",
        r#"{"continue":false,"stop_reason":"policy stop","additional_context":"why"}"#,
    );
    stop["fail_closed"] = json!(true);
    hooks(
        &f,
        json!({"PostToolUse":[{"matcher":"read","hooks":[stop,handler("later", r#"{"continue":true}"#)]}]}),
    );
    let mut live = f.runtime.subscribe();
    let session = f.session("default").await;
    f.prompt(&session, "read").await;
    f.settle(&session).await;
    assert_eq!(
        f.main.requests().len(),
        1,
        "no request after the stopped Turn"
    );
    let state = f.runtime.state(&session).await.unwrap();
    assert_eq!(state.calls["call_read"].status, CallStatus::Ok);
    assert_eq!(
        system_texts(&state),
        ["[hook guard] why", "[hook guard] policy stop"]
    );
    assert!(
        hook_notices(&mut live).contains(&("guard".into(), "policy stop".into())),
        "the stop reason is shown"
    );
    assert!(!f.runtime.is_running(&session));
}

#[tokio::test]
async fn suppress_output_hides_notices_but_keeps_the_receipt_and_deny_reason() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"unsafe"}),
            ),
            text("done"),
        ],
        false,
    );
    let mut quiet = handler(
        "guard",
        r#"{"decision":"deny","reason":"policy","suppress_output":true}"#,
    );
    quiet["system_message"] = json!("Checked quietly");
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[quiet]}]}),
    );
    let mut live = f.runtime.subscribe();
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert!(!f.f.repo.join("a.txt").exists());
    assert!(
        f.output(&session, "call_write")
            .await
            .contains("blocked by hook guard: policy"),
        "the model-facing deny reason is never suppressed"
    );
    assert_eq!(hook_notices(&mut live), []);
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(
        receipts[0].outcome,
        Some(cyber_core::hooks::HookOutcome::Blocked)
    );
    assert_eq!(
        receipts[0]
            .decision
            .as_ref()
            .and_then(|d| d.suppress_output),
        Some(true)
    );
}

/// A PermissionDenied hook for `write` that counts its runs in `denied-count`.
fn denied_hook(output: &str) -> Value {
    json!({"PermissionDenied":[{"matcher":"write","hooks":[{"type":"command","id":"retry","command":format!(
        "cat > denied.json; printf x >> denied-count; printf '%s' '{}'",
        output.replace('\'', "'\\''")
    )}]}]})
}

fn denied_runs(f: &Flow) -> usize {
    std::fs::read_to_string(f.f.repo.join("denied-count"))
        .unwrap_or_default()
        .len()
}

fn write_flow(path: &str) -> Flow {
    Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":path,"content":"original"}),
            ),
            text("done"),
        ],
        false,
    )
}

#[tokio::test]
async fn permission_denied_retry_runs_the_rewritten_call_once() {
    let f = write_flow("protected.txt");
    f.f.set_config(json!({"permissions":{"edit":{"protected*":"deny","*":"allow"}}}));
    hooks(
        &f,
        denied_hook(
            r#"{"decision":"retry","updated_input":{"path":"allowed.txt","content":"safe"}}"#,
        ),
    );
    let session = f.session("default").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert!(!f.f.repo.join("protected.txt").exists());
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("allowed.txt")).unwrap(),
        "safe"
    );
    assert_eq!(denied_runs(&f), 1);
    let event: Value =
        serde_json::from_slice(&std::fs::read(f.f.repo.join("denied.json")).unwrap()).unwrap();
    assert_eq!(event["event"], "PermissionDenied");
    assert_eq!(event["tool_input"]["path"], "protected.txt");
    assert_eq!(event["call_id"], "call_write");
    assert!(
        event["reason"].as_str().unwrap().contains("denied"),
        "{event}"
    );
    let state = f.runtime.state(&session).await.unwrap();
    assert_eq!(state.calls["call_write"].status, CallStatus::Ok);
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].event, "PermissionDenied");
}

#[tokio::test]
async fn permission_denied_retry_cannot_bypass_hard_denies_or_mode_ceilings() {
    // A retried call that is still denied stays denied without a second event.
    let f = write_flow("protected.txt");
    f.f.set_config(json!({"permissions":{"edit":{"protected*":"deny","*":"allow"}}}));
    hooks(
        &f,
        denied_hook(
            r#"{"decision":"retry","updated_input":{"path":"protected-too.txt","content":"x"}}"#,
        ),
    );
    let session = f.session("default").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert!(!f.f.repo.join("protected.txt").exists());
    assert!(!f.f.repo.join("protected-too.txt").exists());
    assert!(
        f.output(&session, "call_write")
            .await
            .contains("Permission denied"),
        "{}",
        f.output(&session, "call_write").await
    );
    assert_eq!(denied_runs(&f), 1);
    let state = f.runtime.state(&session).await.unwrap();
    assert_eq!(state.calls["call_write"].status, CallStatus::Error);

    // The Plan Mode ceiling refuses the retried input as well.
    let f = write_flow("a.txt");
    hooks(
        &f,
        denied_hook(r#"{"decision":"retry","updated_input":{"path":"b.txt","content":"x"}}"#),
    );
    let session = f.session("plan").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert!(!f.f.repo.join("a.txt").exists());
    assert!(!f.f.repo.join("b.txt").exists());
    assert_eq!(denied_runs(&f), 1);

    // A later deny beats an earlier retry, so nothing is re-issued.
    let f = write_flow("protected.txt");
    f.f.set_config(json!({"permissions":{"edit":{"protected*":"deny","*":"allow"}}}));
    hooks(
        &f,
        json!({"PermissionDenied":[{"matcher":"write","hooks":[
            handler("retry", r#"{"decision":"retry","updated_input":{"path":"allowed.txt","content":"x"}}"#),
            handler("veto", r#"{"decision":"deny","reason":"no retries"}"#)
        ]}]}),
    );
    let session = f.session("default").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert!(!f.f.repo.join("allowed.txt").exists());
    assert_eq!(f.runtime.hook_executions(&session, 10).unwrap().len(), 2);
}

#[tokio::test]
async fn permission_denied_retry_input_must_match_the_tool_schema() {
    let f = write_flow("protected.txt");
    f.f.set_config(json!({"permissions":{"edit":{"protected*":"deny","*":"allow"}}}));
    hooks(
        &f,
        denied_hook(r#"{"decision":"retry","updated_input":{"path":42}}"#),
    );
    let session = f.session("default").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    assert_eq!(
        f.output(&session, "call_write").await,
        "hook produced invalid tool input"
    );
    assert_eq!(denied_runs(&f), 1);
}

async fn wait_receipts(
    f: &Flow,
    session: &str,
    count: usize,
) -> Vec<cyber_server::runtime::HookExecutionRecord> {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let receipts = f.runtime.hook_executions(session, 10).unwrap();
            if receipts.len() == count
                && receipts
                    .iter()
                    .all(|receipt| receipt.status != HookExecutionStatus::Running)
            {
                return receipts;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("hook receipts did not settle")
}

#[tokio::test]
async fn async_fail_closed_handler_that_times_out_does_not_block_the_event() {
    let f = write_flow("a.txt");
    let held = json!({"type":"command","id":"background","async":true,"fail_closed":true,"timeout":1,
        "command":"cat >/dev/null; while [ ! -f release ]; do sleep 0.02; done; printf '{\"decision\":\"deny\"}'"});
    hooks(
        &f,
        json!({"PreToolUse":[{"matcher":"write","hooks":[held]}]}),
    );
    let mut live = f.runtime.subscribe();
    let session = f.session("bypass").await;
    f.prompt(&session, "write").await;
    f.settle(&session).await;
    // A blocking fail_closed handler would have timed out and refused the write.
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("a.txt")).unwrap(),
        "original"
    );
    let receipts = wait_receipts(&f, &session, 1).await;
    assert_eq!(receipts[0].status, HookExecutionStatus::Completed);
    assert_eq!(
        receipts[0].outcome,
        Some(cyber_core::hooks::HookOutcome::Timeout)
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        hook_notices(&mut live)
            .iter()
            .any(|(_, message)| message.contains("timed out")),
    );
}

#[tokio::test]
async fn async_notification_hook_does_not_delay_the_permission_prompt() {
    let f = Flow::new(
        vec![
            call(
                "call_write",
                "write",
                json!({"path":"a.txt","content":"safe"}),
            ),
            text("done"),
        ],
        true,
    );
    let webhook = json!({"type":"command","id":"webhook","async":true,"timeout":10,
        "command":"cat > notification.json; while [ ! -f release ]; do sleep 0.02; done; printf '{}'"});
    hooks(
        &f,
        json!({"Notification":[{"matcher":"permission_prompt","hooks":[webhook]}]}),
    );
    let session = f.session("default").await;
    f.prompt(&session, "write").await;
    let pending = f.pending(&session).await;
    f.runtime
        .reply_permission(&pending.id, cyber_server::runtime::PermissionReply::Once)
        .await
        .unwrap();
    f.settle(&session).await;
    assert_eq!(
        std::fs::read_to_string(f.f.repo.join("a.txt")).unwrap(),
        "safe"
    );
    assert_eq!(
        f.runtime.hook_executions(&session, 10).unwrap()[0].status,
        HookExecutionStatus::Running
    );
    std::fs::write(f.f.repo.join("release"), "").unwrap();
    let receipts = wait_receipts(&f, &session, 1).await;
    assert_eq!(receipts[0].event, "Notification");
    assert_eq!(receipts[0].status, HookExecutionStatus::Completed);
    let event: Value =
        serde_json::from_slice(&std::fs::read(f.f.repo.join("notification.json")).unwrap())
            .unwrap();
    assert_eq!(event["notification_type"], "permission_prompt");
    assert_eq!(event["tool_name"], "write");
}

/// A resolver over `CYBER_HOME` (and optional extra variables) whose project
/// configuration is approved at checkout level, never per handler.
fn approved_project(f: &Flow, extra: &[(&str, String)]) {
    let mut env = std::collections::HashMap::from([(
        "CYBER_HOME".to_string(),
        f.f.dir.path().join("hook-config").display().to_string(),
    )]);
    env.extend(
        extra
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone())),
    );
    let paths = Paths::resolve(&env, f.f.dir.path());
    paths.ensure().unwrap();
    let trust_path = paths.trust_file();
    let home = f.f.dir.path().to_path_buf();
    let load = std::sync::Arc::new(move |location: &std::path::Path| {
        config::load(&LoadRequest {
            location,
            paths: &paths,
            env: &env,
            home: &home,
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .map_err(|error| error.to_string())
    });
    let resolved = load(&f.f.repo).unwrap();
    if let Some(digest) = &resolved.trust.digest {
        TrustStore::new(&trust_path)
            .approve(&resolved.trust.checkout_root, digest)
            .unwrap();
    }
    f.f.host
        .attach_hook_config(load, TrustStore::new(trust_path))
        .unwrap();
}

fn marker(f: &Flow, name: &str) -> Value {
    let path = f.f.dir.path().join(name);
    json!({"type":"command","id":name,"command":format!("cat >/dev/null; printf x >> '{}'", path.display())})
}

fn two_writes() -> Vec<Vec<cyber_llm::adapters::ScriptStep>> {
    vec![
        call("call_a", "write", json!({"path":"a.txt","content":"a"})),
        text("done"),
        call("call_b", "write", json!({"path":"b.txt","content":"b"})),
        text("done"),
        call("call_c", "write", json!({"path":"c.txt","content":"c"})),
        text("done"),
    ]
}

#[tokio::test]
async fn untrusted_handlers_share_one_notice_per_session_and_leave_skipped_receipts() {
    let f = Flow::new(two_writes(), false);
    std::fs::write(
        f.f.repo.join("cyber.jsonc"),
        json!({"hooks":{
            "PreToolUse":[{"matcher":"write","hooks":[marker(&f,"pre-one"),marker(&f,"pre-two")]}],
            "PostToolUse":[{"matcher":"write","hooks":[marker(&f,"post")]}]
        }})
        .to_string(),
    )
    .unwrap();
    approved_project(&f, &[]);
    let mut live = f.runtime.subscribe();
    let session = f.session("bypass").await;
    f.prompt(&session, "write a").await;
    f.settle(&session).await;
    f.prompt(&session, "write b").await;
    f.settle(&session).await;
    assert!(
        f.f.repo.join("b.txt").exists(),
        "skips never block the call"
    );
    for name in ["pre-one", "pre-two", "post"] {
        assert!(!f.f.dir.path().join(name).exists(), "{name} ran untrusted");
    }
    let notices: Vec<_> = hook_notices(&mut live)
        .into_iter()
        .filter(|(id, _)| id == "trust")
        .collect();
    assert_eq!(
        notices,
        [(
            "trust".to_string(),
            "3 untrusted hooks changed \u{2014} review with /hooks".to_string()
        )]
    );
    let receipts = f.runtime.hook_executions(&session, 20).unwrap();
    assert_eq!(receipts.len(), 6, "{receipts:#?}");
    for receipt in &receipts {
        assert_eq!(
            receipt.outcome,
            Some(cyber_core::hooks::HookOutcome::Skipped)
        );
        assert_eq!(receipt.status, HookExecutionStatus::Completed);
        assert!(!receipt.once);
        assert_eq!(
            receipt.decision.as_ref().unwrap().reason.as_deref(),
            Some("untrusted")
        );
    }
    // A second Session gets its own single notice.
    let other = f.session("bypass").await;
    f.prompt(&other, "write a").await;
    f.settle(&other).await;
    let again: Vec<_> = hook_notices(&mut live)
        .into_iter()
        .filter(|(id, _)| id == "trust")
        .collect();
    assert_eq!(again.len(), 1, "{again:?}");
}

#[tokio::test]
async fn managed_only_policy_runs_only_managed_hooks_and_counts_skips_per_event() {
    let f = Flow::new(two_writes(), false);
    let managed = f.f.dir.path().join("managed-policy");
    std::fs::create_dir_all(&managed).unwrap();
    std::fs::write(
        managed.join("cyber-policy.jsonc"),
        json!({
            "hooks":{"PreToolUse":[{"matcher":"write","hooks":[marker(&f,"managed")]}]},
            "policy":{"hooks":{"managed_only":true}}
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        f.f.repo.join("cyber.jsonc"),
        json!({"hooks":{"PreToolUse":[{"matcher":"write","hooks":[marker(&f,"project")]}]}})
            .to_string(),
    )
    .unwrap();
    let env = std::collections::HashMap::from([(
        "CYBER_HOME".to_string(),
        f.f.dir.path().join("hook-config").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, f.f.dir.path());
    paths.ensure().unwrap();
    std::fs::write(
        paths.config.join("cyber.jsonc"),
        json!({"hooks":{"PreToolUse":[{"matcher":"write","hooks":[marker(&f,"global")]}]}})
            .to_string(),
    )
    .unwrap();
    approved_project(
        &f,
        &[("CYBER_MANAGED_POLICY_DIR", managed.display().to_string())],
    );
    let mut live = f.runtime.subscribe();
    let session = f.session("bypass").await;
    f.prompt(&session, "write a").await;
    f.settle(&session).await;
    f.prompt(&session, "write b").await;
    f.settle(&session).await;
    let runs = |name: &str| {
        std::fs::read_to_string(f.f.dir.path().join(name))
            .unwrap_or_default()
            .len()
    };
    assert_eq!(runs("managed"), 2, "the managed hook runs for every call");
    assert_eq!(runs("global"), 0);
    assert_eq!(runs("project"), 0);
    let notices: Vec<_> = hook_notices(&mut live)
        .into_iter()
        .filter(|(id, _)| id == "managed-policy" || id == "trust")
        .collect();
    assert_eq!(
        notices,
        [(
            "managed-policy".to_string(),
            "2 hooks skipped by managed-only policy".to_string()
        )]
    );
    let receipts = f.runtime.hook_executions(&session, 20).unwrap();
    let skipped: Vec<_> = receipts
        .iter()
        .filter(|receipt| receipt.outcome == Some(cyber_core::hooks::HookOutcome::Skipped))
        .collect();
    assert_eq!(skipped.len(), 4, "{receipts:#?}");
    assert!(skipped.iter().all(|receipt| {
        receipt.scope != cyber_core::hooks::HookScope::Managed
            && receipt.decision.as_ref().unwrap().reason.as_deref() == Some("managed-only policy")
    }));
    let ran: Vec<_> = receipts
        .iter()
        .filter(|receipt| receipt.outcome == Some(cyber_core::hooks::HookOutcome::Ok))
        .collect();
    assert_eq!(ran.len(), 2, "{receipts:#?}");
    assert!(
        ran.iter()
            .all(|receipt| receipt.scope == cyber_core::hooks::HookScope::Managed)
    );
}
