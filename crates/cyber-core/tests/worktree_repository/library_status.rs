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
