//! Real process-death cuts on the ordinary file-copy restore path: after the Store
//! writer commit but before the native journal acknowledgement, and inside payload
//! reclamation after the durable plan and some unlinks but before completion.
use super::*;
const FILES: usize = 4;

struct Fixture {
    model: scripted_model::ScriptedModel,
    env: Env,
    session: String,
    first: String,
}
impl Fixture {
    /// Non-Git Location with FILES modified tracked files, so every restore change
    /// displaces a payload image into its journal.
    fn new() -> Self {
        let model = scripted_model::ScriptedModel::start(reply);
        let env = Env::new(&model);
        std::fs::remove_dir_all(env.directory.join(".git")).unwrap();
        write_files(&env, "before");
        let session = env.exec(None, "before snapshot");
        let first = env.user_ids(&session)[0].clone();
        write_files(&env, "after");
        env.exec(Some(&session), "after snapshot");
        Self {
            model,
            env,
            session,
            first,
        }
    }
    fn namespace(&self) -> PathBuf {
        let namespaces: Vec<_> =
            std::fs::read_dir(self.env.root.join("cyber-home/data/snapshot/global"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect();
        assert_eq!(namespaces.len(), 1);
        namespaces[0].clone()
    }
    /// The only restore journal and its decoded intent.
    fn journal(&self) -> (String, PathBuf, Value) {
        let journals: Vec<_> = std::fs::read_dir(self.namespace().join("journals"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(journals.len(), 1, "{journals:?}");
        let intent = journals[0].join("intent.json");
        let record = serde_json::from_slice(&std::fs::read(&intent).unwrap()).unwrap();
        let id = journals[0].file_name().unwrap().to_string_lossy().into();
        (id, journals[0].clone(), record)
    }
    fn route(&self, intent: &Value) -> String {
        let activity = intent["origin"]["activity_id"].as_str().unwrap();
        format!("/sessions/{}/rewind/restores/{activity}", self.session)
    }
    fn assert_restored(&self, from: usize) {
        for index in from..FILES {
            assert_eq!(self.env.file(&name(index)).as_deref(), Some("before"));
        }
    }
    /// Later owner edits made while no service runs: a replaced file and a symlink.
    fn edit_as_owner(&self) -> PathBuf {
        let target = self.env.root.join("later-owner-target");
        std::fs::write(&target, "later owner bytes").unwrap();
        let link = self.env.directory.join(name(0));
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        std::fs::write(self.env.directory.join(name(1)), "later owner edit").unwrap();
        target
    }
    fn assert_owner_edits(&self, target: &std::path::Path) {
        assert_eq!(
            std::fs::read_link(self.env.directory.join(name(0))).unwrap(),
            target
        );
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            "later owner bytes"
        );
        assert_eq!(self.env.file(&name(1)).as_deref(), Some("later owner edit"));
        self.assert_restored(2);
    }
    /// Receipt-only lookup and metadata acknowledgement on a fresh service: the same
    /// receipt each time, no source effect, no transcript movement and no inference.
    fn assert_receipt_lookup(&self, route: &str, receipt: &Value, target: &std::path::Path) {
        let messages = self.env.messages(&self.session);
        let requests = self.model.requests().len();
        assert_eq!(
            json(&self.env.cyber(&["api", "GET", route]))["data"],
            *receipt
        );
        for _ in 0..2 {
            let acknowledged = json(&self.env.cyber(&[
                "api",
                "POST",
                &format!("{route}/acknowledge"),
                "-d",
                "{}",
            ]));
            assert_eq!(acknowledged["data"], *receipt);
        }
        self.assert_owner_edits(target);
        assert_eq!(self.env.messages(&self.session), messages);
        assert_eq!(self.model.requests().len(), requests);
    }
}
fn name(index: usize) -> String {
    format!("cut_{index:04}")
}
fn write_files(env: &Env, contents: &str) {
    for index in 0..FILES {
        std::fs::write(env.directory.join(name(index)), contents).unwrap();
    }
}
fn unacknowledged(intent: &Value) -> bool {
    intent
        .get("acknowledged")
        .is_none_or(|value| value == false)
}
/// The latest native activity record carries the committed restore receipt.
fn assert_writer_committed(env: &Env, intent: &Value) {
    let database = std::fs::read_dir(env.root.join("cyber-home/data"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "db"))
        .unwrap();
    let db =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let data: String = db
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
            [intent["origin"]["activity_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    let activity: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(activity["phase"], "snapshot_restore_committed");
    assert_eq!(
        activity["snapshot_restore_receipt"]["origin"],
        intent["origin"]
    );
}
struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait_for(marker: &std::path::Path, child: &mut Child, log: &std::path::Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !marker.exists() {
        assert!(
            child.0.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline,
            "cut was never reached: {}",
            std::fs::read_to_string(log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn ordinary_restore_killed_after_writer_commit_before_journal_ack_reconciles_without_replay() {
    let f = Fixture::new();
    let preview = rewind_preview(&f.env, &f.session, &f.first, "both");
    let requests = f.model.requests().len();
    // The fixture point is read by the service process, so start a fresh one with it.
    exits(&f.env.cyber(&["service", "stop"]), 0);
    let marker = f.env.root.join("writer-commit-cut");
    let log = f.env.root.join("writer-commit-confirm.log");
    let output = std::fs::File::create(&log).unwrap();
    let mut confirm = Child(
        f.env
            .command(&[
                "sessions",
                "rewind",
                &f.session,
                "--to",
                &f.first,
                "--target",
                "both",
                "--confirm",
                preview["confirmation"].as_str().unwrap(),
                "--format",
                "json",
            ])
            .env("CYBER_TEST_PROCESS_CUT", "ordinary-restore-writer-commit")
            .env("CYBER_TEST_PROCESS_CUT_MARKER", &marker)
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .spawn()
            .unwrap(),
    );
    wait_for(&marker, &mut confirm, &log);
    kill_registered_service(&f.env);
    assert!(!confirm.0.wait().unwrap().success());
    // Durable writer receipt and completed filesystem, but no native acknowledgement.
    let (journal, path, intent) = f.journal();
    assert_writer_committed(&f.env, &intent);
    assert_eq!(intent["completed"], true);
    assert!(unacknowledged(&intent));
    f.assert_restored(0);
    let target = f.edit_as_owner();
    // A fresh service reports the committed rewind; nothing is restored again.
    assert!(f.env.user_ids(&f.session).is_empty());
    let route = f.route(&intent);
    let receipt = json(&f.env.cyber(&["api", "GET", &route]))["data"].clone();
    assert_eq!(receipt["origin"], intent["origin"]);
    let after_lookup: Value =
        serde_json::from_slice(&std::fs::read(path.join("intent.json")).unwrap()).unwrap();
    assert!(unacknowledged(&after_lookup), "lookup acknowledged");
    f.assert_receipt_lookup(&route, &receipt, &target);
    assert_owned_snapshot_receipt(&f.env, &f.session, &journal);
    let acknowledged: Value =
        serde_json::from_slice(&std::fs::read(path.join("intent.json")).unwrap()).unwrap();
    assert_eq!(acknowledged["origin"], intent["origin"]);
    assert_eq!(acknowledged["changes"], intent["changes"]);
    assert_eq!(f.model.requests().len(), requests);
}

// Invoked in a separate test process only; ordinary suite discovery is inert.
#[test]
fn ordinary_reclaim_cut_worker() {
    let Some(control) = std::env::var_os("CYBER_BRANCHES_RECLAIM_CUT") else {
        return;
    };
    let control = PathBuf::from(control);
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(control.join("fixture.json")).unwrap()).unwrap();
    let path = |key: &str| PathBuf::from(fixture[key].as_str().unwrap());
    let maintenance = reclamation_support::Maintenance::open(&path("home"));
    let result = maintenance.reclaim_until_killed(
        &path("directory"),
        path("journal").join("stage_0"),
        control.join("ready"),
    );
    panic!("worker returned without reaching the kill boundary: {result}");
}

#[test]
fn ordinary_restore_killed_during_payload_reclaim_resumes_frozen_plan() {
    use cyber_server::runtime::{SnapshotRestoreReceipt, snapshot_reclamation::Receipt};
    let f = Fixture::new();
    let preview = rewind_preview(&f.env, &f.session, &f.first, "both");
    json(&confirm_rewind(
        &f.env, &f.session, &f.first, "both", &preview,
    ));
    let (_, journal, intent) = f.journal();
    assert_eq!(intent["acknowledged"], true);
    let route = f.route(&intent);
    let receipt = json(&f.env.cyber(&["api", "GET", &route]))["data"].clone();
    let typed: Receipt = serde_json::from_value::<SnapshotRestoreReceipt>(receipt.clone())
        .unwrap()
        .into();
    let intent_bytes = std::fs::read(journal.join("intent.json")).unwrap();
    exits(&f.env.cyber(&["service", "stop"]), 0);
    let home = f.env.root.join("cyber-home");
    {
        let maintenance = reclamation_support::Maintenance::open(&home);
        assert_eq!(maintenance.reclaim(&f.env.directory).unwrap(), 0);
        maintenance.age(intent["origin"]["activity_id"].as_str().unwrap().into());
    }
    kill_reclaim_worker(&f, &home, &journal);
    // Death inside the deletion transaction: durable plan, partial unlinks, no completion.
    assert!(!journal.join("stage_0").exists());
    for index in 1..FILES {
        assert!(journal.join(format!("stage_{index}")).exists());
    }
    let target = f.edit_as_owner();
    resume_frozen_plan(&f, &home, &typed, &journal);
    assert_eq!(
        std::fs::read(journal.join("intent.json")).unwrap(),
        intent_bytes
    );
    f.assert_receipt_lookup(&route, &receipt, &target);
    assert!(f.env.user_ids(&f.session).is_empty());
}

/// Run real reclamation in a separate process and SIGKILL it mid-deletion.
fn kill_reclaim_worker(f: &Fixture, home: &std::path::Path, journal: &std::path::Path) {
    use std::os::unix::process::ExitStatusExt;
    let control = f.env.root.join("reclaim-cut");
    std::fs::create_dir(&control).unwrap();
    std::fs::write(
        control.join("fixture.json"),
        json!({"home": home, "directory": f.env.directory, "journal": journal}).to_string(),
    )
    .unwrap();
    let log = control.join("worker.log");
    let output = std::fs::File::create(&log).unwrap();
    let mut worker = Child(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ordinary_cuts::ordinary_reclaim_cut_worker",
                "--nocapture",
            ])
            .env("CYBER_BRANCHES_RECLAIM_CUT", &control)
            .stdout(output.try_clone().unwrap())
            .stderr(output)
            .spawn()
            .unwrap(),
    );
    wait_for(&control.join("ready"), &mut worker, &log);
    worker.0.kill().unwrap();
    assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
}

/// A fresh owner resumes the exact frozen plan once; a second pass is a no-op.
fn resume_frozen_plan(
    f: &Fixture,
    home: &std::path::Path,
    receipt: &cyber_server::runtime::snapshot_reclamation::Receipt,
    journal: &std::path::Path,
) {
    let maintenance = reclamation_support::Maintenance::open(home);
    let frozen = maintenance.policy.record(receipt).unwrap().unwrap();
    assert!(!frozen.completed);
    assert_eq!(frozen.plan.files.len(), FILES);
    assert!(maintenance.reclaim(&f.env.directory).unwrap() > 0);
    let completed = maintenance.policy.record(receipt).unwrap().unwrap();
    assert!(completed.completed);
    assert_eq!(completed.plan, frozen.plan);
    assert_eq!(maintenance.reclaim(&f.env.directory).unwrap(), 0);
    for index in 0..FILES {
        assert!(!journal.join(format!("stage_{index}")).exists());
    }
}
