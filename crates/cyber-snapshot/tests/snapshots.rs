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
    fn shadow_directory(&self) -> PathBuf {
        std::fs::read_dir(self.data.join("snapshot"))
            .unwrap()
            .flat_map(|entry| std::fs::read_dir(entry.unwrap().path()).unwrap())
            .map(|entry| entry.unwrap().path())
            .find(|path| path.join("HEAD").exists())
            .unwrap()
    }
    fn shadow_object(&self, revision: &str) -> PathBuf {
        let shadow = self.shadow_directory();
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(&shadow)
            .args(["rev-parse", revision])
            .output()
            .unwrap();
        assert!(output.status.success());
        let id = String::from_utf8(output.stdout).unwrap();
        let id = id.trim();
        shadow.join("objects").join(&id[..2]).join(&id[2..])
    }
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
async fn disabled_locations_take_no_snapshots() {
    let r = Repo::new();
    *r.config.lock().unwrap() = json!({"snapshots": false});
    assert_eq!(r.snaps.track(&r.dir()).await.unwrap(), None);
    let plain = tempfile::tempdir().unwrap();
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

#[tokio::test]
async fn snapshot_restore_waits_for_the_shared_workspace_mutation_owner() {
    use std::time::Duration;
    let r = Repo::new();
    let before = r.track().await;
    let original = std::fs::read(r.root.join("a.txt")).unwrap();
    std::fs::write(r.root.join("a.txt"), "changed by an editor\n").unwrap();
    let after = r.track().await;
    let lock = cyber_server::workspace::mutation_lock(&r.root).unwrap();
    let held = lock.lock_owned().await;
    let directory = r.dir();
    let restore = r.snaps.restore(&directory, &before, &after);
    tokio::pin!(restore);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut restore)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(r.root.join("a.txt")).unwrap(),
        b"changed by an editor\n"
    );
    drop(held);
    tokio::time::timeout(Duration::from_secs(5), restore)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read(r.root.join("a.txt")).unwrap(), original);
}

#[tokio::test]
async fn reviewed_restore_rechecks_after_waiting_for_workspace_ownership() {
    let r = Repo::new();
    let target = r.track().await;
    r.write("a.txt", "recorded\n");
    let recorded = r.track().await;
    let lock = cyber_server::workspace::mutation_lock(&r.root).unwrap();
    let guard = lock.lock().await;
    let snaps = r.snaps.clone();
    let directory = r.dir();
    let expected = recorded.clone();
    let waiting = tokio::spawn(async move {
        snaps
            .restore_reviewed(
                &directory,
                &target,
                &recorded,
                &expected,
                Arc::new(|| Ok(())),
            )
            .await
    });
    tokio::task::yield_now().await;
    r.write("a.txt", "owner edit while waiting\n");
    drop(guard);
    assert_eq!(waiting.await.unwrap(), Err(RestoreError::Stale));
    assert_eq!(
        r.read("a.txt").as_deref(),
        Some("owner edit while waiting\n")
    );
}

#[tokio::test]
async fn reviewed_restore_refreshes_authority_and_has_a_positive_control() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let r = Repo::new();
    let original = r.read("a.txt");
    let target = r.track().await;
    r.write("a.txt", "recorded\n");
    let recorded = r.track().await;
    let lock = cyber_server::workspace::mutation_lock(&r.root).unwrap();
    let guard = lock.lock().await;
    let allowed = Arc::new(AtomicBool::new(true));
    let permission = allowed.clone();
    let authorize = Arc::new(move || {
        if permission.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("owner admission revoked".into())
        }
    });
    let snaps = r.snaps.clone();
    let directory = r.dir();
    let (wanted, last) = (target.clone(), recorded.clone());
    let waiting = tokio::spawn(async move {
        snaps
            .restore_reviewed(&directory, &wanted, &last, &last, authorize)
            .await
    });
    tokio::task::yield_now().await;
    allowed.store(false, Ordering::SeqCst);
    drop(guard);
    assert!(
        matches!(waiting.await.unwrap(), Err(RestoreError::Failed(message)) if message.contains("revoked"))
    );
    assert_eq!(r.read("a.txt").as_deref(), Some("recorded\n"));
    let restored = r
        .snaps
        .restore_reviewed(&r.dir(), &target, &recorded, &recorded, Arc::new(|| Ok(())))
        .await
        .unwrap();
    assert_eq!(restored, ["a.txt"]);
    assert_eq!(r.read("a.txt"), original);
}

