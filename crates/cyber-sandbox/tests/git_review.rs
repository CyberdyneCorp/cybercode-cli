//! Exercise the helper's `--git-review` protocol against real repositories.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use cyber_core::review::resolve::{FileStatus, MAX_ENGINE_PATCH, MAX_PATCH, ReviewDiff};
use cyber_core::review::{ErrorCode, Target, TargetError};
use cyber_core::worktrees::inspection::OVERRIDES;

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

struct Fixture {
    temp: tempfile::TempDir,
    repo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        std::fs::write(temp.path().join("review.config"), OVERRIDES).unwrap();
        Self { temp, repo }
    }

    fn write(&self, path: &str, contents: &str) {
        std::fs::write(self.repo.join(path), contents).unwrap();
    }

    fn commit(&self, message: &str) -> String {
        git(&self.repo, &["add", "-A"]);
        git(&self.repo, &["commit", "--quiet", "-m", message]);
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    fn invoke_raw(&self, request: &str) -> Output {
        let home = self.temp.path().join("ambient-home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join(".gitconfig"), "this is not git configuration").unwrap();
        Command::new(env!("CARGO_BIN_EXE_cyber-sandbox-exec"))
            .current_dir(&self.repo)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("XDG_CONFIG_HOME", &home)
            .args(["--git-review", request])
            .output()
            .unwrap()
    }

    fn request(&self, location: &Path, target: &str) -> String {
        serde_json::json!({
            "location": location,
            "target": target,
            "overrides": self.temp.path().join("review.config"),
        })
        .to_string()
    }

    fn review_at(&self, location: &Path, target: &str) -> Result<ReviewDiff, TargetError> {
        let output = self.invoke_raw(&self.request(location, target));
        match output.status.code() {
            Some(0) => Ok(serde_json::from_slice(&output.stdout).unwrap()),
            Some(2) => Err(serde_json::from_slice(&output.stdout).unwrap()),
            code => panic!(
                "unexpected exit {code:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        }
    }

    fn review(&self, target: &str) -> Result<ReviewDiff, TargetError> {
        self.review_at(&self.repo, target)
    }
}

fn paths(diff: &ReviewDiff) -> Vec<(&str, FileStatus)> {
    diff.files
        .iter()
        .map(|file| (file.path.as_str(), file.status))
        .collect()
}

#[test]
fn uncommitted_review_includes_staged_unstaged_and_untracked_changes() {
    let fixture = Fixture::new();
    fixture.write("src/lib.rs", "fn a() {}\n");
    fixture.write("src/staged.rs", "fn s() {}\n");
    let head = fixture.commit("initial");
    fixture.write("src/lib.rs", "fn a() { changed(); }\n");
    fixture.write("src/staged.rs", "fn s() { staged(); }\n");
    git(&fixture.repo, &["add", "src/staged.rs"]);
    fixture.write("src/new.rs", "fn new() {}\n");

    let diff = fixture.review_at(&fixture.repo.join("src"), "").unwrap();
    assert_eq!(diff.target, Target::Uncommitted);
    assert_eq!(diff.root, fixture.repo.canonicalize().unwrap());
    assert_eq!(diff.base.as_deref(), Some(head.as_str()));
    assert_eq!(diff.head, None);
    assert_eq!(
        paths(&diff),
        [
            ("src/lib.rs", FileStatus::Modified),
            ("src/new.rs", FileStatus::Added),
            ("src/staged.rs", FileStatus::Modified),
        ]
    );
    let patch = diff.patch();
    assert!(patch.contains("+fn a() { changed(); }"), "{patch}");
    assert!(patch.contains("+fn new() {}"), "{patch}");
    assert!(patch.contains("+fn s() { staged(); }"), "{patch}");
    assert_eq!(diff.files[1].additions, 1);
    assert!(diff.limitations.is_empty(), "{:?}", diff.limitations);
    assert_eq!(diff.patch_sha256.len(), 64);
}

#[test]
fn uncommitted_review_works_in_linked_worktree_and_unborn_head() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one\n");
    let diff = fixture.review("").unwrap();
    assert_eq!(diff.base, None);
    assert_eq!(paths(&diff), [("a.txt", FileStatus::Added)]);

    fixture.commit("initial");
    let checkout = fixture.temp.path().join("checkout");
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "side",
            checkout.to_str().unwrap(),
        ],
    );
    std::fs::write(checkout.join("a.txt"), "two\n").unwrap();
    let diff = fixture.review_at(&checkout, "").unwrap();
    assert_eq!(diff.root, checkout.canonicalize().unwrap());
    assert_eq!(paths(&diff), [("a.txt", FileStatus::Modified)]);
}

