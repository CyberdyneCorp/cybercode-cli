//! Real native cuts for empty cancellation and paired cleanup inventories.
use super::*;
use cyber_server::runtime::{
    SnapshotPreparationReceipt, SnapshotRecoveryReceipt, snapshot_reclamation::Receipt,
};
const FILES: usize = 256;

struct Fixture {
    model: scripted_model::ScriptedModel,
    env: Env,
    session: String,
    first: String,
    messages: Vec<Value>,
}
impl Fixture {
    fn copy(created: bool) -> Self {
        let model = scripted_model::ScriptedModel::start(reply);
        let env = Env::new(&model);
        std::fs::remove_dir_all(env.directory.join(".git")).unwrap();
        Self::new(model, env, None, created)
    }
    #[cfg(feature = "native-git-acceptance")]
    fn managed(created: bool) -> Self {
        let model = scripted_model::ScriptedModel::start(reply);
        let (env, session) = managed_git_fixture(&model);
        Self::new(model, env, Some(session), created)
    }
    fn new(
        model: scripted_model::ScriptedModel,
        env: Env,
        session: Option<String>,
        created: bool,
    ) -> Self {
        if created {
            write_files(&env);
        }
        let session = env.exec(session.as_deref(), "before snapshot");
        let first = env.user_ids(&session)[0].clone();
        if created {
            for index in 0..FILES {
                std::fs::remove_file(env.directory.join(format!("bulk_{index:04}"))).unwrap();
            }
        } else {
            write_files(&env);
        }
        env.exec(Some(&session), "after snapshot");
        let messages = env.messages(&session);
        Self {
            model,
            env,
            session,
            first,
            messages,
        }
    }
}
fn write_files(env: &Env) {
    for index in 0..FILES {
        std::fs::write(env.directory.join(format!("bulk_{index:04}")), "before").unwrap();
    }
}

#[test]
fn copy_cancelled_deletion_preparation_retires_zero_payload() {
    cancelled(Fixture::copy(false));
}
#[test]
fn copy_kept_creation_after_completed_source_kill_retires_zero_payload() {
    kept(Fixture::copy(true));
}
#[cfg(feature = "native-git-acceptance")]
#[test]
fn managed_cancelled_deletion_preparation_retires_zero_payload() {
    cancelled(Fixture::managed(false));
}
#[cfg(feature = "native-git-acceptance")]
#[test]
fn managed_kept_creation_after_completed_source_kill_retires_zero_payload() {
    kept(Fixture::managed(true));
}

fn cancelled(f: Fixture) {
    let git = preparation_git_metadata(&f.env.directory);
    let activity = kill_after_prepared_images(&f.env, &f.session, &f.first);
    let review = json(&f.env.cyber(&[
        "sessions",
        "recovery-preparation",
        &f.session,
        &activity,
        "--format",
        "json",
    ]));
    let body = json!({"confirmation":review["confirmation"],"request_id":"empty-preparation"});
    let reply = json(&preparation_confirm_cli(
        &f.env, &f.session, &activity, &body,
    ));
    let receipt: SnapshotPreparationReceipt = serde_json::from_value(reply.clone()).unwrap();
    let requests = f.model.requests().len();
    retire_empty(&f, Receipt::Preparation(Box::new(receipt)), activity);
    assert_eq!(
        json(&preparation_request_cli(
            &f.env,
            &f.session,
            reply["activity_id"].as_str().unwrap(),
            "empty-preparation",
            true
        )),
        reply
    );
    assert_owner_state(&f, requests, &git);
}

fn kept(f: Fixture) {
    let git = preparation_git_metadata(&f.env.directory);
    kill_completed_creation(&f);
    let (journal, review) = completed_review(&f);
    let request = json!({"journal_id":journal,"action":"keep","confirmation":review["confirmation"],"request_id":"empty-keep"});
    let reply = json(&recovery_confirm_cli(&f.env, &f.session, &request));
    assert_eq!(reply["journal_acknowledged"], true);
    let receipt: SnapshotRecoveryReceipt =
        serde_json::from_value(reply["receipt"].clone()).unwrap();
    let owner = receipt
        .checkout
        .as_ref()
        .unwrap()
        .resolution_origin
        .activity_id
        .clone();
    let requests = f.model.requests().len();
    retire_empty(&f, Receipt::Recovery(receipt), owner);
    // Lookup starts a fresh service; explicit acknowledgement uses retained history.
    let lookup = json(&f.env.cyber(&[
        "sessions",
        "recovery-request",
        &f.session,
        "empty-keep",
        "--acknowledge",
        "--format",
        "json",
    ]));
    assert_eq!(lookup["receipt"], reply["receipt"]);
    assert_owner_state(&f, requests, &git);
}

