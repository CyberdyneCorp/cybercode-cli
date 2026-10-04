//! Backup bundles: create, verify, detect tampering, restore.

use cyber_app::backup;
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_server::runtime::Runtime;
use cyber_store::{Expected, NewEvent, Store, StoreOptions};
use serde_json::json;

fn paths(root: &std::path::Path) -> Paths {
    Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    }
}

/// A database with one session's events and some artifacts.
fn seeded(root: &std::path::Path) -> (Paths, std::path::PathBuf) {
    let p = paths(root);
    p.ensure().unwrap();
    let db = p.data.join("cyber.db");
    let store = Store::open(StoreOptions::new(
        DatabaseLocation::File(db.clone()),
        Runtime::registry(),
    ))
    .unwrap();
    let info = json!({ "info": { "id": "ses_1", "title": "t", "directory": "/r", "parent_id": null, "agent": "build", "model": "m/x", "mode": "default", "created_ms": 1 } });
    store
        .append(
            "ses_1",
            Expected::Seq(-1),
            vec![NewEvent::new("session.created.1", info)],
        )
        .unwrap();
    drop(store);
    std::fs::write(p.data.join("tool-output/tool_1"), "big output").unwrap();
    std::fs::create_dir_all(p.data.join("snapshot/prj_1/abc")).unwrap();
    std::fs::write(
        p.data.join("snapshot/prj_1/abc/HEAD"),
        "ref: refs/heads/main\n",
    )
    .unwrap();
    (p, db)
}

#[test]
fn a_bundle_verifies_and_restores_database_and_artifacts() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, db) = seeded(tmp.path());
    let bundle = tmp.path().join("bundle");
    let manifest = backup::create(&p, &db, &bundle, true).unwrap();
    assert_eq!(manifest.artifacts, vec!["tool-output", "snapshot"]);
    assert!(
        manifest
            .files
            .iter()
            .any(|f| f.path == "artifacts/tool-output/tool_1")
    );
    backup::verify(&bundle).unwrap();

    // Lose everything, then restore.
    std::fs::write(p.data.join("tool-output/tool_1"), "changed after backup").unwrap();
    std::fs::remove_file(&db).unwrap();
    let kept = backup::restore(&p, &db, &bundle).unwrap();
    assert!(
        kept.iter()
            .any(|k| k.to_string_lossy().contains("tool-output.pre-restore-"))
    );
    assert_eq!(
        std::fs::read_to_string(p.data.join("tool-output/tool_1")).unwrap(),
        "big output"
    );
    let store = Store::open(StoreOptions::new(
        DatabaseLocation::File(db),
        Runtime::registry(),
    ))
    .unwrap();
    assert_eq!(store.read_events("ses_1", -1, 10).unwrap().events.len(), 1);
}

#[test]
fn tampered_bundles_fail_verification_and_are_not_restored() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, db) = seeded(tmp.path());
    let bundle = tmp.path().join("bundle");
    backup::create(&p, &db, &bundle, true).unwrap();
    std::fs::write(bundle.join("artifacts/tool-output/tool_1"), "tampered").unwrap();
    std::fs::write(bundle.join("extra.txt"), "x").unwrap();
    let problems = backup::verify(&bundle).unwrap_err();
    assert!(
        problems.contains(&"changed: artifacts/tool-output/tool_1".to_string()),
        "{problems:?}"
    );
    assert!(problems.contains(&"unexpected: extra.txt".to_string()));
    assert!(
        backup::restore(&p, &db, &bundle)
            .unwrap_err()
            .contains("failed verification")
    );
    assert!(db.exists(), "nothing was moved aside");
}

#[test]
fn restore_refuses_while_a_server_is_registered() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, db) = seeded(tmp.path());
    let single = tmp.path().join("copy.db");
    cyber_store::backup::backup(&db, &single).unwrap();
    std::fs::write(
        p.state.join("server.json"),
        r#"{"id":"srv_1","version":"0","url":"http://x","socket":null,"pid":42}"#,
    )
    .unwrap();
    assert!(
        backup::restore(&p, &db, &single)
            .unwrap_err()
            .contains("pid 42")
    );
    std::fs::remove_file(p.state.join("server.json")).unwrap();
    backup::restore(&p, &db, &single).unwrap();
}

#[test]
fn a_failed_backup_leaves_no_partial_bundle() {
    let tmp = tempfile::tempdir().unwrap();
    let p = paths(tmp.path());
    let bundle = tmp.path().join("bundle");
    assert!(backup::create(&p, &tmp.path().join("missing.db"), &bundle, true).is_err());
    assert!(!bundle.exists());
}