#[test]
fn range_and_commit_targets_resolve_to_full_commit_ids() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one\n");
    let root = fixture.commit("root");
    fixture.write("a.txt", "two\n");
    fixture.write("b.txt", "b\n");
    let second = fixture.commit("second");
    git(&fixture.repo, &["tag", "v1"]);

    let range = fixture.review("main~1..v1").unwrap();
    assert_eq!(range.base.as_deref(), Some(root.as_str()));
    assert_eq!(range.head.as_deref(), Some(second.as_str()));
    assert_eq!(
        paths(&range),
        [
            ("a.txt", FileStatus::Modified),
            ("b.txt", FileStatus::Added)
        ]
    );

    let commit = fixture.review("HEAD").unwrap();
    assert_eq!(commit.base.as_deref(), Some(root.as_str()));
    assert_eq!(commit.head.as_deref(), Some(second.as_str()));
    assert_eq!(commit.patch(), range.patch());

    let initial = fixture.review(&root[..12]).unwrap();
    assert_eq!(
        initial.base, None,
        "a root commit diffs against the empty tree"
    );
    assert_eq!(initial.head.as_deref(), Some(root.as_str()));
    assert_eq!(paths(&initial), [("a.txt", FileStatus::Added)]);

    for target in ["missing-branch", "HEAD~5", "main..nope"] {
        let error = fixture.review(target).unwrap_err();
        assert_eq!(error.code, ErrorCode::UnresolvedRef, "{target}");
    }
    let outside = tempfile::tempdir().unwrap();
    let error = fixture.review_at(outside.path(), "").unwrap_err();
    assert_eq!(error.code, ErrorCode::NotARepository);
    let error = fixture.review("pr 1").unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidTarget);
}

#[test]
fn repository_configured_programs_never_run() {
    let fixture = Fixture::new();
    let marker = fixture.temp.path().join("marker");
    let touch = format!("sh -c 'touch {}'", marker.display());
    fixture.write("plain.txt", "one\n");
    // Configure only after the CLI commit, which would itself honour fsmonitor.
    fixture.commit("initial");
    for (key, value) in [
        ("diff.external", touch.as_str()),
        ("diff.x.textconv", touch.as_str()),
        ("filter.x.clean", touch.as_str()),
        ("filter.x.smudge", touch.as_str()),
        ("filter.x.process", touch.as_str()),
        ("filter.x.required", "true"),
        ("core.pager", touch.as_str()),
        ("core.fsmonitor", touch.as_str()),
    ] {
        git(&fixture.repo, &["config", key, value]);
    }
    fixture.write(".gitattributes", "*.txt diff=x filter=x\n");
    fixture.write("plain.txt", "two\n");
    fixture.write("other.txt", "new\n");
    let hooks = fixture.repo.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    for hook in [
        "post-index-change",
        "reference-transaction",
        "post-checkout",
    ] {
        std::fs::write(hooks.join(hook), format!("#!/bin/sh\n{touch}\n")).unwrap();
    }

    for target in ["", "HEAD"] {
        let diff = fixture.review(target).unwrap();
        assert!(!diff.files.is_empty());
    }
    let diff = fixture.review("").unwrap();
    assert!(diff.patch().contains("+two"), "{}", diff.patch());
    assert!(!marker.exists(), "a repository-configured program ran");
}

/// Regression: the isolated engine never saw the user's global excludes, so
/// untracked files `git status` hides (keys, virtualenvs) reached the reviewer.
#[test]
fn uncommitted_review_applies_the_host_supplied_global_excludes() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one\n");
    fixture.commit("initial");
    fixture.write("a.txt", "two\n");
    fixture.write("deploy.pem", "PRIVATE KEY\n");
    std::fs::create_dir_all(fixture.repo.join("venv")).unwrap();
    fixture.write("venv/lib.py", "vendored\n");
    let names = |diff: &ReviewDiff| -> Vec<String> {
        diff.files.iter().map(|file| file.path.clone()).collect()
    };
    let ambient = fixture.review("").unwrap();
    assert_eq!(names(&ambient), ["a.txt", "deploy.pem", "venv/lib.py"]);

    let excludes = fixture.temp.path().join("global.ignore");
    std::fs::write(&excludes, "*.pem\nvenv/\n").unwrap();
    let mut request: serde_json::Value =
        serde_json::from_str(&fixture.request(&fixture.repo, "")).unwrap();
    request["excludes"] = excludes.display().to_string().into();
    let output = fixture.invoke_raw(&request.to_string());
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let diff: ReviewDiff = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(names(&diff), ["a.txt"]);
    assert!(!diff.patch().contains("PRIVATE KEY"));
}

