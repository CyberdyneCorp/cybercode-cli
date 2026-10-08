//! Exercise the installed helper's private protocol, including failure output.

use std::path::Path;
use std::process::{Command, Output};

use cyber_core::worktrees::inspection::{Inspection, OVERRIDES, Operation, Request, Target};

fn git(directory: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .current_dir(directory)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn inspection_helper_returns_scoped_status_and_refuses_foreign_branch() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "--quiet"]);
    std::fs::write(repo.join("tracked.txt"), "initial\n").unwrap();
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--quiet",
            "-m",
            "initial",
        ],
    );
    let worktree = temp.path().join("checkout");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "inspection",
            worktree.to_str().unwrap(),
        ],
    );
    let marker = std::fs::read_to_string(worktree.join(".git")).unwrap();
    let overrides = temp.path().join("inspection.config");
    std::fs::write(&overrides, OVERRIDES).unwrap();
    let mut request = Request {
        target: Target {
            metadata: Path::new(marker.trim().strip_prefix("gitdir: ").unwrap())
                .canonicalize()
                .unwrap(),
            common_dir: repo.join(".git").canonicalize().unwrap(),
            worktree: worktree.canonicalize().unwrap(),
            branch: "inspection".into(),
            base: String::from_utf8(git(&repo, &["rev-parse", "HEAD"]).stdout)
                .unwrap()
                .trim()
                .into(),
            operation: Operation::Changes,
        },
        overrides,
    };
    std::fs::write(worktree.join("tracked.txt"), "modified\n").unwrap();
    let home = temp.path().join("ambient-home");
    let xdg = temp.path().join("ambient-xdg");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(xdg.join("git")).unwrap();
    // Invalid ambient configuration must never affect the owned repository read.
    std::fs::write(home.join(".gitconfig"), "this is not git configuration").unwrap();
    std::fs::write(xdg.join("git/config"), "also invalid configuration").unwrap();
    let invoke = |request: &Request| {
        Command::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))
            .current_dir(&request.target.metadata)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("XDG_CONFIG_HOME", &xdg)
            .args(["--git-inspect", &serde_json::to_string(request).unwrap()])
            .output()
            .unwrap()
    };
    let output = invoke(&request);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Inspection = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report.target, request.target);
    assert!(report.status.dirty);
    assert_eq!(report.files[0].file, "tracked.txt");
    request.target.branch = "foreign".into();
    let output = invoke(&request);
    assert_eq!(output.status.code(), Some(65));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("branch identity"));
}

#[test]
fn inspection_helper_refuses_malformed_and_oversized_requests() {
    for request in ["{}".to_owned(), "x".repeat(8193)] {
        let output = Command::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))
            .args(["--git-inspect", &request])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(65));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}
