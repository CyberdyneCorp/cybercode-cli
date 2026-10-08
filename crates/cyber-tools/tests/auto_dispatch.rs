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