fn completed_review(f: &Fixture) -> (String, Value) {
    let catalog = json(
        &f.env
            .cyber(&["sessions", "recovery", &f.session, "--format", "json"]),
    );
    let entries = catalog.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["filesystem_complete"], true);
    let journal = entries[0]["id"].as_str().unwrap().to_owned();
    let review = json(&f.env.cyber(&[
        "sessions", "recovery", &f.session, &journal, "--action", "keep", "--format", "json",
    ]));
    assert!(
        review["conflicts"].as_array().unwrap().is_empty(),
        "{review}"
    );
    (journal, review)
}

fn retire_empty(f: &Fixture, receipt: Receipt, owner: String) {
    use std::os::unix::fs::PermissionsExt;
    exits(&f.env.cyber(&["service", "stop"]), 0);
    let maintenance = reclamation_support::Maintenance::open(&f.env.root.join("cyber-home"));
    assert_eq!(maintenance.reclaim(&f.env.directory).unwrap(), 0);
    assert!(maintenance.policy.record(&receipt).unwrap().is_none());
    maintenance.age(owner);
    let later = f.env.root.join("later-owner");
    std::fs::write(&later, b"later owner bytes").unwrap();
    std::fs::remove_file(f.env.directory.join("bulk_0000")).unwrap();
    std::os::unix::fs::symlink(&later, f.env.directory.join("bulk_0000")).unwrap();
    std::fs::set_permissions(&later, std::fs::Permissions::from_mode(0o0)).unwrap();
    // Refuse unknown construction even when the expected inventory is empty.
    let journal = maintenance
        .namespace
        .join("journals")
        .join(&receipt.origin().unwrap().binding.journal_id);
    let unknown = journal.join("unknown-construction");
    std::fs::write(&unknown, b"retain unknown bytes").unwrap();
    assert!(maintenance.reclaim(&f.env.directory).is_err());
    assert!(maintenance.policy.record(&receipt).unwrap().is_none());
    assert_eq!(std::fs::read(&unknown).unwrap(), b"retain unknown bytes");
    std::fs::remove_file(unknown).unwrap();
    assert_empty_plan(f, &maintenance, &receipt);
    assert!(
        maintenance
            .retire(&f.env.directory, receipt.clone())
            .unwrap()
            > 0
    );
    let record = maintenance.policy.record(&receipt).unwrap().unwrap();
    assert!(record.retired);
    let checkouts = record.metadata.unwrap().checkouts;
    let managed = f
        .env
        .directory
        .starts_with(f.env.root.join("cyber-home/data/worktrees"));
    let owners = receipt.owners().unwrap();
    assert_eq!(checkouts.len(), if managed { owners.len() } else { 0 });
    assert_retired_checkout_files(&checkouts);
    assert_retired_worker_files(&maintenance.namespace, &receipt);
    for owner in receipt.owners().unwrap() {
        assert!(
            !maintenance
                .namespace
                .join("journals")
                .join(&owner.origin.binding.journal_id)
                .exists()
        );
    }
    assert_eq!(maintenance.retire(&f.env.directory, receipt).unwrap(), 0);
}
fn assert_empty_plan(
    f: &Fixture,
    maintenance: &reclamation_support::Maintenance,
    receipt: &Receipt,
) {
    assert!(
        maintenance
            .plan_only(&f.env.directory)
            .unwrap_err()
            .contains("fixture cut after durable paired plan")
    );
    let pending = maintenance.policy.record(receipt).unwrap().unwrap();
    assert!(!pending.completed && pending.plan.files.is_empty());
    assert_eq!(maintenance.reclaim(&f.env.directory).unwrap(), 0);
    let complete = maintenance.policy.record(receipt).unwrap().unwrap();
    assert!(complete.completed);
    assert_eq!(complete.plan, pending.plan);
    assert_eq!(complete.plan.total_bytes().unwrap(), 0);
}
fn assert_owner_state(f: &Fixture, requests: usize, git: &[(PathBuf, Vec<u8>)]) {
    use std::os::unix::fs::PermissionsExt;
    let later = f.env.root.join("later-owner");
    assert_eq!(
        std::fs::read_link(f.env.directory.join("bulk_0000")).unwrap(),
        later
    );
    assert_eq!(
        std::fs::metadata(&later).unwrap().permissions().mode() & 0o777,
        0
    );
    std::fs::set_permissions(&later, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(std::fs::read(&later).unwrap(), b"later owner bytes");
    for index in 1..FILES {
        assert_eq!(
            f.env.file(&format!("bulk_{index:04}")).as_deref(),
            Some("before")
        );
    }
    for (path, bytes) in git {
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }
    assert_eq!(f.env.messages(&f.session), f.messages);
    assert_eq!(f.model.requests().len(), requests);
}

fn kill_completed_creation(f: &Fixture) {
    use rusqlite::OptionalExtension;
    let selection = reviewed_selection(&f.env, &f.first, "both", "commit");
    let preview = reviewed_preview(&f.env, &f.session, &selection);
    let mut body = selection;
    body["confirmation"] = preview["confirmation"].clone();
    let mut pending = Confirm(
        f.env
            .command(&[
                "api",
                "POST",
                &format!("/sessions/{}/rewind/confirm", f.session),
                "-d",
                &body.to_string(),
            ])
            .stdout(std::fs::File::create(f.env.root.join("zero-confirm.stdout")).unwrap())
            .stderr(std::fs::File::create(f.env.root.join("zero-confirm.stderr")).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while f.env.file("bulk_0000").as_deref() != Some("before") {
        assert!(
            pending.0.try_wait().unwrap().is_none(),
            "restore exited before source creation"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "first source creation was not observed"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let database = std::fs::read_dir(f.env.root.join("cyber-home/data"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "db"))
        .unwrap();
    let db = rusqlite::Connection::open(database).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    db.execute_batch("BEGIN IMMEDIATE").unwrap();
    let origin: Option<String> = db.query_row("SELECT json_extract(data,'$.snapshot_origin') FROM event e WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1 AND json_extract(data,'$.snapshot_source_admitted')=1 AND json_extract(data,'$.snapshot_restore_receipt') IS NULL AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq)", [&f.session], |row| row.get(0)).optional().unwrap();
    let origin: Value =
        serde_json::from_str(&origin.expect("writer cut missed native completion boundary"))
            .unwrap();
    let intent = std::fs::read_dir(f.env.root.join("cyber-home/data/snapshot/global"))
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .join("journals")
                .join(origin["binding"]["journal_id"].as_str().unwrap())
                .join("intent.json")
        })
        .find(|path| path.exists())
        .unwrap();
    wait_completed_intent(&f.env, &intent, &mut pending.0, deadline);
    kill_registered_service(&f.env);
    assert!(!pending.0.wait().unwrap().success());
    db.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        f.env.file(&format!("bulk_{:04}", FILES - 1)).as_deref(),
        Some("before")
    );
}
struct Confirm(std::process::Child);
impl Drop for Confirm {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait_completed_intent(
    env: &Env,
    intent: &std::path::Path,
    pending: &mut std::process::Child,
    deadline: std::time::Instant,
) {
    loop {
        let record = std::fs::read(intent)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .unwrap_or(Value::Null);
        if record["completed"] == true {
            assert!(
                record
                    .get("acknowledged")
                    .is_none_or(|value| value == false)
            );
            assert_eq!(
                std::fs::read_dir(intent.parent().unwrap()).unwrap().count(),
                2
            );
            break;
        }
        assert!(
            pending.try_wait().unwrap().is_none(),
            "restore exited before completed intent: {}",
            restore_preparation_status(env)
        );
        assert!(
            std::time::Instant::now() < deadline,
            "completed intent was not observed"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
