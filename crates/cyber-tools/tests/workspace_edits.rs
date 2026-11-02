//! In-process editors participate in the same workspace mutation gate as snapshots
//! and owner binary saves; permission waits happen before acquiring it.
mod support;
use serde_json::json;
use std::time::Duration;
use support::{Fixture, ok};

#[tokio::test]
async fn text_write_waits_for_the_owned_workspace_guard_then_rechecks_and_writes() {
    let fixture = Fixture::new();
    fixture.set_config(json!({"permissions": {"*": "allow"}}));
    let lock = cyber_server::workspace::mutation_lock(&fixture.repo).unwrap();
    let held = lock.lock_owned().await;
    let write = fixture.call(
        "accept-edits",
        "write",
        json!({"path": "queued.txt", "content": "after native settlement"}),
    );
    tokio::pin!(write);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut write)
            .await
            .is_err()
    );
    assert!(!fixture.repo.join("queued.txt").exists());
    drop(held);
    ok(tokio::time::timeout(Duration::from_secs(5), write)
        .await
        .unwrap());
    assert_eq!(fixture.read("queued.txt"), "after native settlement");
}

#[tokio::test]
async fn patch_waits_for_workspace_settlement_and_preserves_a_later_external_edit() {
    let fixture = Fixture::new();
    fixture.set_config(json!({"permissions": {"*": "allow"}}));
    fixture.write("queued.txt", "original\n");
    let lock = cyber_server::workspace::mutation_lock(&fixture.repo).unwrap();
    let held = lock.lock_owned().await;
    let patch = fixture.call("accept-edits", "apply_patch", json!({"patch": "*** Begin Patch\n*** Update File: queued.txt\n@@\n-original\n+proposed\n*** End Patch"}));
    tokio::pin!(patch);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut patch)
            .await
            .is_err()
    );
    assert_eq!(fixture.read("queued.txt"), "original\n");
    fixture.write("queued.txt", "external\n");
    drop(held);
    let outcome = tokio::time::timeout(Duration::from_secs(5), patch)
        .await
        .unwrap();
    assert!(
        !matches!(outcome, cyber_server::runtime::ToolOutcome::Ok(_)),
        "stale patch succeeded: {outcome:?}"
    );
    assert_eq!(fixture.read("queued.txt"), "external\n");
}

fn turn(fixture: &Fixture, mode: &str) -> cyber_server::runtime::TurnContext {
    cyber_server::runtime::TurnContext {
        session_id: "ses_test".into(),
        directory: fixture.repo.display().to_string(),
        agent: "build".into(),
        mode: mode.into(),
        prefers_apply_patch: false,
        rules: serde_json::Value::Null,
        worktree: Default::default(),
    }
}

#[tokio::test]
async fn binary_edit_file_admission_retains_plan_sandbox_and_private_path_ceilings() {
    use cyber_server::runtime::{Asker, FileDecision, ToolHost};
    use tokio_util::sync::CancellationToken;
    let fixture = Fixture::new();
    fixture.set_config(json!({"permissions": {"*": "allow"}}));
    fixture.write("bytes.bin", "source");
    let paths = vec![fixture.repo.join("bytes.bin")];
    let plan = turn(&fixture, "plan");
    assert_eq!(
        fixture.host.file_decisions(&plan, "read", &paths).await,
        vec![FileDecision::Allow]
    );
    assert_eq!(
        fixture.host.file_decisions(&plan, "edit", &paths).await,
        vec![FileDecision::Deny]
    );
    assert!(
        fixture
            .host
            .authorize_file(
                &plan,
                "edit",
                &paths[0],
                Asker::detached(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    let edit = turn(&fixture, "accept-edits");
    assert_eq!(
        fixture.host.file_decisions(&edit, "edit", &paths).await,
        vec![FileDecision::Allow]
    );
    fixture
        .host
        .authorize_file(
            &edit,
            "edit",
            &paths[0],
            Asker::detached(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.host.file_decisions(&edit, "bash", &paths).await,
        vec![FileDecision::Deny]
    );
    let private = fixture.repo.join("private");
    std::fs::create_dir(&private).unwrap();
    let home = fixture.repo.parent().unwrap().join("home");
    cyber_sandbox::hide_credential_file(&home, private.clone());
    let secret = private.join("draft.json");
    std::fs::write(&secret, "unsaved bytes").unwrap();
    assert_eq!(
        fixture
            .host
            .file_decisions(&edit, "edit", std::slice::from_ref(&secret))
            .await,
        vec![FileDecision::Deny]
    );
    assert!(
        fixture
            .host
            .authorize_file(
                &edit,
                "edit",
                &secret,
                Asker::detached(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    let readonly = Fixture::with_policy(
        "bash",
        cyber_sandbox::find_helper(),
        Some("read-only".into()),
    );
    readonly.set_config(json!({"permissions": {"*": "allow"}}));
    readonly.write("bytes.bin", "source");
    let at = readonly.repo.join("bytes.bin");
    let context = turn(&readonly, "accept-edits");
    assert_eq!(
        readonly
            .host
            .file_decisions(&context, "edit", std::slice::from_ref(&at))
            .await,
        vec![FileDecision::Deny]
    );
    assert!(
        readonly
            .host
            .authorize_file(
                &context,
                "edit",
                &at,
                Asker::detached(),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert_eq!(readonly.read("bytes.bin"), "source");
}
