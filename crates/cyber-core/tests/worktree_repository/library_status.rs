//! Candidate inspection only: production still uses the owned Git execution port.

use super::{Fixture, Name, Path, Repository, Settings};

fn candidate(
    fixture: &Fixture,
    repository: &Repository,
    managed: &cyber_core::worktrees::Managed,
) -> git2::Repository {
    let marker = std::fs::read_to_string(managed.path.join(".git")).unwrap();
    let metadata = Path::new(marker.trim_end().strip_prefix("gitdir: ").unwrap());
    // Open metadata without discovering or entering the long working directory.
    let repo = git2::Repository::open_bare(metadata).unwrap();
    let mut config = git2::Config::new().unwrap();
    config
        .add_file(
            &repository.common_dir.join("config"),
            git2::ConfigLevel::Local,
            false,
        )
        .unwrap();
    config
        .add_file(&fixture.execution.config, git2::ConfigLevel::App, false)
        .unwrap();
    repo.set_config(&config).unwrap();
    repo.set_workdir(&managed.path, false).unwrap();
    assert_eq!(
        repo.path().canonicalize().unwrap(),
        metadata.canonicalize().unwrap()
    );
    assert_eq!(
        repo.workdir().unwrap().canonicalize().unwrap(),
        managed.path
    );
    repo
}

fn status(
    fixture: &Fixture,
    repository: &Repository,
    managed: &cyber_core::worktrees::Managed,
) -> Vec<(Vec<u8>, git2::Status)> {
    let repo = candidate(fixture, repository, managed);
    let metadata = repo.path();
    let paths = [
        repository.common_dir.join("config"),
        metadata.join("index"),
        metadata.join("gitdir"),
        managed.path.join(".git"),
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .update_index(false)
        .no_refresh(true);
    let entries = repo.statuses(Some(&mut options)).unwrap();
    let result = entries
        .iter()
        .map(|entry| (entry.path_bytes().to_vec(), entry.status()))
        .collect();
    for (path, expected) in paths.iter().zip(before) {
        assert_eq!(
            std::fs::read(path).unwrap(),
            expected,
            "{} changed",
            path.display()
        );
    }
    result
}

#[test]
fn library_status_reads_long_worktrees_without_changing_repository_files() {
    let fixture = Fixture::new();
    // This isolated fixture override never modifies the repository or user configuration.
    std::fs::write(&fixture.execution.config, "[core]\nlongpaths = true\n").unwrap();
    std::fs::write(fixture.repo.join(".gitignore"), ".env\n").unwrap();
    fixture.git(&["add", ".gitignore"]);
    fixture.git(&["commit", "--quiet", "-m", "ignore environment"]);
    std::fs::write(fixture.repo.join(".worktreeinclude"), ".env\n").unwrap();
    std::fs::write(fixture.repo.join(".env"), "retained environment").unwrap();
    let repository = fixture.repository();
    let settings = Settings {
        root: Some(
            fixture
                .data
                .join("long-segment".repeat(8))
                .join("another-segment".repeat(8)),
        ),
        ..Default::default()
    };
    let managed = fixture
        .create(
            &repository,
            &Name::parse("library-long-path").unwrap(),
            &settings,
        )
        .unwrap();
    assert!(managed.path.as_os_str().len() > 260);
    assert!(status(&fixture, &repository, &managed).is_empty());
    std::fs::write(managed.path.join(".env"), "changed ignored environment").unwrap();
    assert!(status(&fixture, &repository, &managed).is_empty());
    std::fs::create_dir(managed.path.join("nested")).unwrap();
    std::fs::write(
        managed.path.join("nested/new-user-file.txt"),
        "retain untracked",
    )
    .unwrap();
    let changes = status(&fixture, &repository, &managed);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].0, b"nested/new-user-file.txt");
    assert!(changes[0].1.is_wt_new());
    std::fs::remove_dir_all(managed.path.join("nested")).unwrap();
    std::fs::write(managed.path.join("tracked.txt"), "edited tracked file").unwrap();
    let changes = status(&fixture, &repository, &managed);
    assert_eq!(changes.len(), 1);
    assert!(changes[0].1.is_wt_modified());
    let repo = candidate(&fixture, &repository, &managed);
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("tracked.txt")).unwrap();
    index.write().unwrap();
    let changes = status(&fixture, &repository, &managed);
    assert_eq!(changes.len(), 1);
    assert!(changes[0].1.is_index_modified());
    std::fs::remove_file(managed.path.join("tracked.txt")).unwrap();
    let changes = status(&fixture, &repository, &managed);
    assert_eq!(changes.len(), 1);
    assert!(changes[0].1.is_index_modified());
    assert!(changes[0].1.is_wt_deleted());
}

fn request(
    fixture: &Fixture,
    repository: &Repository,
    managed: &cyber_core::worktrees::Managed,
) -> cyber_core::worktrees::inspection::Request {
    use cyber_core::worktrees::inspection::{OVERRIDES, Operation, Request, Target};
    std::fs::write(&fixture.execution.config, OVERRIDES).unwrap();
    let marker = std::fs::read_to_string(managed.path.join(".git")).unwrap();
    Request {
        target: Target {
            metadata: Path::new(marker.trim_end().strip_prefix("gitdir: ").unwrap())
                .canonicalize()
                .unwrap(),
            common_dir: repository.common_dir.clone(),
            worktree: managed.path.clone(),
            branch: managed.branch.clone(),
            base: managed.base.clone(),
            operation: Operation::Changes,
        },
        overrides: fixture.execution.config.clone(),
    }
}

