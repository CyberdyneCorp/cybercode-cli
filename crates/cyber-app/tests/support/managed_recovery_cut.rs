//! Paired keep/rollback and prepared-cancellation receipts created after a real
//! SIGKILL of the managed restore, then killed again during metadata retirement.
//! Partial restores are observed only through source bytes; prepared restores are
//! fenced only by real SQLite writer contention. No row or native hook is replaced.
use super::managed_retirement_cut::{
    Control, Retiring, Scenario, age, journal_path, kill_at_cut, paths_at, reclaim_payloads, resume,
};
use super::*;
use cyber_server::runtime::{
    SnapshotPreparationReceipt, SnapshotRecoveryReceipt, snapshot_reclamation::Receipt,
};
use rusqlite::OptionalExtension;
use std::os::unix::process::ExitStatusExt;

const STAGE: &str = "managed-interruption.json";
const BULK: usize = 1280;

#[tokio::test]
async fn paired_managed_metadata_process_death_resumes_both_owner_bindings() {
    for (scenario, cut) in [
        (Scenario::Keep, "acknowledgement"),
        (Scenario::Rollback, "claim"),
    ] {
        let control = Control(tempfile::tempdir().unwrap());
        let root = interrupt(control.path(), false);
        kill_at_cut(control.path(), cut, scenario, Some(&root)).await;
        assert_eq!(resume(control.path(), cut).await, 2, "{scenario:?}");
    }
}

#[tokio::test]
async fn prepared_cancellation_metadata_process_death_resumes_frozen_plan() {
    for cut in ["acknowledgement", "claim"] {
        let control = Control(tempfile::tempdir().unwrap());
        let root = interrupt(control.path(), true);
        kill_at_cut(control.path(), cut, Scenario::Prepared, Some(&root)).await;
        assert_eq!(resume(control.path(), cut).await, 1);
    }
}

/// Inert in ordinary discovery: builds the managed fixture and confirms a
/// restore that its parent kills partway through source replacement or after
/// prepared images while source admission is fenced.
#[tokio::test]
async fn managed_interrupted_restore_child() {
    let Ok(control) = std::env::var("CYBER_MANAGED_INTERRUPT_CONTROL") else {
        return;
    };
    let prepared = std::env::var("CYBER_MANAGED_INTERRUPT").unwrap() == "prepared";
    let mut f = Fixture::managed().await;
    // The parent and the later retirement child own this fixture after the kill.
    f._temp.disable_cleanup(true);
    let first = images(&f, prepared).await;
    let root = f.paths.data.parent().unwrap();
    let stage = json!({"root":root,"directory":f.directory,"session":f.session});
    std::fs::write(root.join(STAGE), serde_json::to_vec(&stage).unwrap()).unwrap();
    // The parent learns the retained root only after the stage file is complete.
    let staged = Path::new(&control).join(STAGE);
    std::fs::write(
        staged.with_extension("tmp"),
        serde_json::to_vec(&stage).unwrap(),
    )
    .unwrap();
    std::fs::rename(staged.with_extension("tmp"), staged).unwrap();
    let mut request = json!({"message_id":first,"target":"code","action":"commit","expected_directory":f.directory});
    let preview = f
        .client
        .post(
            &format!("/sessions/{}/rewind/preview", f.session),
            request.clone(),
        )
        .await
        .unwrap();
    request["confirmation"] = preview["data"]["confirmation"].clone();
    let reply = f
        .client
        .post(&format!("/sessions/{}/rewind/confirm", f.session), request)
        .await;
    panic!("managed restore finished before its process death: {reply:?}");
}
fn bulk(directory: &Path, bytes: &str) {
    for index in 0..BULK {
        std::fs::write(directory.join(format!("bulk_{index:04}")), bytes).unwrap();
    }
}
/// Two snapshots over many small files leave a real window between exchanges.
/// Prepared images stay below the fixture's snapshot file-size limit.
async fn images(f: &Fixture, prepared: bool) -> String {
    let (before, after) = if prepared {
        (
            format!("b\0{}", "b".repeat(3000)),
            format!("a\0{}", "a".repeat(3000)),
        )
    } else {
        ("before".to_owned(), "after".to_owned())
    };
    bulk(&f.directory, &before);
    std::os::unix::fs::symlink("bulk_0000", f.directory.join("a_link")).unwrap();
    git(&f.directory, &["add", "--all"]);
    let first = f.prompt("before", 1).await;
    bulk(&f.directory, &after);
    std::fs::remove_file(f.directory.join("a_link")).unwrap();
    let target = if prepared { "bulk_1279" } else { "bulk_0001" };
    std::os::unix::fs::symlink(target, f.directory.join("a_link")).unwrap();
    std::fs::write(f.directory.join("aa_removed"), "after-only source").unwrap();
    f.prompt("after", 2).await;
    first
}

