use super::*;
use cyber_core::worktrees::{Managed, RemovalActivity, RemovalPhase, RemovalRecord};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct NoSetup;
impl cyber_core::worktrees::SetupExecution for NoSetup {
    fn run<'a>(
        &'a self,
        _: &'a Path,
        _: &'a str,
        _: &'a dyn cyber_core::worktrees::SetupSink,
    ) -> cyber_core::worktrees::SetupFuture<'a> {
        Box::pin(async { panic!("setup dispatched during removal") })
    }
}
impl cyber_core::worktrees::SetupSink for NoSetup {
    fn emit(&self, _: cyber_core::worktrees::SetupEvent<'_>) -> io::Result<()> {
        panic!("setup event emitted during removal")
    }
}

struct Activity {
    in_use: bool,
    held: Arc<AtomicBool>,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            in_use: false,
            held: Arc::new(AtomicBool::new(false)),
        }
    }
}

struct Guard(Arc<AtomicBool>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl RemovalActivity for Activity {
    fn reserve(&self, managed: &Managed) -> io::Result<Box<dyn Send>> {
        assert!(RepositoryLock::try_acquire(&managed.common_dir)?.is_none());
        if self.in_use {
            return Err(io::Error::other("Worktree in use by ses_active"));
        }
        assert!(!self.held.swap(true, Ordering::SeqCst));
        Ok(Box::new(Guard(Arc::clone(&self.held))))
    }
}

#[derive(Clone, Copy)]
enum Failure {
    BeforeTree,
    AfterTree,
    SuspendAfterTree,
    BeforeBranch,
    AfterBranch,
    RecreateBranch,
    ReplacePath,
    MoveBranch,
}

struct Interrupted<'a> {
    fixture: &'a Fixture,
    managed: &'a Managed,
    held: Arc<AtomicBool>,
    failure: Failure,
}

impl GitExecution for Interrupted<'_> {
    fn run<'a>(&'a self, directory: &'a Path, args: &'a [OsString]) -> GitFuture<'a> {
        Box::pin(async move {
            assert!(
                self.held.load(Ordering::SeqCst),
                "activity guard dropped before Git settlement"
            );
            let tree = args
                .windows(2)
                .any(|pair| pair[0] == "worktree" && pair[1] == "remove");
            let branch =
                args.iter().any(|arg| arg == "update-ref") && args.iter().any(|arg| arg == "-d");
            if (tree && matches!(self.failure, Failure::BeforeTree))
                || (branch && matches!(self.failure, Failure::BeforeBranch))
            {
                return Err(io::Error::other("injected before mutation"));
            }
            let output = self.fixture.execution.invoke(directory, args)?;
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            if tree {
                self.after_tree().await?;
            }
            if branch {
                self.after_branch()?;
            }
            Ok(output)
        })
    }
}

impl Interrupted<'_> {
    async fn after_tree(&self) -> io::Result<()> {
        match self.failure {
            Failure::AfterTree => Err(io::Error::other("lost tree acknowledgement")),
            Failure::SuspendAfterTree => std::future::pending().await,
            Failure::ReplacePath => {
                std::fs::create_dir(&self.managed.path)?;
                std::fs::write(self.managed.path.join("user.txt"), "replacement")
            }
            Failure::MoveBranch => {
                let head = self.fixture.git(&["rev-parse", "HEAD"]);
                self.fixture.git(&[
                    "update-ref",
                    &format!("refs/heads/{}", self.managed.branch),
                    &head,
                ]);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn after_branch(&self) -> io::Result<()> {
        match self.failure {
            Failure::AfterBranch => Err(io::Error::other("lost branch acknowledgement")),
            Failure::RecreateBranch => {
                self.fixture.git(&[
                    "update-ref",
                    &format!("refs/heads/{}", self.managed.branch),
                    &self.managed.base,
                ]);
                Err(io::Error::other(
                    "branch recreated after lost acknowledgement",
                ))
            }
            _ => Ok(()),
        }
    }
}

fn managed(fixture: &Fixture, repo: &Repository) -> Managed {
    fixture
        .create(
            repo,
            &Name::parse("remove-me").unwrap(),
            &Settings::default(),
        )
        .unwrap()
}

fn journal(repo: &Repository) -> RemovalRecord {
    serde_json::from_slice(
        &std::fs::read(
            repo.common_dir
                .join("cyber-worktree-removals/remove-me.json"),
        )
        .unwrap(),
    )
    .unwrap()
}

fn interrupted<'a>(
    fixture: &'a Fixture,
    managed: &'a Managed,
    activity: &Activity,
    failure: Failure,
) -> Interrupted<'a> {
    Interrupted {
        fixture,
        managed,
        held: Arc::clone(&activity.held),
        failure,
    }
}