#[tokio::test]
async fn reviewed_restore_rechecks_admission_before_each_file_and_keeps_backups() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let r = Repo::new();
    let original = r.read("a.txt");
    let target = r.track().await;
    r.write("a.txt", "recorded a\n");
    r.write("b.txt", "recorded b\n");
    let recorded = r.track().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = calls.clone();
    let authorize = Arc::new(move || {
        if gate.fetch_add(1, Ordering::SeqCst) < 3 {
            Ok(())
        } else {
            Err("revoked before the second file".into())
        }
    });
    let result = r
        .snaps
        .restore_reviewed(&r.dir(), &target, &recorded, &recorded, authorize)
        .await;
    assert!(
        matches!(result, Err(RestoreError::Failed(message)) if message.contains("second file"))
    );
    assert_eq!(r.read("a.txt"), original);
    assert_eq!(r.read("b.txt").as_deref(), Some("recorded b\n"));
    let backups = std::fs::read_dir(r.data.join("snapshot/backups")).unwrap();
    let saved = backups
        .map(|entry| entry.unwrap().path().join("a.txt"))
        .find_map(|path| std::fs::read_to_string(path).ok());
    assert_eq!(saved.as_deref(), Some("recorded a\n"));
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn exclusion_history_changes_identity_even_when_git_bytes_are_identical() {
    let r = Repo::new();
    r.write(".gitignore", "*.secret\n");
    let first = r.track().await;
    r.write("private.secret", "never captured");
    let second = r.track().await;
    assert_ne!(
        first, second,
        "identical Git trees can have different exclusions"
    );
    assert!(
        r.snaps
            .changed(&r.dir(), &first, &second)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        r.snaps
            .diff(&r.dir(), &first, &second)
            .await
            .unwrap()
            .is_empty()
    );
    std::fs::remove_file(r.root.join("private.secret")).unwrap();
    let reopened = GitSnapshots::new(r.data.clone(), Arc::new(|_| json!({})));
    assert!(
        reopened
            .exclusions(&r.dir(), vec![first])
            .await
            .unwrap()
            .is_empty()
    );
    let evidence = reopened.exclusions(&r.dir(), vec![second]).await.unwrap();
    assert!(
        evidence.iter().any(|p| p.ends_with(": private.secret")),
        "{evidence:?}"
    );
}