/// Run the restore child and SIGKILL it at the selected boundary; returns the
/// retained fixture root.
fn interrupt(control: &Path, prepared: bool) -> PathBuf {
    let log = control.join("restore.log");
    let file = std::fs::File::create(&log).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "managed_recovery_cut::managed_interrupted_restore_child",
            "--nocapture",
        ])
        .env("CYBER_MANAGED_INTERRUPT_CONTROL", control)
        .env(
            "CYBER_MANAGED_INTERRUPT",
            if prepared { "prepared" } else { "partial" },
        )
        .stdout(file.try_clone().unwrap())
        .stderr(file)
        .spawn()
        .unwrap();
    let report = || std::fs::read_to_string(&log).unwrap_or_default();
    let staged = control.join(STAGE);
    if !observe(&mut child, 120, || staged.exists()) {
        reap(&mut child);
        panic!("restore child never staged: {}", report());
    }
    let stage: Value = serde_json::from_slice(&std::fs::read(staged).unwrap()).unwrap();
    let root = PathBuf::from(stage["root"].as_str().unwrap());
    if prepared {
        kill_prepared(&root, &stage, &mut child, &report);
    } else {
        kill_partial(&stage, &mut child, &report);
    }
    root
}
fn observe(child: &mut std::process::Child, seconds: u64, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
    loop {
        if ready() {
            return true;
        }
        if child.try_wait().unwrap().is_some() || std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn reap(child: &mut std::process::Child) -> std::process::ExitStatus {
    let _ = child.kill();
    child.wait().unwrap()
}
fn kill_partial(stage: &Value, child: &mut std::process::Child, report: &dyn Fn() -> String) {
    let directory = PathBuf::from(stage["directory"].as_str().unwrap());
    let read = |name: &str| std::fs::read_to_string(directory.join(name)).ok();
    // Native inventory and durable stages precede the first source exchange.
    let observed = observe(child, 120, || {
        read("bulk_0000").as_deref() == Some("before")
    });
    let status = reap(child);
    assert!(observed, "source exchange was never observed: {}", report());
    assert_eq!(status.signal(), Some(9));
    assert_eq!(read("bulk_0000").as_deref(), Some("before"));
    assert_eq!(
        read("bulk_1279").as_deref(),
        Some("after"),
        "kill did not retain a partial namespace"
    );
}
fn kill_prepared(
    root: &Path,
    stage: &Value,
    child: &mut std::process::Child,
    report: &dyn Fn() -> String,
) {
    let session = stage["session"].as_str().unwrap();
    let db = rusqlite::Connection::open(root.join("data/cyber.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    let mut activity = None;
    let origin = observe(child, 60, || {
        activity = unadmitted_origin(&db, session);
        activity.is_some()
    });
    if !origin {
        reap(child);
        panic!("preparation origin was not observed: {}", report());
    }
    let activity = activity.unwrap();
    // Real SQLite writer contention holds source admission, not preparation I/O.
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let intent = intent_path(root, &db, &activity);
    let prepared = observe(child, 120, || prepared_identities(&intent));
    let status = reap(child);
    db.execute_batch("ROLLBACK").unwrap();
    assert!(
        prepared,
        "prepared identities were not published: {}",
        report()
    );
    assert_eq!(status.signal(), Some(9));
    assert_eq!(latest(&db, &activity)["snapshot_source_admitted"], false);
}
fn unadmitted_origin(db: &rusqlite::Connection, session: &str) -> Option<String> {
    db.query_row("SELECT aggregate_id FROM event e WHERE type='native.activity.changed.1' AND seq=1 AND json_extract(data,'$.session_id')=?1 AND json_extract(data,'$.snapshot_origin') IS NOT NULL AND json_extract(data,'$.snapshot_source_admitted')=0 AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq)", [session], |row| row.get(0)).optional().unwrap()
}
fn latest(db: &rusqlite::Connection, activity: &str) -> Value {
    let raw: String = db
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
            [activity],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}
fn intent_path(root: &Path, db: &rusqlite::Connection, activity: &str) -> PathBuf {
    let record = latest(db, activity);
    assert_eq!(record["snapshot_source_admitted"], false);
    let journal = record["snapshot_origin"]["binding"]["journal_id"]
        .as_str()
        .unwrap();
    journal_path(&root.join("data"), journal).join("intent.json")
}
fn prepared_identities(intent: &Path) -> bool {
    std::fs::read(intent)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|intent| {
            intent["ownership"].is_object()
                && intent["applied"].as_array().is_some_and(Vec::is_empty)
        })
}

/// In the retirement child: settle the interrupted owner through the real
/// client review/confirmation and reclaim its payloads.
pub(super) async fn resolved(scenario: Scenario, root: &Path) -> Retiring {
    let stage: Value = serde_json::from_slice(&std::fs::read(root.join(STAGE)).unwrap()).unwrap();
    let directory = PathBuf::from(stage["directory"].as_str().unwrap());
    let session = stage["session"].as_str().unwrap().to_owned();
    let paths = paths_at(root);
    let app = build(&paths, &directory, root).await;
    let client = Client::embedded(app.embedded()).at(directory.to_str().unwrap());
    let (receipt, endpoint, activity) = if scenario == Scenario::Prepared {
        cancel(&client, &session).await
    } else {
        recover(&client, &session, scenario.action()).await
    };
    age(&app.store, activity);
    reclaim_payloads(&paths, &app.store, &directory, &receipt).await;
    Retiring {
        store: app.store.clone(),
        paths,
        project: root.join("project"),
        directory,
        session,
        receipt,
        endpoint,
        _owner: Box::new(app),
    }
}
async fn recover(client: &Client, session: &str, action: &str) -> (Receipt, String, String) {
    let base = format!("/sessions/{session}/rewind/recovery");
    let journals = client.get(&base).await.unwrap()["data"].clone();
    assert_eq!(journals.as_array().unwrap().len(), 1, "{journals}");
    assert_eq!(journals[0]["filesystem_complete"], false);
    let journal = journals[0]["id"].clone();
    let review = client
        .post(
            &format!("{base}/review"),
            json!({"journal_id":journal,"action":action}),
        )
        .await
        .unwrap()["data"]
        .clone();
    assert!(
        review["conflicts"].as_array().unwrap().is_empty(),
        "{review}"
    );
    let request_id = format!("managed-metadata-cut-{action}");
    let result = client
        .post(
            &format!("{base}/confirm"),
            json!({"journal_id":journal,"action":action,"confirmation":review["confirmation"],"request_id":request_id}),
        )
        .await
        .unwrap()["data"]
        .clone();
    assert_eq!(result["state"], "acknowledged", "{result}");
    let receipt: SnapshotRecoveryReceipt =
        serde_json::from_value(result["receipt"].clone()).unwrap();
    let activity = receipt
        .checkout
        .as_ref()
        .unwrap()
        .resolution_origin
        .activity_id
        .clone();
    let endpoint = format!("{base}/requests/{request_id}");
    (Receipt::Recovery(receipt), endpoint, activity)
}
async fn cancel(client: &Client, session: &str) -> (Receipt, String, String) {
    let base = format!("/sessions/{session}/rewind/preparations");
    let list = client.get(&base).await.unwrap()["data"].clone();
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    let activity = list[0]["activity_id"].as_str().unwrap().to_owned();
    let base = format!("{base}/{activity}");
    let review = client
        .post(&format!("{base}/review"), json!({}))
        .await
        .unwrap()["data"]
        .clone();
    let request_id = "managed-metadata-cut-prepared";
    let reply = client
        .post(
            &format!("{base}/confirm"),
            json!({"confirmation":review["confirmation"],"request_id":request_id}),
        )
        .await
        .unwrap()["data"]
        .clone();
    let receipt: SnapshotPreparationReceipt = serde_json::from_value(reply).unwrap();
    assert_eq!(receipt.activity_id, activity);
    let endpoint = format!("{base}/requests/{request_id}");
    (Receipt::Preparation(Box::new(receipt)), endpoint, activity)
}
