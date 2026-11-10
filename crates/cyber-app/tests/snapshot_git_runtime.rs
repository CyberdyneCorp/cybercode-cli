#![cfg(all(
    feature = "native-git-acceptance",
    any(target_os = "linux", target_os = "macos")
))]
//! The real App/Store/worker path in an explicitly non-default acceptance build.
#[path = "support/scripted_model.rs"]
mod scripted_model;
use cyber_app::{App, AppOptions};
use cyber_client::Client;
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_server::runtime::SnapshotRestoreReceipt;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

struct Fixture {
    _temp: tempfile::TempDir,
    app: App,
    client: Client,
    model: scripted_model::ScriptedModel,
    paths: Paths,
    project: PathBuf,
    directory: PathBuf,
    session: String,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self._temp.disable_cleanup(true);
            eprintln!(
                "Preserved failed Git App fixture at {}",
                self._temp.path().display()
            );
        }
    }
}

impl Fixture {
    async fn new(format: &str, scoped: bool) -> Self {
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
        git(
            &root,
            &[
                "init",
                "--quiet",
                &format!("--object-format={format}"),
                project.to_str().unwrap(),
            ],
        );
        let directory = if scoped {
            project.join("scope")
        } else {
            project.clone()
        };
        std::fs::create_dir_all(&directory).unwrap();
        let model = scripted_model::ScriptedModel::start(|_| json!({"content":"Done."}));
        let mut config = model.config();
        config["snapshots"] = json!({"max_file_bytes":4096});
        std::fs::write(paths.config.join("cyber.jsonc"), config.to_string()).unwrap();
        let app = build(&paths, &directory, &root).await;
        let client = Client::embedded(app.embedded()).at(directory.to_str().unwrap());
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
        Self {
            _temp: temp,
            app,
            client,
            model,
            paths,
            project,
            directory,
            session,
        }
    }
    async fn prompt(&self, text: &str, count: usize) -> String {
        self.client
            .post(
                &format!("/sessions/{}/prompt", self.session),
                json!({"parts":[{"type":"text","text":text}]}),
            )
            .await
            .unwrap();
        for _ in 0..1000 {
            let messages = self.messages().await;
            let messages = messages["data"].as_array().unwrap();
            if messages
                .iter()
                .filter(|message| message["kind"] == "assistant")
                .count()
                == count
                && !self.app.runtime.is_running(&self.session)
            {
                return messages
                    .iter()
                    .rfind(|message| message["kind"] == "user")
                    .unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("scripted prompt did not complete");
    }
    async fn messages(&self) -> Value {
        self.client
            .get(&format!("/sessions/{}/messages?order=asc", self.session))
            .await
            .unwrap()
    }
    fn receipt(&self) -> SnapshotRestoreReceipt {
        let session = self.session.clone();
        self.app.store.read(move |db| {
            let raw: String = db.query_row("SELECT json_extract(data,'$.snapshot_restore_receipt') FROM event WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1 AND json_extract(data,'$.snapshot_restore_receipt') IS NOT NULL ORDER BY rowid DESC LIMIT 1", [session], |row| row.get(0))?;
            Ok(serde_json::from_str(&raw).unwrap())
        }).unwrap()
    }
    fn journal(&self, receipt: &SnapshotRestoreReceipt) -> PathBuf {
        std::fs::read_dir(self.paths.data.join("snapshot/global"))
            .unwrap()
            .map(|entry| {
                entry
                    .unwrap()
                    .path()
                    .join("journals")
                    .join(&receipt.origin.binding.journal_id)
            })
            .find(|path| path.exists())
            .unwrap()
    }
}
async fn build(paths: &Paths, directory: &Path, root: &Path) -> App {
    App::build(AppOptions {
        paths: paths.clone(),
        home: root.join("home"),
        database: DatabaseLocation::File(paths.data.join("cyber.db")),
        default_directory: directory.into(),
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
    .unwrap()
}
fn git(directory: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn image(value: u8) -> Vec<u8> {
    let mut bytes = vec![value; 2 * 1024 * 1024 + 8192];
    bytes[0] = 0;
    bytes
}

#[tokio::test]
async fn managed_worktree_in_default_data_directory_restores_through_owned_worker() {
    let mut f = Fixture::new("sha1", false).await;
    std::fs::write(f.project.join("tracked"), "initial").unwrap();
    git(&f.project, &["add", "tracked"]);
    git(
        &f.project,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "Initial",
        ],
    );
    let created = f
        .client
        .post(
            "/worktrees",
            json!({"name":"managed", "session":{"model":"test/main"}}),
        )
        .await
        .unwrap();
    f.directory = PathBuf::from(created["data"]["worktree"]["path"].as_str().unwrap());
    f.session = created["data"]["session"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    f.client = Client::embedded(f.app.embedded()).at(f.directory.to_str().unwrap());
    assert!(f.directory.starts_with(f.paths.data.join("worktrees")));
    let index_path = PathBuf::from(git(
        &f.directory,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    ));
    let index = std::fs::read(&index_path).unwrap();
    let config = std::fs::read(f.project.join(".git/config")).unwrap();
    std::fs::write(f.directory.join("tracked"), "before").unwrap();
    let first = f.prompt("before", 1).await;
    std::fs::write(f.directory.join("tracked"), "after").unwrap();
    f.prompt("after", 2).await;
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
    tokio::time::timeout(
        Duration::from_secs(30),
        f.client
            .post(&format!("/sessions/{}/rewind/confirm", f.session), request),
    )
    .await
    .expect("managed confirmation timed out")
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.directory.join("tracked")).unwrap(),
        "before"
    );
    let receipt = f.receipt();
    assert_eq!(
        serde_json::to_value(&receipt.origin).unwrap()["worktree_id"],
        created["data"]["worktree"]["id"]
    );
    let checkout = receipt.checkout.as_ref().unwrap();
    let journal = f.journal(&receipt);
    let phase: Value = serde_json::from_slice(
        &std::fs::read(
            journal
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("workers")
                .join(checkout.lifetime.source_phase.as_ref().unwrap()),
        )
        .unwrap(),
    )
    .unwrap();
    let bindings: Vec<cyber_core::worktrees::CheckoutBinding> =
        serde_json::from_value(phase["checkout_bindings"].clone()).unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(
        bindings[0].worktree_id,
        receipt.origin.worktree_id.as_deref().unwrap()
    );
    assert_eq!(
        checkout
            .lifetime
            .checkout_resources_digest(&bindings)
            .unwrap(),
        checkout.resources_digest
    );
    assert_eq!(std::fs::read(&index_path).unwrap(), index);
    assert_eq!(
        std::fs::read(f.project.join(".git/config")).unwrap(),
        config
    );
    reconcile_after_owner_edit(&f, &receipt).await;
    reclaim_restore_payloads(&f, &receipt).await;
    assert_eq!(std::fs::read(&index_path).unwrap(), index);
    assert_eq!(
        std::fs::read(f.project.join(".git/config")).unwrap(),
        config
    );
    f.app.runtime.shutdown().await;
}

async fn reclaim_restore_payloads(f: &Fixture, receipt: &SnapshotRestoreReceipt) {
    use cyber_server::runtime::snapshot_reclamation::Receipt;
    let policy = cyber_app::snapshot_reclamation::policy(f.app.store.clone());
    let snapshots = cyber_snapshot::GitSnapshots::with_reclamation(
        f.paths.data.clone(),
        Arc::new(|_| json!({"snapshots":{"retention_days":7}})),
        vec![
            f.paths.config.clone(),
            f.paths.state.clone(),
            f.paths.cache.clone(),
        ],
        None,
        Some(policy.clone()),
    );
    let journal = f.journal(receipt);
    let intent = std::fs::read(journal.join("intent.json")).unwrap();
    let inference = f.model.requests().len();
    assert_eq!(
        snapshots
            .reclaim(f.directory.to_str().unwrap())
            .await
            .unwrap(),
        0
    );
    let owner = receipt.origin.activity_id.clone();
    f.app
        .store
        .transaction(move |db| {
            db.execute(
                "UPDATE event SET time=time-?1 WHERE aggregate_id=?2",
                rusqlite::params![8i64 * 86400000, owner],
            )?;
            Ok(())
        })
        .unwrap();
    assert_replaced_link_payload_refused(f, receipt, &snapshots, policy.as_ref()).await;
    assert!(
        snapshots
            .reclaim(f.directory.to_str().unwrap())
            .await
            .unwrap()
            > 0
    );
    let record = policy
        .record(&Receipt::Restore(receipt.clone()))
        .unwrap()
        .unwrap();
    assert!(record.completed);
    assert_eq!(
        snapshots
            .reclaim(f.directory.to_str().unwrap())
            .await
            .unwrap(),
        0
    );
    assert_retained_intent_metadata(&journal, &intent);
    retire_restore_metadata(f, receipt, &snapshots, policy.as_ref()).await;
    assert_reclaimed_source_and_receipt(f, receipt, inference).await;
}

fn assert_retained_intent_metadata(journal: &Path, intent: &[u8]) {
    assert_eq!(std::fs::read(journal.join("intent.json")).unwrap(), intent);
    let header: Value = serde_json::from_slice(intent).unwrap();
    assert_eq!(header["version"], 7);
    assert!(header["updates"].is_object());
    assert!(journal.join("intent.next").is_file());
    assert_eq!(std::fs::read_dir(journal).unwrap().count(), 2);
}

async fn retire_restore_metadata(
    f: &Fixture,
    receipt: &SnapshotRestoreReceipt,
    snapshots: &cyber_snapshot::GitSnapshots,
    policy: &dyn cyber_snapshot::reclamation::Policy,
) {
    use cyber_server::runtime::snapshot_reclamation::Receipt;
    let journal = f.journal(receipt);
    let namespace = journal.parent().unwrap().parent().unwrap().to_path_buf();
    let checkout = receipt.checkout.as_ref().unwrap();
    let phase = namespace
        .join("workers")
        .join(checkout.lifetime.source_phase.as_ref().unwrap());
    let data: Value = serde_json::from_slice(&std::fs::read(&phase).unwrap()).unwrap();
    let bindings: Vec<cyber_core::worktrees::CheckoutBinding> =
        serde_json::from_value(data["checkout_bindings"].clone()).unwrap();
    let creations = bindings
        .iter()
        .map(|binding| {
            let path = binding
                .common_dir
                .join("cyber-worktrees")
                .join(format!("{}.json", binding.name));
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    let retained = Receipt::Restore(receipt.clone());
    assert!(
        snapshots
            .retire(f.directory.to_str().unwrap(), retained.clone())
            .await
            .unwrap()
            > 0
    );
    let record = policy.record(&retained).unwrap().unwrap();
    assert!(record.completed && record.retired);
    let plan = record.metadata.unwrap();
    assert_managed_retirement_frame(&plan, &record.plan);
    assert_eq!(
        plan.checkouts
            .iter()
            .map(|plan| plan.binding.clone())
            .collect::<Vec<_>>(),
        bindings
    );
    assert!(!journal.exists());
    assert_eq!(
        std::fs::symlink_metadata(phase).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    for binding in &bindings {
        assert_checkout_metadata_removed(binding);
    }
    for (path, bytes) in creations {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    assert_eq!(
        snapshots
            .retire(f.directory.to_str().unwrap(), retained)
            .await
            .unwrap(),
        0
    );
}
fn assert_managed_retirement_frame(
    plan: &cyber_server::runtime::snapshot_reclamation::metadata::Plan,
    payload: &cyber_server::runtime::snapshot_reclamation::Plan,
) {
    plan.validate(payload).unwrap();
    if plan.checkouts.is_empty() {
        return;
    }
    for mutation in ["receipt", "resources", "owner", "duplicate", "alias"] {
        let mut foreign = plan.clone();
        match mutation {
            "receipt" => foreign.checkouts[0].receipt_digest = "f".repeat(64),
            "resources" => foreign.checkouts[0].resources_digest = "f".repeat(64),
            "owner" => foreign.checkouts[0].binding.owner_id = "op_foreign".into(),
            "duplicate" => foreign.checkouts.push(foreign.checkouts[0].clone()),
            "alias" => {
                foreign.checkouts[0].acknowledgement.identity =
                    foreign.checkouts[0].claim.identity.clone()
            }
            _ => unreachable!(),
        }
        assert!(foreign.validate(payload).is_err(), "{mutation}");
    }
}

fn assert_checkout_metadata_removed(binding: &cyber_core::worktrees::CheckoutBinding) {
    let directory = binding
        .common_dir
        .join("cyber-worktree-activity")
        .join(&binding.worktree_id);
    for suffix in [".lock", ".snapshot-ack"] {
        let path = directory.join(format!("{}{suffix}", binding.owner_id));
        assert_eq!(
            std::fs::symlink_metadata(path).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }
}

async fn assert_replaced_link_payload_refused(
    f: &Fixture,
    receipt: &SnapshotRestoreReceipt,
    snapshots: &cyber_snapshot::GitSnapshots,
    policy: &dyn cyber_snapshot::reclamation::Policy,
) {
    use cyber_server::runtime::snapshot_reclamation::Receipt;
    let journal = f.journal(receipt);
    let intent: Value =
        serde_json::from_slice(&std::fs::read(journal.join("intent.json")).unwrap()).unwrap();
    let slot = intent["changes"]
        .as_array()
        .unwrap()
        .iter()
        .position(|change| change["before"]["kind"] == "symlink");
    let Some(slot) = slot else { return };
    let path = journal.join(format!("stage_{slot}"));
    let target = std::fs::read_link(&path).unwrap();
    assert!(
        !path.exists(),
        "journal link target must be dangling in this fixture"
    );
    let saved = f._temp.path().join("retained-link-payload");
    std::fs::rename(&path, &saved).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let error = snapshots
        .reclaim(f.directory.to_str().unwrap())
        .await
        .unwrap_err();
    assert!(error.contains("original inode changed"), "{error}");
    assert!(
        policy
            .record(&Receipt::Restore(receipt.clone()))
            .unwrap()
            .is_none()
    );
    assert_eq!(std::fs::read_link(&path).unwrap(), target);
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&saved, &path).unwrap();
}

async fn assert_reclaimed_source_and_receipt(
    f: &Fixture,
    receipt: &SnapshotRestoreReceipt,
    inference: usize,
) {
    let reopened = build(&f.paths, &f.directory, f._temp.path()).await;
    let client = Client::embedded(reopened.embedded()).at(f.directory.to_str().unwrap());
    receipt_roundtrip(&client, &f.session, receipt).await;
    assert!(
        std::fs::symlink_metadata(f.directory.join("tracked"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(f._temp.path().join("later-owner")).unwrap(),
        "later owner bytes"
    );
    assert_eq!(f.model.requests().len(), inference);
    reopened.runtime.shutdown().await;
}

#[tokio::test]
async fn actual_git_worker_commits_receipt_and_reconciles_without_source_replay() {
    for (format, scoped) in [("sha1", false), ("sha1", true), ("sha256", true)] {
        let f = Fixture::new(format, scoped).await;
        let root_lock = cyber_server::workspace::mutation_lock(&f.project).unwrap();
        let scope_lock = cyber_server::workspace::mutation_lock(&f.directory).unwrap();
        assert!(
            Arc::ptr_eq(&root_lock, &scope_lock),
            "{format} scoped={scoped}"
        );
        std::fs::write(f.directory.join("tracked"), image(b'b')).unwrap();
        std::fs::write(f.directory.join("owner"), vec![b'o'; 8192]).unwrap();
        std::os::unix::fs::symlink("tracked", f.directory.join("link")).unwrap();
        let tracked = if scoped { "scope/tracked" } else { "tracked" };
        git(&f.project, &["add", "--", tracked]);
        let index = std::fs::read(f.project.join(".git/index")).unwrap();
        let user_config = std::fs::read(f.project.join(".git/config")).unwrap();
        let first = f.prompt("before", 1).await;
        std::fs::write(f.directory.join("tracked"), image(b'a')).unwrap();
        std::fs::remove_file(f.directory.join("link")).unwrap();
        std::os::unix::fs::symlink("owner", f.directory.join("link")).unwrap();
        f.prompt("after", 2).await;
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
        f.client
            .post(&format!("/sessions/{}/rewind/confirm", f.session), request)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(f.directory.join("tracked")).unwrap(),
            image(b'b')
        );
        assert_eq!(
            std::fs::read_link(f.directory.join("link")).unwrap(),
            PathBuf::from("tracked")
        );
        assert_eq!(
            std::fs::read(f.directory.join("owner")).unwrap(),
            vec![b'o'; 8192]
        );
        assert_eq!(std::fs::read(f.project.join(".git/index")).unwrap(), index);
        assert_eq!(
            std::fs::read(f.project.join(".git/config")).unwrap(),
            user_config
        );
        let receipt = f.receipt();
        assert!(receipt.checkout.is_some());
        let journal = f.journal(&receipt);
        let intent: Value =
            serde_json::from_slice(&std::fs::read(journal.join("intent.json")).unwrap()).unwrap();
        assert_eq!(intent["backend"], "git");
        assert_eq!(intent["completed"], true);
        assert_eq!(intent["acknowledged"], true);
        assert_eq!(receipt.origin.binding.directory, f.directory);
        reconcile_after_owner_edit(&f, &receipt).await;
        let intent: Value =
            serde_json::from_slice(&std::fs::read(journal.join("intent.json")).unwrap()).unwrap();
        assert!(
            intent["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change["before"]["kind"] == "symlink")
        );
        reclaim_restore_payloads(&f, &receipt).await;
        f.app.runtime.shutdown().await;
    }
}
struct Preserved {
    messages: Value,
    inference: usize,
    outside: PathBuf,
}
async fn preserved_owner_edit(f: &Fixture) -> Preserved {
    let messages = f.messages().await;
    let inference = f.model.requests().len();
    let outside = f._temp.path().join("later-owner");
    std::fs::write(&outside, "later owner bytes").unwrap();
    std::fs::remove_file(f.directory.join("tracked")).unwrap();
    std::os::unix::fs::symlink(&outside, f.directory.join("tracked")).unwrap();
    Preserved {
        messages,
        inference,
        outside,
    }
}
async fn receipt_roundtrip(client: &Client, session: &str, receipt: &SnapshotRestoreReceipt) {
    let endpoint = format!(
        "/sessions/{session}/rewind/restores/{}",
        receipt.origin.activity_id
    );
    let lookup = client.get(&endpoint).await.unwrap();
    assert_eq!(
        serde_json::from_value::<SnapshotRestoreReceipt>(lookup["data"].clone()).unwrap(),
        *receipt
    );
    let acknowledgement = client
        .post(&format!("{endpoint}/acknowledge"), json!({}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_value::<SnapshotRestoreReceipt>(acknowledgement["data"].clone()).unwrap(),
        *receipt
    );
}
async fn assert_preserved(f: &Fixture, client: &Client, state: &Preserved) {
    assert_eq!(
        std::fs::read_link(f.directory.join("tracked")).unwrap(),
        state.outside
    );
    assert_eq!(std::fs::read(&state.outside).unwrap(), b"later owner bytes");
    assert_eq!(
        client
            .get(&format!("/sessions/{}/messages?order=asc", f.session))
            .await
            .unwrap(),
        state.messages
    );
    assert_eq!(f.model.requests().len(), state.inference);
}
async fn reconcile_after_owner_edit(f: &Fixture, receipt: &SnapshotRestoreReceipt) {
    let state = preserved_owner_edit(f).await;
    for _ in 0..2 {
        receipt_roundtrip(&f.client, &f.session, receipt).await;
    }
    assert_preserved(f, &f.client, &state).await;
    f.app.runtime.shutdown().await;
    let reopened = build(&f.paths, &f.directory, f._temp.path()).await;
    let client = Client::embedded(reopened.embedded()).at(f.directory.to_str().unwrap());
    receipt_roundtrip(&client, &f.session, receipt).await;
    assert_preserved(f, &client, &state).await;
    reopened.runtime.shutdown().await;
}