/// Regression: the host-supplied global excludes applied even when the
/// repository set its own `core.excludesFile`, which git uses instead.
#[test]
fn a_repository_excludes_file_replaces_the_host_supplied_global_one() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one\n");
    fixture.commit("initial");
    fixture.write("deploy.pem", "PRIVATE KEY\n");
    fixture.write("build.log", "noise\n");
    let local = fixture.temp.path().join("local.ignore");
    std::fs::write(&local, "*.log\n").unwrap();
    git(
        &fixture.repo,
        &["config", "core.excludesFile", &local.display().to_string()],
    );
    let global = fixture.temp.path().join("global.ignore");
    std::fs::write(&global, "*.pem\n").unwrap();
    let mut request: serde_json::Value =
        serde_json::from_str(&fixture.request(&fixture.repo, "")).unwrap();
    request["excludes"] = global.display().to_string().into();
    let output = fixture.invoke_raw(&request.to_string());
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let diff: ReviewDiff = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(paths(&diff), [("deploy.pem", FileStatus::Added)]);
}

#[test]
fn clean_tree_is_empty_and_oversized_patches_are_truncated() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one\n");
    fixture.commit("initial");
    let clean = fixture.review("").unwrap();
    assert!(clean.files.is_empty());
    assert!(clean.patch().is_empty());
    assert!(clean.limitations.is_empty());

    let line = format!("{}\n", "x".repeat(99));
    fixture.write("big.txt", &line.repeat(MAX_PATCH / 100 + 100));
    fixture.write("later.txt", "later\n");
    fixture.write("bin.dat", "\0\u{1}\u{2}binary");
    let diff = fixture.review("").unwrap();
    assert!(diff.patch().len() <= MAX_ENGINE_PATCH);
    let big = diff.files.iter().find(|f| f.path == "big.txt").unwrap();
    let big = big.patch.as_deref().unwrap();
    assert!(big.len() <= MAX_PATCH && big.ends_with('\n'));
    let binary = diff.files.iter().find(|f| f.path == "bin.dat").unwrap();
    assert!(binary.binary && binary.patch.is_none());
    // Each file is capped on its own; the host spends the shared budget.
    let later = diff.files.iter().find(|f| f.path == "later.txt").unwrap();
    assert_eq!(
        later.patch.as_deref().map(|p| p.contains("+later")),
        Some(true)
    );
    for name in ["bin.dat", "big.txt"] {
        assert!(
            diff.limitations.iter().any(|item| item.contains(name)),
            "{name}: {:?}",
            diff.limitations
        );
    }
}

#[test]
fn malformed_or_unknown_field_requests_are_refused() {
    let fixture = Fixture::new();
    let mut unknown: serde_json::Value =
        serde_json::from_str(&fixture.request(&fixture.repo, "")).unwrap();
    unknown["extra"] = true.into();
    for request in [
        "{}".to_owned(),
        "x".repeat(8193),
        unknown.to_string(),
        fixture.request(&fixture.repo, "--output=/tmp/x"),
        fixture.request(&fixture.repo, "HEAD@{1}"),
    ] {
        let output = fixture.invoke_raw(&request);
        assert_eq!(output.status.code(), Some(65), "{request}");
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
    let tampered = fixture.temp.path().join("tampered.config");
    std::fs::write(&tampered, "[core]\npager = less\n").unwrap();
    let request =
        serde_json::json!({"location": fixture.repo, "target": "", "overrides": tampered});
    let output = fixture.invoke_raw(&request.to_string());
    assert_eq!(output.status.code(), Some(65));
}

#[test]
fn merge_base_ranges_exclude_base_branch_commits_after_the_fork() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one\n");
    let fork = fixture.commit("root");
    git(&fixture.repo, &["checkout", "--quiet", "-b", "feature"]);
    fixture.write("feature.txt", "feature\n");
    let head = fixture.commit("feature");
    git(&fixture.repo, &["checkout", "--quiet", "main"]);
    fixture.write("main.txt", "later on main\n");
    let base = fixture.commit("main advances");

    let mut request: serde_json::Value =
        serde_json::from_str(&fixture.request(&fixture.repo, &format!("{base}..{head}"))).unwrap();
    request["merge_base"] = true.into();
    let output = fixture.invoke_raw(&request.to_string());
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let diff: ReviewDiff = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(diff.base.as_deref(), Some(fork.as_str()));
    assert_eq!(diff.head.as_deref(), Some(head.as_str()));
    assert_eq!(paths(&diff), [("feature.txt", FileStatus::Added)]);

    let direct = fixture.review(&format!("{base}..{head}")).unwrap();
    assert_eq!(direct.base.as_deref(), Some(base.as_str()));
    assert_eq!(
        paths(&direct),
        [
            ("feature.txt", FileStatus::Added),
            ("main.txt", FileStatus::Deleted)
        ],
        "a plain range diffs the two commits directly"
    );
}
