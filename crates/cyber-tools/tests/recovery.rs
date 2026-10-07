//! Read-only reconciliation of file mutations after a crash.

mod support;

use cyber_server::runtime::{CallState, CallStatus, Reconciliation, RetrySafety, ToolHost};
use serde_json::{Value, json};
use support::Fixture;

fn call(name: &str, input: Value) -> CallState {
    CallState {
        structured_output: None,
        call_id: "c1".into(),
        message_id: "m1".into(),
        name: name.into(),
        arguments: input.to_string(),
        input: Some(input),
        retry_safety: RetrySafety::Reconcile,
        attempt: 1,
        status: CallStatus::Dispatched,
        output: None,
    }
}

async fn reconcile(f: &Fixture, name: &str, input: Value) -> Reconciliation {
    f.host
        .reconcile(&f.repo.display().to_string(), &call(name, input))
        .await
}

fn kind(r: &Reconciliation) -> &'static str {
    match r {
        Reconciliation::Succeeded { .. } => "succeeded",
        Reconciliation::NotApplied { .. } => "not_applied",
        Reconciliation::Unknown => "unknown",
    }
}

#[tokio::test]
async fn write_succeeded_only_when_content_matches() {
    let f = Fixture::new();
    f.write("a.txt", "new\r\n");
    assert_eq!(
        kind(&reconcile(&f, "write", json!({"path": "a.txt", "content": "new\n"})).await),
        "succeeded"
    );
    assert_eq!(
        kind(&reconcile(&f, "write", json!({"path": "a.txt", "content": "other\n"})).await),
        "unknown"
    );
}

#[tokio::test]
async fn edit_distinguishes_applied_from_not_applied() {
    let f = Fixture::new();
    f.write("a.rs", "let x = 2;\n");
    let input = json!({"path": "a.rs", "old_string": "x = 1", "new_string": "x = 2"});
    assert_eq!(
        kind(&reconcile(&f, "edit", input.clone()).await),
        "succeeded"
    );
    f.write("a.rs", "let x = 1;\n");
    assert_eq!(kind(&reconcile(&f, "edit", input).await), "not_applied");
}

#[tokio::test]
async fn apply_patch_reports_partial_application_as_unknown() {
    let f = Fixture::new();
    let patch = "*** Begin Patch\n*** Add File: new.txt\n+hello\n*** Update File: keep.txt\n@@\n alpha\n-beta\n+BETA\n*** End Patch";
    f.write("keep.txt", "alpha\nbeta\n");
    assert_eq!(
        kind(&reconcile(&f, "apply_patch", json!({"patch": patch})).await),
        "not_applied"
    );
    f.write("new.txt", "hello\n");
    assert_eq!(
        kind(&reconcile(&f, "apply_patch", json!({"patch": patch})).await),
        "unknown"
    );
    f.write("keep.txt", "alpha\nBETA\n");
    assert_eq!(
        kind(&reconcile(&f, "apply_patch", json!({"patch": patch})).await),
        "succeeded"
    );
}

#[tokio::test]
async fn shell_commands_are_never_reconciled() {
    let f = Fixture::new();
    assert_eq!(
        kind(&reconcile(&f, "bash", json!({"command": "touch x"})).await),
        "unknown"
    );
}
