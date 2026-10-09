use super::*;
use cyber_core::worktrees::CheckoutActivity;

#[test]
fn mcp_owner_is_independent_of_sessions_and_retains_the_lock_through_durable_settlement() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("mcp-owner").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    assert!(block_on(repository.claim(&fixture.execution, &managed, "mcs_server")).is_err());
    assert!(block_on(repository.claim_mcp(&fixture.execution, &managed, "ses_user")).is_err());
    let session = block_on(repository.claim(&fixture.execution, &managed, "ses_user")).unwrap();
    let server =
        block_on(repository.claim_mcp(&fixture.execution, &managed, "mcs_server")).unwrap();
    session.settle().unwrap();
    assert!(
        block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, false))
            .is_err()
    );
    let retained = server.settle_retained().unwrap();
    assert!(
        block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, false))
            .is_err()
    );
    assert!(managed.path.join("tracked.txt").exists());
    drop(retained);
    block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, false)).unwrap();
    assert!(!managed.path.exists());
}

#[test]
fn disposed_mcp_checkout_activity_remains_unknown_and_cannot_be_reclaimed() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("mcp-unknown").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let server =
        block_on(repository.claim_mcp(&fixture.execution, &managed, "mcs_server")).unwrap();
    drop(server);
    assert!(block_on(repository.claim_mcp(&fixture.execution, &managed, "mcs_server")).is_err());
    let error = block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, false))
        .unwrap_err();
    assert!(error.to_string().contains("unknown"));
    assert!(managed.path.join("tracked.txt").exists());
}

#[test]
fn location_lookup_identifies_owned_subdirectories_without_adopting_primary_or_unmanaged_roots() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("location").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let nested = managed.path.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let (found, ownership) = Repository::managed_at(&nested).unwrap().unwrap();
    assert_eq!(ownership, managed);
    assert_eq!(found.common_dir, repository.common_dir);
    // An inner repository is still deleted along with its enclosing checkout.
    std::fs::create_dir(nested.join(".git")).unwrap();
    assert_eq!(Repository::managed_at(&nested).unwrap().unwrap().1, managed);
    let inner = fixture
        .create(
            &repository,
            &Name::parse("inner").unwrap(),
            &Settings {
                root: Some(nested.join("checkouts")),
                ..Default::default()
            },
        )
        .unwrap();
    let enclosing = Repository::managed_locations_at(&inner.path).unwrap();
    assert_eq!(enclosing.len(), 2);
    assert_eq!(enclosing[0].1, inner);
    assert_eq!(enclosing[1].1, managed);
    assert!(Repository::managed_at(&fixture.repo).unwrap().is_none());
    assert!(
        Repository::managed_at(fixture._temp.path())
            .unwrap()
            .is_none()
    );
    let unrelated = fixture._temp.path().join("unmanaged/location");
    fixture.git(&[
        "worktree",
        "add",
        "--detach",
        unrelated.to_str().unwrap(),
        "HEAD",
    ]);
    assert!(Repository::managed_at(&unrelated).unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn location_lookup_refuses_symlinked_ownership_directory() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("lookup").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let records = repository.common_dir.join("cyber-worktrees");
    let moved = repository.common_dir.join("saved-records");
    std::fs::rename(&records, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &records).unwrap();
    assert!(Repository::managed_at(&managed.path).is_err());
    assert!(managed.path.join("tracked.txt").exists());
}

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
    assert!(error.to_string().contains("ses_second"), "{error}");
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
    drop(lease);
    // Windows locks also prohibit reads through a separately opened handle.
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&before).unwrap()["settled"],
        false
    );
    let probe = std::fs::File::open(&path).unwrap();
    probe.try_lock().unwrap();
    // A transient inherited/duplicated descriptor must not retain this probe's lock.
    let retained_probe = probe.try_clone().unwrap();
    probe.unlock().unwrap();
    drop(probe);
    for force in [false, true] {
        let error =
            block_on(repository.remove(&fixture.execution, &CheckoutActivity, &managed, force))
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("outcome unknown for ses_abandoned"),
            "{error}"
        );
    }
    let error = block_on(repository.claim(&fixture.execution, &managed, "ses_abandoned"))
        .err()
        .unwrap();
    assert!(error.to_string().contains("recovery is required"));
    assert_eq!(std::fs::read(path).unwrap(), before);
    drop(retained_probe);
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
