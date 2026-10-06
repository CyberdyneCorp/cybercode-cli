use super::*;
use cyber_core::worktrees::ListedWorktree;

struct InterruptedExecution;

impl GitExecution for InterruptedExecution {
    fn run<'a>(&'a self, _: &'a Path, _: &'a [OsString]) -> GitFuture<'a> {
        Box::pin(async { Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled")) })
    }
}

#[test]
fn listing_propagates_cancellation_and_releases_repository_lock() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("cancelled").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    assert_eq!(
        block_on(repository.list(&InterruptedExecution))
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    assert_eq!(
        block_on(repository.list(&fixture.execution)).unwrap(),
        vec![ListedWorktree::Ready(managed)]
    );
}

#[test]
fn listing_verifies_ready_records_and_preserves_user_edits() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    assert!(
        block_on(repository.list(&fixture.execution))
            .unwrap()
            .is_empty()
    );
    let zeta = fixture
        .create(
            &repository,
            &Name::parse("zeta").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let alpha = fixture
        .create(
            &repository,
            &Name::parse("alpha").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    std::fs::write(alpha.path.join("tracked.txt"), "edited").unwrap();
    assert_eq!(
        block_on(repository.list(&fixture.execution)).unwrap(),
        vec![
            ListedWorktree::Ready(alpha.clone()),
            ListedWorktree::Ready(zeta)
        ]
    );
    assert_eq!(
        std::fs::read_to_string(alpha.path.join("tracked.txt")).unwrap(),
        "edited"
    );
}

#[test]
fn listing_keeps_pending_and_invalid_records_visible() {
    let mut fixture = Fixture::new();
    let repository = fixture.repository();
    fixture.execution.mode = Mode::FailAfterCreation;
    assert!(
        fixture
            .create(
                &repository,
                &Name::parse("pending").unwrap(),
                &Settings::default()
            )
            .is_err()
    );
    let records = repository.common_dir.join("cyber-worktrees");
    std::fs::write(records.join("broken.json"), b"not json").unwrap();
    std::fs::write(records.join("ignored.tmp"), b"temporary record").unwrap();
    let pending_bytes = std::fs::read(records.join("pending.json")).unwrap();
    let entries = block_on(repository.list(&fixture.execution)).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(matches!(&entries[0], ListedWorktree::Invalid { name, .. } if name == "broken"));
    assert!(
        matches!(&entries[1], ListedWorktree::Pending(managed) if managed.name == "pending" && !managed.ready)
    );
    assert_eq!(
        std::fs::read(records.join("pending.json")).unwrap(),
        pending_bytes
    );
}

#[test]
fn listing_reports_changed_branch_and_refuses_lock_contention() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("changed").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let output = fixture
        .execution
        .invoke(
            &managed.path,
            &["checkout".into(), "-b".into(), "user-branch".into()],
        )
        .unwrap();
    assert!(output.status.success());
    assert!(
        matches!(&block_on(repository.list(&fixture.execution)).unwrap()[0], ListedWorktree::Invalid { name, error } if name == "changed" && error.contains("branch changed"))
    );
    let _lock = RepositoryLock::try_acquire(&repository.common_dir)
        .unwrap()
        .unwrap();
    assert_eq!(
        block_on(repository.list(&fixture.execution))
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn listing_refuses_mismatched_and_oversized_records() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let mut managed = fixture
        .create(
            &repository,
            &Name::parse("owned").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let records = repository.common_dir.join("cyber-worktrees");
    managed.name = "other".into();
    std::fs::write(
        records.join("owned.json"),
        serde_json::to_vec(&managed).unwrap(),
    )
    .unwrap();
    std::fs::write(records.join("huge.json"), vec![b' '; 1024 * 1024 + 1]).unwrap();
    let entries = block_on(repository.list(&fixture.execution)).unwrap();
    assert!(
        matches!(&entries[0], ListedWorktree::Invalid { name, error } if name == "huge" && error.contains("exceeds"))
    );
    assert!(
        matches!(&entries[1], ListedWorktree::Invalid { name, error } if name == "owned" && error.contains("does not match"))
    );
    assert!(managed.path.join("tracked.txt").exists());
}

#[cfg(unix)]
#[test]
fn listing_refuses_symlinked_records_and_ownership_directory() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let records = repository.common_dir.join("cyber-worktrees");
    std::fs::create_dir(&records).unwrap();
    let outside = fixture._temp.path().join("outside");
    std::fs::write(&outside, b"do not read").unwrap();
    std::os::unix::fs::symlink(&outside, records.join("linked.json")).unwrap();
    assert!(
        matches!(&block_on(repository.list(&fixture.execution)).unwrap()[0], ListedWorktree::Invalid { name, error } if name == "linked" && error.contains("regular file"))
    );
    std::fs::remove_file(records.join("linked.json")).unwrap();
    std::fs::remove_dir(&records).unwrap();
    std::os::unix::fs::symlink(fixture._temp.path(), &records).unwrap();
    assert!(
        block_on(repository.list(&fixture.execution))
            .unwrap_err()
            .to_string()
            .contains("regular directory")
    );
    assert_eq!(std::fs::read(&outside).unwrap(), b"do not read");
}
