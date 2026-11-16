//! Real process death inside payload cleanup for settled keep/rollback recoveries
//! and cancelled fully prepared restores: a separate maintenance process runs the
//! production reclamation and is SIGKILLed after the durable plan and the first
//! unlink; a fresh owner completes the same frozen plan exactly once.
use super::*;
use cyber_server::runtime::{
    SnapshotPreparationReceipt, SnapshotRecoveryReceipt, snapshot_reclamation::Receipt,
};

// Invoked in a separate test process only; ordinary suite discovery is inert.
#[test]
fn cleanup_cut_worker() {
    let Some(control) = std::env::var_os("CYBER_CLEANUP_CUT") else {
        return;
    };
    let control = PathBuf::from(control);
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(control.join("fixture.json")).unwrap()).unwrap();
    let payloads = fixture["payloads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| PathBuf::from(path.as_str().unwrap()))
        .collect();
    let maintenance =
        reclamation_support::Maintenance::open(&PathBuf::from(fixture["home"].as_str().unwrap()));
    let result = maintenance.reclaim_until_any_killed(
        &PathBuf::from(fixture["directory"].as_str().unwrap()),
        payloads,
        control.join("ready"),
    );
    panic!("worker returned without reaching the kill boundary: {result}");
}

impl Fixture {
    fn home(&self) -> PathBuf {
        self.env.root.join("cyber-home")
    }
    /// Every displaced or target image retained in the given journals.
    fn stages(&self, journals: &[&str]) -> Vec<PathBuf> {
        let mut stages = Vec::new();
        for journal in journals {
            let directory = self.namespace().join("journals").join(journal);
            for entry in std::fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_name().to_string_lossy().starts_with("stage_") {
                    stages.push(entry.path());
                }
            }
        }
        stages.sort();
        stages
    }
    /// Run production reclamation in a separate process and SIGKILL it once one of
    /// `payloads` is unlinked inside the open deletion transaction.
    fn kill_cleanup_worker(&self, payloads: &[PathBuf]) {
        use std::os::unix::process::ExitStatusExt;
        let control = self.env.root.join("cleanup-cut");
        std::fs::create_dir(&control).unwrap();
        std::fs::write(
            control.join("fixture.json"),
            json!({"home": self.home(), "directory": self.env.directory, "payloads": payloads})
                .to_string(),
        )
        .unwrap();
        let log = control.join("worker.log");
        let output = std::fs::File::create(&log).unwrap();
        let mut worker = Child(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "restore_cuts::cleanup::cleanup_cut_worker",
                    "--nocapture",
                ])
                .env("CYBER_CLEANUP_CUT", &control)
                .stdout(output.try_clone().unwrap())
                .stderr(output)
                .spawn()
                .unwrap(),
        );
        wait_for(&control.join("ready"), &mut worker, &log);
        worker.0.kill().unwrap();
        assert_eq!(worker.0.wait().unwrap().signal(), Some(9));
    }
}

/// Partially unlinked payloads and an uncompleted durable plan after the kill; a
/// fresh owner then completes that exact plan once and a second pass is a no-op.
fn kill_and_resume_cleanup(f: &Fixture, receipt: &Receipt, payloads: &[PathBuf]) {
    let maintenance = reclamation_support::Maintenance::open(&f.home());
    assert!(maintenance.policy.record(receipt).unwrap().is_none());
    drop(maintenance);
    f.kill_cleanup_worker(payloads);
    let missing = payloads.iter().filter(|path| !path.exists()).count();
    assert!(
        missing >= 1 && missing < payloads.len(),
        "kill missed the partial window"
    );
    let target = f.edit_as_owner();
    let maintenance = reclamation_support::Maintenance::open(&f.home());
    let frozen = maintenance.policy.record(receipt).unwrap().unwrap();
    assert!(!frozen.completed);
    assert_eq!(frozen.plan.files.len(), payloads.len());
    assert!(maintenance.reclaim(&f.env.directory).unwrap() > 0);
    let completed = maintenance.policy.record(receipt).unwrap().unwrap();
    assert!(completed.completed);
    assert_eq!(completed.plan, frozen.plan);
    assert_eq!(maintenance.reclaim(&f.env.directory).unwrap(), 0);
    assert!(payloads.iter().all(|path| !path.exists()));
    f.assert_owner_edits(&target);
}