#[tokio::test]
async fn newly_excluded_git_file_never_becomes_a_deletion_or_restore_target() {
    let r = Repo::new();
    let before = r.track().await;
    r.write("a.txt", "agent edit\n");
    let after = r.track().await;
    *r.config.lock().unwrap() = json!({"snapshots":{"ignore":["a.txt"]}});
    let current = r.snaps.track(&r.dir()).await.unwrap().unwrap();
    assert!(current.skipped.iter().any(|p| p == "a.txt"));
    assert!(
        r.snaps
            .diff(&r.dir(), &after, &current.tree)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        r.snaps
            .preview_restore(&r.dir(), &before, &after, &current.tree)
            .await
            .unwrap()
            .is_empty()
    );
    r.snaps
        .restore_reviewed(
            &r.dir(),
            &before,
            &after,
            &current.tree,
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap();
    assert_eq!(r.read("a.txt").as_deref(), Some("agent edit\n"));
}

#[tokio::test]
async fn legacy_git_trees_remain_readable_with_explicit_unknown_exclusions() {
    let r = Repo::new();
    let legacy = git(&r.root, &["rev-parse", "HEAD^{tree}"])
        .trim()
        .to_owned();
    let current = r.track().await;
    assert!(
        r.snaps
            .diff(&r.dir(), &legacy, &current)
            .await
            .unwrap()
            .is_empty()
    );
    let evidence = r.snaps.exclusions(&r.dir(), vec![legacy]).await.unwrap();
    assert!(evidence[0].contains("legacy snapshot"));
}

#[tokio::test]
async fn exclusion_records_are_bound_to_the_exact_git_location() {
    let r = Repo::new();
    r.write("sub/x.txt", "local file\n");
    let root = r.track().await;
    let sub = r.root.join("sub").display().to_string();
    let local = r.snaps.track(&sub).await.unwrap().unwrap();
    assert!(
        r.snaps
            .diff(&sub, &local.tree, &local.tree)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(r.snaps.diff(&sub, &root, &local.tree).await.is_err());
    assert!(r.snaps.exclusions(&sub, vec![root]).await.is_err());
    assert_eq!(r.read("sub/x.txt").as_deref(), Some("local file\n"));
}

#[tokio::test]
async fn ignored_build_directories_use_one_bounded_exclusion_prefix() {
    let r = Repo::new();
    std::fs::create_dir(r.root.join("target")).unwrap();
    for index in 0..4100 {
        std::fs::write(r.root.join(format!("target/artifact-{index}")), b"").unwrap();
    }
    let snapshot = r.snaps.track(&r.dir()).await.unwrap().unwrap();
    assert_eq!(snapshot.skipped, ["target/"]);
    let evidence = r
        .snaps
        .exclusions(&r.dir(), vec![snapshot.tree])
        .await
        .unwrap();
    assert_eq!(evidence.len(), 1);
    assert!(evidence[0].ends_with(": target/"));
}

#[cfg(unix)]
#[tokio::test]
async fn git_restore_materializes_literal_unusual_paths_without_touching_the_user_index() {
    let r = Repo::new();
    let names = ["[ab].txt", "a.txt", "b.txt", ":(glob)*", "tab\tline\nfile"];
    for (index, name) in names.iter().enumerate() {
        std::fs::write(r.root.join(name), format!("before {index}\n")).unwrap();
    }
    let index = std::fs::read(r.root.join(".git/index")).unwrap();
    let target = r.snaps.track(&r.dir()).await.unwrap().unwrap();
    for (item, name) in names.iter().enumerate() {
        std::fs::write(r.root.join(name), format!("after {item}\n")).unwrap();
    }
    let recorded = r.snaps.track(&r.dir()).await.unwrap().unwrap();
    r.snaps
        .restore_reviewed(
            &r.dir(),
            &target.tree,
            &recorded.tree,
            &recorded.tree,
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap();
    for (item, name) in names.iter().enumerate() {
        assert_eq!(
            std::fs::read_to_string(r.root.join(name)).unwrap(),
            format!("before {item}\n")
        );
    }
    assert_eq!(std::fs::read(r.root.join(".git/index")).unwrap(), index);
}

#[cfg(unix)]
#[tokio::test]
async fn corrupt_git_snapshot_blob_refuses_restore_and_preserves_current_source() {
    use std::os::unix::fs::PermissionsExt;
    let r = Repo::new();
    std::fs::write(r.root.join("a.txt"), "unique original snapshot image").unwrap();
    let target = r.snaps.track(&r.dir()).await.unwrap().unwrap();
    std::fs::write(r.root.join("a.txt"), "replacement snapshot image").unwrap();
    let recorded = r.snaps.track(&r.dir()).await.unwrap().unwrap();
    let project = std::fs::read_dir(r.data.join("snapshot"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let shadow = std::fs::read_dir(project)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.join("HEAD").exists())
        .unwrap();
    let object = |snapshot: &str| {
        let out = Command::new("git")
            .arg("--git-dir")
            .arg(&shadow)
            .args(["rev-parse", &format!("{snapshot}:a.txt")])
            .output()
            .unwrap();
        assert!(out.status.success());
        let id = String::from_utf8(out.stdout).unwrap();
        let id = id.trim();
        shadow.join("objects").join(&id[..2]).join(&id[2..])
    };
    let original = object(&target.tree);
    let replacement = std::fs::read(object(&recorded.tree)).unwrap();
    std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&original, replacement).unwrap();
    let index = std::fs::read(r.root.join(".git/index")).unwrap();
    assert!(
        r.snaps
            .restore_reviewed(
                &r.dir(),
                &target.tree,
                &recorded.tree,
                &recorded.tree,
                Arc::new(|| Ok(()))
            )
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(r.root.join("a.txt")).unwrap(),
        "replacement snapshot image"
    );
    assert_eq!(std::fs::read(r.root.join(".git/index")).unwrap(), index);
}

#[cfg(unix)]
#[tokio::test]
async fn corrupt_git_tag_root_and_nested_tree_cannot_turn_restore_into_a_silent_noop() {
    use std::os::unix::fs::PermissionsExt;
    for suffix in ["", "^{tree}", ":nested"] {
        let r = Repo::new();
        std::fs::create_dir(r.root.join("nested")).unwrap();
        std::fs::write(r.root.join("nested/file"), "before graph image").unwrap();
        let target = r.snaps.track(&r.dir()).await.unwrap().unwrap();
        std::fs::write(r.root.join("nested/file"), "after graph image").unwrap();
        let recorded = r.snaps.track(&r.dir()).await.unwrap().unwrap();
        let original = r.shadow_object(&format!("{}{suffix}", target.tree));
        let replacement =
            std::fs::read(r.shadow_object(&format!("{}{suffix}", recorded.tree))).unwrap();
        std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(original, replacement).unwrap();
        let index = std::fs::read(r.root.join(".git/index")).unwrap();
        assert!(
            r.snaps
                .restore_reviewed(
                    &r.dir(),
                    &target.tree,
                    &recorded.tree,
                    &recorded.tree,
                    Arc::new(|| Ok(()))
                )
                .await
                .is_err(),
            "{suffix}"
        );
        assert_eq!(
            std::fs::read_to_string(r.root.join("nested/file")).unwrap(),
            "after graph image"
        );
        assert_eq!(std::fs::read(r.root.join(".git/index")).unwrap(), index);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn git_location_paths_are_literal_and_preserve_backslashes_without_leaking_sibling_files() {
    let r = Repo::new();
    for scope in ["[ab]", "back\\slash", "a", "b"] {
        std::fs::create_dir(r.root.join(scope)).unwrap();
        std::fs::write(r.root.join(scope).join("file"), "before scoped image").unwrap();
    }
    let index = std::fs::read(r.root.join(".git/index")).unwrap();
    for scope in ["[ab]", "back\\slash"] {
        let location = r.root.join(scope).to_str().unwrap().to_owned();
        let target = r.snaps.track(&location).await.unwrap().unwrap();
        std::fs::write(r.root.join(scope).join("file"), "after scoped image").unwrap();
        std::fs::write(r.root.join("a/file"), "later sibling owner").unwrap();
        std::fs::write(r.root.join("b/file"), "later sibling owner").unwrap();
        let recorded = r.snaps.track(&location).await.unwrap().unwrap();
        assert_eq!(
            r.snaps
                .changed(&location, &target.tree, &recorded.tree)
                .await
                .unwrap(),
            [format!("{scope}/file")]
        );
        r.snaps
            .restore_reviewed(
                &location,
                &target.tree,
                &recorded.tree,
                &recorded.tree,
                Arc::new(|| Ok(())),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(r.root.join(scope).join("file")).unwrap(),
            "before scoped image"
        );
        assert_eq!(
            std::fs::read_to_string(r.root.join("a/file")).unwrap(),
            "later sibling owner"
        );
        assert_eq!(
            std::fs::read_to_string(r.root.join("b/file")).unwrap(),
            "later sibling owner"
        );
    }
    assert_eq!(std::fs::read(r.root.join(".git/index")).unwrap(), index);
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess fixture invoked by pathspec environment test"]
fn snapshot_pathspec_environment_child() {
    git_location_paths_are_literal_and_preserve_backslashes_without_leaking_sibling_files();
}

#[cfg(unix)]
#[test]
fn snapshot_capture_does_not_inherit_glob_pathspec_environment() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "snapshot_pathspec_environment_child",
            "--ignored",
        ])
        .env("GIT_GLOB_PATHSPECS", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
