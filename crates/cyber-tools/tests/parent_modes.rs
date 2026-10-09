//! Ancestor Modes constrain child tool admission without replacing child restrictions.
mod support;

use cyber_server::runtime::{CreateSession, PermissionReply};
use serde_json::{Value, json};
use support::flow::{Flow, call, text};

async fn session(flow: &Flow, parent: Option<String>, mode: &str, rules: Value) -> String {
    flow.runtime
        .create_session(CreateSession {
            directory: flow.f.repo.display().to_string(),
            model: "test/main".into(),
            parent_id: parent,
            mode: Some(mode.into()),
            rules: Some(rules),
            ..Default::default()
        })
        .await
        .unwrap()
        .id
}

fn write_flow() -> Flow {
    Flow::new(
        vec![
            call(
                "write",
                "write",
                json!({"path":"child.txt","content":"changed"}),
            ),
            text("done"),
        ],
        false,
    )
}

async fn refused_write(flow: &Flow, child: &str) -> String {
    flow.prompt(child, "write child.txt").await;
    flow.settle(child).await;
    assert!(!flow.f.repo.join("child.txt").exists());
    serde_json::to_string(&flow.runtime.state(child).await.unwrap().calls).unwrap()
}

#[tokio::test]
async fn parent_default_requires_approval_despite_child_bypass() {
    let flow = write_flow();
    let parent = session(&flow, None, "default", Value::Null).await;
    let child = session(&flow, Some(parent), "bypass", Value::Null).await;
    let result = refused_write(&flow, &child).await;
    assert!(result.contains("Denied"), "{result}");
}

#[tokio::test]
async fn parent_plan_refuses_a_child_preapproved_mutation() {
    let flow = write_flow();
    let parent = session(&flow, None, "plan", Value::Null).await;
    let child = session(&flow, Some(parent), "bypass", json!({"edit":"allow"})).await;
    let result = refused_write(&flow, &child).await;
    assert!(result.contains("Plan mode is read-only"), "{result}");
}

#[tokio::test]
async fn grandparent_plan_remains_a_ceiling() {
    let flow = write_flow();
    let grandparent = session(&flow, None, "plan", Value::Null).await;
    let parent = session(&flow, Some(grandparent), "bypass", Value::Null).await;
    let child = session(&flow, Some(parent), "bypass", json!({"edit":"allow"})).await;
    let result = refused_write(&flow, &child).await;
    assert!(result.contains("Plan mode is read-only"), "{result}");
}

#[tokio::test]
async fn parent_dont_ask_refuses_child_accept_edits_auto_approval() {
    let flow = write_flow();
    let parent = session(&flow, None, "dont-ask", Value::Null).await;
    let child = session(&flow, Some(parent), "accept-edits", Value::Null).await;
    let result = refused_write(&flow, &child).await;
    assert!(result.contains("Not pre-approved"), "{result}");
}

#[tokio::test]
async fn child_default_is_not_widened_to_parent_accept_edits() {
    let flow = write_flow();
    let parent = session(&flow, None, "accept-edits", Value::Null).await;
    let child = session(&flow, Some(parent), "default", Value::Null).await;
    let result = refused_write(&flow, &child).await;
    assert!(result.contains("Denied"), "{result}");
}

#[tokio::test]
async fn an_active_parent_retains_its_pinned_plan_mode_after_a_bypass_selection() {
    let flow = Flow::new(
        vec![
            call("parent_read", "read", json!({"path":".env"})),
            call(
                "child_write",
                "write",
                json!({"path":"child.txt","content":"changed"}),
            ),
            text("child done"),
            text("parent done"),
        ],
        true,
    );
    flow.f.write(".env", "secret");
    let parent = session(&flow, None, "plan", Value::Null).await;
    flow.prompt(&parent, "read .env").await;
    let pending = flow.pending(&parent).await;
    flow.runtime.switch_mode(&parent, "bypass").await.unwrap();
    assert_eq!(
        flow.runtime
            .state(&parent)
            .await
            .unwrap()
            .effective_mode(true),
        "plan"
    );
    let child = session(
        &flow,
        Some(parent.clone()),
        "bypass",
        json!({"edit":"allow"}),
    )
    .await;
    flow.prompt(&child, "write child.txt").await;
    flow.settle(&child).await;
    let result = serde_json::to_string(&flow.runtime.state(&child).await.unwrap().calls).unwrap();
    flow.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    flow.settle(&parent).await;
    assert!(!flow.f.repo.join("child.txt").exists());
    assert!(result.contains("Plan mode is read-only"), "{result}");
}

#[tokio::test]
async fn an_unknown_parent_mode_refuses_child_dispatch() {
    let flow = write_flow();
    // Exercise tool admission independently of the earlier memory context gate.
    flow.f.set_config(json!({"memory":{"enabled":false}}));
    let parent = session(&flow, None, "unknown", Value::Null).await;
    let child = session(&flow, Some(parent), "bypass", Value::Null).await;
    let result = refused_write(&flow, &child).await;
    assert!(result.contains("Unknown parent Mode"), "{result}");
}

fn policy(mode: cyber_tools::permissions::Mode) -> cyber_tools::permissions::Policy {
    cyber_tools::permissions::Policy {
        rules: cyber_tools::permissions::defaults(&[], true),
        saved: vec![],
        mode,
        parent_modes: vec![],
        location: "/repo".into(),
        home: "/home/user".into(),
        plan_file: "/repo/.cyber/plans/ses_child.md".into(),
    }
}