fn intents(f: &Fixture, journals: &[&str]) -> Vec<Vec<u8>> {
    journals
        .iter()
        .map(|journal| {
            std::fs::read(
                f.namespace()
                    .join("journals")
                    .join(journal)
                    .join("intent.json"),
            )
            .unwrap()
        })
        .collect()
}

fn paired_cleanup_cut(action: &str) {
    let f = Fixture::new();
    let messages = f.env.messages(&f.session);
    let (journal, _) = kill_after_first_progress(&f);
    let request_id = format!("cleanup-{action}");
    let result = f.confirm_acknowledged(&f.request(&journal, action, &request_id));
    let requests = f.model.requests().len();
    let receipt: SnapshotRecoveryReceipt =
        serde_json::from_value(result["receipt"].clone()).unwrap();
    let owner = receipt
        .checkout
        .as_ref()
        .unwrap()
        .resolution_origin
        .activity_id
        .clone();
    let resolution = receipt.resolution_id.clone();
    let journals = [journal.as_str(), resolution.as_str()];
    let payloads = f.stages(&journals);
    let before = intents(&f, &journals);
    exits(&f.env.cyber(&["service", "stop"]), 0);
    {
        let maintenance = reclamation_support::Maintenance::open(&f.home());
        assert_eq!(maintenance.reclaim(&f.env.directory).unwrap(), 0);
        maintenance.age(owner);
    }
    kill_and_resume_cleanup(&f, &Receipt::Recovery(receipt), &payloads);
    assert_eq!(intents(&f, &journals), before);
    // A fresh service still answers the request with the same receipt, no replay.
    let lookup = f.lookup(&request_id, true);
    assert_eq!(lookup["receipt"], result["receipt"]);
    f.assert_quiet(&messages, requests);
}

#[test]
fn kept_recovery_payload_cleanup_killed_mid_deletion_resumes_frozen_plan() {
    paired_cleanup_cut("keep");
}

#[test]
fn rolled_back_recovery_payload_cleanup_killed_mid_deletion_resumes_frozen_plan() {
    paired_cleanup_cut("rollback");
}

#[test]
fn cancelled_preparation_payload_cleanup_killed_mid_deletion_resumes_frozen_plan() {
    let f = Fixture::new();
    let messages = f.env.messages(&f.session);
    f.kill_rewind_at("copy-restore-images-prepared");
    let (journal, intent) = f.only_journal();
    assert_eq!(intent["applied"], json!([]));
    assert_eq!(f.payloads(&journal), FILES);
    f.assert_uniform("after");
    let (activity, reply) = cancel_preparation(&f, "prepared-cleanup");
    let requests = f.model.requests().len();
    let receipt: SnapshotPreparationReceipt = serde_json::from_value(reply.clone()).unwrap();
    let payloads = f.stages(&[journal.as_str()]);
    assert_eq!(payloads.len(), FILES);
    let directory = f.namespace().join("journals").join(&journal);
    let cancellation = std::fs::read(directory.join("preparation.json")).unwrap();
    let before = intents(&f, &[journal.as_str()]);
    exits(&f.env.cyber(&["service", "stop"]), 0);
    {
        let maintenance = reclamation_support::Maintenance::open(&f.home());
        assert_eq!(maintenance.reclaim(&f.env.directory).unwrap(), 0);
        maintenance.age(receipt.activity_id.clone());
    }
    kill_and_resume_cleanup(&f, &Receipt::Preparation(Box::new(receipt)), &payloads);
    assert_eq!(intents(&f, &[journal.as_str()]), before);
    assert_eq!(
        std::fs::read(directory.join("preparation.json")).unwrap(),
        cancellation
    );
    // The retained request still returns the same receipt on a fresh service.
    let body = json!({"confirmation":reply["confirmation"],"request_id":"prepared-cleanup"});
    assert_eq!(
        json(&preparation_confirm_cli(
            &f.env, &f.session, &activity, &body
        )),
        reply
    );
    f.assert_quiet(&messages, requests);
}
