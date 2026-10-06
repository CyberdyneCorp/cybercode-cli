use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::task::{Context, Poll};

use cyber_core::worktrees::{GitExecution, GitFuture, Name, Repository, RepositoryLock, Settings};
use futures::executor::block_on;

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    FailAfterCreation,
    SuspendAfterCreation,
}

struct Execution {
    config: PathBuf,
    hooks: PathBuf,
    mode: Mode,
}

impl Execution {
    fn invoke(&self, directory: &Path, args: &[OsString]) -> io::Result<Output> {
        let mut command = Command::new("git");
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap());
        if let Some(root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", root);
        }
        command
            .env("TEMP", self.config.parent().unwrap())
            .env("TMP", self.config.parent().unwrap())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.config)
            .current_dir(directory)
            .args(["-c", "core.fsmonitor=false", "-c"])
            .arg(format!("core.hooksPath={}", self.hooks.display()))
            .args(args)
            .output()
    }
}

impl GitExecution for Execution {
    fn run<'a>(&'a self, directory: &'a Path, args: &'a [OsString]) -> GitFuture<'a> {
        Box::pin(async move {
            let output = self.invoke(directory, args)?;
            if args
                .windows(2)
                .any(|pair| pair[0] == "worktree" && pair[1] == "add")
                && output.status.success()
            {
                match self.mode {
                    Mode::Normal => {}
                    Mode::FailAfterCreation => {
                        return Err(io::Error::other("injected failure after creation"));
                    }
                    Mode::SuspendAfterCreation => return std::future::pending().await,
                }
            }
            Ok(output)
        })
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    data: PathBuf,
    execution: Execution,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("source");
        std::fs::create_dir(&repo).unwrap();
        let config = temp.path().join("git-config");
        std::fs::write(&config, "").unwrap();
        let data = temp.path().join("data");
        let execution = Execution {
            config,
            hooks: temp.path().join("no-hooks"),
            mode: Mode::Normal,
        };
        let fixture = Self {
            _temp: temp,
            repo,
            data,
            execution,
        };
        fixture.git(&["init", "--quiet"]);
        fixture.git(&["config", "user.email", "test@example.invalid"]);
        fixture.git(&["config", "user.name", "Worktree Test"]);
        std::fs::write(fixture.repo.join("tracked.txt"), "base").unwrap();
        fixture.git(&["add", "tracked.txt"]);
        fixture.git(&["commit", "--quiet", "-m", "initial"]);
        fixture
    }
    fn git(&self, args: &[&str]) -> String {
        let output = self
            .execution
            .invoke(
                &self.repo,
                &args.iter().map(OsString::from).collect::<Vec<_>>(),
            )
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim_end().into()
    }
    fn repository(&self) -> Repository {
        block_on(Repository::discover(&self.execution, &self.repo)).unwrap()
    }
    fn create(
        &self,
        repository: &Repository,
        name: &Name,
        settings: &Settings,
    ) -> io::Result<cyber_core::worktrees::Managed> {
        block_on(repository.create(&self.execution, settings, &self.data, "prj_test", name))
    }
}

#[test]
fn creates_real_worktree_and_reuses_without_overwriting_user_edits() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let name = Name::parse("experiment").unwrap();
    let managed = fixture
        .create(&repository, &name, &Settings::default())
        .unwrap();
    assert!(managed.ready);
    assert_eq!(managed.branch, "cyber/experiment");
    assert_eq!(managed.base, fixture.git(&["rev-parse", "HEAD"]));
    assert_eq!(
        managed.path,
        fixture
            .data
            .join("worktrees/prj_test/experiment")
            .canonicalize()
            .unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(managed.path.join("tracked.txt")).unwrap(),
        "base"
    );
    std::fs::write(managed.path.join("tracked.txt"), "user edits").unwrap();
    let reused = fixture
        .create(&repository, &name, &Settings::default())
        .unwrap();
    assert_eq!(managed, reused);
    assert_eq!(
        std::fs::read_to_string(managed.path.join("tracked.txt")).unwrap(),
        "user edits"
    );
    let linked = block_on(Repository::discover(&fixture.execution, &managed.path)).unwrap();
    assert_eq!(linked.common_dir, repository.common_dir);
    let _owner = RepositoryLock::try_acquire(&repository.common_dir)
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture
            .create(
                &linked,
                &Name::parse("second").unwrap(),
                &Settings::default()
            )
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn custom_root_branch_and_commit_base_are_applied() {
    let fixture = Fixture::new();
    let base = fixture.git(&["rev-parse", "HEAD"]);
    std::fs::write(fixture.repo.join("tracked.txt"), "new HEAD").unwrap();
    fixture.git(&["commit", "--quiet", "-am", "advance"]);
    let settings = Settings {
        root: Some(PathBuf::from("../custom")),
        branch_prefix: "task/".into(),
        base: base.clone(),
        ..Default::default()
    };
    let managed = fixture
        .create(
            &fixture.repository(),
            &Name::parse("old-base").unwrap(),
            &settings,
        )
        .unwrap();
    assert_eq!(managed.branch, "task/old-base");
    assert_eq!(managed.base, base);
    assert_eq!(
        managed.path.parent().unwrap(),
        fixture
            .repo
            .parent()
            .unwrap()
            .join("custom")
            .canonicalize()
            .unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(managed.path.join("tracked.txt")).unwrap(),
        "base"
    );
}

