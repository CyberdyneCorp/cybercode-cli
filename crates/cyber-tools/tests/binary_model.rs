//! Real runtime dispatch into the shared native binary engine, with no provider.
#![cfg(any(target_os = "macos", target_os = "linux"))]
mod support;
use cyber_core::binary::storage::StorageRoot;
use cyber_server::runtime::{CallStatus, NoSnapshots, RetrySafety};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use support::{
    Fixture,
    flow::{Flow, call, text},
};

fn input(hash: &str) -> Value {
    json!({"operation":"patch","path":"bytes.bin","expected_base_sha256":hash,
        "splices":[{"offset":"1","delete_length":"2","expected_hex":"ff80","insert_hex":"00fe01"}]})
}
fn flow(input: Value, config: Value) -> Flow {
    interactive_flow(input, config, false)
}
fn interactive_flow(input: Value, config: Value, interactive: bool) -> Flow {
    let f = Fixture::with_policy(
        "bash",
        cyber_sandbox::find_helper(),
        Some("workspace-write".into()),
    );
    f.set_config(config);
    std::fs::write(f.repo.join("bytes.bin"), [0, 255, 128, 0, 10]).unwrap();
    let flow = Flow::with(
        f,
        vec![call("change", "binary", input), text("done")],
        interactive,
        Arc::new(NoSnapshots),
    );
    flow.runtime
        .configure_binary_storage(Arc::new(StorageRoot::prepare(flow.f.dir.path()).unwrap()))
        .unwrap();
    flow
}

