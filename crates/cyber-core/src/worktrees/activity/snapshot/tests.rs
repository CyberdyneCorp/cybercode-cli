//! Real process death inside private construction. Partial constructor evidence
//! stays independently fenced: no stable claim or acknowledgement appears, it is
//! listed read-only, and removal and retirement refuse it as same-owner unknown
//! constructor evidence. Nothing here grants construction cleanup authority.
use super::*;
use crate::worktrees::{CheckoutActivity, RemovalActivity};
use std::io::Read;

const ROOT: &str = "CYBER_SNAPSHOT_CONSTRUCTION_ROOT";
const PHASE: &str = "CYBER_SNAPSHOT_CONSTRUCTION_PHASE";
const FENCE: &str = "same-owner unknown constructor";

fn fixture(root: &Path) -> Managed {
    let common_dir = root.join("common");
    let path = root.join("work");
    std::fs::create_dir_all(common_dir.join("cyber-worktrees")).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    let managed = Managed {
        id: "wt_fixture".into(),
        name: "fixture".into(),
        path,
        branch: "cyber/fixture".into(),
        base: "main".into(),
        common_dir,
        ready: true,
        included: vec![],
    };
    std::fs::write(
        managed.common_dir.join("cyber-worktrees/fixture.json"),
        serde_json::to_vec(&managed).unwrap(),
    )
    .unwrap();
    std::fs::write(managed.common_dir.join("cyber-worktree.lock"), "").unwrap();
    managed
}
fn stored(root: &Path) -> Managed {
    serde_json::from_slice(
        &std::fs::read(root.join("common/cyber-worktrees/fixture.json")).unwrap(),
    )
    .unwrap()
}
fn owner(managed: &Managed, activity: &str) -> CheckoutOwner {
    CheckoutOwner {
        activity_id: activity.into(),
        session_id: "ses_fixture".into(),
        session_created_id: "evt_fixture".into(),
        directory: managed.path.clone(),
        lifetime_digest: "a".repeat(64),
    }
}
fn directory(managed: &Managed) -> PathBuf {
    managed
        .common_dir
        .join("cyber-worktree-activity")
        .join(&managed.id)
}
fn authority(binding: &CheckoutBinding) -> CheckoutAcknowledgement {
    CheckoutAcknowledgement::new(binding, &"a".repeat(64), &"b".repeat(64)).unwrap()
}
fn stable_binding(managed: &Managed, activity: &str) -> CheckoutBinding {
    let path = directory(managed).join(format!("{activity}.lock"));
    let record: UseRecord = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    record.snapshot_binding.unwrap()
}
fn park(root: &Path) -> io::Result<()> {
    std::fs::write(root.join("ready"), b"parked")?;
    std::io::stdin().read_exact(&mut [0])?;
    Ok(())
}

/// Parks after the private stage is complete and flushed, before its
/// no-replace publication. The parent test kills this process there.
#[test]
#[ignore = "subprocess fixture invoked by the construction kill tests"]
fn construction_child() {
    let root = PathBuf::from(std::env::var_os(ROOT).unwrap());
    let managed = stored(&root);
    match std::env::var(PHASE).unwrap().as_str() {
        "claim" => {
            let owner = owner(&managed, "op_cut");
            let _ = publication::publish_before(&managed, "op_cut", Some(owner), |_| park(&root));
        }
        "acknowledgement" => {
            let binding = stable_binding(&managed, "op_cut");
            let mut claim =
                reacquire(&managed, &binding, binding.lifetime.as_ref().unwrap()).unwrap();
            let _ = authority(&binding).publish_before(&mut claim, || park(&root));
        }
        phase => panic!("unknown construction phase {phase}"),
    }
}

/// Run the child to its parked cut, SIGKILL it and return the retained stage.
fn kill_during(root: &Path, phase: &str, prefix: &str) -> PathBuf {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "worktrees::activity::snapshot::tests::construction_child",
            "--ignored",
        ])
        .env(ROOT, root)
        .env(PHASE, phase)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !root.join("ready").exists() {
        assert!(child.try_wait().unwrap().is_none(), "child exited early");
        assert!(std::time::Instant::now() < deadline, "cut never reached");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let managed = stored(root);
    let stage = std::fs::read_dir(directory(&managed))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(prefix)
        })
        .expect("flushed private stage");
    let held = File::open(&stage).unwrap();
    assert!(matches!(
        held.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    child.kill().unwrap(); // SIGKILL on Unix
    child.wait().unwrap();
    std::fs::remove_file(root.join("ready")).unwrap();
    stage
}

fn assert_fenced(error: io::Error) {
    assert!(error.to_string().contains(FENCE), "{error}");
}
fn assert_listed(managed: &Managed, stage: &Path, kind: SnapshotConstructionKind) {
    let listed = constructions::inventory(managed).unwrap();
    let name = stage.file_name().unwrap().to_string_lossy();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].name, name);
    assert_eq!(listed[0].kind, kind);
    assert_eq!(listed[0].owner_id.as_deref(), Some("op_cut"));
    assert_eq!(listed[0].worktree_id, managed.id);
}