#[test]
fn unmanaged_target_and_invalid_base_are_preserved_without_ownership() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let name = Name::parse("existing").unwrap();
    let target = fixture.data.join("worktrees/prj_test/existing");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("user.txt"), "preserve").unwrap();
    assert_eq!(
        fixture
            .create(&repository, &name, &Settings::default())
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(
        std::fs::read_to_string(target.join("user.txt")).unwrap(),
        "preserve"
    );
    assert!(
        !repository
            .common_dir
            .join("cyber-worktrees/existing.json")
            .exists()
    );
    let settings = Settings {
        base: "--help".into(),
        ..Default::default()
    };
    assert!(
        fixture
            .create(&repository, &Name::parse("bad-base").unwrap(), &settings)
            .is_err()
    );
    assert!(
        !repository
            .common_dir
            .join("cyber-worktrees/bad-base.json")
            .exists()
    );
}

#[test]
fn failed_creation_keeps_pending_record_files_and_branch_for_recovery() {
    let mut fixture = Fixture::new();
    let repository = fixture.repository();
    fixture.execution.mode = Mode::FailAfterCreation;
    let name = Name::parse("interrupted").unwrap();
    assert!(
        fixture
            .create(&repository, &name, &Settings::default())
            .is_err()
    );
    fixture.execution.mode = Mode::Normal;
    let record = repository
        .common_dir
        .join("cyber-worktrees/interrupted.json");
    let managed: cyber_core::worktrees::Managed =
        serde_json::from_slice(&std::fs::read(record).unwrap()).unwrap();
    assert!(!managed.ready);
    std::fs::write(managed.path.join("tracked.txt"), "user edits after failure").unwrap();
    assert!(
        fixture
            .create(&repository, &name, &Settings::default())
            .unwrap_err()
            .to_string()
            .contains("recovery")
    );
    assert_eq!(
        std::fs::read_to_string(managed.path.join("tracked.txt")).unwrap(),
        "user edits after failure"
    );
    assert_eq!(
        fixture.git(&["rev-parse", "cyber/interrupted"]),
        managed.base
    );
    assert!(
        RepositoryLock::try_acquire(&repository.common_dir)
            .unwrap()
            .is_some()
    );
}

#[test]
fn cancellation_releases_lock_and_preserves_pending_creation() {
    let mut fixture = Fixture::new();
    let repository = fixture.repository();
    fixture.execution.mode = Mode::SuspendAfterCreation;
    let settings = Settings::default();
    let name = Name::parse("cancelled").unwrap();
    let mut future = Box::pin(repository.create(
        &fixture.execution,
        &settings,
        &fixture.data,
        "prj_test",
        &name,
    ));
    let mut context = Context::from_waker(futures::task::noop_waker_ref());
    assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
    assert!(
        RepositoryLock::try_acquire(&repository.common_dir)
            .unwrap()
            .is_none()
    );
    drop(future);
    assert!(
        RepositoryLock::try_acquire(&repository.common_dir)
            .unwrap()
            .is_some()
    );
    let record = repository.common_dir.join("cyber-worktrees/cancelled.json");
    let managed: cyber_core::worktrees::Managed =
        serde_json::from_slice(&std::fs::read(record).unwrap()).unwrap();
    assert!(!managed.ready);
    assert_eq!(
        std::fs::read_to_string(managed.path.join("tracked.txt")).unwrap(),
        "base"
    );
}

#[test]
fn changed_branch_refuses_reuse_and_preserves_user_files() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let name = Name::parse("changed").unwrap();
    let managed = fixture
        .create(&repository, &name, &Settings::default())
        .unwrap();
    let args = ["switch", "-c", "user-branch"].map(OsString::from);
    let output = fixture.execution.invoke(&managed.path, &args).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(
        managed.path.join("tracked.txt"),
        "preserve after branch switch",
    )
    .unwrap();
    assert_eq!(
        fixture
            .create(&repository, &name, &Settings::default())
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        std::fs::read_to_string(managed.path.join("tracked.txt")).unwrap(),
        "preserve after branch switch"
    );
}

#[cfg(windows)]
#[test]
fn windows_long_paths_are_preserved_at_the_git_argument_boundary() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let root = fixture
        .data
        .join("long-segment".repeat(8))
        .join("another-segment".repeat(8));
    let settings = Settings {
        root: Some(root),
        ..Default::default()
    };
    let managed = fixture
        .create(&repository, &Name::parse("long-path").unwrap(), &settings)
        .unwrap();
    assert!(managed.path.as_os_str().len() > 260);
    assert_eq!(
        std::fs::read_to_string(managed.path.join("tracked.txt")).unwrap(),
        "base"
    );
    assert_eq!(
        fixture
            .create(&repository, &Name::parse("long-path").unwrap(), &settings)
            .unwrap(),
        managed
    );
}

#[cfg(windows)]
#[test]
fn windows_ambiguous_path_names_fail_before_ownership_or_branch_mutation() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let name = Name::parse("trailing.").unwrap();
    assert!(
        fixture
            .create(&repository, &name, &Settings::default())
            .is_err()
    );
    assert!(
        !repository
            .common_dir
            .join("cyber-worktrees/trailing..json")
            .exists()
    );
    assert!(
        !fixture
            .git(&["branch", "--list", "cyber/trailing."])
            .contains("trailing")
    );
}
