mod support;

use cyber_server::runtime::{CreateSession, NoSnapshots, PendingKind, PermissionReply};
use serde_json::{Value, json};
use std::sync::Arc;
use support::Fixture;
use support::flow::{Flow, Scripts, call, text};

fn flow(count: usize, replies: Scripts, interactive: bool) -> Flow {
    let mut turns: Scripts = (0..count)
        .map(|i| {
            call(
                &format!("c{i}"),
                "write",
                json!({"path":format!("result{i}.txt"),"content":"approved"}),
            )
        })
        .collect();
    turns.push(text("done"));
    Flow::with_models(
        Fixture::new(),
        turns,
        interactive,
        Arc::new(NoSnapshots),
        vec![("test/summary", replies)],
    )
}
fn verdict(decision: &str) -> Scripts {
    vec![text(
        &json!({"decision":decision,"reason":"Reviewed repository edit"}).to_string(),
    )]
}

async fn replay_settled(f: &Flow, id: &str, receipt: &cyber_server::runtime::AutoOverrideReceipt) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let events = f.f.store.read_events(id, -1, 100).unwrap().events;
            if events.iter().any(|e| {
                e.kind == "permission.auto_override.changed.1"
                    && e.data["receipt"]["id"] == receipt.id
                    && matches!(e.data["status"].as_str(), Some("settled" | "cancelled"))
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned approval replay must settle");
}

#[tokio::test]
async fn confirmed_override_replays_exact_blocked_input_once_without_another_model_turn() {
    let f = flow(1, verdict("block"), true);
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    let before = f.main.requests().len();
    let receipt = f.runtime.approve_auto(&id).await.unwrap();
    assert!(f.runtime.approve_auto(&id).await.is_err());
    let pending = f.pending(&id).await;
    let PendingKind::Permission(ask) = pending.kind else {
        panic!("permission required")
    };
    assert_eq!(ask.action, "auto_override");
    assert_eq!(
        ask.metadata["classifier_reason"],
        "Reviewed repository edit"
    );
    assert_eq!(
        ask.metadata["input"],
        json!({"path":"result0.txt","content":"approved"})
    );
    assert_eq!(ask.metadata["requires_confirmation"], true);
    assert!(ask.always_patterns.is_empty());
    assert!(!f.f.repo.join("result0.txt").exists());
    f.runtime
        .reply_permission(&pending.id, PermissionReply::Always)
        .await
        .unwrap();
    replay_settled(&f, &id, &receipt).await;
    assert_eq!(f.f.read("result0.txt"), "approved");
    assert_eq!(f.main.requests().len(), before);
    assert!(f.runtime.approve_auto(&id).await.is_err());
    assert_eq!(
        f.runtime.state(&id).await.unwrap().calls[&receipt.call_id].status,
        cyber_server::runtime::CallStatus::Ok
    );
    assert!(
        cyber_tools::permissions::saved::list(&f.f.store, None)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn rejected_override_has_no_effect_and_can_be_confirmed_later() {
    let f = flow(1, verdict("block"), true);
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    let first = f.runtime.approve_auto(&id).await.unwrap();
    let pending = f.pending(&id).await;
    f.runtime
        .reply_permission(
            &pending.id,
            PermissionReply::Reject {
                message: Some("Not now".into()),
            },
        )
        .await
        .unwrap();
    replay_settled(&f, &id, &first).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    let second = f.runtime.approve_auto(&id).await.unwrap();
    let pending = f.pending(&id).await;
    f.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    replay_settled(&f, &id, &second).await;
    assert_eq!(f.f.read("result0.txt"), "approved");
}

#[tokio::test]
async fn override_preserves_current_hard_deny_and_auto_block_rules() {
    for config in [
        json!({"permissions":{"edit":"deny"}}),
        json!({"permissions":{"auto_mode":{"rules":{"always_block":[{"action":"edit","resource":"*"}]}}}}),
    ] {
        let f = flow(1, verdict("block"), true);
        let id = f.session("auto").await;
        f.prompt(&id, "Write a file").await;
        f.settle(&id).await;
        let receipt = f.runtime.approve_auto(&id).await.unwrap();
        let pending = f.pending(&id).await;
        f.f.set_config(config);
        f.runtime
            .reply_permission(&pending.id, PermissionReply::Once)
            .await
            .unwrap();
        replay_settled(&f, &id, &receipt).await;
        assert!(!f.f.repo.join("result0.txt").exists());
        let state = f.runtime.state(&id).await.unwrap();
        assert!(
            state
                .calls
                .get(&receipt.call_id)
                .is_none_or(|c| c.status == cyber_server::runtime::CallStatus::Error)
        );
        assert!(f.runtime.approve_auto(&id).await.is_err());
    }
}

#[tokio::test]
async fn unattended_override_never_executes_and_policy_blocks_are_not_override_candidates() {
    let f = flow(1, verdict("block"), false);
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    let receipt = f.runtime.approve_auto(&id).await.unwrap();
    replay_settled(&f, &id, &receipt).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    let policy = flow(1, verdict("allow"), true);
    policy.f.set_config(json!({"permissions":{"auto_mode":{"rules":{"always_block":[{"action":"edit","resource":"*"}]}}}}));
    let other = policy.session("auto").await;
    policy.prompt(&other, "Write a file").await;
    policy.settle(&other).await;
    assert_eq!(decisions(&policy, &other)[0]["decision"], "block");
    assert!(policy.runtime.approve_auto(&other).await.is_err());
}

#[tokio::test]
async fn pending_override_cancellation_acknowledges_without_effects_or_a_stale_prompt() {
    let f = flow(1, verdict("block"), true);
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    let receipt = f.runtime.approve_auto(&id).await.unwrap();
    let pending = f.pending(&id).await;
    let events = f.f.store.read_events(&id, -1, 100).unwrap().events;
    let mut claim = events
        .iter()
        .find(|e| e.kind == "permission.auto_override.changed.1")
        .unwrap()
        .data
        .clone();
    claim["status"] = json!("consumed");
    assert!(
        f.f.store
            .append(
                &id,
                cyber_store::Expected::Any,
                vec![cyber_store::NewEvent::new(
                    "permission.auto_override.changed.1",
                    claim
                )]
            )
            .is_err()
    );
    f.runtime.interrupt(&id).await.unwrap();
    replay_settled(&f, &id, &receipt).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    assert!(f.runtime.pending_requests(Some(&id)).is_empty());
    assert!(
        f.runtime
            .reply_permission(&pending.id, PermissionReply::Once)
            .await
            .is_err()
    );
    let next = f.runtime.approve_auto(&id).await.unwrap();
    let pending = f.pending(&id).await;
    f.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    replay_settled(&f, &id, &next).await;
    assert_eq!(f.f.read("result0.txt"), "approved");
}

#[tokio::test]
async fn override_cannot_replace_a_parent_manual_approval_added_during_confirmation() {
    let f = flow(1, verdict("block"), true);
    let parent = f.session("auto").await;
    let id = f
        .runtime
        .create_session(CreateSession {
            directory: f.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("auto".into()),
            parent_id: Some(parent.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    let receipt = f.runtime.approve_auto(&id).await.unwrap();
    let confirmation = f.pending(&id).await;
    f.runtime.switch_mode(&parent, "default").await.unwrap();
    f.runtime
        .reply_permission(&confirmation.id, PermissionReply::Once)
        .await
        .unwrap();
    let manual = f.pending(&id).await;
    let PendingKind::Permission(ask) = &manual.kind else {
        panic!("manual permission required")
    };
    assert_eq!(ask.action, "edit");
    assert_ne!(manual.id, confirmation.id);
    assert!(!f.f.repo.join("result0.txt").exists());
    f.runtime
        .reply_permission(
            &manual.id,
            PermissionReply::Reject {
                message: Some("Decline parent approval".into()),
            },
        )
        .await
        .unwrap();
    replay_settled(&f, &id, &receipt).await;
    assert!(!f.f.repo.join("result0.txt").exists());
}

#[tokio::test]
async fn override_preserves_plan_mode_selected_during_confirmation() {
    let f = flow(1, verdict("block"), true);
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    let receipt = f.runtime.approve_auto(&id).await.unwrap();
    let pending = f.pending(&id).await;
    f.runtime.switch_mode(&id, "plan").await.unwrap();
    f.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    replay_settled(&f, &id, &receipt).await;
    assert!(!f.f.repo.join("result0.txt").exists());
}

#[tokio::test]
async fn override_keeps_the_original_apply_patch_tool_without_model_inference() {
    let input =
        json!({"patch":"*** Begin Patch\n*** Add File: patched.txt\n+confirmed\n*** End Patch"});
    let f = Flow::with_models(
        Fixture::new(),
        vec![text("unused")],
        true,
        Arc::new(NoSnapshots),
        vec![
            (
                "test/patch",
                vec![call("c0", "apply_patch", input), text("done")],
            ),
            ("test/summary", verdict("block")),
        ],
    );
    let id = f
        .runtime
        .create_session(CreateSession {
            directory: f.f.repo.display().to_string(),
            model: "test/patch".into(),
            mode: Some("auto".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    f.prompt(&id, "Patch a file").await;
    f.settle(&id).await;
    assert_eq!(decisions(&f, &id)[0]["decision"], "block");
    let before = f.requests("test/patch").len();
    let receipt = f.runtime.approve_auto(&id).await.unwrap();
    let pending = f.pending(&id).await;
    f.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    replay_settled(&f, &id, &receipt).await;
    assert_eq!(f.f.read("patched.txt"), "confirmed\n");
    assert_eq!(f.requests("test/patch").len(), before);
}
fn decisions(flow: &Flow, id: &str) -> Vec<Value> {
    flow.f
        .store
        .read_events(id, -1, 100)
        .unwrap()
        .events
        .into_iter()
        .filter(|e| e.kind == "permission.auto_decided.1")
        .map(|e| e.data)
        .collect()
}

#[tokio::test]
async fn ancestor_auto_block_precedes_child_accept_edits_without_classifier() {
    let f = flow(1, verdict("allow"), false);
    f.f.set_config(json!({"permissions":{"auto_mode":{"rules":{"always_block":[
        {"action":"edit","resource":"*"}
    ]}}}}));
    let parent = f.session("auto").await;
    let child = f
        .runtime
        .create_session(CreateSession {
            directory: f.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("accept-edits".into()),
            parent_id: Some(parent.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    f.prompt(&child, "Write a file").await;
    f.settle(&child).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    let rows = decisions(&f, &child);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["decision"], "block");
    assert!(rows[0]["reason"].as_str().unwrap().contains(&parent));
    assert!(rows[0]["model"].is_null());
    assert!(
        f.output(&child, "c0")
            .await
            .contains("Blocked by auto mode:")
    );
}

#[tokio::test]
async fn non_auto_parent_does_not_activate_auto_configuration() {
    let f = flow(1, verdict("block"), false);
    f.f.set_config(json!({"permissions":{"auto_mode":{"rules":{"always_block":[
        {"action":"edit","resource":"*"}
    ]}}}}));
    let parent = f.session("accept-edits").await;
    let child = f
        .runtime
        .create_session(CreateSession {
            directory: f.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("accept-edits".into()),
            parent_id: Some(parent),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    f.prompt(&child, "Write a file").await;
    f.settle(&child).await;
    assert_eq!(f.f.read("result0.txt"), "approved");
    assert!(decisions(&f, &child).is_empty());
}

#[tokio::test]
async fn grandparent_auto_block_survives_an_intermediate_non_auto_parent() {
    let f = flow(1, verdict("allow"), false);
    f.f.set_config(json!({"permissions":{"auto_mode":{"rules":{"always_block":[
        {"action":"edit","resource":"*"}
    ]}}}}));
    let grandparent = f.session("auto").await;
    let mut parent = grandparent.clone();
    for _ in 0..2 {
        parent = f
            .runtime
            .create_session(CreateSession {
                directory: f.f.repo.display().to_string(),
                model: "test/main".into(),
                mode: Some("accept-edits".into()),
                parent_id: Some(parent),
                ..Default::default()
            })
            .await
            .unwrap()
            .id;
    }
    f.prompt(&parent, "Write a file").await;
    f.settle(&parent).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    let rows = decisions(&f, &parent);
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["reason"].as_str().unwrap().contains(&grandparent));
    assert_eq!(rows[0]["decision"], "block");
    assert!(rows[0]["model"].is_null());
}

#[tokio::test]
async fn classifier_allow_executes_the_actual_tool_without_an_attached_user() {
    let f = flow(1, verdict("allow"), false);
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file in this repository").await;
    f.settle(&id).await;
    assert_eq!(f.f.read("result0.txt"), "approved");
    assert_eq!(decisions(&f, &id)[0]["decision"], "allow");
    assert!(
        cyber_tools::permissions::saved::list(&f.f.store, None)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn classifier_block_is_model_visible_and_precedes_file_effects() {
    let f = flow(1, verdict("block"), false);
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    assert!(
        f.output(&id, "c0")
            .await
            .contains("Blocked by auto mode: Reviewed repository edit")
    );
    assert_eq!(decisions(&f, &id)[0]["decision"], "block");
}

#[tokio::test]
async fn malformed_classifier_reply_falls_back_to_an_interactive_request() {
    let f = flow(1, vec![text("malformed")], true);
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    let pending = f.pending(&id).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    assert_eq!(decisions(&f, &id)[0]["decision"], "fallback");
    f.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    f.settle(&id).await;
    assert_eq!(f.f.read("result0.txt"), "approved");
}

#[tokio::test]
async fn three_blocks_use_interactive_fallback_for_the_fourth_call() {
    let f = flow(4, (0..3).flat_map(|_| verdict("block")).collect(), true);
    let id = f.session("auto").await;
    f.prompt(&id, "Write four files").await;
    let pending = f.pending(&id).await;
    let rows = decisions(&f, &id);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[3]["decision"], "fallback");
    for i in 0..3 {
        assert!(!f.f.repo.join(format!("result{i}.txt")).exists());
    }
    f.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    f.settle(&id).await;
    assert_eq!(f.f.read("result3.txt"), "approved");
}

#[tokio::test]
async fn a_hard_rule_deny_never_calls_the_classifier() {
    let f = flow(1, verdict("allow"), false);
    f.f.set_config(json!({"permissions":{"edit":"deny"}}));
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    assert!(decisions(&f, &id).is_empty());
}

#[tokio::test]
async fn parent_default_approval_cannot_be_replaced_by_child_auto_classification() {
    let f = flow(1, verdict("allow"), false);
    let parent = f.session("default").await;
    let id = f
        .runtime
        .create_session(CreateSession {
            directory: f.f.repo.display().to_string(),
            model: "test/main".into(),
            mode: Some("auto".into()),
            parent_id: Some(parent),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    assert!(decisions(&f, &id).is_empty());
}

fn single(name: &str, input: Value, replies: Scripts, interactive: bool) -> Flow {
    Flow::with_models(
        Fixture::new(),
        vec![call("c0", name, input), text("done")],
        interactive,
        Arc::new(NoSnapshots),
        vec![("test/summary", replies)],
    )
}

#[tokio::test]
async fn unavailable_or_malformed_review_never_executes_unattended() {
    for replies in [Vec::new(), vec![text("not JSON")]] {
        let f = flow(1, replies, false);
        let id = f.session("auto").await;
        f.prompt(&id, "Write a file").await;
        f.settle(&id).await;
        assert!(!f.f.repo.join("result0.txt").exists());
        assert_eq!(decisions(&f, &id)[0]["decision"], "fallback");
        assert!(
            f.output(&id, "c0")
                .await
                .contains("explicit approval required")
        );
    }
}

#[tokio::test]
async fn protected_file_write_keeps_individual_approval_despite_an_allow_reply() {
    let f = single(
        "write",
        json!({"path":"cyber.jsonc","content":"changed"}),
        verdict("allow"),
        true,
    );
    let id = f.session("auto").await;
    f.prompt(&id, "Update configuration").await;
    let pending = f.pending(&id).await;
    let PendingKind::Permission(ask) = &pending.kind else {
        panic!("expected permission");
    };
    assert_eq!(ask.metadata["requires_confirmation"], true);
    assert!(decisions(&f, &id).is_empty());
    assert!(!f.f.repo.join("cyber.jsonc").exists());
    f.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    f.settle(&id).await;
    assert_eq!(f.f.read("cyber.jsonc"), "changed");
}

#[tokio::test]
async fn credential_read_cannot_receive_classifier_approval() {
    let f = single("read", json!({"path":".env"}), verdict("allow"), false);
    f.f.write(".env", "SECRET=private");
    let id = f.session("auto").await;
    f.prompt(&id, "Read .env").await;
    f.settle(&id).await;
    assert!(decisions(&f, &id).is_empty());
    assert!(!f.output(&id, "c0").await.contains("SECRET=private"));
}

#[tokio::test]
async fn critical_removal_is_refused_before_classifier_or_native_dispatch() {
    let f = single(
        "bash",
        json!({"command":"rm -rf ."}),
        verdict("allow"),
        false,
    );
    f.f.write("keep.txt", "user content");
    let id = f.session("auto").await;
    f.prompt(&id, "Delete this checkout").await;
    f.settle(&id).await;
    assert!(decisions(&f, &id).is_empty());
    assert_eq!(f.f.read("keep.txt"), "user content");
    assert!(f.output(&id, "c0").await.contains("Refused:"));
}

#[tokio::test]
async fn force_push_classifier_block_is_returned_before_command_launch() {
    let f = single(
        "bash",
        json!({"command":"git push --force origin main"}),
        vec![text(
            r#"{"decision":"block","reason":"Force push is irreversible"}"#,
        )],
        false,
    );
    let id = f.session("auto").await;
    f.prompt(&id, "Push changes").await;
    f.settle(&id).await;
    assert_eq!(decisions(&f, &id)[0]["decision"], "block");
    assert!(
        f.output(&id, "c0")
            .await
            .contains("Blocked by auto mode: Force push is irreversible")
    );
}

#[tokio::test]
async fn rejected_classifier_decision_commit_refuses_actual_tool_effects() {
    let f = flow(1, verdict("allow"), true);
    f.f.store.transaction(|tx| {
        tx.execute_batch("CREATE TRIGGER reject_auto BEFORE INSERT ON event WHEN NEW.type='permission.auto_decided.1' BEGIN SELECT RAISE(ABORT, 'review write failed'); END;")?;
        Ok(())
    }).unwrap();
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    assert!(decisions(&f, &id).is_empty());
    assert!(f.runtime.pending_requests(Some(&id)).is_empty());
    assert!(f.output(&id, "c0").await.contains("Auto review failed:"));
}

#[tokio::test]
async fn auto_always_block_beats_explicit_allow_without_classifier_call() {
    let f = flow(1, verdict("allow"), false);
    f.f.set_config(json!({"permissions":{"edit":"allow","auto_mode":{"rules":{"always_block":[{"action":"edit","resource":"result*.txt"}]}}}}));
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    let rows = decisions(&f, &id);
    assert_eq!(rows[0]["decision"], "block");
    assert_eq!(rows[0]["model"], Value::Null);
}

#[tokio::test]
async fn auto_always_allow_is_durable_and_does_not_consult_classifier() {
    let f = flow(1, verdict("block"), false);
    f.f.set_config(json!({"permissions":{"auto_mode":{"rules":{"always_allow":[{"action":"edit","resource":"result*.txt"}]}}}}));
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    assert_eq!(f.f.read("result0.txt"), "approved");
    assert_eq!(decisions(&f, &id)[0]["model"], Value::Null);
}

#[tokio::test]
async fn configured_deny_fallback_does_not_open_interactive_approval() {
    let f = flow(1, vec![text("malformed")], true);
    f.f.set_config(json!({"permissions":{"auto_mode":{"fallback":"deny"}}}));
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    assert!(!f.f.repo.join("result0.txt").exists());
    assert!(f.runtime.pending_requests(Some(&id)).is_empty());
    assert_eq!(decisions(&f, &id)[0]["decision"], "fallback");
}

#[tokio::test]
async fn read_only_classification_is_opt_in_and_rules_still_precede_it() {
    for (settings, allowed) in [
        (json!({}), true),
        (json!({"classify_read_only":true}), false),
        (
            json!({"rules":{"always_block":[{"action":"read","resource":"note.txt"}]}}),
            false,
        ),
    ] {
        let f = single("read", json!({"path":"note.txt"}), verdict("block"), false);
        f.f.write("note.txt", "repository note");
        f.f.set_config(json!({"permissions":{"read":"ask","auto_mode":settings}}));
        let id = f.session("auto").await;
        f.prompt(&id, "Read the note").await;
        f.settle(&id).await;
        assert_eq!(
            f.output(&id, "c0").await.contains("repository note"),
            allowed
        );
    }
}

#[tokio::test]
async fn trusted_policy_is_appended_to_the_classifier_system_prompt() {
    let f = flow(1, verdict("allow"), false);
    f.f.set_config(
        json!({"permissions":{"auto_mode":{"policy":"Only update files for ticket 42"}}}),
    );
    let id = f.session("auto").await;
    f.prompt(&id, "Write a file").await;
    f.settle(&id).await;
    let requests = f.requests("test/summary");
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .system
            .iter()
            .any(|text| text.contains("Only update files for ticket 42"))
    );
    assert!(requests[0].tools.is_empty());
}

#[tokio::test]
async fn auto_allow_rule_never_widens_hard_or_individual_confirmation_ceilings() {
    let f = single(
        "write",
        json!({"path":"cyber.jsonc","content":"changed"}),
        verdict("allow"),
        false,
    );
    f.f.set_config(json!({"permissions":{"auto_mode":{"rules":{"always_allow":[{"action":"*","resource":"*"}]}}}}));
    let id = f.session("auto").await;
    f.prompt(&id, "Update configuration").await;
    f.settle(&id).await;
    assert!(!f.f.repo.join("cyber.jsonc").exists());
    assert!(decisions(&f, &id).is_empty());
    assert!(f.requests("test/summary").is_empty());
}

#[tokio::test]
async fn mutating_tool_external_directory_check_cannot_use_read_only_skip() {
    let fixture = Fixture::new();
    let target = fixture.dir.path().join("outside.txt");
    let invocation = call("c0", "write", json!({"path":target,"content":"changed"}));
    let f = Flow::with_models(
        fixture,
        vec![invocation, text("done")],
        false,
        Arc::new(NoSnapshots),
        vec![("test/summary", verdict("block"))],
    );
    let id = f.session("auto").await;
    f.prompt(&id, "Write outside the repository").await;
    f.settle(&id).await;
    assert!(!target.exists());
    let rows = decisions(&f, &id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["action"], "external_directory");
    assert_eq!(rows[0]["decision"], "block");
}

#[tokio::test]
async fn checkout_statistics_reset_preserves_other_scopes_and_audit_history() {
    use cyber_server::runtime::auto_statistics;
    let f = Flow::with_models(
        Fixture::new(),
        vec![
            call(
                "first",
                "write",
                json!({"path":"first.txt","content":"one"}),
            ),
            text("done"),
            call(
                "second",
                "write",
                json!({"path":"second.txt","content":"two"}),
            ),
            text("done"),
        ],
        false,
        Arc::new(NoSnapshots),
        vec![(
            "test/summary",
            vec![verdict("allow")[0].clone(), verdict("block")[0].clone()],
        )],
    );
    let first = f.session("auto").await;
    f.prompt(&first, "Write a file").await;
    f.settle(&first).await;
    let other = f.f.dir.path().join("other-checkout");
    std::fs::create_dir_all(other.join(".git")).unwrap();
    let other = std::fs::canonicalize(other).unwrap();
    let second = f
        .runtime
        .create_session(CreateSession {
            directory: other.display().to_string(),
            model: "test/main".into(),
            mode: Some("auto".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    f.prompt(&second, "Write another file").await;
    f.settle(&second).await;
    let root = f.f.repo.display().to_string();
    let other = other.display().to_string();
    let a = auto_statistics::show(&f.f.store, &root).unwrap();
    let b = auto_statistics::show(&f.f.store, &other).unwrap();
    assert_eq!((a.allowed, a.blocked, a.classifier, a.policy), (1, 0, 1, 0));
    assert_eq!((b.allowed, b.blocked, b.classifier, b.policy), (0, 1, 1, 0));
    let events = f.f.store.read_events(&first, -1, 100).unwrap().events;
    auto_statistics::reset(&f.f.store, &root).unwrap();
    let reset = auto_statistics::show(&f.f.store, &root).unwrap();
    assert_eq!((reset.allowed, reset.blocked, reset.classifier), (0, 0, 0));
    assert!(reset.reset_at.is_some());
    assert_eq!(auto_statistics::show(&f.f.store, &other).unwrap(), b);
    assert_eq!(
        f.f.store.read_events(&first, -1, 100).unwrap().events.len(),
        events.len()
    );
}
