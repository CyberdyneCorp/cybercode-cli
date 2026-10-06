use super::*;
use cyber_core::worktrees::CheckoutActivity;

struct Worker(std::process::Child);

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "invoked by cross-process lease tests"]
fn lease_worker() {
    let Some(root) = std::env::var_os("CYBER_LEASE_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let execution = Execution {
        config: root.join("git-config"),
        hooks: root.join("no-hooks"),
        mode: Mode::Normal,
    };
    let managed = serde_json::from_slice(&std::fs::read(root.join("lease.json")).unwrap()).unwrap();
    let repository = block_on(Repository::discover(&execution, &root.join("source"))).unwrap();
    let lease = block_on(repository.claim(&execution, &managed, "ses_worker")).unwrap();
    std::fs::write(root.join("lease-ready"), "ready").unwrap();
    let mut request = String::new();
    std::io::stdin().read_line(&mut request).unwrap();
    assert_eq!(request.trim(), "settle");
    lease.settle().unwrap();
}

fn spawn_worker(fixture: &Fixture, managed: &cyber_core::worktrees::Managed) -> Worker {
    let root = fixture._temp.path();
    std::fs::write(
        root.join("lease.json"),
        serde_json::to_vec(managed).unwrap(),
    )
    .unwrap();
    let mut worker = Worker(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "activity::lease_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("CYBER_LEASE_TEST_ROOT", root)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !root.join("lease-ready").exists() {
        assert!(
            worker.0.try_wait().unwrap().is_none(),
            "lease worker exited before admission"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "lease worker admission timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    worker
}

#[test]
fn independent_process_lease_blocks_removal_and_settles_explicitly() {
    use std::io::Write;
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("worker").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let mut worker = spawn_worker(&fixture, &managed);
    let error = block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, true))
        .unwrap_err();
    assert!(error.to_string().contains("in use by ses_worker"));
    worker
        .0
        .stdin
        .take()
        .unwrap()
        .write_all(b"settle\n")
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(status) = worker.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "lease worker settlement timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, false)).unwrap();
}

#[test]
fn killed_process_leaves_unknown_activity_and_preserves_checkout() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("killed").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let mut worker = spawn_worker(&fixture, &managed);
    worker.0.kill().unwrap();
    worker.0.wait().unwrap();
    let error = block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, true))
        .unwrap_err();
    assert!(error.to_string().contains("outcome unknown for ses_worker"));
    assert!(managed.path.join("tracked.txt").exists());
    assert!(
        !managed
            .common_dir
            .join("cyber-worktree-removals/killed.json")
            .exists()
    );
}

#[test]
fn live_leases_refuse_removal_until_every_session_explicitly_settles() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("leased").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let first = block_on(repository.claim(&fixture.execution, &managed, "ses_first")).unwrap();
    let second = block_on(repository.claim(&fixture.execution, &managed, "ses_second")).unwrap();
    assert!(block_on(repository.claim(&fixture.execution, &managed, "ses_first")).is_err());
    for force in [false, true] {
        let error =
            block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, force))
                .unwrap_err();
        assert!(error.to_string().contains("ses_first"));
        assert!(managed.path.join("tracked.txt").exists());
        assert!(
            !managed
                .common_dir
                .join("cyber-worktree-removals/leased.json")
                .exists()
        );
    }
    first.settle().unwrap();
    let error = block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, true))
        .unwrap_err();
    assert!(error.to_string().contains("ses_second"));
    second.settle().unwrap();
    block_on(repository.claim(&fixture.execution, &managed, "ses_first"))
        .unwrap()
        .settle()
        .unwrap();
    block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, false)).unwrap();
    assert!(!managed.path.exists());
}

#[test]
fn abandoned_lease_requires_recovery_even_after_kernel_lock_release() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("abandoned").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let lease = block_on(repository.claim(&fixture.execution, &managed, "ses_abandoned")).unwrap();
    let path = managed
        .common_dir
        .join("cyber-worktree-activity")
        .join(&managed.id)
        .join("ses_abandoned.lock");
    let before = std::fs::read(&path).unwrap();
    drop(lease);
    let probe = std::fs::File::open(&path).unwrap();
    probe.try_lock().unwrap();
    drop(probe);
    for force in [false, true] {
        let error =
            block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, force))
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("outcome unknown for ses_abandoned")
        );
    }
    let error = block_on(repository.claim(&fixture.execution, &managed, "ses_abandoned"))
        .err()
        .unwrap();
    assert!(error.to_string().contains("recovery is required"));
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(managed.path.join("tracked.txt").exists());
    assert!(
        !managed
            .common_dir
            .join("cyber-worktree-removals/abandoned.json")
            .exists()
    );
}

#[test]
fn claims_refuse_stale_ownership_and_invalid_session_names_before_creating_records() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let mut managed = fixture
        .create(
            &repository,
            &Name::parse("identity").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    for session in ["ses_", "ses_../escape", "other_session"] {
        assert!(block_on(repository.claim(&fixture.execution, &managed, session)).is_err());
    }
    managed.id = "wt_replacement".into();
    assert!(block_on(repository.claim(&fixture.execution, &managed, "ses_valid")).is_err());
    assert!(!managed.common_dir.join("cyber-worktree-activity").exists());
}
