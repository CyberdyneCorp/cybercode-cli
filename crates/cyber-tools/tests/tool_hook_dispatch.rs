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
    assert!(f.runtime.hook_executions(&first, 10).unwrap().is_empty());
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
async fn unavailable_async_definition_does_not_deduplicate_a_supported_guard() {
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
    let receipts = f.runtime.hook_executions(&session, 10).unwrap();
    assert_eq!(receipts.len(), 2);
    assert!(
        receipts
            .iter()
            .any(|record| record.outcome == Some(cyber_core::hooks::HookOutcome::Error))
    );
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
    let refusal = json!({"type":"command","id":"refusal","timeout":10,"fail_closed":true,"command":"cat >/dev/null; while [ ! -f started-a ]; do sleep 0.02; done; printf '{\"additional_context\":\"not admitted\"}'"});
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
            .contains("hook context/continuation admission unavailable")
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