#[test]
fn inspection_engine_reports_changes_and_ignored_without_writing_metadata() {
    use cyber_core::worktrees::inspection::read;
    let fixture = Fixture::new();
    std::fs::write(fixture.repo.join(".gitignore"), ".env\n").unwrap();
    fixture.git(&["add", ".gitignore"]);
    fixture.git(&["commit", "--quiet", "-m", "ignore environment"]);
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("inspection").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let request = request(&fixture, &repository, &managed);
    let paths = [
        request.target.metadata.join("index"),
        request.target.metadata.join("gitdir"),
        repository.common_dir.join("config"),
        managed.path.join(".git"),
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    assert!(!read(&request).unwrap().status.dirty);
    std::fs::write(
        managed.path.join("tracked.txt"),
        "replacement\nsecond line\n",
    )
    .unwrap();
    std::fs::write(managed.path.join("new.txt"), "untracked\n").unwrap();
    std::fs::write(managed.path.join(".env"), "ignored secret\n").unwrap();
    let report = read(&request).unwrap();
    assert_eq!(report.target, request.target);
    assert!(report.status.dirty);
    assert_eq!(report.files.len(), 1);
    assert_eq!(report.files[0].file, "tracked.txt");
    assert_eq!(report.files[0].additions, Some(2));
    assert_eq!(report.files[0].deletions, Some(1));
    assert_eq!(report.untracked, [std::path::PathBuf::from("new.txt")]);
    assert_eq!(report.ignored, [std::path::PathBuf::from(".env")]);
    for (path, expected) in paths.iter().zip(before) {
        assert_eq!(
            std::fs::read(path).unwrap(),
            expected,
            "{} changed",
            path.display()
        );
    }
}

#[test]
fn inspection_engine_refuses_changed_identity_and_nonfixed_overrides() {
    use cyber_core::worktrees::inspection::{OVERRIDES, read};
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let managed = fixture
        .create(
            &repository,
            &Name::parse("inspection-refusal").unwrap(),
            &Settings::default(),
        )
        .unwrap();
    let request = request(&fixture, &repository, &managed);
    let mut changed = request.clone();
    changed.target.branch = "foreign".into();
    assert!(
        read(&changed)
            .unwrap_err()
            .to_string()
            .contains("branch identity")
    );
    changed = request.clone();
    changed.target.common_dir = fixture.data.clone();
    assert!(
        read(&changed)
            .unwrap_err()
            .to_string()
            .contains("repository identity")
    );
    std::fs::write(
        &request.overrides,
        format!("{OVERRIDES}[include]\npath = arbitrary\n"),
    )
    .unwrap();
    assert!(
        read(&request)
            .unwrap_err()
            .to_string()
            .contains("fixed read-only policy")
    );
    std::fs::write(&request.overrides, OVERRIDES).unwrap();
    std::fs::write(
        request.target.metadata.join("gitdir"),
        fixture.repo.join(".git").to_str().unwrap(),
    )
    .unwrap();
    assert!(read(&request).is_err());
}

#[test]
fn owned_inspection_helper_reads_long_worktree_changes_and_ignored_files() {
    use super::{GitExecution, block_on};
    use cyber_core::worktrees::inspection::Operation;
    let fixture = Fixture::new();
    std::fs::write(fixture.repo.join(".gitignore"), ".env\n").unwrap();
    fixture.git(&["add", ".gitignore"]);
    fixture.git(&["commit", "--quiet", "-m", "ignore environment"]);
    let repository = fixture.repository();
    let settings = Settings {
        root: Some(
            fixture
                .data
                .join("segment".repeat(15))
                .join("another".repeat(15)),
        ),
        ..Default::default()
    };
    let managed = fixture
        .create(
            &repository,
            &Name::parse("owned-inspection").unwrap(),
            &settings,
        )
        .unwrap();
    assert!(managed.path.as_os_str().len() > 260);
    let mut request = request(&fixture, &repository, &managed);
    let clean = block_on(fixture.execution.inspect(&request.target)).unwrap();
    assert!(!clean.status.dirty);
    std::fs::write(managed.path.join("tracked.txt"), "modified\n").unwrap();
    std::fs::write(managed.path.join("new.txt"), "untracked\n").unwrap();
    std::fs::write(managed.path.join(".env"), "ignored\n").unwrap();
    let report = block_on(fixture.execution.inspect(&request.target)).unwrap();
    assert!(report.status.dirty);
    assert_eq!(report.files[0].file, "tracked.txt");
    assert_eq!(report.untracked, [std::path::PathBuf::from("new.txt")]);
    assert_eq!(report.ignored, [std::path::PathBuf::from(".env")]);
    request.target.operation = Operation::Status;
    let report = block_on(fixture.execution.inspect(&request.target)).unwrap();
    assert!(report.status.dirty);
    assert!(report.files.is_empty());
    assert!(report.ignored.is_empty());
    // On Windows these public reads exercise production routing to the helper.
    assert!(
        block_on(repository.status(&fixture.execution, &managed))
            .unwrap()
            .dirty
    );
    let changes = block_on(repository.changes(&fixture.execution, &managed)).unwrap();
    assert!(changes.dirty);
    assert_eq!(changes.files.len(), 3);
}
