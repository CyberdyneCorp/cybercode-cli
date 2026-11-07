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
    f.app.runtime.shutdown().await;
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
