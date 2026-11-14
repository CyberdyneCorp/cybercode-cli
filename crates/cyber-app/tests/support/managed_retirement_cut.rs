//! Actual SIGKILL during managed metadata deletion, using only the configuration
//! callback to observe a real unlink. No native source or deletion hook is replaced.
//! The parked child retires an ordinary restore, a paired keep/rollback recovery
//! or a prepared cancellation; the parent resumes each through periodic maintenance.
use super::*;
use cyber_server::runtime::{
    SnapshotPreparationReceipt, SnapshotRecoveryReceipt, snapshot_reclamation::Receipt,
};
use cyber_snapshot::{GitSnapshots, reclamation::Policy};
use std::os::unix::{fs::PermissionsExt, process::ExitStatusExt};

pub(super) struct Control(pub(super) tempfile::TempDir);
impl Control {
    pub(super) fn path(&self) -> &Path {
        self.0.path()
    }
}
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

/// The settled receipt whose metadata the child retires.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Scenario {
    Ordinary,
    Keep,
    Rollback,
    Prepared,
}
impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Keep => "keep",
            Self::Rollback => "rollback",
            Self::Prepared => "prepared",
        }
    }
    fn parse(name: &str) -> Self {
        [Self::Ordinary, Self::Keep, Self::Rollback, Self::Prepared]
            .into_iter()
            .find(|scenario| scenario.name() == name)
            .unwrap()
    }
    pub(super) fn action(self) -> &'static str {
        self.name()
    }
    /// Rebuild the exact typed receipt; untagged deserialization would be ambiguous.
    fn receipt(self, raw: &Value) -> Receipt {
        let raw = raw.clone();
        match self {
            Self::Ordinary => Receipt::Restore(serde_json::from_value(raw).unwrap()),
            Self::Keep | Self::Rollback => {
                Receipt::Recovery(serde_json::from_value::<SnapshotRecoveryReceipt>(raw).unwrap())
            }
            Self::Prepared => Receipt::Preparation(Box::new(
                serde_json::from_value::<SnapshotPreparationReceipt>(raw).unwrap(),
            )),
        }
    }
    /// The receipt inside a lookup or acknowledgement reply, normalized by type.
    fn replied(self, data: &Value) -> Value {
        let raw = match self {
            Self::Keep | Self::Rollback => &data["receipt"],
            Self::Ordinary | Self::Prepared => data,
        };
        serde_json::to_value(self.receipt(raw)).unwrap()
    }
}

/// A completed payload plan whose managed metadata is ready for retirement.
pub(super) struct Retiring {
    pub(super) store: Arc<cyber_store::Store>,
    pub(super) paths: Paths,
    pub(super) project: PathBuf,
    pub(super) directory: PathBuf,
    pub(super) session: String,
    pub(super) receipt: Receipt,
    /// Client lookup path; `/acknowledge` is appended for explicit acknowledgement.
    pub(super) endpoint: String,
    /// Keeps the live App (and any fixture) open until the process is killed.
    pub(super) _owner: Box<dyn std::any::Any>,
}

