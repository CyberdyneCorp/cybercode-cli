#![cfg(any(target_os = "macos", target_os = "linux"))]
//! Real App restores/receipts and native private payloads; no source writes by cleanup.
#[path = "support/scripted_model.rs"]
mod scripted_model;
use cyber_app::{App, AppOptions};
use cyber_client::Client;
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_server::runtime::{SnapshotRestoreReceipt, Snapshots, snapshot_reclamation as records};
use cyber_snapshot::{
    GitSnapshots,
    reclamation::{Collect, Delete, Policy, Settlement},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Fixture {
    _temp: tempfile::TempDir,
    app: App,
    client: Client,
    model: scripted_model::ScriptedModel,
    paths: Paths,
    project: PathBuf,
    session: String,
    receipt: SnapshotRestoreReceipt,
    journal: PathBuf,
    config: Arc<Mutex<Value>>,
    snapshots: Arc<GitSnapshots>,
}
impl Fixture {
    async fn new() -> Self {
        Self::in_directory(tempfile::tempdir().unwrap()).await
    }
    async fn in_directory(temp: tempfile::TempDir) -> Self {
        let root = temp.path().canonicalize().unwrap();
        let paths = Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        paths.ensure().unwrap();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        let model = scripted_model::ScriptedModel::start(|_| json!({"content":"Done."}));
        std::fs::write(paths.config.join("cyber.jsonc"), model.config().to_string()).unwrap();
        let app = App::build(AppOptions {
            paths: paths.clone(),
            home: root.join("home"),
            database: DatabaseLocation::File(paths.data.join("cyber.db")),
            default_directory: project.clone(),
            sandbox_policy: None,
            snapshots: true,
            interactive: true,
            password: None,
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: None,
        })
        .await
        .unwrap();
        let client = Client::embedded(app.embedded()).at(project.to_str().unwrap());
        let session = client
            .post(
                "/sessions",
                json!({"model":"test/main","mode":"accept-edits"}),
            )
            .await
            .unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        for index in 0..2 {
            std::fs::write(
                project.join(format!("file_{index}")),
                "before\n".repeat(4096),
            )
            .unwrap();
        }
        prompt(&app, &client, &session, "before snapshot", 1).await;
        let messages = client
            .get(&format!("/sessions/{session}/messages?order=asc"))
            .await
            .unwrap();
        let first = messages["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["kind"] == "user")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        for index in 0..2 {
            std::fs::write(
                project.join(format!("file_{index}")),
                "after!\n".repeat(4096),
            )
            .unwrap();
        }
        prompt(&app, &client, &session, "after snapshot", 2).await;
        let mut selection = json!({"message_id":first,"target":"code","action":"commit","expected_directory":project});
        let preview = client
            .post(
                &format!("/sessions/{session}/rewind/preview"),
                selection.clone(),
            )
            .await
            .unwrap();
        selection["confirmation"] = preview["data"]["confirmation"].clone();
        client
            .post(&format!("/sessions/{session}/rewind/confirm"), selection)
            .await
            .unwrap();
        let owner = session.clone();
        let receipt: SnapshotRestoreReceipt = app.store.read(move |db| {
            let raw: String = db.query_row("SELECT json_extract(data,'$.snapshot_restore_receipt') FROM event WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1 AND json_extract(data,'$.snapshot_restore_receipt') IS NOT NULL ORDER BY rowid DESC LIMIT 1", [owner], |row| row.get(0))?;
            Ok(serde_json::from_str(&raw).unwrap())
        }).unwrap();
        let journal = std::fs::read_dir(paths.data.join("snapshot/global"))
            .unwrap()
            .map(|entry| {
                entry
                    .unwrap()
                    .path()
                    .join("journals")
                    .join(&receipt.origin.binding.journal_id)
            })
            .find(|path| path.exists())
            .unwrap();
        assert!(journal.join("stage_0").exists() && journal.join("stage_1").exists());
        let config = Arc::new(Mutex::new(json!({"snapshots":{"retention_days":7}})));
        let snapshots = maintenance(
            &paths,
            config.clone(),
            cyber_app::snapshot_reclamation::policy(app.store.clone()),
        );
        Self {
            _temp: temp,
            app,
            client,
            model,
            paths,
            project,
            session,
            receipt,
            journal,
            config,
            snapshots,
        }
    }
    fn age(&self) {
        let owner = self.receipt.origin.activity_id.clone();
        // Only the retention clock is aged. Receipt/native ownership bytes and
        // original event sequence remain those of the actual committed restore.
        self.app
            .store
            .transaction(move |db| {
                db.execute(
                    "UPDATE event SET time=time-?1 WHERE aggregate_id=?2",
                    rusqlite::params![8i64 * 86400000, owner],
                )?;
                Ok(())
            })
            .unwrap();
    }
    async fn reclaim(&self) -> Result<u64, String> {
        self.snapshots.reclaim(self.project.to_str().unwrap()).await
    }
    fn record(&self) -> Option<records::Record> {
        let receipt = self.receipt.clone();
        self.app
            .store
            .read(move |db| records::lookup(db, &receipt))
            .unwrap()
    }
}
fn maintenance(
    paths: &Paths,
    config: Arc<Mutex<Value>>,
    policy: Arc<dyn Policy>,
) -> Arc<GitSnapshots> {
    GitSnapshots::with_reclamation(
        paths.data.clone(),
        Arc::new(move |_| config.lock().unwrap().clone()),
        vec![
            paths.config.clone(),
            paths.state.clone(),
            paths.cache.clone(),
            paths.tmp.clone(),
        ],
        None,
        Some(policy),
    )
}
async fn prompt(app: &App, client: &Client, session: &str, text: &str, expected: usize) {
    client
        .post(
            &format!("/sessions/{session}/prompt"),
            json!({"parts":[{"type":"text","text":text}]}),
        )
        .await
        .unwrap();
    for _ in 0..1000 {
        let messages = client
            .get(&format!("/sessions/{session}/messages"))
            .await
            .unwrap();
        let answered = messages["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["kind"] == "assistant")
            .count();
        if answered >= expected && !app.runtime.is_running(session) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("scripted prompt did not finish");
}

#[tokio::test]
async fn acknowledged_ordinary_payloads_are_reclaimed_without_source_or_reference_image_changes() {
    use sha2::Digest;
    let f = Fixture::new().await;
    let legacy_frame = serde_json::to_vec(&("snapshot-reclamation-v1", &f.receipt)).unwrap();
    let legacy_id = format!("snapshot_reclaim_{:x}", sha2::Sha256::digest(legacy_frame));
    assert_eq!(records::Plan::id(&f.receipt).unwrap(), legacy_id);
    assert_eq!(
        serde_json::to_value(records::Receipt::Restore(f.receipt.clone())).unwrap(),
        serde_json::to_value(&f.receipt).unwrap()
    );
    f.age();
    let messages = f
        .client
        .get(&format!("/sessions/{}/messages", f.session))
        .await
        .unwrap();
    let inference = f.model.requests().len();
    let intent = std::fs::read(f.journal.join("intent.json")).unwrap();
    let files = f.journal.parent().unwrap().parent().unwrap().join("files");
    let images = image_bytes(&files);
    let target = f.project.parent().unwrap().join("later-owner");
    std::fs::write(&target, "later owner bytes").unwrap();
    std::fs::remove_file(f.project.join("file_0")).unwrap();
    std::os::unix::fs::symlink(&target, f.project.join("file_0")).unwrap();
    assert_eq!(
        f.reclaim().await.unwrap(),
        2 * ("after!\n".len() * 4096) as u64
    );
    assert!(!f.journal.join("stage_0").exists() && !f.journal.join("stage_1").exists());
    assert_eq!(
        std::fs::read(f.journal.join("intent.json")).unwrap(),
        intent
    );
    assert_eq!(image_bytes(&files), images);
    assert!(f.record().unwrap().completed);
    assert_eq!(f.reclaim().await.unwrap(), 0);
    f.client
        .post(
            &format!(
                "/sessions/{}/rewind/restores/{}/acknowledge",
                f.session, f.receipt.origin.activity_id
            ),
            json!({}),
        )
        .await
        .unwrap();
    assert!(
        std::fs::symlink_metadata(f.project.join("file_0"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "later owner bytes"
    );
    assert_eq!(
        f.client
            .get(&format!("/sessions/{}/messages", f.session))
            .await
            .unwrap(),
        messages
    );
    assert_eq!(f.model.requests().len(), inference);
    f.app.runtime.shutdown().await;
}
fn image_bytes(path: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_str().unwrap().to_owned(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn periodic_collection_applies_the_same_retention_and_receipt_admission() {
    let f = Fixture::new().await;
    let intent = std::fs::read(f.journal.join("intent.json")).unwrap();
    let files = f.journal.parent().unwrap().parent().unwrap().join("files");
    let images = image_bytes(&files);
    f.snapshots.gc().await;
    assert!(f.record().is_none() && f.journal.join("stage_0").exists());
    f.age();
    f.snapshots.gc().await;
    assert!(f.record().unwrap().completed);
    assert!(!f.journal.join("stage_0").exists() && !f.journal.join("stage_1").exists());
    assert_eq!(
        std::fs::read(f.journal.join("intent.json")).unwrap(),
        intent
    );
    assert_eq!(image_bytes(&files), images);
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn retention_disabled_capture_disabled_and_live_lifetime_do_not_license_cleanup() {
    let f = Fixture::new().await;
    assert_eq!(f.reclaim().await.unwrap(), 0);
    f.age();
    *f.config.lock().unwrap() = json!({"snapshots":{"retention_days":0}});
    assert_eq!(f.reclaim().await.unwrap(), 0);
    *f.config.lock().unwrap() = json!({"snapshots":false});
    assert_eq!(f.reclaim().await.unwrap(), 0);
    *f.config.lock().unwrap() = json!({});
    let reader = cyber_server::workspace::mutation_lock(&f.project)
        .unwrap()
        .lock_owned()
        .await;
    assert!(f.reclaim().await.is_err());
    assert!(f.record().is_none());
    drop(reader);
    let lifetime = f.receipt.checkout.as_ref().unwrap().lifetime.clone();
    let worker = f
        .journal
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("workers")
        .join(lifetime.owner.activity_id);
    let held = std::fs::File::open(worker).unwrap();
    held.try_lock().unwrap();
    assert!(f.reclaim().await.is_err());
    assert!(f.record().is_none() && f.journal.join("stage_0").exists());
    drop(held);
    assert!(f.reclaim().await.unwrap() > 0);
    f.app.runtime.shutdown().await;
}

struct Cut {
    inner: Arc<dyn Policy>,
    after_delete: bool,
    pause: Option<PathBuf>,
    tamper: Option<PathBuf>,
}
impl Policy for Cut {
    fn receipt(
        &self,
        origin: &cyber_server::runtime::SnapshotOrigin,
    ) -> Result<Option<Settlement>, String> {
        self.inner.receipt(origin)
    }
    fn record(&self, receipt: &records::Receipt) -> Result<Option<records::Record>, String> {
        self.inner.record(receipt)
    }
    fn reclaim(&self, plan: records::Plan, delete: Delete) -> Result<u64, String> {
        let after = self.after_delete;
        let pause = self.pause.clone();
        let tamper = self.tamper.clone();
        self.inner.reclaim(
            plan,
            Box::new(move || {
                if let Some(path) = tamper {
                    std::fs::write(path, b"later metadata owner bytes").unwrap();
                    return delete();
                }
                if after {
                    delete()?;
                }
                if let Some(marker) = pause {
                    std::fs::write(marker, b"native cleanup boundary reached").unwrap();
                    loop {
                        std::thread::park();
                    }
                }
                Err("injected cleanup interruption".into())
            }),
        )
    }
    fn collect(&self, collect: Collect) -> Result<(), String> {
        self.inner.collect(collect)
    }
    fn refused(&self, directory: &str, error: &str) {
        self.inner.refused(directory, error);
    }
}
#[tokio::test]
async fn durable_plan_survives_interruption_before_and_after_private_deletion() {
    for after_delete in [false, true] {
        let f = Fixture::new().await;
        f.age();
        let policy = Arc::new(Cut {
            inner: cyber_app::snapshot_reclamation::policy(f.app.store.clone()),
            after_delete,
            tamper: None,
            pause: None,
        });
        let cut = maintenance(&f.paths, f.config.clone(), policy);
        assert!(cut.reclaim(f.project.to_str().unwrap()).await.is_err());
        let record = f.record().unwrap();
        assert!(!record.completed && !record.plan.files.is_empty());
        assert_eq!(f.journal.join("stage_0").exists(), !after_delete);
        assert!(f.reclaim().await.unwrap() > 0);
        assert!(f.record().unwrap().completed);
        assert!(!f.journal.join("stage_0").exists());
        f.app.runtime.shutdown().await;
    }
}

// Invoked in a separate test process only; ordinary suite discovery is inert.
#[tokio::test]
async fn reclamation_process_worker() {
    let Ok(control) = std::env::var("CYBER_RECLAMATION_PROCESS_FIXTURE") else {
        return;
    };
    let control = PathBuf::from(control);
    let f = Fixture::in_directory(tempfile::tempdir_in(&control).unwrap()).await;
    f.age();
    std::fs::write(
        control.join("fixture.json"),
        serde_json::to_vec(&json!({
            "root": f._temp.path(), "project": f.project,
            "journal": f.journal, "receipt": f.receipt,
        }))
        .unwrap(),
    )
    .unwrap();
    let policy = Arc::new(Cut {
        inner: cyber_app::snapshot_reclamation::policy(f.app.store.clone()),
        after_delete: std::env::var("CYBER_RECLAMATION_AFTER_DELETE").unwrap() == "true",
        tamper: None,
        pause: Some(control.join("ready")),
    });
    let cut = maintenance(&f.paths, f.config.clone(), policy);
    let result = cut.reclaim(f.project.to_str().unwrap()).await;
    panic!("worker returned without reaching kill boundary: {result:?}");
}

#[tokio::test]
async fn actual_process_death_preserves_plan_and_resumes_without_source_replay() {
    use std::os::unix::process::ExitStatusExt;
    for after_delete in [false, true] {
        let control = tempfile::tempdir().unwrap();
        let log = std::fs::File::create(control.path().join("worker.log")).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "reclamation_process_worker", "--nocapture"])
            .env("CYBER_RECLAMATION_PROCESS_FIXTURE", control.path())
            .env("CYBER_RECLAMATION_AFTER_DELETE", after_delete.to_string())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let ready = loop {
            if control.path().join("ready").exists() {
                break true;
            }
            if child.try_wait().unwrap().is_some() || std::time::Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        // Always reap the child, including failed fixture startup/timeouts.
        let _ = child.kill();
        let status = child.wait().unwrap();
        assert!(
            ready,
            "worker failed: {}",
            std::fs::read_to_string(control.path().join("worker.log")).unwrap()
        );
        assert_eq!(status.signal(), Some(9));
        let fixture: Value =
            serde_json::from_slice(&std::fs::read(control.path().join("fixture.json")).unwrap())
                .unwrap();
        let root = PathBuf::from(fixture["root"].as_str().unwrap());
        let project = PathBuf::from(fixture["project"].as_str().unwrap());
        let journal = PathBuf::from(fixture["journal"].as_str().unwrap());
        let receipt: SnapshotRestoreReceipt =
            serde_json::from_value(fixture["receipt"].clone()).unwrap();
        let intent = std::fs::read(journal.join("intent.json")).unwrap();
        assert_eq!(journal.join("stage_0").exists(), !after_delete);
        // Install a later owner's route after death: resumption must never replay
        // restore effects or overwrite it, even when all payloads are gone.
        let target = root.join("later-owner");
        std::fs::write(&target, "later owner bytes").unwrap();
        std::fs::remove_file(project.join("file_0")).unwrap();
        std::os::unix::fs::symlink(&target, project.join("file_0")).unwrap();
        let paths = Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        let store = Arc::new(
            cyber_store::Store::open(cyber_store::StoreOptions::new(
                DatabaseLocation::File(paths.data.join("cyber.db")),
                cyber_server::runtime::Runtime::registry(),
            ))
            .unwrap(),
        );
        let policy = cyber_app::snapshot_reclamation::policy(store.clone());
        assert!(
            !policy
                .record(&receipt.clone().into())
                .unwrap()
                .unwrap()
                .completed
        );
        let snapshots = maintenance(
            &paths,
            Arc::new(Mutex::new(json!({"snapshots":{"retention_days":7}}))),
            policy.clone(),
        );
        assert!(snapshots.reclaim(project.to_str().unwrap()).await.unwrap() > 0);
        assert!(
            policy
                .record(&receipt.clone().into())
                .unwrap()
                .unwrap()
                .completed
        );
        assert_eq!(
            snapshots.reclaim(project.to_str().unwrap()).await.unwrap(),
            0
        );
        assert!(!journal.join("stage_0").exists() && !journal.join("stage_1").exists());
        assert_eq!(std::fs::read(journal.join("intent.json")).unwrap(), intent);
        assert_eq!(std::fs::read_link(project.join("file_0")).unwrap(), target);
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            "later owner bytes"
        );
    }
}

#[tokio::test]
async fn cap_pressure_reclaims_only_eligible_payloads_then_retries_capture() {
    let f = Fixture::new().await;
    f.age();
    let namespace = f.journal.parent().unwrap().parent().unwrap();
    let cap = private_bytes(namespace) + 4096;
    *f.config.lock().unwrap() = json!({"snapshots":{"retention_days":7,"fallback_max_bytes":cap}});
    let requests = f.model.requests().len();
    std::fs::write(f.project.join("zz_new"), vec![83u8; 8192]).unwrap();
    let captured = f
        .snapshots
        .track(f.project.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(captured.tree.starts_with("copy:"));
    assert!(f.record().unwrap().completed);
    assert!(!f.journal.join("stage_0").exists());
    assert!(private_bytes(namespace) <= cap);
    assert_eq!(
        std::fs::read(f.project.join("zz_new")).unwrap(),
        vec![83u8; 8192]
    );
    assert_eq!(f.model.requests().len(), requests);
    f.app.runtime.shutdown().await;
}
fn private_bytes(path: &std::path::Path) -> u64 {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                private_bytes(&path)
            } else {
                std::fs::symlink_metadata(path).unwrap().len()
            }
        })
        .sum()
}

#[tokio::test]
async fn missing_replaced_aliased_and_unknown_payloads_refuse_before_any_unlink() {
    for kind in [
        "missing",
        "replacement",
        "alias",
        "unknown",
        "symlink",
        "phase",
    ] {
        let f = Fixture::new().await;
        f.age();
        corrupt_payload(&f, kind);
        assert!(f.reclaim().await.is_err(), "{kind}");
        assert!(f.record().is_none(), "{kind}");
        assert!(f.journal.join("stage_1").exists(), "{kind}");
        assert_eq!(
            std::fs::read_to_string(f.project.join("file_1")).unwrap(),
            "before\n".repeat(4096)
        );
        f.app.runtime.shutdown().await;
    }
}
fn corrupt_payload(f: &Fixture, kind: &str) {
    let stage = f.journal.join("stage_0");
    let owner = f.paths.data.join("owner-kept");
    match kind {
        "phase" => {
            let binding = &f.receipt.checkout.as_ref().unwrap().lifetime;
            let path = f
                .journal
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("workers")
                .join(binding.source_phase.as_ref().unwrap());
            let mut phase: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            phase["admitted"] = false.into();
            phase["origin"] = Value::Null;
            std::fs::write(path, phase.to_string()).unwrap();
        }
        "missing" => std::fs::remove_file(stage).unwrap(),
        "replacement" => {
            let bytes = std::fs::read(&stage).unwrap();
            std::fs::rename(&stage, owner).unwrap();
            std::fs::write(stage, bytes).unwrap();
        }
        "alias" => std::fs::hard_link(stage, owner).unwrap(),
        "unknown" => std::fs::write(f.journal.join("owner-kept"), "owner bytes").unwrap(),
        "symlink" => {
            std::fs::write(&owner, "owner target bytes").unwrap();
            std::fs::remove_file(&stage).unwrap();
            std::os::unix::fs::symlink(owner, stage).unwrap();
        }
        _ => panic!("unknown corruption"),
    }
}

#[tokio::test]
async fn independent_unknown_owner_and_missing_location_view_keep_their_fences() {
    for mode in ["same", "missing", "other", "missing_status"] {
        let f = Fixture::new().await;
        f.age();
        let other = f.project.parent().unwrap().join("other-location");
        std::fs::create_dir(&other).unwrap();
        let foreign = f
            .client
            .at(other.to_str().unwrap())
            .post("/sessions", json!({"model":"test/main"}))
            .await
            .unwrap()["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let source = match mode {
            "same" => &f.session,
            "missing" => "ses_missing",
            _ => &foreign,
        };
        // Retained legacy uncertainty, not a fabricated successful process.
        let mut evidence = json!({"session_id":source,"status":"unknown","phase":"legacy_process"});
        if mode == "missing_status" {
            evidence.as_object_mut().unwrap().remove("status");
        }
        let retained = evidence.clone();
        f.app
            .store
            .append(
                "op_legacy_unknown",
                cyber_store::Expected::Seq(-1),
                vec![cyber_store::NewEvent::new(
                    "native.activity.changed.1",
                    evidence,
                )],
            )
            .unwrap();
        assert!(f.reclaim().await.is_err());
        assert!(f.record().is_none() && f.journal.join("stage_0").exists());
        let raw:String = f.app.store.read(|db| Ok(db.query_row("SELECT data FROM event WHERE aggregate_id='op_legacy_unknown' ORDER BY seq DESC LIMIT 1",[],|row|row.get(0))?)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&raw).unwrap(), retained);
        f.app.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn reference_writers_cannot_interleave_with_native_collection() {
    let f = Fixture::new().await;
    let policy = cyber_app::snapshot_reclamation::policy(f.app.store.clone());
    let (started, entered) = std::sync::mpsc::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let collecting = std::thread::spawn(move || {
        policy.collect(Box::new(move |roots| {
            assert!(!roots.is_empty());
            started.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        }))
    });
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let wrote = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (store, flag) = (f.app.store.clone(), wrote.clone());
    let writer = std::thread::spawn(move || {
        store.transaction(move |_| {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!wrote.load(std::sync::atomic::Ordering::SeqCst));
    release.send(()).unwrap();
    collecting.join().unwrap().unwrap();
    writer.join().unwrap().unwrap();
    assert!(wrote.load(std::sync::atomic::Ordering::SeqCst));
    f.app.runtime.shutdown().await;
}

async fn planned_without_deletion(f: &Fixture) {
    let policy = Arc::new(Cut {
        inner: cyber_app::snapshot_reclamation::policy(f.app.store.clone()),
        after_delete: false,
        tamper: None,
        pause: None,
    });
    assert!(
        maintenance(&f.paths, f.config.clone(), policy)
            .reclaim(f.project.to_str().unwrap())
            .await
            .is_err()
    );
    assert!(!f.record().unwrap().completed);
}
#[tokio::test]
async fn frozen_plan_accepts_partial_progress_but_not_replaced_remaining_images() {
    for replace in [false, true] {
        let f = Fixture::new().await;
        f.age();
        planned_without_deletion(&f).await;
        // Storage-state cut of one already-planned private unlink, not a claim
        // of a process-kill acceptance cut.
        std::fs::remove_file(f.journal.join("stage_0")).unwrap();
        if replace {
            let path = f.journal.join("stage_1");
            let bytes = std::fs::read(&path).unwrap();
            std::fs::rename(&path, f.paths.data.join("preserved-original")).unwrap();
            std::fs::write(&path, bytes).unwrap();
            assert!(f.reclaim().await.is_err());
            assert!(!f.record().unwrap().completed && path.exists());
        } else {
            assert!(f.reclaim().await.unwrap() > 0);
            assert!(f.record().unwrap().completed && !f.journal.join("stage_1").exists());
        }
        f.app.runtime.shutdown().await;
    }
}
#[tokio::test]
async fn committed_plan_cannot_bypass_later_retention_or_unknown_owner_fences() {
    let f = Fixture::new().await;
    f.age();
    planned_without_deletion(&f).await;
    *f.config.lock().unwrap() = json!({"snapshots":{"retention_days":0}});
    assert_eq!(f.reclaim().await.unwrap(), 0);
    *f.config.lock().unwrap() = json!({});
    f.app
        .store
        .append(
            "op_late_unknown",
            cyber_store::Expected::Seq(-1),
            vec![cyber_store::NewEvent::new(
                "native.activity.changed.1",
                json!({"session_id":f.session,"status":"unknown","phase":"legacy_process"}),
            )],
        )
        .unwrap();
    assert!(f.reclaim().await.is_err());
    assert!(!f.record().unwrap().completed && f.journal.join("stage_0").exists());
    f.app.runtime.shutdown().await;
}
#[tokio::test]
async fn reclamation_projectors_roll_back_foreign_plans_and_invalid_completions() {
    let f = Fixture::new().await;
    f.age();
    planned_without_deletion(&f).await;
    let plan = f.record().unwrap().plan;
    let id = records::Plan::id(&plan.receipt).unwrap();
    let notifications = f.app.store.subscribe();
    let mut wrong = plan.clone();
    let records::Receipt::Restore(receipt) = &mut wrong.receipt else {
        panic!("ordinary fixture");
    };
    receipt.events_digest = "0".repeat(64);
    let foreign = records::Plan::id(&wrong.receipt).unwrap();
    assert!(
        f.app
            .store
            .append(
                &foreign,
                cyber_store::Expected::Seq(-1),
                vec![cyber_store::NewEvent::new(records::PLANNED, json!(wrong))]
            )
            .is_err()
    );
    assert!(f.app.store.aggregate_seq(&foreign).unwrap().is_none());
    let completion = records::Completion {
        plan_id: id.clone(),
        removed_bytes: plan.total_bytes().unwrap() + 1,
    };
    assert!(
        f.app
            .store
            .append(
                &id,
                cyber_store::Expected::Seq(0),
                vec![cyber_store::NewEvent::new(
                    records::COMPLETED,
                    json!(completion)
                )]
            )
            .is_err()
    );
    assert!(!f.record().unwrap().completed);
    assert!(notifications.try_recv().is_err());
    assert!(f.journal.join("stage_0").exists());
    f.app.runtime.shutdown().await;
}
#[tokio::test]
async fn an_oversized_reference_view_never_becomes_an_empty_cleanup_view() {
    let f = Fixture::new().await;
    f.age();
    let roots = (0..100001u32)
        .map(|index| format!("copy:{index:064x}"))
        .collect::<Vec<_>>();
    f.app.store.append("op_retained_history",cyber_store::Expected::Seq(-1),vec![cyber_store::NewEvent::new("native.activity.changed.1",json!({"session_id":f.session,"status":"settled","phase":"historical_references","retained_refs":roots}))]).unwrap();
    assert!(f.reclaim().await.is_err());
    assert!(f.record().is_none() && f.journal.join("stage_0").exists());
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn historical_reclamation_reprojects_after_later_fences_and_source_deletion() {
    for deleted in [false, true] {
        let f = Fixture::new().await;
        f.age();
        assert!(f.reclaim().await.unwrap() > 0);
        if deleted {
            f.app.runtime.delete(&f.session).await.unwrap();
        } else {
            f.app
                .store
                .append(
                    "op_later_unknown",
                    cyber_store::Expected::Seq(-1),
                    vec![cyber_store::NewEvent::new(
                        "native.activity.changed.1",
                        json!({"session_id":f.session,"status":"unknown","phase":"legacy_process"}),
                    )],
                )
                .unwrap();
        }
        let count = f
            .app
            .store
            .reproject(
                &[],
                "snapshot.reclamation.",
                Arc::new(|db, event| records::project(db, event)),
            )
            .unwrap();
        assert_eq!(count, 2);
        assert!(f.record().unwrap().completed);
        assert!(!f.journal.join("stage_0").exists());
        f.app.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn fallback_collection_honors_disabled_and_zero_retention_at_the_store_writer() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    };
    paths.ensure().unwrap();
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("a.txt"), "unreferenced bytes").unwrap();
    let store = Arc::new(
        cyber_store::Store::open(cyber_store::StoreOptions::new(
            DatabaseLocation::File(paths.data.join("cyber.db")),
            cyber_server::runtime::Runtime::registry(),
        ))
        .unwrap(),
    );
    let config = Arc::new(Mutex::new(json!({})));
    let snapshots = maintenance(
        &paths,
        config.clone(),
        cyber_app::snapshot_reclamation::policy(store),
    );
    snapshots
        .track(project.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    let namespace = std::fs::read_dir(paths.data.join("snapshot/global"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let retained = age_fallback_collection_images(&namespace);
    for settings in [
        json!({"snapshots":false}),
        json!({"snapshots":{"retention_days":0}}),
    ] {
        *config.lock().unwrap() = settings;
        snapshots.gc().await;
        for (path, bytes) in &retained {
            assert_eq!(std::fs::read(path).unwrap(), *bytes);
        }
    }
    *config.lock().unwrap() = json!({});
    snapshots.gc().await;
    for (path, _) in retained {
        assert!(!path.exists());
    }
}

fn age_fallback_collection_images(namespace: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut retained = vec![];
    for directory in [namespace.join("manifests"), namespace.join("files")] {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.parent().unwrap().ends_with("manifests") {
                let mut manifest: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                manifest["accessed"] = json!(0);
                std::fs::write(&path, manifest.to_string()).unwrap();
            }
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1)),
                )
                .unwrap();
            retained.push((path.clone(), std::fs::read(path).unwrap()));
        }
    }
    retained
}

#[tokio::test]
async fn a_foreign_intent_spare_refuses_cleanup_before_plan_without_touching_source() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    f.age();
    let active = std::fs::read(f.journal.join("intent.json")).unwrap();
    let spare = f.journal.join("intent.next");
    let bytes = std::fs::read(&spare).unwrap();
    let retained = f.paths.tmp.join("retained-original-spare");
    std::fs::rename(&spare, &retained).unwrap();
    std::fs::write(&spare, &bytes).unwrap();
    std::fs::set_permissions(&spare, std::fs::Permissions::from_mode(0o600)).unwrap();
    let inference = f.model.requests().len();
    let error = f.reclaim().await.unwrap_err();
    assert!(error.contains("Intent slot identity changed"), "{error}");
    assert!(f.record().is_none());
    assert!(f.journal.join("stage_0").exists());
    assert_eq!(std::fs::read(&spare).unwrap(), bytes);
    assert_eq!(
        std::fs::read(f.journal.join("intent.json")).unwrap(),
        active
    );
    assert_eq!(
        std::fs::read_to_string(f.project.join("file_0")).unwrap(),
        "before\n".repeat(4096)
    );
    std::fs::remove_file(&spare).unwrap();
    std::fs::rename(retained, &spare).unwrap();
    assert!(f.reclaim().await.unwrap() > 0);
    assert_eq!(std::fs::read(spare).unwrap(), bytes);
    assert_eq!(f.model.requests().len(), inference);
    f.app.runtime.shutdown().await;
}

#[tokio::test]
async fn a_spare_metadata_change_after_planning_refuses_before_private_unlinks() {
    let f = Fixture::new().await;
    f.age();
    let spare = f.journal.join("intent.next");
    let original = std::fs::read(&spare).unwrap();
    let cut = maintenance(
        &f.paths,
        f.config.clone(),
        Arc::new(Cut {
            inner: cyber_app::snapshot_reclamation::policy(f.app.store.clone()),
            after_delete: false,
            pause: None,
            tamper: Some(spare.clone()),
        }),
    );
    let error = cut.reclaim(f.project.to_str().unwrap()).await.unwrap_err();
    assert!(
        error.contains("metadata inode or generation changed"),
        "{error}"
    );
    assert!(!f.record().unwrap().completed);
    assert!(f.journal.join("stage_0").exists() && f.journal.join("stage_1").exists());
    assert_eq!(
        std::fs::read(&spare).unwrap(),
        b"later metadata owner bytes"
    );
    std::fs::write(&spare, original).unwrap();
    assert!(f.reclaim().await.unwrap() > 0);
    f.app.runtime.shutdown().await;
}