#[test]
fn clean_removal_settles_and_old_identity_replay_preserves_a_new_creation() {
    let fixture = Fixture::new();
    let repo = fixture.repository();
    let first = managed(&fixture, &repo);
    let activity = Activity::default();
    let done = block_on(repo.remove(&fixture.execution, &activity, &first, false)).unwrap();
    assert_eq!(done.phase, RemovalPhase::Completed);
    assert!(!first.path.exists());
    assert!(fixture.git(&["branch", "--list", &first.branch]).is_empty());
    assert!(block_on(repo.list(&fixture.execution)).unwrap().is_empty());
    let second = managed(&fixture, &repo);
    assert_ne!(first.id, second.id);
    std::fs::write(second.path.join("user.txt"), "new work").unwrap();
    assert_eq!(
        block_on(repo.remove(&fixture.execution, &activity, &first, false)).unwrap(),
        done
    );
    assert_eq!(
        std::fs::read_to_string(second.path.join("user.txt")).unwrap(),
        "new work"
    );
    assert!(!activity.held.load(Ordering::SeqCst));
}

#[test]
fn removal_from_selected_linked_location_uses_a_surviving_git_launch_directory() {
    let fixture = Fixture::new();
    let repo = fixture.repository();
    let owned = managed(&fixture, &repo);
    let linked = block_on(Repository::discover(&fixture.execution, &owned.path)).unwrap();
    assert_eq!(linked.root, owned.path);
    let result =
        block_on(linked.remove(&fixture.execution, &Activity::default(), &owned, false)).unwrap();
    assert_eq!(result.phase, RemovalPhase::Completed);
    assert!(!owned.path.exists());
    assert!(repo.common_dir.exists());
    assert!(fixture.repo.join("tracked.txt").exists());
}

#[test]
fn dirty_or_ahead_work_requires_force_and_activity_always_refuses() {
    let fixture = Fixture::new();
    let repo = fixture.repository();
    let owned = managed(&fixture, &repo);
    let activity = Activity::default();
    std::fs::write(owned.path.join("user.txt"), "keep").unwrap();
    assert!(
        block_on(repo.remove(&fixture.execution, &activity, &owned, false))
            .unwrap_err()
            .to_string()
            .contains("uncommitted")
    );
    std::fs::remove_file(owned.path.join("user.txt")).unwrap();
    let output = fixture
        .execution
        .invoke(
            &owned.path,
            &[
                "commit".into(),
                "--allow-empty".into(),
                "-m".into(),
                "ahead".into(),
            ],
        )
        .unwrap();
    assert!(output.status.success());
    assert!(block_on(repo.remove(&fixture.execution, &activity, &owned, false)).is_err());
    let active = Activity {
        in_use: true,
        ..Default::default()
    };
    assert_eq!(
        block_on(repo.remove(&fixture.execution, &active, &owned, true))
            .unwrap_err()
            .to_string(),
        "Worktree in use by ses_active"
    );
    assert!(
        !repo
            .common_dir
            .join("cyber-worktree-removals/remove-me.json")
            .exists()
    );
    assert!(owned.path.exists());
    block_on(repo.remove(&fixture.execution, &activity, &owned, true)).unwrap();
    assert!(!owned.path.exists());
}

#[test]
fn unknown_tree_intent_blocks_redispatch_reuse_setup_and_status() {
    let fixture = Fixture::new();
    let repo = fixture.repository();
    let owned = managed(&fixture, &repo);
    let activity = Activity::default();
    let execution = interrupted(&fixture, &owned, &activity, Failure::BeforeTree);
    assert!(block_on(repo.remove(&execution, &activity, &owned, false)).is_err());
    assert_eq!(journal(&repo).phase, RemovalPhase::TreeRemovalIntent);
    assert!(
        block_on(repo.remove(&fixture.execution, &activity, &owned, false))
            .unwrap_err()
            .to_string()
            .contains("outcome is unknown")
    );
    assert!(
        fixture
            .create(
                &repo,
                &Name::parse(&owned.name).unwrap(),
                &Settings::default()
            )
            .is_err()
    );
    assert!(block_on(repo.status(&fixture.execution, &owned)).is_err());
    assert!(
        block_on(repo.setup(
            &fixture.execution,
            &NoSetup,
            &owned,
            &Settings::default(),
            &NoSetup
        ))
        .is_err()
    );
    assert!(
        matches!(&block_on(repo.list(&fixture.execution)).unwrap()[0], cyber_core::worktrees::ListedWorktree::Invalid { name, error } if name == "remove-me" && error.contains("recovery"))
    );
    assert_eq!(
        std::fs::read_to_string(owned.path.join("tracked.txt")).unwrap(),
        "base"
    );
}