pub(super) fn maintenance(paths: &Paths, policy: Arc<dyn Policy>) -> Arc<GitSnapshots> {
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
pub(super) fn paths_at(root: &Path) -> Paths {
    Paths {
        data: root.join("data"),
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
        tmp: root.join("tmp"),
    }
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
/// Make the settled owner eligible for the default retention period.
pub(super) fn age(store: &cyber_store::Store, activity: String) {
    store
        .transaction(move |db| {
            db.execute(
                "UPDATE event SET time=time-?1 WHERE aggregate_id=?2",
                rusqlite::params![8i64 * 86400000, activity],
            )?;
            Ok(())
        })
        .unwrap();
}
/// Real payload reclamation must complete before metadata retirement can start.
pub(super) async fn reclaim_payloads(
    paths: &Paths,
    store: &Arc<cyber_store::Store>,
    directory: &Path,
    receipt: &Receipt,
) {
    let policy = cyber_app::snapshot_reclamation::policy(store.clone());
    let snapshots = maintenance(paths, policy.clone());
    assert!(
        snapshots
            .reclaim(directory.to_str().unwrap())
            .await
            .unwrap()
            > 0
    );
    assert!(policy.record(receipt).unwrap().unwrap().completed);
}
pub(super) fn journal_path(data: &Path, id: &str) -> PathBuf {
    std::fs::read_dir(data.join("snapshot/global"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("journals").join(id))
        .find(|path| path.exists())
        .unwrap()
}
fn leaf(binding: &cyber_core::worktrees::CheckoutBinding, suffix: &str) -> PathBuf {
    binding
        .common_dir
        .join("cyber-worktree-activity")
        .join(&binding.worktree_id)
        .join(format!("{}{suffix}", binding.owner_id))
}
fn suffix(cut: &str) -> &'static str {
    if cut == "acknowledgement" {
        ".snapshot-ack"
    } else {
        ".lock"
    }
}

async fn ordinary() -> Retiring {
    let mut f = Fixture::managed().await;
    // The parent owns this fixture through killing/reopening, including any
    // error before the observed cut. Keep all evidence on a child failure.
    f._temp.disable_cleanup(true);
    let receipt = restore_tracked(&f).await;
    age(&f.app.store, receipt.origin.activity_id.clone());
    let endpoint = format!(
        "/sessions/{}/rewind/restores/{}",
        f.session, receipt.origin.activity_id
    );
    let receipt = Receipt::Restore(receipt);
    reclaim_payloads(&f.paths, &f.app.store, &f.directory, &receipt).await;
    Retiring {
        store: f.app.store.clone(),
        paths: f.paths.clone(),
        project: f.project.clone(),
        directory: f.directory.clone(),
        session: f.session.clone(),
        receipt,
        endpoint,
        _owner: Box::new(f),
    }
}

#[tokio::test]
async fn managed_metadata_retirement_child() {
    let Ok(control) = std::env::var("CYBER_MANAGED_METADATA_CUT_CONTROL") else {
        return;
    };
    let control = PathBuf::from(control);
    let cut = std::env::var("CYBER_MANAGED_METADATA_CUT").unwrap();
    let scenario = Scenario::parse(&std::env::var("CYBER_MANAGED_METADATA_SCENARIO").unwrap());
    let retiring = match scenario {
        Scenario::Ordinary => ordinary().await,
        _ => {
            let root = PathBuf::from(std::env::var("CYBER_MANAGED_METADATA_ROOT").unwrap());
            super::managed_recovery_cut::resolved(scenario, &root).await
        }
    };
    let bindings = bindings(&retiring.paths, &retiring.receipt);
    write_fixture(&control, &retiring, scenario, &bindings);
    let watch: Vec<_> = bindings
        .iter()
        .map(|binding| leaf(binding, suffix(&cut)))
        .collect();
    let marker = control.join("ready");
    let config = Arc::new(move |_: &Path| {
        if watch.iter().any(|path| !path.exists()) {
            std::fs::write(&marker, b"observed exact native checkout unlink").unwrap();
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        json!({})
    });
    let paths = &retiring.paths;
    let snapshots = GitSnapshots::with_reclamation(
        paths.data.clone(),
        config,
        vec![
            paths.config.clone(),
            paths.state.clone(),
            paths.cache.clone(),
            paths.tmp.clone(),
        ],
        None,
        Some(cyber_app::snapshot_reclamation::policy(
            retiring.store.clone(),
        )),
    );
    snapshots
        .retire(
            retiring.directory.to_str().unwrap(),
            retiring.receipt.clone(),
        )
        .await
        .unwrap();
    panic!("managed metadata worker passed its selected cut");
}
/// Every distinct checkout binding recorded by the receipt owners' source phases.
fn bindings(paths: &Paths, receipt: &Receipt) -> Vec<cyber_core::worktrees::CheckoutBinding> {
    let mut all = Vec::new();
    for owner in receipt.owners().unwrap() {
        let journal = journal_path(&paths.data, &owner.origin.binding.journal_id);
        let phase = journal
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("workers")
            .join(owner.lifetime.source_phase.as_ref().unwrap());
        let phase: Value = serde_json::from_slice(&std::fs::read(phase).unwrap()).unwrap();
        let found: Vec<cyber_core::worktrees::CheckoutBinding> =
            serde_json::from_value(phase["checkout_bindings"].clone()).unwrap();
        for binding in found {
            if !all.contains(&binding) {
                all.push(binding);
            }
        }
    }
    assert!(!all.is_empty());
    all
}
fn write_fixture(
    control: &Path,
    retiring: &Retiring,
    scenario: Scenario,
    bindings: &[cyber_core::worktrees::CheckoutBinding],
) {
    let index = PathBuf::from(git(
        &retiring.directory,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    ));
    let config = retiring.project.join(".git/config");
    let creations = bindings.iter().map(|binding| {
        binding
            .common_dir
            .join("cyber-worktrees")
            .join(format!("{}.json", binding.name))
    });
    let protected = [index, config]
        .into_iter()
        .chain(creations)
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            json!({"path":path,"bytes":bytes})
        })
        .collect::<Vec<_>>();
    let mut journals = Vec::new();
    for owner in retiring.receipt.owners().unwrap() {
        let journal = journal_path(&retiring.paths.data, &owner.origin.binding.journal_id);
        if !journals.contains(&journal) {
            journals.push(journal);
        }
    }
    let data = json!({"scenario":scenario.name(),"root":retiring.paths.data.parent().unwrap(),
        "directory":retiring.directory,"journals":journals,"receipt":retiring.receipt,
        "endpoint":retiring.endpoint,"session":retiring.session,
        "session_sequence":sequence(&retiring.store,&retiring.session),"protected":protected});
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
        kill_at_cut(control.path(), cut, Scenario::Ordinary, None).await;
        assert_eq!(resume(control.path(), cut).await, 1);
    }
}
pub(super) async fn kill_at_cut(
    control: &Path,
    cut: &str,
    scenario: Scenario,
    root: Option<&Path>,
) {
    let log = std::fs::File::create(control.join("worker.log")).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "managed_retirement_cut::managed_metadata_retirement_child",
            "--nocapture",
        ])
        .env("CYBER_MANAGED_METADATA_CUT_CONTROL", control)
        .env("CYBER_MANAGED_METADATA_CUT", cut)
        .env("CYBER_MANAGED_METADATA_SCENARIO", scenario.name());
    if let Some(root) = root {
        command.env("CYBER_MANAGED_METADATA_ROOT", root);
    }
    // Inherit the short CI fixture root. Nesting another TempDir under the
    // control directory exceeds Linux's Unix socket limit during worktree
    // setup. The child retains its own fixture and reports its actual root.
    let mut child = command
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    // Recovery/cancellation setup reopens an App and reclaims every payload first.
    let seconds = if scenario == Scenario::Ordinary {
        90
    } else {
        180
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds);
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
/// Resume after the kill and return how many checkout bindings the frozen plan holds.
pub(super) async fn resume(control: &Path, cut: &str) -> usize {
    let data: Value =
        serde_json::from_slice(&std::fs::read(control.join("fixture.json")).unwrap()).unwrap();
    let scenario = Scenario::parse(data["scenario"].as_str().unwrap());
    let root = PathBuf::from(data["root"].as_str().unwrap());
    let directory = PathBuf::from(data["directory"].as_str().unwrap());
    let journals: Vec<PathBuf> = serde_json::from_value(data["journals"].clone()).unwrap();
    let retained = scenario.receipt(&data["receipt"]);
    let paths = paths_at(&root);
    let store = Arc::new(
        cyber_store::Store::open(cyber_store::StoreOptions::new(
            DatabaseLocation::File(paths.data.join("cyber.db")),
            cyber_server::runtime::Runtime::registry(),
        ))
        .unwrap(),
    );
    let policy = cyber_app::snapshot_reclamation::policy(store.clone());
    let before = policy.record(&retained).unwrap().unwrap();
    assert_cut_state(&before, &journals, cut);
    let frozen = before.metadata.unwrap();
    assert_eq!(
        frozen
            .checkouts
            .iter()
            .map(|plan| plan.binding.clone())
            .collect::<Vec<_>>(),
        bindings(&paths, &retained)
    );
    let owner = later_owner(&root, &directory);
    let snapshots = maintenance(&paths, policy.clone());
    snapshots.gc().await;
    let completed = policy.record(&retained).unwrap().unwrap();
    assert!(completed.retired);
    assert_eq!(completed.metadata.as_ref(), Some(&frozen));
    assert!(journals.iter().all(|journal| !journal.exists()));
    assert_eq!(
        snapshots
            .retire(directory.to_str().unwrap(), retained)
            .await
            .unwrap(),
        0
    );
    for plan in &frozen.checkouts {
        assert_checkout_metadata_removed(&plan.binding);
    }
    assert_protected(&data);
    assert_acknowledged(&paths, &store, &data, scenario, &owner).await;
    std::fs::remove_dir_all(root).unwrap();
    frozen.checkouts.len()
}
/// A later owner replaces a source file with a link to an unreadable file.
fn later_owner(root: &Path, directory: &Path) -> PathBuf {
    let owner = root.join("later-owner");
    std::fs::write(&owner, b"owner after managed metadata process death").unwrap();
    std::fs::remove_file(directory.join("tracked")).unwrap();
    std::os::unix::fs::symlink(&owner, directory.join("tracked")).unwrap();
    std::fs::set_permissions(&owner, std::fs::Permissions::from_mode(0o0)).unwrap();
    owner
}
async fn assert_acknowledged(
    paths: &Paths,
    store: &cyber_store::Store,
    data: &Value,
    scenario: Scenario,
    owner: &Path,
) {
    let directory = PathBuf::from(data["directory"].as_str().unwrap());
    let session = data["session"].as_str().unwrap();
    let expected_sequence = data["session_sequence"].as_i64().unwrap();
    assert_eq!(sequence(store, session), expected_sequence);
    let app = build(paths, &directory, paths.data.parent().unwrap()).await;
    let client = Client::embedded(app.embedded()).at(directory.to_str().unwrap());
    let endpoint = data["endpoint"].as_str().unwrap();
    let lookup = client.get(endpoint).await.unwrap();
    assert_eq!(scenario.replied(&lookup["data"]), data["receipt"]);
    let acknowledgement = client
        .post(&format!("{endpoint}/acknowledge"), json!({}))
        .await
        .unwrap();
    assert_eq!(scenario.replied(&acknowledgement["data"]), data["receipt"]);
    assert_eq!(sequence(store, session), expected_sequence);
    assert_eq!(
        std::fs::read_link(directory.join("tracked")).unwrap(),
        owner
    );
    std::fs::set_permissions(owner, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        std::fs::read(owner).unwrap(),
        b"owner after managed metadata process death"
    );
    app.runtime.shutdown().await;
}
/// The cut is exact: an acknowledgement cut removed one acknowledgement and no
/// claim; a claim cut removed every acknowledgement and exactly one claim.
fn assert_cut_state(
    before: &cyber_server::runtime::snapshot_reclamation::Record,
    journals: &[PathBuf],
    cut: &str,
) {
    let frozen = before.metadata.as_ref().unwrap();
    assert!(before.completed && !before.retired && frozen.version == 2);
    let missing = |suffix: &str| {
        frozen
            .checkouts
            .iter()
            .filter(|plan| !leaf(&plan.binding, suffix).exists())
            .count()
    };
    let expected = if cut == "acknowledgement" {
        (1, 0)
    } else {
        (frozen.checkouts.len(), 1)
    };
    assert_eq!(
        (missing(".snapshot-ack"), missing(".lock")),
        expected,
        "{cut} cut"
    );
    for journal in journals {
        assert!(journal.join("intent.json").exists());
    }
}

fn assert_protected(data: &Value) {
    for entry in data["protected"].as_array().unwrap() {
        let path = entry["path"].as_str().unwrap();
        let bytes: Vec<u8> = serde_json::from_value(entry["bytes"].clone()).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}
