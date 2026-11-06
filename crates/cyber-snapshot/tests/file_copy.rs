#![cfg(any(target_os = "macos", target_os = "linux"))]
//! Actual non-Git bytes, namespace boundaries, retained intents and three-way restore.
use cyber_server::runtime::{RestoreError, Snapshot, Snapshots};
use cyber_snapshot::GitSnapshots;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
struct Project {
    _temp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    config: Arc<Mutex<Value>>,
    snapshots: Arc<GitSnapshots>,
}
impl Project {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("work");
        let data = base.join("data");
        std::fs::create_dir(&root).unwrap();
        let config = Arc::new(Mutex::new(json!({})));
        let shared = config.clone();
        let snapshots = GitSnapshots::new(
            data.clone(),
            Arc::new(move |_| shared.lock().unwrap().clone()),
        );
        Self {
            _temp: temp,
            root,
            data,
            config,
            snapshots,
        }
    }
    fn dir(&self) -> String {
        self.root.display().to_string()
    }
    fn write(&self, path: &str, bytes: impl AsRef<[u8]>) {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    async fn track(&self) -> Snapshot {
        self.snapshots.track(&self.dir()).await.unwrap().unwrap()
    }
}

#[tokio::test]
async fn replaced_location_after_plan_never_supplies_a_new_restore_target() {
    refuses_replaced_location(false).await;
    refuses_replaced_location(true).await;
}

async fn refuses_replaced_location(symlink: bool) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let p = Project::new();
    p.write("a.txt", "before\n");
    let before = p.track().await;
    p.write("a.txt", "after\n");
    let after = p.track().await;
    let root = p.root.clone();
    let displaced = root.with_file_name("displaced-root");
    let moved = displaced.clone();
    let calls = AtomicUsize::new(0);
    let result = p
        .snapshots
        .restore_reviewed(
            &p.dir(),
            &before.tree,
            &after.tree,
            &after.tree,
            Arc::new(move || {
                if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                    std::fs::rename(&root, &moved).unwrap();
                    if symlink {
                        std::os::unix::fs::symlink(&moved, &root).unwrap();
                    } else {
                        std::fs::create_dir(&root).unwrap();
                        std::fs::write(root.join("a.txt"), "after\n").unwrap();
                    }
                }
                Ok(())
            }),
        )
        .await;
    assert!(
        matches!(result, Err(RestoreError::Failed(message)) if message.contains("Location identity changed"))
    );
    assert_eq!(std::fs::read(p.root.join("a.txt")).unwrap(), b"after\n");
    assert_eq!(std::fs::read(displaced.join("a.txt")).unwrap(), b"after\n");
}