#[test]
fn missing_tree_and_branch_acknowledgements_reconcile_without_repeating_deletion() {
    for failure in [Failure::AfterTree, Failure::AfterBranch] {
        let fixture = Fixture::new();
        let repo = fixture.repository();
        let owned = managed(&fixture, &repo);
        let activity = Activity::default();
        let execution = interrupted(&fixture, &owned, &activity, failure);
        assert!(block_on(repo.remove(&execution, &activity, &owned, false)).is_err());
        assert!(!owned.path.exists());
        let done = block_on(repo.remove(&fixture.execution, &activity, &owned, false)).unwrap();
        assert_eq!(done.phase, RemovalPhase::Completed);
        assert!(fixture.git(&["branch", "--list", &owned.branch]).is_empty());
    }
}

#[test]
fn unknown_branch_intent_preserves_same_oid_recreated_branch() {
    for failure in [Failure::BeforeBranch, Failure::RecreateBranch] {
        let fixture = Fixture::new();
        let repo = fixture.repository();
        let owned = managed(&fixture, &repo);
        let activity = Activity::default();
        let execution = interrupted(&fixture, &owned, &activity, failure);
        assert!(block_on(repo.remove(&execution, &activity, &owned, false)).is_err());
        assert_eq!(journal(&repo).phase, RemovalPhase::BranchRemovalIntent);
        assert!(
            block_on(repo.remove(&fixture.execution, &activity, &owned, false))
                .unwrap_err()
                .to_string()
                .contains("Branch removal outcome is unknown")
        );
        assert_eq!(
            fixture.git(&["rev-parse", &format!("refs/heads/{}", owned.branch)]),
            owned.base
        );
    }
}

#[test]
fn replacement_path_and_branch_movement_survive_removal_failure() {
    for failure in [Failure::ReplacePath, Failure::MoveBranch] {
        let fixture = Fixture::new();
        let repo = fixture.repository();
        let owned = managed(&fixture, &repo);
        fixture.git(&["commit", "--allow-empty", "-m", "new source head"]);
        let activity = Activity::default();
        let execution = interrupted(&fixture, &owned, &activity, failure);
        assert!(block_on(repo.remove(&execution, &activity, &owned, false)).is_err());
        assert!(block_on(repo.remove(&fixture.execution, &activity, &owned, false)).is_err());
        assert!(!fixture.git(&["branch", "--list", &owned.branch]).is_empty());
        if matches!(failure, Failure::ReplacePath) {
            assert_eq!(
                std::fs::read_to_string(owned.path.join("user.txt")).unwrap(),
                "replacement"
            );
        }
    }
}

#[test]
fn cancellation_after_tree_removal_releases_activity_and_repository_guards() {
    let fixture = Fixture::new();
    let repo = fixture.repository();
    let owned = managed(&fixture, &repo);
    let activity = Activity::default();
    let execution = interrupted(&fixture, &owned, &activity, Failure::SuspendAfterTree);
    let mut future = Box::pin(repo.remove(&execution, &activity, &owned, false));
    let waker = std::task::Waker::noop();
    assert!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(waker))
            .is_pending()
    );
    assert!(activity.held.load(Ordering::SeqCst));
    assert!(
        RepositoryLock::try_acquire(&repo.common_dir)
            .unwrap()
            .is_none()
    );
    drop(future);
    assert!(!activity.held.load(Ordering::SeqCst));
    assert!(
        RepositoryLock::try_acquire(&repo.common_dir)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        block_on(repo.remove(&fixture.execution, &activity, &owned, false))
            .unwrap()
            .phase,
        RemovalPhase::Completed
    );
}

