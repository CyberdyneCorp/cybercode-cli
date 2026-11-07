use super::*;
use crate::worktrees::{CheckoutActivity, CheckoutOwner, RemovalActivity};
use std::io::Read;

fn fixture(root: &std::path::Path) -> Managed {
    let common = root.join("common");
    let path = root.join("work");
    std::fs::create_dir_all(common.join("cyber-worktrees")).unwrap();
    std::fs::create_dir_all(&path).unwrap();
    let managed = Managed {
        id: "wt_fixture".into(),
        name: "fixture".into(),
        path,
        branch: "cyber/fixture".into(),
        base: "main".into(),
        common_dir: common,
        ready: true,
        included: vec![],
    };
    std::fs::write(
        root.join("common/cyber-worktrees/fixture.json"),
        serde_json::to_vec(&managed).unwrap(),
    )
    .unwrap();
    managed
}
fn directory(managed: &Managed) -> PathBuf {
    managed
        .common_dir
        .join("cyber-worktree-activity")
        .join(&managed.id)
}
fn original(managed: &Managed) -> CheckoutBinding {
    let owner = CheckoutOwner {
        activity_id: "op_fixture".into(),
        session_id: "ses_fixture".into(),
        session_created_id: "evt_fixture".into(),
        directory: managed.path.clone(),
        lifetime_digest: "a".repeat(64),
    };
    let lease =
        super::super::publication::publish(managed, &owner.activity_id, Some(owner.clone()))
            .unwrap();
    lease.snapshot_binding().unwrap().clone()
}
fn claim(managed: &Managed, binding: &CheckoutBinding) -> SnapshotClaim {
    super::super::reacquire(managed, binding, binding.lifetime.as_ref().unwrap()).unwrap()
}
fn authority(binding: &CheckoutBinding) -> CheckoutAcknowledgement {
    CheckoutAcknowledgement::new(binding, &"a".repeat(64), &"b".repeat(64)).unwrap()
}

#[test]
fn acknowledgement_is_exact_idempotent_and_preserves_original_and_independent_unknown_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let managed = fixture(&temp.path().canonicalize().unwrap());
    let binding = original(&managed);
    let path = directory(&managed).join("op_fixture.lock");
    let original = std::fs::read(&path).unwrap();
    let mut proof = claim(&managed, &binding);
    let authority = authority(&binding);
    let mut legacy = binding.clone();
    legacy.version = 1;
    legacy.lifetime = None;
    assert!(CheckoutAcknowledgement::new(&legacy, &"a".repeat(64), &"b".repeat(64)).is_err());
    authority.retain_existing(&mut proof).unwrap();
    let marker = directory(&managed).join("op_fixture.snapshot-ack");
    assert!(!marker.exists());
    assert!(CheckoutActivity.reserve(&managed).is_err());
    authority.publish(&mut proof).unwrap();
    let bytes = std::fs::read(&marker).unwrap();
    authority.publish(&mut proof).unwrap();
    assert_eq!(std::fs::read(&marker).unwrap(), bytes);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(
        CheckoutAcknowledgement::new(&binding, &"a".repeat(64), &"c".repeat(64))
            .unwrap()
            .retain_existing(&mut proof)
            .is_err()
    );
    assert!(CheckoutActivity.reserve(&managed).is_err()); // actual retained original lock
    drop(proof);
    let mut reloaded = claim(&managed, &binding);
    authority.publish(&mut reloaded).unwrap();
    drop(reloaded);
    drop(CheckoutActivity.reserve(&managed).unwrap());
    let mut foreign = managed.clone();
    foreign.path = temp.path().join("foreign-location");
    assert!(CheckoutActivity.reserve(&foreign).is_err());
    let independent = directory(&managed).join("ses_other.lock");
    let record = UseRecord {
        worktree_id: managed.id.clone(),
        owner_id: "ses_other".into(),
        settled: false,
        snapshot_binding: None,
    };
    let bytes = serde_json::to_vec(&record).unwrap();
    std::fs::write(&independent, &bytes).unwrap();
    assert!(CheckoutActivity.reserve(&managed).is_err());
    assert_eq!(std::fs::read(independent).unwrap(), bytes);
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn marker_replacement_alias_and_foreign_construction_never_gain_removal_authority() {
    for case in [
        "replacement",
        "alias",
        "foreign-stage",
        "unacknowledged-stage",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let managed = fixture(&temp.path().canonicalize().unwrap());
        let binding = original(&managed);
        let mut proof = claim(&managed, &binding);
        let authority = authority(&binding);
        if case != "unacknowledged-stage" {
            authority.publish(&mut proof).unwrap();
        }
        drop(proof);
        let marker = directory(&managed).join("op_fixture.snapshot-ack");
        match case {
            "replacement" => {
                let bytes = std::fs::read(&marker).unwrap();
                std::fs::rename(&marker, marker.with_extension("retained")).unwrap();
                std::fs::write(&marker, &bytes).unwrap();
                assert!(
                    authority
                        .retain_existing(&mut claim(&managed, &binding))
                        .is_err()
                );
                assert_eq!(std::fs::read(marker).unwrap(), bytes);
            }
            "alias" => {
                std::fs::hard_link(&marker, marker.with_extension("alias")).unwrap();
                assert!(
                    authority
                        .retain_existing(&mut claim(&managed, &binding))
                        .is_err()
                );
            }
            _ => {
                let owner = if case == "foreign-stage" {
                    "op_foreign"
                } else {
                    "op_fixture"
                };
                let stage = directory(&managed).join(format!(
                    "{}{}",
                    stage_prefix(owner),
                    ulid::Ulid::new()
                ));
                std::fs::write(&stage, b"incomplete retained metadata").unwrap();
            }
        }
        assert!(CheckoutActivity.reserve(&managed).is_err(), "{case}");
    }
}