#[tokio::test]
async fn binary_patch_approval_contains_exact_byte_review_and_fresh_revocation_wins() {
    use cyber_server::runtime::{PendingKind, PermissionReply};
    let hash = format!("{:x}", Sha256::digest([0, 255, 128, 0, 10]));
    for revoke in [false, true] {
        let flow = interactive_flow(
            input(&hash),
            json!({"permissions":{"read":"allow","edit":"ask"}}),
            true,
        );
        let id = flow.session("default").await;
        flow.prompt(&id, "review and apply").await;
        let pending = flow.pending(&id).await;
        let PendingKind::Permission(ask) = pending.kind else {
            panic!("byte edit approval required")
        };
        assert_eq!(ask.action, "edit");
        assert_eq!(ask.metadata["byte_review"]["draft"]["base_sha256"], hash);
        assert_eq!(
            ask.metadata["byte_review"]["draft"]["ranges"][0]["before_hex"],
            "ff80"
        );
        assert_eq!(
            ask.metadata["byte_review"]["draft"]["ranges"][0]["after_hex"],
            "00fe01"
        );
        if revoke {
            flow.f
                .set_config(json!({"permissions":{"read":"allow","edit":"deny"}}));
        }
        flow.runtime
            .reply_permission(&pending.id, PermissionReply::Once)
            .await
            .unwrap();
        flow.settle(&id).await;
        let state = flow.runtime.state(&id).await.unwrap();
        assert_eq!(
            state.calls["change"].status,
            if revoke {
                CallStatus::Error
            } else {
                CallStatus::Ok
            },
            "{:?}",
            state.calls["change"]
        );
        assert_eq!(
            std::fs::read(flow.f.repo.join("bytes.bin")).unwrap(),
            if revoke {
                vec![0, 255, 128, 0, 10]
            } else {
                vec![0, 0, 254, 1, 0, 10]
            }
        );
        flow.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn stopping_during_binary_review_leaves_the_source_unchanged() {
    let hash = format!("{:x}", Sha256::digest([0, 255, 128, 0, 10]));
    let flow = interactive_flow(
        input(&hash),
        json!({"permissions":{"read":"allow","edit":"ask"}}),
        true,
    );
    let id = flow.session("default").await;
    flow.prompt(&id, "review patch").await;
    let _pending = flow.pending(&id).await;
    flow.runtime.interrupt(&id).await.unwrap();
    flow.settle(&id).await;
    assert_eq!(
        std::fs::read(flow.f.repo.join("bytes.bin")).unwrap(),
        [0, 255, 128, 0, 10]
    );
    assert!(
        flow.runtime
            .binary_storage()
            .unwrap()
            .session_saved_references(&id, &flow.f.repo, || false)
            .unwrap()
            .is_empty()
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn dispatched_plan_read_returns_bounded_bytes_without_a_private_draft() {
    let flow = flow(
        json!({"operation":"read","path":"bytes.bin","limit":3}),
        json!({"permissions":{"read":"allow","edit":"deny"}}),
    );
    let id = flow.session("plan").await;
    flow.prompt(&id, "inspect bytes").await;
    flow.settle(&id).await;
    let state = flow.runtime.state(&id).await.unwrap();
    let call = &state.calls["change"];
    assert_eq!(call.status, CallStatus::Ok, "{call:?}");
    let page: Value = serde_json::from_str(call.output.as_ref().unwrap()).unwrap();
    assert_eq!(page["hex"], "00ff80");
    assert_eq!(page["length"], "3");
    assert!(
        std::fs::read_dir(flow.runtime.binary_storage().unwrap().path())
            .unwrap()
            .next()
            .is_none()
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn exact_model_binary_patch_installs_bytes_and_publishes_saved_history() {
    let hash = format!("{:x}", Sha256::digest([0, 255, 128, 0, 10]));
    let flow = flow(
        input(&hash),
        json!({"permissions":{"read":"allow","edit":"allow"}}),
    );
    let id = flow.session("default").await;
    flow.prompt(&id, "apply the exact binary patch").await;
    flow.settle(&id).await;
    let state = flow.runtime.state(&id).await.unwrap();
    let call = &state.calls["change"];
    assert_eq!(call.status, CallStatus::Ok, "{call:?}");
    assert_eq!(call.retry_safety, RetrySafety::Never);
    let receipt: Value = serde_json::from_str(call.output.as_ref().unwrap()).unwrap();
    assert_eq!(receipt["operation"]["outcome"], "acknowledged");
    assert_eq!(receipt["operation"]["file_effect"], "changed");
    assert_eq!(
        std::fs::read(flow.f.repo.join("bytes.bin")).unwrap(),
        [0, 0, 254, 1, 0, 10]
    );
    let storage = flow.runtime.binary_storage().unwrap();
    let refs = storage
        .session_saved_references(&id, &flow.f.repo, || false)
        .unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].version.before_sha256, hash);
    assert_eq!(
        refs[0].version.after_sha256,
        format!("{:x}", Sha256::digest([0, 0, 254, 1, 0, 10]))
    );
    flow.runtime.shutdown().await;
}

#[tokio::test]
async fn binary_model_patch_refuses_plan_denied_reads_denied_edits_stale_hash_and_wrong_bytes() {
    let hash = format!("{:x}", Sha256::digest([0, 255, 128, 0, 10]));
    let allow = json!({"permissions":{"read":"allow","edit":"allow"}});
    let mut wrong_bytes = input(&hash);
    wrong_bytes["splices"][0]["expected_hex"] = json!("ffff");
    for (mode, config, patch) in [
        ("plan", allow.clone(), input(&hash)),
        (
            "default",
            json!({"permissions":{"read":"deny","edit":"allow"}}),
            input(&hash),
        ),
        (
            "default",
            json!({"permissions":{"read":"allow","edit":"deny"}}),
            input(&hash),
        ),
        ("default", allow.clone(), input(&"0".repeat(64))),
        ("default", allow, wrong_bytes),
    ] {
        let flow = flow(patch, config);
        let id = flow.session(mode).await;
        flow.prompt(&id, "apply the exact binary patch").await;
        flow.settle(&id).await;
        let state = flow.runtime.state(&id).await.unwrap();
        assert_eq!(
            state.calls["change"].status,
            CallStatus::Error,
            "{mode}: {:?}",
            state.calls["change"]
        );
        assert_eq!(
            std::fs::read(flow.f.repo.join("bytes.bin")).unwrap(),
            [0, 255, 128, 0, 10]
        );
        assert!(
            flow.runtime
                .binary_storage()
                .unwrap()
                .session_saved_references(&id, &flow.f.repo, || false)
                .unwrap()
                .is_empty()
        );
        flow.runtime.shutdown().await;
    }
}
