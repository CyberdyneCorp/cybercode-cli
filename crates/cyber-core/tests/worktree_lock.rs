use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use cyber_core::worktrees::RepositoryLock;

fn probe(git_dir: &Path, expected: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "lock_probe_worker", "--nocapture"])
        .env("CYBER_WORKTREE_LOCK_DIR", git_dir)
        .env("CYBER_WORKTREE_LOCK_EXPECT", expected);
    command
}

fn assert_probe(git_dir: &Path, expected: &str) {
    let output = probe(git_dir, expected).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn lock_probe_worker() {
    let Some(git_dir) = std::env::var_os("CYBER_WORKTREE_LOCK_DIR") else {
        return;
    };
    let expected = std::env::var("CYBER_WORKTREE_LOCK_EXPECT").unwrap();
    let guard = RepositoryLock::try_acquire(Path::new(&git_dir)).unwrap();
    match expected.as_str() {
        "busy" => assert!(guard.is_none()),
        "acquired" => assert!(guard.is_some()),
        "hold" => {
            assert!(guard.is_some());
            println!("worktree-lock-held");
            std::io::stdout().flush().unwrap();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).unwrap();
        }
        other => panic!("unknown probe {other}"),
    }
    drop(guard);
}

#[test]
fn independent_processes_contend_and_owner_drop_releases_without_truncation() {
    let repo = tempfile::tempdir().unwrap();
    let lock_path = repo.path().join("cyber-worktree.lock");
    std::fs::write(&lock_path, "persistent metadata").unwrap();
    assert_eq!(
        std::fs::read_to_string(&lock_path).unwrap(),
        "persistent metadata"
    );
    let owner = RepositoryLock::try_acquire(repo.path()).unwrap().unwrap();
    assert_probe(repo.path(), "busy");
    drop(owner);
    assert_probe(repo.path(), "acquired");
    assert_eq!(
        std::fs::read_to_string(&lock_path).unwrap(),
        "persistent metadata"
    );
}

#[test]
fn independent_repositories_do_not_contend_and_io_failures_are_not_busy() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let _owner = RepositoryLock::try_acquire(first.path()).unwrap().unwrap();
    assert_probe(second.path(), "acquired");
    assert_eq!(
        RepositoryLock::try_acquire(&first.path().join("missing"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn forced_owner_termination_releases_the_same_persistent_lock() {
    let repo = tempfile::tempdir().unwrap();
    let mut child = probe(repo.path(), "hold")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        assert_ne!(
            output.read_line(&mut line).unwrap(),
            0,
            "worker exited before holding lock"
        );
        if line.trim() == "worktree-lock-held" {
            break;
        }
        line.clear();
    }
    assert!(child.try_wait().unwrap().is_none());
    assert_probe(repo.path(), "busy");
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(repo.path().join("cyber-worktree.lock").exists());
    assert_probe(repo.path(), "acquired");
}