#[test]
fn unchanged_ignored_inclusions_can_be_removed_but_new_or_edited_ignored_files_are_kept() {
    for change in [None, Some(".env"), Some("cache")] {
        let fixture = Fixture::new();
        std::fs::write(fixture.repo.join(".gitignore"), ".env\ncache\n").unwrap();
        fixture.git(&["add", ".gitignore"]);
        fixture.git(&["commit", "--quiet", "-m", "ignore local files"]);
        std::fs::write(fixture.repo.join(".worktreeinclude"), ".env\n").unwrap();
        std::fs::write(fixture.repo.join(".env"), "initial local config").unwrap();
        let repo = fixture.repository();
        let owned = managed(&fixture, &repo);
        if let Some(path) = change {
            std::fs::write(owned.path.join(path), "keep ignored edit").unwrap();
        }
        let result = block_on(repo.remove(&fixture.execution, &Activity::default(), &owned, false));
        assert_eq!(result.is_ok(), change.is_none());
        if let Some(path) = change {
            assert_eq!(
                std::fs::read_to_string(owned.path.join(path)).unwrap(),
                "keep ignored edit"
            );
        }
    }
}

#[test]
fn incomplete_removal_stays_visible_after_ownership_unlink() {
    let fixture = Fixture::new();
    let repo = fixture.repository();
    let owned = managed(&fixture, &repo);
    let activity = Activity::default();
    let mut done = block_on(repo.remove(&fixture.execution, &activity, &owned, false)).unwrap();
    done.phase = RemovalPhase::BranchRemoved;
    std::fs::write(
        repo.common_dir
            .join("cyber-worktree-removals/remove-me.json"),
        serde_json::to_vec(&done).unwrap(),
    )
    .unwrap();
    std::fs::remove_dir(repo.common_dir.join("cyber-worktrees")).unwrap();
    assert!(
        matches!(&block_on(repo.list(&fixture.execution)).unwrap()[0], cyber_core::worktrees::ListedWorktree::Invalid { name, error } if name == "remove-me" && error.contains("BranchRemoved"))
    );
    assert!(
        fixture
            .create(
                &repo,
                &Name::parse(&owned.name).unwrap(),
                &Settings::default()
            )
            .is_err()
    );
    assert_eq!(
        block_on(repo.remove(&fixture.execution, &activity, &owned, false))
            .unwrap()
            .phase,
        RemovalPhase::Completed
    );
}

#[test]
fn removal_refuses_stale_identity_or_unowned_records_before_mutation() {
    let fixture = Fixture::new();
    let repo = fixture.repository();
    let owned = managed(&fixture, &repo);
    let activity = Activity::default();
    let mut stale = owned.clone();
    stale.id = "wt_other".into();
    assert!(block_on(repo.remove(&fixture.execution, &activity, &stale, true)).is_err());
    assert_eq!(
        std::fs::read_to_string(owned.path.join("tracked.txt")).unwrap(),
        "base"
    );
    assert!(
        !repo
            .common_dir
            .join("cyber-worktree-removals/remove-me.json")
            .exists()
    );
    let _lock = RepositoryLock::try_acquire(&repo.common_dir)
        .unwrap()
        .unwrap();
    assert_eq!(
        block_on(repo.remove(&fixture.execution, &activity, &owned, true))
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
}

#[cfg(unix)]
#[test]
fn removal_refuses_symlinked_journal_directory_and_preserves_ignored_symlinks() {
    let fixture = Fixture::new();
    std::fs::write(fixture.repo.join(".gitignore"), ".env\n").unwrap();
    fixture.git(&["add", ".gitignore"]);
    fixture.git(&["commit", "--quiet", "-m", "ignore local config"]);
    let repo = fixture.repository();
    let owned = managed(&fixture, &repo);
    let outside = fixture._temp.path().join("secret");
    std::fs::write(&outside, "keep secret").unwrap();
    std::os::unix::fs::symlink(&outside, owned.path.join(".env")).unwrap();
    let activity = Activity::default();
    assert!(block_on(repo.remove(&fixture.execution, &activity, &owned, false)).is_err());
    let other_dir = fixture._temp.path().join("outside-journal");
    std::fs::create_dir(&other_dir).unwrap();
    std::os::unix::fs::symlink(&other_dir, repo.common_dir.join("cyber-worktree-removals"))
        .unwrap();
    assert!(
        block_on(repo.remove(&fixture.execution, &activity, &owned, true))
            .unwrap_err()
            .to_string()
            .contains("regular directory")
    );
    assert!(std::fs::read_dir(other_dir).unwrap().next().is_none());
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "keep secret");
    assert!(owned.path.exists());
}