#[test]
fn namespace_replacement_before_acknowledgement_never_installs_a_marker_in_the_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let managed = fixture(&temp.path().canonicalize().unwrap());
    let binding = original(&managed);
    let mut proof = claim(&managed, &binding);
    let directory = directory(&managed);
    let retained = directory.with_extension("retained");
    let result = authority(&binding).publish_before(&mut proof, || {
        std::fs::rename(&directory, &retained)?;
        std::fs::create_dir(&directory)
    });
    assert!(result.is_err());
    assert!(!directory.join("op_fixture.snapshot-ack").exists());
    assert!(!retained.join("op_fixture.snapshot-ack").exists());
    assert_eq!(std::fs::read_dir(&retained).unwrap().count(), 2);
    std::fs::remove_dir(&directory).unwrap(); // restore only the fixture's new empty replacement
    std::fs::rename(&retained, &directory).unwrap();
    authority(&binding).publish(&mut proof).unwrap();
    drop(proof);
    drop(CheckoutActivity.reserve(&managed).unwrap());
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 3);
}

#[test]
#[ignore = "subprocess fixture invoked by acknowledgement kill test"]
fn acknowledgement_child() {
    let root = PathBuf::from(std::env::var_os("CYBER_CHECKOUT_ACK_ROOT").unwrap());
    let managed: Managed = serde_json::from_slice(
        &std::fs::read(root.join("common/cyber-worktrees/fixture.json")).unwrap(),
    )
    .unwrap();
    let record: UseRecord = serde_json::from_slice(
        &std::fs::read(directory(&managed).join("op_fixture.lock")).unwrap(),
    )
    .unwrap();
    let binding = record.snapshot_binding.unwrap();
    let mut claim = claim(&managed, &binding);
    authority(&binding)
        .publish_before(&mut claim, || {
            std::fs::write(root.join("ready"), b"prepared").unwrap();
            std::io::stdin().read_exact(&mut [0]).unwrap();
            Ok(())
        })
        .unwrap();
}

#[test]
fn killed_acknowledgement_resumes_without_rewriting_claim_or_deleting_partial_stage() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let managed = fixture(&root);
    let binding = original(&managed);
    let path = directory(&managed).join("op_fixture.lock");
    let original = std::fs::read(&path).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "worktrees::activity::snapshot::acknowledgement::tests::acknowledgement_child",
            "--ignored",
        ])
        .env("CYBER_CHECKOUT_ACK_ROOT", &root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !root.join("ready").exists() {
        assert!(child.try_wait().unwrap().is_none());
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        super::super::reacquire(&managed, &binding, binding.lifetime.as_ref().unwrap()).is_err()
    );
    let stage = std::fs::read_dir(directory(&managed))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("ack_preparing_")
        })
        .unwrap();
    let staged = File::open(&stage).unwrap();
    assert!(matches!(
        staged.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    child.kill().unwrap();
    child.wait().unwrap();
    let bytes = std::fs::read(&stage).unwrap();
    assert!(CheckoutActivity.reserve(&managed).is_err());
    let mut proof = claim(&managed, &binding);
    authority(&binding).publish(&mut proof).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read(&stage).unwrap(), bytes);
    drop(proof);
    let fence = CheckoutActivity.reserve(&managed).unwrap();
    assert!(matches!(
        staged.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(fence);
    staged.try_lock().unwrap();
    staged.unlock().unwrap();
    assert_eq!(std::fs::read(&stage).unwrap(), bytes);
}