#[test]
fn killed_checkout_claim_construction_retains_private_evidence_and_fences_reclamation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let managed = fixture(&root);
    let stage = kill_during(&root, "claim", "preparing_");
    let bytes = std::fs::read(&stage).unwrap();
    let record: UseRecord = serde_json::from_slice(&bytes).unwrap();
    let binding = record.snapshot_binding.clone().unwrap();
    assert!(!record.settled);
    assert!(!directory(&managed).join("op_cut.lock").exists());
    assert!(!directory(&managed).join("op_cut.snapshot-ack").exists());
    assert_listed(&managed, &stage, SnapshotConstructionKind::CheckoutClaim);
    // The stage is not a claim: no reacquisition, no reuse and no removal.
    assert!(reacquire(&managed, &binding, binding.lifetime.as_ref().unwrap()).is_err());
    assert!(publication::publish(&managed, "op_cut", Some(owner(&managed, "op_cut"))).is_err());
    assert_fenced(CheckoutActivity.reserve(&managed).err().unwrap());
    // A later independent worker is admitted, yet its acknowledged retirement
    // and removal still leave this constructor fenced.
    let mut later = publication::publish(&managed, "op_later", Some(owner(&managed, "op_later")))
        .unwrap()
        .retain_snapshot()
        .unwrap();
    authority(later.binding()).publish(&mut later).unwrap();
    drop(CheckoutRetirementGuard::prepare(vec![later]).unwrap());
    assert_fenced(CheckoutActivity.reserve(&managed).err().unwrap());
    assert_eq!(std::fs::read(&stage).unwrap(), bytes);
    assert!(!directory(&managed).join("op_cut.lock").exists());
}

#[test]
fn killed_acknowledgement_construction_retains_stage_and_fences_retirement() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let managed = fixture(&root);
    let lease = publication::publish(&managed, "op_cut", Some(owner(&managed, "op_cut"))).unwrap();
    let binding = lease.snapshot_binding().unwrap().clone();
    drop(lease); // exited worker: unsettled claim, no acknowledgement
    let claim_path = directory(&managed).join("op_cut.lock");
    let claim_bytes = std::fs::read(&claim_path).unwrap();
    let stage = kill_during(&root, "acknowledgement", "ack_preparing_");
    let stage_bytes = std::fs::read(&stage).unwrap();
    let marker = directory(&managed).join("op_cut.snapshot-ack");
    assert!(!marker.exists());
    assert_listed(
        &managed,
        &stage,
        SnapshotConstructionKind::CheckoutAcknowledgement,
    );
    let mut claim = reacquire(&managed, &binding, binding.lifetime.as_ref().unwrap()).unwrap();
    assert!(authority(&binding).require_existing(&mut claim).is_err());
    assert!(!marker.exists(), "maintenance must not publish a marker");
    assert!(CheckoutActivity.reserve(&managed).is_err());
    // Exact receipt reconciliation resumes in a fresh stage. The killed
    // stage is never adopted, and retirement refuses while it remains.
    authority(&binding).publish(&mut claim).unwrap();
    assert!(marker.exists());
    let refused = CheckoutRetirementGuard::prepare(vec![claim]).err().unwrap();
    assert_fenced(refused);
    assert_eq!(std::fs::read(&stage).unwrap(), stage_bytes);
    assert_eq!(std::fs::read(&claim_path).unwrap(), claim_bytes);
    // Removal pins, but never deletes, the stage of an exactly acknowledged owner.
    drop(CheckoutActivity.reserve(&managed).unwrap());
    assert_eq!(std::fs::read(&stage).unwrap(), stage_bytes);
}

#[test]
fn malformed_or_foreign_construction_names_are_listed_and_fence_removal() {
    // The length-mismatched name is malformed (no owner); the length-8 one is a
    // well-formed acknowledgement stage for an owner that has no acknowledgement.
    for (name, owner) in [
        ("preparing_6_op_cut_".to_string(), None),
        (
            format!("ack_preparing_9_op_other_{}", ulid::Ulid::new()),
            None,
        ),
        (
            format!("ack_preparing_8_op_other_{}", ulid::Ulid::new()),
            Some("op_other"),
        ),
        (
            format!("preparing_6_op_cut_{}", ulid::Ulid::new()),
            Some("op_cut"),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let managed = fixture(&temp.path().canonicalize().unwrap());
        drop(
            publication::publish(&managed, "op_seed", None)
                .unwrap()
                .settle(),
        );
        std::fs::write(directory(&managed).join(&name), b"unknown constructor").unwrap();
        let listed = constructions::inventory(&managed).unwrap();
        assert_eq!(listed.len(), 1, "{name}");
        assert_eq!(listed[0].owner_id.as_deref(), owner, "{name}");
        assert_fenced(CheckoutActivity.reserve(&managed).err().unwrap());
        assert_eq!(
            std::fs::read(directory(&managed).join(&name)).unwrap(),
            b"unknown constructor"
        );
    }
}
