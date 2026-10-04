//! Shadow-git snapshots against real repositories.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use cyber_server::runtime::{RestoreError, Snapshots};
use cyber_snapshot::GitSnapshots;
use serde_json::{Value, json};

struct Repo {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    config: Arc<Mutex<Value>>,
    snaps: Arc<GitSnapshots>,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

impl Repo {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        git(&root, &["add", "."]);
        git(
            &root,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        );
        let config = Arc::new(Mutex::new(json!({})));
        let shared = Arc::clone(&config);
        let snaps = GitSnapshots::new(
            base.join("data"),
            Arc::new(move |_| shared.lock().unwrap().clone()),
        );
        Self {
            _tmp: tmp,
            root,
            data: base.join("data"),
            config,
            snaps,
        }
    }

    fn dir(&self) -> String {
        self.root.display().to_string()
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(rel)).ok()
    }

    async fn track(&self) -> String {
        self.snaps.track(&self.dir()).await.unwrap().unwrap().tree
    }
}

#[tokio::test]
async fn snapshots_capture_untracked_files_without_touching_the_user_repository() {
    let r = Repo::new();
    let status_before = git(&r.root, &["status", "--porcelain"]);
    let index_before = std::fs::read(r.root.join(".git/index")).unwrap();
    let first = r.track().await;
    r.write("new.txt", "hello\n");
    r.write("target/out.bin", "ignored\n");
    r.write("a.txt", "one\nTWO\nthree\nfour\nfive\n");
    let second = r.track().await;
    let mut changed = r.snaps.changed(&r.dir(), &first, &second).await.unwrap();
    changed.sort();
    assert_eq!(changed, vec!["a.txt", "new.txt"]);
    assert_eq!(
        std::fs::read(r.root.join(".git/index")).unwrap(),
        index_before
    );
    assert_eq!(
        git(&r.root, &["status", "--porcelain"]),
        format!("{status_before} M a.txt\n?? new.txt\n")
    );
    assert!(git(&r.root, &["stash", "list"]).is_empty());
}

#[tokio::test]
async fn large_untracked_files_are_skipped() {
    let r = Repo::new();
    *r.config.lock().unwrap() = json!({"snapshots": {"max_file_bytes": 10}});
    r.write("big.log", "this is longer than ten bytes\n");
    let snap = r.snaps.track(&r.dir()).await.unwrap().unwrap();
    assert_eq!(snap.skipped, vec!["big.log"]);
}

#[tokio::test]
async fn diff_reports_status_and_line_counts() {
    let r = Repo::new();
    let first = r.track().await;
    r.write("a.txt", "one\nTWO\nthree\nfour\nfive\nsix\n");
    r.write("b.txt", "new\n");
    let second = r.track().await;
    let diffs = r.snaps.diff(&r.dir(), &first, &second).await.unwrap();
    let a = diffs.iter().find(|d| d.file == "a.txt").unwrap();
    assert_eq!(
        (a.status.as_str(), a.additions, a.deletions),
        ("modified", 2, 1)
    );
    assert!(a.patch.contains("+TWO"));
    let b = diffs.iter().find(|d| d.file == "b.txt").unwrap();
    assert_eq!((b.status.as_str(), b.additions), ("added", 1));
}

#[tokio::test]
async fn restore_reverts_agent_edits_and_keeps_unrelated_user_edits() {
    let r = Repo::new();
    r.write("notes.md", "user notes\n");
    let before = r.track().await;
    r.write("a.txt", "one\ntwo\nthree\nfour\nFIVE\n");
    r.write("created.txt", "by agent\n");
    let after = r.track().await;
    // The user edits another line of the same file, and another file, after the agent.
    r.write("a.txt", "ONE\ntwo\nthree\nfour\nFIVE\n");
    r.write("notes.md", "user notes, edited\n");
    let mut restored = r.snaps.restore(&r.dir(), &before, &after).await.unwrap();
    restored.sort();
    assert_eq!(restored, vec!["a.txt", "created.txt"]);
    assert_eq!(r.read("a.txt").unwrap(), "ONE\ntwo\nthree\nfour\nfive\n");
    assert_eq!(r.read("created.txt"), None);
    assert_eq!(r.read("notes.md").unwrap(), "user notes, edited\n");
}

#[tokio::test]
async fn conflicting_edits_fail_without_writing_anything() {
    let r = Repo::new();
    let before = r.track().await;
    r.write("a.txt", "one\ntwo\nTHREE\nfour\nfive\n");
    r.write("b.txt", "agent\n");
    let after = r.track().await;
    r.write("a.txt", "one\ntwo\nthree?\nfour\nfive\n");
    let err = r
        .snaps
        .restore(&r.dir(), &before, &after)
        .await
        .unwrap_err();
    assert_eq!(err, RestoreError::Conflict(vec!["a.txt".into()]));
    assert_eq!(
        r.read("b.txt").unwrap(),
        "agent\n",
        "nothing is written when any path conflicts"
    );
    assert_eq!(r.read("a.txt").unwrap(), "one\ntwo\nthree?\nfour\nfive\n");
}

#[tokio::test]
async fn restore_brings_back_deleted_files_and_keeps_a_backup() {
    let r = Repo::new();
    let before = r.track().await;
    std::fs::remove_file(r.root.join("a.txt")).unwrap();
    r.write("x.txt", "temp\n");
    let after = r.track().await;
    r.snaps.restore(&r.dir(), &before, &after).await.unwrap();
    assert_eq!(r.read("a.txt").unwrap(), "one\ntwo\nthree\nfour\nfive\n");
    assert_eq!(r.read("x.txt"), None);
    let backups = r.data.join("snapshot/backups");
    let saved: Vec<_> = std::fs::read_dir(backups).unwrap().flatten().collect();
    assert_eq!(
        std::fs::read_to_string(saved[0].path().join("x.txt")).unwrap(),
        "temp\n"
    );
}

#[tokio::test]
async fn disabled_or_non_git_locations_take_no_snapshots() {
    let r = Repo::new();
    *r.config.lock().unwrap() = json!({"snapshots": false});
    assert_eq!(r.snaps.track(&r.dir()).await.unwrap(), None);
    let plain = tempfile::tempdir().unwrap();
    *r.config.lock().unwrap() = json!({});
    assert_eq!(
        r.snaps
            .track(&plain.path().display().to_string())
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn gc_runs_on_shadow_repositories() {
    let r = Repo::new();
    r.track().await;
    r.snaps.gc().await;
    assert!(r.data.join("snapshot").exists());
}