#[tokio::test]
async fn non_git_snapshots_hash_bytes_preserve_owner_edits_and_restore_without_a_repository() {
    let p = Project::new();
    p.write("src/a.txt", "one\ntwo\nthree\nfour\nfive\nsix\n");
    let before = p.track().await;
    assert!(before.tree.starts_with("copy:"));
    p.write("src/a.txt", "ONE\ntwo\nthree\nfour\nfive\nsix\n");
    p.write("new.bin", [0, 255, 128]);
    let after = p.track().await;
    p.write("src/a.txt", "ONE\ntwo\nthree\nfour\nfive\nSIX\n");
    p.write("owner.txt", "keep\n");
    let current = p.track().await;
    let preview = p
        .snapshots
        .preview_restore(&p.dir(), &before.tree, &after.tree, &current.tree)
        .await
        .unwrap();
    assert_eq!(preview.len(), 2);
    assert!(!preview.iter().any(|f| f.file == "owner.txt"));
    assert!(
        std::fs::read_to_string(p.root.join("src/a.txt"))
            .unwrap()
            .starts_with("ONE")
    );
    let result = p
        .snapshots
        .restore_reviewed(
            &p.dir(),
            &before.tree,
            &after.tree,
            &current.tree,
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(
        std::fs::read_to_string(p.root.join("src/a.txt")).unwrap(),
        "one\ntwo\nthree\nfour\nfive\nSIX\n"
    );
    assert!(!p.root.join("new.bin").exists());
    assert_eq!(
        std::fs::read_to_string(p.root.join("owner.txt")).unwrap(),
        "keep\n"
    );
    assert!(!p.root.join(".git").exists());
    assert!(
        p.snapshots
            .recovery(&p.dir())
            .await
            .unwrap()
            .iter()
            .all(|j| j.filesystem_complete)
    );
}
#[tokio::test]
async fn same_size_same_timestamp_is_hashed_and_old_previews_refuse() {
    let p = Project::new();
    p.write("a.txt", "aaaa");
    let before = p.track().await;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(p.root.join("a.txt"))
        .unwrap();
    let timestamp = file.metadata().unwrap().modified().unwrap();
    p.write("a.txt", "bbbb");
    file.set_times(std::fs::FileTimes::new().set_modified(timestamp))
        .unwrap();
    let after = p.track().await;
    assert_ne!(before.tree, after.tree);
    assert_eq!(
        p.snapshots
            .changed(&p.dir(), &before.tree, &after.tree)
            .await
            .unwrap(),
        ["a.txt"]
    );
    p.write("a.txt", "cccc");
    assert!(matches!(
        p.snapshots
            .restore_reviewed(
                &p.dir(),
                &before.tree,
                &after.tree,
                &after.tree,
                Arc::new(|| Ok(()))
            )
            .await,
        Err(RestoreError::Stale)
    ));
    assert_eq!(std::fs::read(p.root.join("a.txt")).unwrap(), b"cccc");
}
#[tokio::test]
async fn ignores_size_private_roots_and_aliases_are_disclosed_and_never_captured() {
    use std::os::unix::fs::symlink;
    let p = Project::new();
    *p.config.lock().unwrap() =
        json!({"snapshots":{"ignore":["configured.txt"],"max_file_bytes":64}});
    p.write(".gitignore", "ignored/\n");
    p.write("ignored/file", "hidden");
    p.write("configured.txt", "hidden");
    p.write("large.bin", vec![1; 65]);
    p.write(".cyber/private", "secret");
    p.write("plain", "visible");
    symlink(p.root.join(".cyber"), p.root.join("alias")).unwrap();
    std::fs::hard_link(p.root.join("plain"), p.root.join("hard-link")).unwrap();
    let snapshot = p.track().await;
    for name in [
        "ignored",
        "configured.txt",
        "large.bin",
        ".cyber",
        "alias",
        "hard-link",
        "plain",
    ] {
        assert!(
            snapshot.skipped.iter().any(|s| s.ends_with(name)),
            "{name}: {:?}",
            snapshot.skipped
        );
    }
    let root = p.data.join("snapshot/global");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(root).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
#[tokio::test]
async fn storage_cap_and_foreign_location_do_not_authorize_restore() {
    let p = Project::new();
    p.write("a.txt", "before");
    let before = p.track().await;
    let other = Project::new();
    other.write("a.txt", "other");
    let current = other.track().await;
    assert!(
        other
            .snapshots
            .preview_restore(&other.dir(), &before.tree, &current.tree, &current.tree)
            .await
            .is_err()
    );
    *p.config.lock().unwrap() = json!({"snapshots":{"fallback_max_bytes":1}});
    p.write("b.txt", "cap exceeds");
    let err = p.snapshots.track(&p.dir()).await.unwrap_err();
    assert!(err.contains("cap reached"), "{err}");
    assert_eq!(std::fs::read(p.root.join("a.txt")).unwrap(), b"before");
}
#[tokio::test]
async fn admission_loss_after_a_replacement_retains_journal_and_blocks_replay() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let p = Project::new();
    p.write("a.txt", "old a");
    p.write("b.txt", "old b");
    let before = p.track().await;
    p.write("a.txt", "new a");
    p.write("b.txt", "new b");
    let after = p.track().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let result = p
        .snapshots
        .restore_reviewed(
            &p.dir(),
            &before.tree,
            &after.tree,
            &after.tree,
            Arc::new(move || {
                if count.fetch_add(1, Ordering::SeqCst) >= 2 {
                    Err("admission revoked".into())
                } else {
                    Ok(())
                }
            }),
        )
        .await;
    assert!(result.is_err());
    let journal = p.snapshots.recovery(&p.dir()).await.unwrap();
    assert_eq!(journal.len(), 1);
    assert!(!journal[0].filesystem_complete);
    assert_eq!(journal[0].applied, ["a.txt"]);
    let next = p.snapshots.track(&p.dir()).await.unwrap_err();
    assert!(next.contains("requires recovery"), "{next}");
    assert_eq!(std::fs::read(p.root.join("a.txt")).unwrap(), b"old a");
    assert_eq!(std::fs::read(p.root.join("b.txt")).unwrap(), b"new b");
}

fn namespace(p: &Project) -> PathBuf {
    std::fs::read_dir(p.data.join("snapshot/global"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
}
#[tokio::test]
async fn retention_prunes_orphans_but_preserves_durable_reference_roots() {
    use std::collections::BTreeSet;
    let mut p = Project::new();
    p.write("a.txt", "first");
    let first = p.track().await;
    p.write("a.txt", "second");
    let second = p.track().await;
    let manifests = namespace(&p).join("manifests");
    for file in std::fs::read_dir(&manifests).unwrap() {
        let path = file.unwrap().path();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["accessed"] = json!(0);
        std::fs::write(path, value.to_string()).unwrap();
    }
    let orphan = format!("tmp_{}", ulid::Ulid::new());
    for directory in [&manifests, &namespace(&p).join("files")] {
        let path = directory.join(&orphan);
        std::fs::write(&path, b"unpublished").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
            )
            .unwrap();
    }
    let protected = BTreeSet::from([first.tree.clone()]);
    let config = p.config.clone();
    p.snapshots = GitSnapshots::with_policy(
        p.data.clone(),
        Arc::new(move |_| config.lock().unwrap().clone()),
        vec![],
        Some(Arc::new(move || Ok(protected.clone()))),
    );
    p.snapshots.gc().await;
    assert!(!manifests.join(&orphan).exists());
    assert!(!namespace(&p).join("files").join(&orphan).exists());
    assert!(
        manifests
            .join(first.tree.strip_prefix("copy:").unwrap())
            .exists()
    );
    assert!(
        !manifests
            .join(second.tree.strip_prefix("copy:").unwrap())
            .exists()
    );
    assert!(
        p.snapshots
            .diff(&p.dir(), &first.tree, &first.tree)
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn configured_private_roots_and_parent_symlink_replacement_refuse_restore() {
    use std::os::unix::fs::symlink;
    let mut p = Project::new();
    let private = p.root.join("operator-state");
    std::fs::create_dir(&private).unwrap();
    p.write("operator-state/password", "secret");
    p.snapshots =
        GitSnapshots::with_policy(p.data.clone(), Arc::new(|_| json!({})), vec![private], None);
    p.write("nested/a.txt", "first");
    let first = p.track().await;
    assert!(first.skipped.iter().any(|s| s.ends_with("operator-state")));
    p.write("nested/a.txt", "second");
    let second = p.track().await;
    std::fs::rename(p.root.join("nested"), p.root.join("moved")).unwrap();
    symlink(p.root.join("moved"), p.root.join("nested")).unwrap();
    assert!(
        p.snapshots
            .restore_reviewed(
                &p.dir(),
                &first.tree,
                &second.tree,
                &second.tree,
                Arc::new(|| Ok(()))
            )
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(p.root.join("moved/a.txt")).unwrap(),
        b"second"
    );
}

/// Child-only worker. The parent kills it while admission waits between two
/// actual native replacements; there is no production crash switch.
#[test]
fn killed_restore_worker() {
    let Some(base) = std::env::var_os("CYBER_COPY_CRASH_TEST") else {
        return;
    };
    let base = PathBuf::from(base);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let snapshots = GitSnapshots::new(base.join("data"), Arc::new(|_| json!({})));
        let directory = base.join("work").display().to_string();
        let ids: Value =
            serde_json::from_slice(&std::fs::read(base.join("ids.json")).unwrap()).unwrap();
        let marker = base.join("restore-paused");
        let count = std::sync::atomic::AtomicUsize::new(0);
        let _ = snapshots
            .restore_reviewed(
                &directory,
                ids[0].as_str().unwrap(),
                ids[1].as_str().unwrap(),
                ids[1].as_str().unwrap(),
                Arc::new(move || {
                    if count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 2 {
                        std::fs::write(&marker, b"first native replacement acknowledged").unwrap();
                        loop {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                    }
                    Ok(())
                }),
            )
            .await;
    });
}
#[tokio::test]
async fn process_kill_between_replacements_retains_preimages_and_never_replays() {
    let p = Project::new();
    p.write("a.txt", "before a");
    p.write("b.txt", "before b");
    let before = p.track().await;
    p.write("a.txt", "after a");
    p.write("b.txt", "after b");
    let after = p.track().await;
    let base = p.root.parent().unwrap();
    std::fs::write(
        base.join("ids.json"),
        json!([before.tree, after.tree]).to_string(),
    )
    .unwrap();
    let mut worker = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "killed_restore_worker", "--nocapture"])
        .env("CYBER_COPY_CRASH_TEST", base)
        .env("PATH", "")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !base.join("restore-paused").exists() {
        if std::time::Instant::now() >= deadline || worker.try_wait().unwrap().is_some() {
            let _ = worker.kill();
            let _ = worker.wait();
            panic!("native worker did not reach the between-file cut");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    worker.kill().unwrap();
    worker.wait().unwrap();
    let reopened = GitSnapshots::new(p.data.clone(), Arc::new(|_| json!({})));
    let recovery = reopened.recovery(&p.dir()).await.unwrap();
    assert_eq!(recovery.len(), 1);
    assert!(!recovery[0].filesystem_complete);
    assert_eq!(recovery[0].applied, ["a.txt"]);
    assert!(
        reopened
            .track(&p.dir())
            .await
            .unwrap_err()
            .contains("requires recovery")
    );
    assert_eq!(std::fs::read(p.root.join("a.txt")).unwrap(), b"before a");
    assert_eq!(std::fs::read(p.root.join("b.txt")).unwrap(), b"after b");
    let journal = namespace(&p).join("journals").join(&recovery[0].id);
    assert_eq!(std::fs::read(journal.join("stage_0")).unwrap(), b"after a");
}

#[tokio::test]
async fn historical_exclusions_do_not_become_deletions_and_configured_ignores_take_priority() {
    let p = Project::new();
    p.write("a.txt", "abc");
    let before = p.track().await;
    *p.config.lock().unwrap() = json!({"snapshots":{"max_file_bytes":4,"ignore":["secret.txt"]}});
    p.write("a.txt", "oversize");
    p.write("secret.txt", "key");
    p.write(".gitignore", "!secret.txt\n");
    let after = p.track().await;
    assert!(after.skipped.iter().any(|p| p == "size: a.txt"));
    assert!(after.skipped.iter().any(|p| p == "ignore: secret.txt"));
    assert!(
        !p.snapshots
            .changed(&p.dir(), &before.tree, &after.tree)
            .await
            .unwrap()
            .contains(&"a.txt".into())
    );
    assert!(
        p.snapshots
            .exclusions(&p.dir(), vec![after.tree])
            .await
            .unwrap()
            .iter()
            .any(|p| p.ends_with("size: a.txt"))
    );
}
#[tokio::test]
async fn executable_mode_is_reviewed_and_restored() {
    use std::os::unix::fs::PermissionsExt;
    let p = Project::new();
    p.write("run.sh", "echo done\n");
    std::fs::set_permissions(
        p.root.join("run.sh"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let before = p.track().await;
    std::fs::set_permissions(
        p.root.join("run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let after = p.track().await;
    let preview = p
        .snapshots
        .preview_restore(&p.dir(), &before.tree, &after.tree, &after.tree)
        .await
        .unwrap();
    assert!(preview[0].patch.contains("old mode 0755"));
    assert!(preview[0].patch.contains("new mode 0644"));
    p.snapshots
        .restore_reviewed(
            &p.dir(),
            &before.tree,
            &after.tree,
            &after.tree,
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::metadata(p.root.join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_acl_grants_cannot_make_private_snapshot_storage_readable() {
    use std::os::unix::fs::PermissionsExt;
    let p = Project::new();
    p.write("a.txt", "secret bytes");
    let global = p.data.join("snapshot/global");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::set_permissions(&global, std::fs::Permissions::from_mode(0o700)).unwrap();
    let result = std::process::Command::new("chmod")
        .args(["+a", "everyone allow read,search"])
        .arg(&global)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let error = p.snapshots.track(&p.dir()).await.unwrap_err();
    assert!(error.contains("ACL grants"), "{error}");
    assert_eq!(std::fs::read_dir(global).unwrap().count(), 0);
}

#[tokio::test]
async fn recovery_inspection_does_not_wait_for_a_paused_native_writer() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct Release(Arc<AtomicBool>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let p = Project::new();
    p.write("a.txt", "old a");
    p.write("b.txt", "old b");
    let before = p.track().await;
    p.write("a.txt", "new a");
    p.write("b.txt", "new b");
    let after = p.track().await;
    let released = Arc::new(AtomicBool::new(false));
    let _release = Release(released.clone());
    let paused = Arc::new(tokio::sync::Notify::new());
    let signal = paused.clone();
    let snapshots = p.snapshots.clone();
    let directory = p.dir();
    let resume = released.clone();
    let task = tokio::spawn(async move {
        let count = AtomicUsize::new(0);
        snapshots
            .restore_reviewed(
                &directory,
                &before.tree,
                &after.tree,
                &after.tree,
                Arc::new(move || {
                    if count.fetch_add(1, Ordering::SeqCst) == 2 {
                        signal.notify_one();
                        while !resume.load(Ordering::SeqCst) {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                    Ok(())
                }),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), paused.notified())
        .await
        .unwrap();
    let view = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        p.snapshots.recovery(&p.dir()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(view.len(), 1);
    assert!(!view[0].filesystem_complete);
    assert_eq!(view[0].applied, ["a.txt"]);
    released.store(true, Ordering::SeqCst);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn deleted_parent_is_recreated_privately_with_original_file_ownership_and_mode() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let p = Project::new();
    p.write("private/a.txt", "private source");
    std::fs::set_permissions(
        p.root.join("private"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::set_permissions(
        p.root.join("private/a.txt"),
        std::fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let metadata = std::fs::metadata(p.root.join("private/a.txt")).unwrap();
    let before = p.track().await;
    std::fs::remove_dir_all(p.root.join("private")).unwrap();
    let after = p.track().await;
    p.snapshots
        .restore_reviewed(
            &p.dir(),
            &before.tree,
            &after.tree,
            &after.tree,
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap();
    let restored = std::fs::metadata(p.root.join("private/a.txt")).unwrap();
    assert_eq!(
        (restored.uid(), restored.gid(), restored.mode() & 0o777),
        (metadata.uid(), metadata.gid(), 0o640)
    );
    assert_eq!(
        std::fs::metadata(p.root.join("private")).unwrap().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::read(p.root.join("private/a.txt")).unwrap(),
        b"private source"
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn file_acl_is_excluded_instead_of_recreated_with_wider_access() {
    let p = Project::new();
    p.write("sensitive", "private source");
    let result = std::process::Command::new("chmod")
        .args(["+a", "everyone deny write"])
        .arg(p.root.join("sensitive"))
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let snapshot = p.track().await;
    assert!(snapshot.skipped.iter().any(|s| s == "metadata: sensitive"));
    assert_eq!(
        std::fs::read_dir(namespace(&p).join("files"))
            .unwrap()
            .count(),
        0
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn extended_metadata_is_excluded_instead_of_dropped_on_recreation() {
    let p = Project::new();
    p.write("sensitive", "private source");
    let file = std::fs::File::open(p.root.join("sensitive")).unwrap();
    rustix::fs::fsetxattr(
        &file,
        "user.cyber-snapshot-test",
        b"metadata",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    let snapshot = p.track().await;
    assert!(snapshot.skipped.iter().any(|s| s == "metadata: sensitive"));
    assert_eq!(
        std::fs::read_dir(namespace(&p).join("files"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn conflicting_owner_edit_refuses_the_entire_restore_before_replacement() {
    let p = Project::new();
    p.write("a.txt", "old a\n");
    p.write("b.txt", "base\n");
    let before = p.track().await;
    p.write("a.txt", "new a\n");
    p.write("b.txt", "agent\n");
    let after = p.track().await;
    p.write("b.txt", "owner\n");
    let current = p.track().await;
    let result = p
        .snapshots
        .restore_reviewed(
            &p.dir(),
            &before.tree,
            &after.tree,
            &current.tree,
            Arc::new(|| Ok(())),
        )
        .await;
    assert!(matches!(result, Err(RestoreError::Conflict(paths)) if paths == ["b.txt"]));
    assert_eq!(std::fs::read(p.root.join("a.txt")).unwrap(), b"new a\n");
    assert_eq!(std::fs::read(p.root.join("b.txt")).unwrap(), b"owner\n");
    assert!(p.snapshots.recovery(&p.dir()).await.unwrap().is_empty());
}

#[tokio::test]
async fn restore_reserves_displaced_preimages_before_changing_source_files() {
    let p = Project::new();
    p.write("a.txt", "old a\n");
    let before = p.track().await;
    p.write("a.txt", "new a\n");
    let after = p.track().await;
    let mut used = 0u64;
    for name in ["files", "manifests"] {
        for entry in std::fs::read_dir(namespace(&p).join(name)).unwrap() {
            used += entry.unwrap().metadata().unwrap().len();
        }
    }
    *p.config.lock().unwrap() = json!({"snapshots":{"fallback_max_bytes":used}});
    let result = p
        .snapshots
        .restore_reviewed(
            &p.dir(),
            &before.tree,
            &after.tree,
            &after.tree,
            Arc::new(|| Ok(())),
        )
        .await;
    assert!(
        matches!(result, Err(RestoreError::Failed(message)) if message.contains("cap reached"))
    );
    assert_eq!(std::fs::read(p.root.join("a.txt")).unwrap(), b"new a\n");
    assert!(p.snapshots.recovery(&p.dir()).await.unwrap().is_empty());
}
