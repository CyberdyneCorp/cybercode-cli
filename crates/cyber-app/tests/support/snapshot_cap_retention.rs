//! 3.3 through a real App: visible cap unavailability, cap-pressure retirement
//! that never touches unexpired evidence, and staged/branch roots past retention.
use super::*;

fn set_app_cap(f: &Fixture, cap: u64) {
    let path = f.paths.config.join("cyber.jsonc");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["snapshots"] = json!({"fallback_max_bytes": cap});
    std::fs::write(path, config.to_string()).unwrap();
}
async fn session(f: &Fixture) -> String {
    f.client
        .post(
            "/sessions",
            json!({"model":"test/main","mode":"accept-edits"}),
        )
        .await
        .unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn user_ids(f: &Fixture, session: &str) -> Vec<String> {
    f.client
        .get(&format!("/sessions/{session}/messages?order=asc"))
        .await
        .unwrap()["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["kind"] == "user")
        .map(|message| message["id"].as_str().unwrap().to_owned())
        .collect()
}
fn selection(f: &Fixture, message: &str, target: &str, action: &str) -> Value {
    json!({"message_id":message,"target":target,"action":action,"expected_directory":f.project})
}
async fn preview(f: &Fixture, session: &str, selection: &Value) -> Result<Value, String> {
    f.client
        .post(
            &format!("/sessions/{session}/rewind/preview"),
            selection.clone(),
        )
        .await
        .map_err(|error| error.to_string())
}
async fn confirm(f: &Fixture, session: &str, mut selection: Value) {
    let review = preview(f, session, &selection).await.unwrap();
    selection["confirmation"] = review["data"]["confirmation"].clone();
    f.client
        .post(&format!("/sessions/{session}/rewind/confirm"), selection)
        .await
        .unwrap();
}
fn roots(f: &Fixture) -> std::collections::BTreeSet<String> {
    f.app.store.read(records::references).unwrap()
}
fn age_all_events(f: &Fixture) {
    f.app
        .store
        .transaction(|db| {
            db.execute(
                "UPDATE event SET time=time-?1",
                rusqlite::params![8i64 * 86400000],
            )?;
            Ok(())
        })
        .unwrap();
}
fn namespace(f: &Fixture) -> PathBuf {
    f.journal.parent().unwrap().parent().unwrap().to_path_buf()
}
fn manifest(f: &Fixture, root: &str) -> PathBuf {
    namespace(f)
        .join("manifests")
        .join(root.strip_prefix("copy:").unwrap())
}

#[tokio::test]
async fn lowered_cap_disables_code_rewind_visibly_and_keeps_conversation_rewind() {
    let f = Fixture::new().await;
    let first = user_ids(&f, &f.session).await[0].clone();
    let sources = image_bytes(&f.project);
    let inference = f.model.requests().len();
    set_app_cap(&f, 1);
    // The first refusal reports the capture error; the repeated one must still
    // name the cap through the recorded unavailable reason, never a generic text.
    for target in ["code", "both", "code"] {
        let error = preview(&f, &f.session, &selection(&f, &first, target, "commit"))
            .await
            .unwrap_err();
        assert!(
            error.contains("Non-Git snapshot storage cap reached")
                && error.contains("conversation rewind remains available"),
            "{target}: {error}"
        );
    }
    let review = preview(
        &f,
        &f.session,
        &selection(&f, &first, "conversation", "commit"),
    )
    .await
    .unwrap();
    assert!(review["data"]["code"].is_null());
    confirm(
        &f,
        &f.session,
        selection(&f, &first, "conversation", "commit"),
    )
    .await;
    assert!(user_ids(&f, &f.session).await.is_empty());
    assert_eq!(image_bytes(&f.project), sources);
    assert_eq!(f.model.requests().len(), inference);
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn cap_pressure_retirement_frees_only_acknowledged_payloads_then_capture_succeeds() {
    let f = Fixture::new().await;
    let later = Restore::new(&f.app, &f.paths, &f.project, Some("later!\n".repeat(4096))).await;
    let later_evidence = image_bytes(&later.journal);
    assert!(later_evidence.contains_key("stage_0") && later_evidence.contains_key("intent.json"));
    let lookup = |receipt: &SnapshotRestoreReceipt| {
        let receipt = receipt.clone();
        f.app
            .store
            .read(move |db| records::lookup(db, &receipt))
            .unwrap()
    };
    let namespace = namespace(&f);
    let inference = f.model.requests().len();
    std::fs::write(f.project.join("zz_new"), vec![83u8; 8192]).unwrap();
    // Nothing is past retention yet: cap pressure must refuse rather than make room.
    *f.config.lock().unwrap() = json!({"snapshots":{"retention_days":7,
        "fallback_max_bytes":private_bytes(&namespace) + 4096}});
    let refused = f.snapshots.track(f.project.to_str().unwrap()).await;
    assert!(refused.unwrap_err().contains("cap reached"));
    assert!(f.record().is_none() && f.journal.join("stage_0").exists());
    assert!(lookup(&later.receipt).is_none());
    assert_eq!(image_bytes(&later.journal), later_evidence);
    // Only the aged, settled restore becomes eligible: payload first, then metadata.
    f.age();
    assert!(f.reclaim().await.unwrap() > 0);
    assert!(f.record().unwrap().completed && !f.record().unwrap().retired);
    let cap = private_bytes(&namespace) + 4096;
    *f.config.lock().unwrap() = json!({"snapshots":{"retention_days":7,"fallback_max_bytes":cap}});
    let captured = f
        .snapshots
        .track(f.project.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(captured.tree.starts_with("copy:"));
    assert!(private_bytes(&namespace) <= cap);
    assert!(f.record().unwrap().retired && !f.journal.exists());
    assert!(lookup(&later.receipt).is_none());
    assert_eq!(image_bytes(&later.journal), later_evidence);
    assert_eq!(
        std::fs::read(f.project.join("file_0")).unwrap(),
        "before\n".repeat(4096).as_bytes()
    );
    assert_eq!(f.model.requests().len(), inference);
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn collection_preserves_staged_undo_and_inactive_branch_roots_past_retention() {
    let f = Fixture::new().await;
    // Inactive branch: the second turn's roots leave the active path.
    let branch = session(&f).await;
    std::fs::write(f.project.join("branch.txt"), "branch one\n").unwrap();
    prompt(&f.app, &f.client, &branch, "branch one", 1).await;
    let before_branch_turn = roots(&f);
    std::fs::write(f.project.join("branch.txt"), "branch-only bytes\n").unwrap();
    prompt(&f.app, &f.client, &branch, "branch two", 2).await;
    let branch_only: Vec<String> = roots(&f).difference(&before_branch_turn).cloned().collect();
    assert!(!branch_only.is_empty());
    let first = user_ids(&f, &branch).await[0].clone();
    confirm(&f, &branch, selection(&f, &first, "conversation", "commit")).await;
    assert!(user_ids(&f, &branch).await.is_empty());
    std::fs::write(f.project.join("branch.txt"), "later owner\n").unwrap();
    // Staged undo: an unrelated owner file makes the stage baseline a unique root.
    let staged = session(&f).await;
    std::fs::write(f.project.join("stage.txt"), "stage one\n").unwrap();
    prompt(&f.app, &f.client, &staged, "stage one", 1).await;
    std::fs::write(f.project.join("stage.txt"), "stage two\n").unwrap();
    prompt(&f.app, &f.client, &staged, "stage two", 2).await;
    std::fs::write(f.project.join("notes.txt"), "owner notes\n").unwrap();
    let before_stage = roots(&f);
    let first = user_ids(&f, &staged).await[0].clone();
    confirm(&f, &staged, selection(&f, &first, "code", "stage")).await;
    let stage_only: Vec<String> = roots(&f).difference(&before_stage).cloned().collect();
    assert!(!stage_only.is_empty());
    assert_eq!(
        std::fs::read_to_string(f.project.join("stage.txt")).unwrap(),
        "stage one\n"
    );
    // A true orphan: captured, never referenced by any durable record.
    std::fs::write(f.project.join("orphan.txt"), "orphan bytes\n").unwrap();
    let orphan = f
        .snapshots
        .track(f.project.to_str().unwrap())
        .await
        .unwrap()
        .unwrap()
        .tree;
    std::fs::remove_file(f.project.join("orphan.txt")).unwrap();
    let referenced = roots(&f);
    assert!(!referenced.contains(&orphan));
    age_all_events(&f);
    age_fallback_collection_images(&namespace(&f));
    // Each pass settles one journal per Location; collection waits until none remain.
    let journals = || {
        std::fs::read_dir(namespace(&f).join("journals"))
            .unwrap()
            .count()
    };
    assert_eq!(journals(), 2);
    f.snapshots.gc().await;
    assert_eq!(journals(), 1);
    assert!(manifest(&f, &orphan).exists());
    f.snapshots.gc().await;
    assert_eq!(journals(), 0);
    for root in branch_only.iter().chain(&stage_only).chain(&referenced) {
        assert!(
            manifest(&f, root).exists(),
            "referenced {root} was collected"
        );
    }
    assert!(!manifest(&f, &orphan).exists(), "orphan manifest survived");
    // The protected boundary stays usable: clearing the stage restores the baseline.
    confirm(&f, &staged, selection(&f, &first, "code", "clear")).await;
    assert_eq!(
        std::fs::read_to_string(f.project.join("stage.txt")).unwrap(),
        "stage two\n"
    );
    assert_eq!(
        std::fs::read_to_string(f.project.join("notes.txt")).unwrap(),
        "owner notes\n"
    );
    f.app.runtime.shutdown().await;
}
