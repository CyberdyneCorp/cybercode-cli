//! Actual SIGKILL during managed metadata deletion, using only the configuration
//! callback to observe a real unlink. No native source or deletion hook is replaced.
use super::*;
use cyber_server::runtime::snapshot_reclamation::Receipt;
use cyber_snapshot::{GitSnapshots, reclamation::Policy};
use std::os::unix::{fs::PermissionsExt, process::ExitStatusExt};

struct Control(tempfile::TempDir);
impl Drop for Control {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.disable_cleanup(true);
            eprintln!(
                "Preserved managed retirement cut at {}",
                self.0.path().display()
            );
        }
    }
}
fn maintenance(paths: &Paths, policy: Arc<dyn Policy>) -> Arc<GitSnapshots> {
    GitSnapshots::with_reclamation(
        paths.data.clone(),
        Arc::new(|_| json!({})),
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
fn sequence(store: &cyber_store::Store, session: &str) -> i64 {
    let session = session.to_owned();
    store
        .read(move |db| {
            Ok(db.query_row(
                "SELECT coalesce(max(seq),-1) FROM event WHERE aggregate_id=?1",
                [session],
                |row| row.get(0),
            )?)
        })
        .unwrap()
}

#[tokio::test]
async fn managed_metadata_retirement_child() {
    let Ok(control) = std::env::var("CYBER_MANAGED_METADATA_CUT_CONTROL") else {
        return;
    };
    let control = PathBuf::from(control);
    let cut = std::env::var("CYBER_MANAGED_METADATA_CUT").unwrap();
    let mut f = Fixture::managed().await;
    // The parent owns this fixture through killing/reopening, including any
    // error before the observed cut. Keep all evidence on a child failure.
    f._temp.disable_cleanup(true);
    let receipt = restore_tracked(&f).await;
    let policy = cyber_app::snapshot_reclamation::policy(f.app.store.clone());
    let snapshots = maintenance(&f.paths, policy.clone());
    let activity = receipt.origin.activity_id.clone();
    f.app
        .store
        .transaction(move |db| {
            db.execute(
                "UPDATE event SET time=time-?1 WHERE aggregate_id=?2",
                rusqlite::params![8i64 * 86400000, activity],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(
        snapshots
            .reclaim(f.directory.to_str().unwrap())
            .await
            .unwrap()
            > 0
    );
    let journal = f.journal(&receipt);
    let phase = journal
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("workers")
        .join(
            receipt
                .checkout
                .as_ref()
                .unwrap()
                .lifetime
                .source_phase
                .as_ref()
                .unwrap(),
        );
    let phase: Value = serde_json::from_slice(&std::fs::read(phase).unwrap()).unwrap();
    let binding: cyber_core::worktrees::CheckoutBinding =
        serde_json::from_value(phase["checkout_bindings"][0].clone()).unwrap();
    let activity = binding
        .common_dir
        .join("cyber-worktree-activity")
        .join(&binding.worktree_id);
    let watch = activity.join(format!(
        "{}{}",
        binding.owner_id,
        if cut == "acknowledgement" {
            ".snapshot-ack"
        } else {
            ".lock"
        }
    ));
    write_fixture(&control, &f, &receipt, &journal, &binding);
    let marker = control.join("ready");
    let config = Arc::new(move |_: &Path| {
        if !watch.exists() {
            std::fs::write(&marker, b"observed exact native checkout unlink").unwrap();
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        json!({})
    });
    let snapshots = GitSnapshots::with_reclamation(
        f.paths.data.clone(),
        config,
        vec![
            f.paths.config.clone(),
            f.paths.state.clone(),
            f.paths.cache.clone(),
            f.paths.tmp.clone(),
        ],
        None,
        Some(policy),
    );
    snapshots
        .retire(f.directory.to_str().unwrap(), Receipt::Restore(receipt))
        .await
        .unwrap();
    panic!("managed metadata worker passed its selected cut");
}
fn write_fixture(
    control: &Path,
    f: &Fixture,
    receipt: &SnapshotRestoreReceipt,
    journal: &Path,
    binding: &cyber_core::worktrees::CheckoutBinding,
) {
    let index = PathBuf::from(git(
        &f.directory,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    ));
    let config = f.project.join(".git/config");
    let creation = binding
        .common_dir
        .join("cyber-worktrees")
        .join(format!("{}.json", binding.name));
    let protected = [index, config, creation]
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            json!({"path":path,"bytes":bytes})
        })
        .collect::<Vec<_>>();
    let data = json!({"root":f._temp.path(),"directory":f.directory,"journal":journal,"receipt":receipt,
        "session":f.session,"session_sequence":sequence(&f.app.store,&f.session),"protected":protected});
    std::fs::write(
        control.join("fixture.json"),
        serde_json::to_vec(&data).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn managed_metadata_process_death_resumes_frozen_checkouts_through_periodic_maintenance() {
    for cut in ["acknowledgement", "claim"] {
        let control = Control(tempfile::tempdir().unwrap());
        kill_at_cut(control.0.path(), cut).await;
        resume(control.0.path(), cut).await;
    }
}
async fn kill_at_cut(control: &Path, cut: &str) {
    let log = std::fs::File::create(control.join("worker.log")).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "managed_retirement_cut::managed_metadata_retirement_child",
            "--nocapture",
        ])
        .env("CYBER_MANAGED_METADATA_CUT_CONTROL", control)
        .env("CYBER_MANAGED_METADATA_CUT", cut)
        .env("TMPDIR", control)
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    let ready = loop {
        if control.join("ready").exists() {
            break true;
        }
        if child.try_wait().unwrap().is_some() || std::time::Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let _ = child.kill();
    let status = child.wait().unwrap();
    assert!(
        ready,
        "missed {cut} native cut; retained {}: {}",
        control.display(),
        std::fs::read_to_string(control.join("worker.log")).unwrap()
    );
    assert_eq!(status.signal(), Some(9));
}
async fn resume(control: &Path, cut: &str) {
    let data: Value =
        serde_json::from_slice(&std::fs::read(control.join("fixture.json")).unwrap()).unwrap();
    let root = PathBuf::from(data["root"].as_str().unwrap());
    let directory = PathBuf::from(data["directory"].as_str().unwrap());
    let journal = PathBuf::from(data["journal"].as_str().unwrap());
    let receipt: SnapshotRestoreReceipt = serde_json::from_value(data["receipt"].clone()).unwrap();
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
    let retained = Receipt::Restore(receipt.clone());
    let before = policy.record(&retained).unwrap().unwrap();
    assert_cut_state(&before, &journal, cut);
    let frozen = before.metadata.unwrap();
    let binding = frozen.checkouts[0].binding.clone();
    let owner = root.join("later-owner");
    std::fs::write(&owner, b"owner after managed metadata process death").unwrap();
    std::fs::remove_file(directory.join("tracked")).unwrap();
    std::os::unix::fs::symlink(&owner, directory.join("tracked")).unwrap();
    std::fs::set_permissions(&owner, std::fs::Permissions::from_mode(0o0)).unwrap();
    let snapshots = maintenance(&paths, policy.clone());
    snapshots.gc().await;
    let completed = policy.record(&retained).unwrap().unwrap();
    assert!(completed.retired);
    assert_eq!(completed.metadata, Some(frozen));
    assert!(!journal.exists());
    assert_eq!(
        snapshots
            .retire(directory.to_str().unwrap(), retained)
            .await
            .unwrap(),
        0
    );
    assert_checkout_metadata_removed(&binding);
    assert_protected(&data);
    let session = data["session"].as_str().unwrap();
    let expected_sequence = data["session_sequence"].as_i64().unwrap();
    assert_eq!(sequence(&store, session), expected_sequence);
    let app = build(&paths, &directory, &root).await;
    let client = Client::embedded(app.embedded()).at(directory.to_str().unwrap());
    receipt_roundtrip(&client, session, &receipt).await;
    assert_eq!(sequence(&store, session), expected_sequence);
    assert_eq!(
        std::fs::read_link(directory.join("tracked")).unwrap(),
        owner
    );
    std::fs::set_permissions(&owner, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        std::fs::read(owner).unwrap(),
        b"owner after managed metadata process death"
    );
    app.runtime.shutdown().await;
}
fn assert_cut_state(
    before: &cyber_server::runtime::snapshot_reclamation::Record,
    journal: &Path,
    cut: &str,
) {
    let frozen = before.metadata.as_ref().unwrap();
    assert!(before.completed && !before.retired && frozen.version == 2);
    let binding = &frozen.checkouts[0].binding;
    let activity = binding
        .common_dir
        .join("cyber-worktree-activity")
        .join(&binding.worktree_id);
    assert!(
        !activity
            .join(format!("{}.snapshot-ack", binding.owner_id))
            .exists()
    );
    assert_eq!(
        activity.join(format!("{}.lock", binding.owner_id)).exists(),
        cut == "acknowledgement"
    );
    assert!(journal.join("intent.json").exists());
}

fn assert_protected(data: &Value) {
    for entry in data["protected"].as_array().unwrap() {
        let path = entry["path"].as_str().unwrap();
        let bytes: Vec<u8> = serde_json::from_value(entry["bytes"].clone()).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}