#[test]
fn dont_ask_refuses_protected_path_asks_without_creating_an_approval() {
    use cyber_tools::permissions::{Decision, Mode, Request};
    let request = Request {
        action: "edit".into(),
        resources: vec![".git/config".into()],
        mutates: vec!["/repo/.git/config".into()],
        file_edit: true,
        ..Request::default()
    };
    assert_eq!(
        policy(Mode::DontAsk).decide(&request),
        Decision::Deny("Not pre-approved (dont-ask mode)".into())
    );
}

#[tokio::test]
async fn parent_default_allows_a_child_write_after_explicit_approval() {
    let flow = Flow::new(
        vec![
            call(
                "write",
                "write",
                json!({"path":"child.txt","content":"approved"}),
            ),
            text("done"),
        ],
        true,
    );
    let parent = session(&flow, None, "default", Value::Null).await;
    let child = session(&flow, Some(parent), "bypass", Value::Null).await;
    flow.prompt(&child, "write child.txt").await;
    let pending = flow.pending(&child).await;
    let cyber_server::runtime::PendingKind::Permission(ask) = &pending.kind else {
        panic!("expected permission")
    };
    assert_eq!(ask.action, "edit");
    assert!(!flow.f.repo.join("child.txt").exists());
    flow.runtime
        .reply_permission(&pending.id, PermissionReply::Once)
        .await
        .unwrap();
    flow.settle(&child).await;
    assert_eq!(flow.f.read("child.txt"), "approved");
}

fn effect(decision: cyber_tools::permissions::Decision) -> cyber_tools::permissions::Effect {
    use cyber_tools::permissions::{Decision, Effect};
    match decision {
        Decision::Allow => Effect::Allow,
        Decision::Ask => Effect::Ask,
        Decision::Deny(_) => Effect::Deny,
    }
}

#[test]
fn every_mode_pair_intersects_mutation_decisions_and_allows_ordinary_reads() {
    use cyber_tools::permissions::{Effect, Mode, Request};
    let cases = [
        (Mode::Default, Effect::Ask),
        (Mode::AcceptEdits, Effect::Allow),
        (Mode::Plan, Effect::Deny),
        (Mode::Auto, Effect::Ask),
        (Mode::DontAsk, Effect::Deny),
        (Mode::Bypass, Effect::Allow),
    ];
    let write = Request {
        action: "edit".into(),
        resources: vec!["a.txt".into()],
        mutates: vec!["/repo/a.txt".into()],
        file_edit: true,
        ..Request::default()
    };
    let read = Request {
        action: "read".into(),
        resources: vec!["a.txt".into()],
        read_only: true,
        ..Request::default()
    };
    for (child, child_effect) in cases {
        for (parent, parent_effect) in cases {
            let mut p = policy(child);
            p.parent_modes = vec![parent];
            let expected = match (child_effect, parent_effect) {
                (Effect::Deny, _) | (_, Effect::Deny) => Effect::Deny,
                (Effect::Ask, _) | (_, Effect::Ask) => Effect::Ask,
                _ => Effect::Allow,
            };
            assert_eq!(effect(p.decide(&write)), expected, "{child:?}/{parent:?}");
            assert_eq!(
                effect(p.decide(&read)),
                Effect::Allow,
                "{child:?}/{parent:?}"
            );
        }
    }
}

#[test]
fn an_earlier_parent_ask_cannot_mask_a_later_plan_denial() {
    use cyber_tools::permissions::{Decision, Mode, Request};
    let mut p = policy(Mode::Bypass);
    p.parent_modes = vec![Mode::Default, Mode::Plan];
    let write = Request {
        action: "edit".into(),
        resources: vec!["a.txt".into()],
        mutates: vec!["/repo/a.txt".into()],
        file_edit: true,
        ..Request::default()
    };
    assert!(matches!(p.decide(&write), Decision::Deny(reason) if reason.starts_with("Plan mode")));
}

#[test]
fn a_child_plan_file_allowance_still_respects_parent_default_approval() {
    use cyber_tools::permissions::{Decision, Mode, Request};
    let mut p = policy(Mode::Plan);
    p.parent_modes = vec![Mode::Default];
    let write = Request {
        action: "edit".into(),
        resources: vec![".cyber/plans/ses_child.md".into()],
        mutates: vec![p.plan_file.clone()],
        file_edit: true,
        ..Request::default()
    };
    assert_eq!(p.decide(&write), Decision::Ask);
}

#[test]
fn parent_auto_refuses_a_critical_removal_that_child_default_would_ask_for() {
    use cyber_tools::permissions::{Decision, Mode, RemovalRisk, Request};
    let mut p = policy(Mode::Default);
    p.parent_modes = vec![Mode::Auto];
    let removal = Request {
        action: "bash".into(),
        resources: vec!["rm -rf /repo".into()],
        mutates: vec!["/repo".into()],
        removal_risk: Some(RemovalRisk::Critical("/repo".into())),
        ..Request::default()
    };
    assert!(matches!(p.decide(&removal), Decision::Deny(reason) if reason.starts_with("Refused:")));
}

#[test]
fn saved_approvals_cannot_widen_parent_plan() {
    use cyber_tools::permissions::{Decision, Effect, Mode, Request, Rule};
    let mut p = policy(Mode::Bypass);
    p.parent_modes = vec![Mode::Plan];
    p.saved = vec![Rule::new("edit", "*", Effect::Allow, "saved")];
    let write = Request {
        action: "edit".into(),
        resources: vec!["a.txt".into()],
        mutates: vec!["/repo/a.txt".into()],
        file_edit: true,
        ..Request::default()
    };
    assert!(matches!(p.decide(&write), Decision::Deny(reason) if reason.starts_with("Plan mode")));
}
