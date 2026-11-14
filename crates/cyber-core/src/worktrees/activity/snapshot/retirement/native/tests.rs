//! Native primitive tests only; actual Store/receipt authorization lives in App tests.
use super::*;
use crate::worktrees::{CheckoutAcknowledgement, CheckoutOwner, Managed, RepositoryLock};

struct Fixture {
    temp: tempfile::TempDir,
    managed: Managed,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.temp.disable_cleanup(true);
            eprintln!(
                "Preserved checkout retirement fixture at {}",
                self.temp.path().display()
            );
        }
    }
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let common_dir = root.join("common");
        let path = root.join("work");
        std::fs::create_dir_all(common_dir.join("cyber-worktrees")).unwrap();
        std::fs::create_dir(&path).unwrap();
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
        Self { temp, managed }
    }
    fn claim(&self, activity: &str) -> SnapshotClaim {
        let owner = CheckoutOwner {
            activity_id: activity.into(),
            session_id: "ses_fixture".into(),
            session_created_id: "evt_fixture".into(),
            directory: self.managed.path.clone(),
            lifetime_digest: "a".repeat(64),
        };
        super::super::super::publication::publish(&self.managed, activity, Some(owner))
            .unwrap()
            .retain_snapshot()
            .unwrap()
    }
    fn acknowledged(&self, activity: &str) -> SnapshotClaim {
        let mut claim = self.claim(activity);
        CheckoutAcknowledgement::new(claim.binding(), &"a".repeat(64), &"b".repeat(64))
            .unwrap()
            .publish(&mut claim)
            .unwrap();
        claim
    }
    fn directory(&self) -> PathBuf {
        self.managed
            .common_dir
            .join("cyber-worktree-activity/wt_fixture")
    }
    fn plans(&self) -> Vec<CheckoutRetirementPlan> {
        CheckoutRetirementGuard::prepare(vec![self.acknowledged("op_first")])
            .unwrap()
            .plans()
    }
}
fn remove_all(guard: &mut CheckoutRetirementGuard) -> usize {
    let mut count = 0;
    while guard.remove_next().unwrap() {
        count += 1;
    }
    count
}

#[test]
fn exact_retirement_preserves_shared_creation_unknown_activity_and_source() {
    let f = Fixture::new();
    let mut guard = CheckoutRetirementGuard::prepare(vec![f.acknowledged("op_first")]).unwrap();
    let creation = f.managed.common_dir.join("cyber-worktrees/fixture.json");
    let bytes = std::fs::read(&creation).unwrap();
    let unknown = f.directory().join("op_unknown.lock");
    std::fs::write(&unknown, b"independent unknown evidence").unwrap();
    std::fs::write(f.managed.path.join("owner"), b"later owner source").unwrap();
    assert!(
        RepositoryLock::try_acquire(&f.managed.common_dir)
            .unwrap()
            .is_none()
    );
    assert_eq!(remove_all(&mut guard), 2);
    guard.verify().unwrap();
    assert!(
        RepositoryLock::try_acquire(&f.managed.common_dir)
            .unwrap()
            .is_none()
    );
    assert_eq!(std::fs::read(&creation).unwrap(), bytes);
    assert_eq!(
        std::fs::read(&unknown).unwrap(),
        b"independent unknown evidence"
    );
    assert_eq!(
        std::fs::read(f.managed.path.join("owner")).unwrap(),
        b"later owner source"
    );
    drop(guard);
    assert!(
        RepositoryLock::try_acquire(&f.managed.common_dir)
            .unwrap()
            .is_some()
    );
}

#[test]
fn frozen_native_frame_resumes_each_leaf_cut_and_repeated_completion_without_recreation() {
    for cut in 0..=2 {
        let f = Fixture::new();
        let mut guard = CheckoutRetirementGuard::prepare(vec![f.acknowledged("op_first")]).unwrap();
        let frozen = serde_json::to_vec(&guard.plans()).unwrap();
        for _ in 0..cut {
            assert!(guard.remove_next().unwrap());
        }
        drop(guard);
        let plans = serde_json::from_slice(&frozen).unwrap();
        let mut reopened = CheckoutRetirementGuard::resume_committed(plans).unwrap();
        assert_eq!(remove_all(&mut reopened), 2 - cut);
        assert_eq!(serde_json::to_vec(&reopened.plans()).unwrap(), frozen);
        drop(reopened);
        let mut repeated =
            CheckoutRetirementGuard::resume_committed(serde_json::from_slice(&frozen).unwrap())
                .unwrap();
        assert_eq!(remove_all(&mut repeated), 0);
    }
}

#[test]
fn two_consumed_owners_share_the_repository_lock_and_keep_their_inventory_order() {
    let f = Fixture::new();
    let mut guard = CheckoutRetirementGuard::prepare(vec![
        f.acknowledged("op_first"),
        f.acknowledged("op_second"),
    ])
    .unwrap();
    let plans = guard.plans();
    assert_eq!(plans[0].binding.owner_id, "op_first");
    assert_eq!(plans[1].binding.owner_id, "op_second");
    assert_eq!(
        plans[0].repository_lock_identity,
        plans[1].repository_lock_identity
    );
    assert!(guard.remove_next().unwrap() && guard.remove_next().unwrap());
    assert!(
        f.directory().join("op_first.lock").exists()
            && f.directory().join("op_second.lock").exists()
    );
    assert!(
        !f.directory().join("op_first.snapshot-ack").exists()
            && !f.directory().join("op_second.snapshot-ack").exists()
    );
    assert_eq!(remove_all(&mut guard), 2);
}

#[test]
fn missing_acknowledgement_busy_repository_and_live_original_refuse_without_unlink() {
    let f = Fixture::new();
    assert!(CheckoutRetirementGuard::prepare(vec![f.claim("op_unacknowledged")]).is_err());
    let claim = f.acknowledged("op_first");
    let lock = RepositoryLock::try_acquire(&f.managed.common_dir)
        .unwrap()
        .unwrap();
    assert!(CheckoutRetirementGuard::prepare(vec![claim]).is_err());
    drop(lock);
    let mut proof = super::super::super::reacquire(
        &f.managed,
        &{
            let raw = std::fs::read(f.directory().join("op_first.lock")).unwrap();
            serde_json::from_slice::<UseRecord>(&raw)
                .unwrap()
                .snapshot_binding
                .unwrap()
        },
        &CheckoutOwner {
            activity_id: "op_first".into(),
            session_id: "ses_fixture".into(),
            session_created_id: "evt_fixture".into(),
            directory: f.managed.path.clone(),
            lifetime_digest: "a".repeat(64),
        },
    )
    .unwrap();
    CheckoutAcknowledgement::new(proof.binding(), &"a".repeat(64), &"b".repeat(64))
        .unwrap()
        .require_existing(&mut proof)
        .unwrap();
    let plans = CheckoutRetirementGuard::prepare(vec![proof])
        .unwrap()
        .plans();
    let live = super::super::super::reacquire(
        &f.managed,
        &plans[0].binding,
        plans[0].binding.lifetime.as_ref().unwrap(),
    )
    .unwrap();
    assert!(CheckoutRetirementGuard::resume_committed(plans).is_err());
    assert!(
        f.directory().join("op_first.lock").exists()
            && f.directory().join("op_first.snapshot-ack").exists()
    );
    drop(live);
}

#[test]
fn replacement_changed_bytes_alias_and_same_owner_construction_are_preserved() {
    for kind in ["replacement", "changed", "alias", "construction", "context"] {
        let f = Fixture::new();
        let mut plans = f.plans();
        let ack = f.directory().join("op_first.snapshot-ack");
        let before = std::fs::read(&ack).unwrap();
        match kind {
            "replacement" => {
                std::fs::rename(&ack, f.temp.path().join("retained-ack")).unwrap();
                std::fs::write(&ack, &before).unwrap();
            }
            "changed" => std::fs::write(&ack, b"changed native evidence").unwrap(),
            "alias" => std::fs::hard_link(&ack, f.temp.path().join("alias")).unwrap(),
            "construction" => std::fs::write(
                f.directory()
                    .join(format!("ack_preparing_8_op_first_{}", ulid::Ulid::new())),
                b"unknown constructor",
            )
            .unwrap(),
            "context" => plans[0].receipt_digest = "c".repeat(64),
            _ => unreachable!(),
        }
        assert!(
            CheckoutRetirementGuard::resume_committed(plans).is_err(),
            "{kind}"
        );
        assert!(ack.exists() && f.directory().join("op_first.lock").exists());
    }
}

#[test]
fn fifo_acknowledgement_refuses_without_waiting_for_a_writer() {
    use std::os::unix::fs::FileTypeExt;
    let f = Fixture::new();
    let plans = f.plans();
    let ack = f.directory().join("op_first.snapshot-ack");
    std::fs::rename(&ack, f.temp.path().join("retained-ack")).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&ack)
            .status()
            .unwrap()
            .success()
    );
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        send.send(CheckoutRetirementGuard::resume_committed(plans).is_err())
            .unwrap();
    });
    assert!(
        receive
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("native metadata open blocked on a FIFO")
    );
    worker.join().unwrap();
    assert!(ack.symlink_metadata().unwrap().file_type().is_fifo());
    assert!(f.directory().join("op_first.lock").exists());
}

#[test]
fn changed_original_context_refuses_before_freezing_or_removing_metadata() {
    let f = Fixture::new();
    let claim = f.acknowledged("op_first");
    let path = f.directory().join("op_first.lock");
    let mut record: UseRecord = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record.settled = true;
    let changed = serde_json::to_vec(&record).unwrap();
    std::fs::write(&path, &changed).unwrap();
    assert!(CheckoutRetirementGuard::prepare(vec![claim]).is_err());
    assert_eq!(std::fs::read(path).unwrap(), changed);
    assert!(f.directory().join("op_first.snapshot-ack").exists());
}

#[test]
fn native_plan_data_bounds_reject_oversized_leaves_and_resource_vectors_before_native_io() {
    let f = Fixture::new();
    let plan = f.plans().remove(0);
    let mut oversized = plan.clone();
    oversized.claim.bytes = 1024 * 1024 + 1;
    assert!(oversized.validate().is_err());
    let duplicate = vec![plan.clone(), plan.clone()];
    assert!(CheckoutRetirementPlan::validate_set(&duplicate).is_err());
    let oversized = vec![plan; 8193];
    let error = CheckoutRetirementPlan::validate_set(&oversized).unwrap_err();
    assert!(error.to_string().contains("resource limit"));
    assert!(f.directory().join("op_first.lock").exists());
    assert!(f.directory().join("op_first.snapshot-ack").exists());
}
