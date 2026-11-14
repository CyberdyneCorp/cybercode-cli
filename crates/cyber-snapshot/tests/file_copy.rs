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
    let p = Project::new();
    p.write("a.txt", "old a");
    p.write("b.txt", "old b");
    let before = p.track().await;
    p.write("a.txt", "new a");
    p.write("b.txt", "new b");
    let after = p.track().await;
    let first = p.root.join("a.txt");
    let result = p
        .snapshots
        .restore_reviewed(
            &p.dir(),
            &before.tree,
            &after.tree,
            &after.tree,
            Arc::new(move || {
                if std::fs::read(&first).ok().as_deref() == Some(b"old a") {
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
        let first = base.join("work/a.txt");
        let _ = snapshots
            .restore_reviewed(
                &directory,
                ids[0].as_str().unwrap(),
                ids[1].as_str().unwrap(),
                ids[1].as_str().unwrap(),
                Arc::new(move || {
                    if std::fs::read(&first).ok().as_deref() == Some(b"before a") {
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
    use std::sync::atomic::{AtomicBool, Ordering};
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
    let first = p.root.join("a.txt");
    let task = tokio::spawn(async move {
        let signaled = AtomicBool::new(false);
        snapshots
            .restore_reviewed(
                &directory,
                &before.tree,
                &after.tree,
                &after.tree,
                Arc::new(move || {
                    if std::fs::read(&first).ok().as_deref() == Some(b"old a")
                        && !signaled.swap(true, Ordering::SeqCst)
                    {
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

/// Age both published roots and private orphan/temporary bytes without changing
/// their content identities; collection receives a complete empty reference set.
async fn aged_collection_fixture() -> (Project, Vec<(PathBuf, Vec<u8>)>) {
    let mut p = Project::new();
    for text in ["first", "second"] {
        p.write("a.txt", text);
        p.track().await;
    }
    let namespace = namespace(&p);
    let manifests = namespace.join("manifests");
    for entry in std::fs::read_dir(&manifests).unwrap() {
        let path = entry.unwrap().path();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["accessed"] = json!(0);
        std::fs::write(path, value.to_string()).unwrap();
    }
    let orphan = b"unpublished object";
    std::fs::write(
        namespace
            .join("files")
            .join(blake3::hash(orphan).to_hex().as_str()),
        orphan,
    )
    .unwrap();
    let mut retained = vec![];
    for directory in [manifests, namespace.join("files")] {
        std::fs::write(
            directory.join(format!("tmp_{}", ulid::Ulid::new())),
            b"unpublished temporary",
        )
        .unwrap();
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
                )
                .unwrap();
            retained.push((path.clone(), std::fs::read(path).unwrap()));
        }
    }
    let config = p.config.clone();
    p.snapshots = GitSnapshots::with_policy(
        p.data.clone(),
        Arc::new(move |_| config.lock().unwrap().clone()),
        vec![],
        Some(Arc::new(|| Ok(Default::default()))),
    );
    (p, retained)
}
fn assert_collection_retained(retained: &[(PathBuf, Vec<u8>)]) {
    for (path, bytes) in retained {
        assert_eq!(
            std::fs::read(path).ok().as_ref(),
            Some(bytes),
            "{}",
            path.display()
        );
    }
}
#[tokio::test]
async fn collection_disabled_snapshots_preserves_manifests_objects_and_temporary_bytes() {
    let (p, retained) = aged_collection_fixture().await;
    *p.config.lock().unwrap() = json!({"snapshots":false});
    p.snapshots.gc().await;
    assert_collection_retained(&retained);
}
#[tokio::test]
async fn collection_zero_retention_preserves_orphan_and_temporary_bytes() {
    let (p, retained) = aged_collection_fixture().await;
    *p.config.lock().unwrap() = json!({"snapshots":{"retention_days":0}});
    p.snapshots.gc().await;
    assert_collection_retained(&retained);
}
#[tokio::test]
async fn collection_unavailable_configuration_preserves_the_complete_inventory() {
    let (p, retained) = aged_collection_fixture().await;
    *p.config.lock().unwrap() = Value::Null;
    p.snapshots.gc().await;
    assert_collection_retained(&retained);
    assert!(!namespace(&p).join("collection.json").exists());
}
#[tokio::test]
async fn collection_missing_location_evidence_preserves_orphans() {
    let (p, mut retained) = aged_collection_fixture().await;
    retained.retain(|(path, _)| {
        if path.parent().unwrap().ends_with("manifests")
            && !path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("tmp_")
        {
            std::fs::remove_file(path).unwrap();
            false
        } else {
            true
        }
    });
    p.snapshots.gc().await;
    assert_collection_retained(&retained);
}
#[tokio::test]
async fn collection_corrupt_inventory_refuses_before_any_manifest_is_removed() {
    let (p, retained) = aged_collection_fixture().await;
    std::fs::write(
        namespace(&p).join("manifests").join("f".repeat(64)),
        b"malformed manifest",
    )
    .unwrap();
    p.snapshots.gc().await;
    assert_collection_retained(&retained);
}
#[tokio::test]
async fn collection_policy_revocation_before_unlink_preserves_the_inventory() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (mut p, retained) = aged_collection_fixture().await;
    let calls = AtomicUsize::new(0);
    p.snapshots = GitSnapshots::with_policy(
        p.data.clone(),
        Arc::new(move |_| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                json!({})
            } else {
                json!({"snapshots":false})
            }
        }),
        vec![],
        Some(Arc::new(|| Ok(Default::default()))),
    );
    p.snapshots.gc().await;
    assert_collection_retained(&retained);
}

#[tokio::test]
async fn collection_orphan_age_uses_the_configured_retention_instead_of_seven_days() {
    let (p, _) = aged_collection_fixture().await;
    *p.config.lock().unwrap() = json!({"snapshots":{"retention_days":30}});
    let orphan = namespace(&p)
        .join("files")
        .join(blake3::hash(b"unpublished object").to_hex().as_str());
    std::fs::OpenOptions::new()
        .write(true)
        .open(&orphan)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(
            std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 86400),
        ))
        .unwrap();
    p.snapshots.gc().await;
    assert_eq!(std::fs::read(orphan).unwrap(), b"unpublished object");
}

#[tokio::test]
async fn collection_foreign_location_manifest_does_not_select_another_policy() {
    let (p, retained) = aged_collection_fixture().await;
    let foreign = Project::new();
    foreign.write("foreign.txt", "another Location");
    let snapshot = foreign.track().await;
    let name = snapshot.tree.strip_prefix("copy:").unwrap();
    std::fs::copy(
        namespace(&foreign).join("manifests").join(name),
        namespace(&p).join("manifests").join(name),
    )
    .unwrap();
    p.snapshots.gc().await;
    assert_collection_retained(&retained);
}

#[tokio::test]
async fn collection_policy_revocation_after_manifest_deletion_preserves_objects() {
    let (mut p, retained) = aged_collection_fixture().await;
    let manifests = namespace(&p).join("manifests");
    for entry in std::fs::read_dir(&manifests).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("tmp_")
        {
            std::fs::remove_file(path).unwrap();
        }
    }
    p.snapshots = GitSnapshots::with_policy(
        p.data.clone(),
        Arc::new(move |_| {
            if std::fs::read_dir(&manifests).unwrap().next().is_none() {
                json!({"snapshots":false})
            } else {
                json!({})
            }
        }),
        vec![],
        Some(Arc::new(|| Ok(Default::default()))),
    );
    p.snapshots.gc().await;
    let objects: Vec<_> = retained
        .into_iter()
        .filter(|(path, _)| path.parent().unwrap().ends_with("files"))
        .collect();
    assert_collection_retained(&objects);
    // Simulate a fresh collector after the stop between manifest and object
    // removal. It must use durable Location evidence rather than orphan names.
    p.snapshots = GitSnapshots::with_policy(
        p.data.clone(),
        Arc::new(|_| json!({})),
        vec![],
        Some(Arc::new(|| Ok(Default::default()))),
    );
    p.snapshots.gc().await;
    for (path, _) in objects {
        assert!(
            !path.exists(),
            "old orphan survived restart: {}",
            path.display()
        );
    }
}

#[tokio::test]
async fn collection_temporary_alias_refuses_before_any_manifest_is_removed() {
    let (p, retained) = aged_collection_fixture().await;
    let target = p.root.join("owner-evidence");
    std::fs::write(&target, b"owner bytes").unwrap();
    let alias = namespace(&p)
        .join("manifests")
        .join(format!("tmp_{}", ulid::Ulid::new()));
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    p.snapshots.gc().await;
    assert_collection_retained(&retained);
    assert_eq!(std::fs::read(target).unwrap(), b"owner bytes");
    assert!(
        std::fs::symlink_metadata(alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
#[tokio::test]
async fn collection_unknown_object_refuses_before_any_manifest_is_removed() {
    let (p, retained) = aged_collection_fixture().await;
    let unknown = namespace(&p).join("files").join("foreign-owner-evidence");
    std::fs::write(&unknown, b"unknown owner").unwrap();
    p.snapshots.gc().await;
    assert_collection_retained(&retained);
    assert_eq!(std::fs::read(unknown).unwrap(), b"unknown owner");
}

#[tokio::test]
async fn collection_dangling_reference_preserves_orphans_despite_valid_provenance() {
    let (mut p, retained) = aged_collection_fixture().await;
    let manifest = retained
        .iter()
        .find(|(path, _)| {
            path.parent().unwrap().ends_with("manifests")
                && !path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("tmp_")
        })
        .unwrap()
        .0
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    p.snapshots.gc().await;
    let objects = restore_old_orphans(&retained);
    let provenance = std::fs::read(namespace(&p).join("collection.json")).unwrap();
    p.snapshots = GitSnapshots::with_policy(
        p.data.clone(),
        Arc::new(|_| json!({})),
        vec![],
        Some(Arc::new(move || {
            Ok(std::collections::BTreeSet::from([format!(
                "copy:{manifest}"
            )]))
        })),
    );
    p.snapshots.gc().await;
    assert_collection_retained(&objects);
    assert_eq!(
        std::fs::read(namespace(&p).join("collection.json")).unwrap(),
        provenance
    );
}

#[tokio::test]
async fn collection_resolves_other_location_roots_before_reclaiming_old_orphans() {
    let (mut p, retained) = aged_collection_fixture().await;
    let original_namespace = namespace(&p);
    p.snapshots.gc().await;
    let objects = restore_old_orphans(&retained);
    let other = p.root.parent().unwrap().join("other-work");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("owner"), b"other Location bytes").unwrap();
    let protected = p
        .snapshots
        .track(other.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    let root = protected.tree.clone();
    p.snapshots = GitSnapshots::with_policy(
        p.data.clone(),
        Arc::new(|_| json!({})),
        vec![],
        Some(Arc::new(move || {
            Ok(std::collections::BTreeSet::from([
                root.clone(),
                "a".repeat(40),
            ]))
        })),
    );
    p.snapshots.gc().await;
    for (path, _) in objects {
        assert!(!path.exists(), "{}", path.display());
    }
    assert!(
        std::fs::read_dir(original_namespace.join("files"))
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(
        std::fs::read(other.join("owner")).unwrap(),
        b"other Location bytes"
    );
    assert!(
        p.snapshots
            .changed(other.to_str().unwrap(), &protected.tree, &protected.tree)
            .await
            .unwrap()
            .is_empty()
    );
}

fn restore_old_orphans(retained: &[(PathBuf, Vec<u8>)]) -> Vec<(PathBuf, Vec<u8>)> {
    retained
        .iter()
        .filter(|(path, _)| path.parent().unwrap().ends_with("files"))
        .map(|(path, bytes)| {
            std::fs::write(path, bytes).unwrap();
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
                .unwrap();
            (path.clone(), bytes.clone())
        })
        .collect()
}

fn mode(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}

#[tokio::test]
async fn rename_is_restored_as_delete_plus_create_and_preserves_unrelated_owner_edits() {
    for edited in [false, true] {
        renamed_restore(edited).await;
    }
}

/// A non-Git rename has no move record: undo removes the new name only when its
/// bytes still match the captured agent result and recreates the old name.
async fn renamed_restore(edited: bool) {
    use std::os::unix::fs::PermissionsExt;
    let p = Project::new();
    p.write("a.txt", "renamed bytes\n");
    p.write("c.txt", "owner base\n");
    std::fs::set_permissions(p.root.join("a.txt"), PermissionsExt::from_mode(0o640)).unwrap();
    let before = p.track().await;
    std::fs::rename(p.root.join("a.txt"), p.root.join("b.txt")).unwrap();
    let after = p.track().await;
    p.write("c.txt", "owner edit\n");
    if edited {
        p.write("b.txt", "owner edit after rename\n");
    }
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
    assert_eq!(
        std::fs::read(p.root.join("c.txt")).unwrap(),
        b"owner edit\n"
    );
    if edited {
        return assert_edited_rename_refused(&p, result).await;
    }
    assert_eq!(result.unwrap(), ["a.txt", "b.txt"]);
    assert!(!p.root.join("b.txt").exists());
    assert_eq!(
        std::fs::read(p.root.join("a.txt")).unwrap(),
        b"renamed bytes\n"
    );
    assert_eq!(mode(&p.root.join("a.txt")), 0o640);
}

/// An owner edit to the new name is not the captured agent result: the whole
/// restore refuses, so neither name changes and no journal is left.
async fn assert_edited_rename_refused(p: &Project, result: Result<Vec<String>, RestoreError>) {
    assert!(matches!(result, Err(RestoreError::Conflict(paths)) if paths == ["b.txt"]));
    assert!(!p.root.join("a.txt").exists());
    assert_eq!(
        std::fs::read(p.root.join("b.txt")).unwrap(),
        b"owner edit after rename\n"
    );
    assert!(p.snapshots.recovery(&p.dir()).await.unwrap().is_empty());
}

fn mkfifo(path: &std::path::Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

fn inode(path: &std::path::Path) -> (u64, std::fs::FileType) {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).unwrap();
    (metadata.ino(), metadata.file_type())
}

/// FIFOs and sockets stand in for every non-regular entry (device nodes take the same
/// `!is_file()` branch but cannot be created without privilege). An open on a FIFO
/// without a writer blocks, so a bounded restore also shows it is never opened.
#[tokio::test]
async fn fifo_socket_and_device_entries_are_disclosed_unsupported_and_never_restored_or_deleted() {
    use std::os::unix::fs::FileTypeExt;
    let p = Project::new();
    p.write("a.txt", "before\n");
    mkfifo(&p.root.join("old-pipe"));
    let before = p.track().await;
    assert!(before.skipped.iter().any(|s| s == "unsupported: old-pipe"));
    std::fs::remove_file(p.root.join("old-pipe")).unwrap();
    p.write("a.txt", "after\n");
    mkfifo(&p.root.join("new-pipe"));
    let _socket = std::os::unix::net::UnixListener::bind(p.root.join("sock")).unwrap();
    let after = p.track().await;
    for name in ["new-pipe", "sock"] {
        let label = format!("unsupported: {name}");
        assert!(after.skipped.contains(&label), "{:?}", after.skipped);
    }
    let disclosed = p
        .snapshots
        .exclusions(&p.dir(), vec![before.tree.clone(), after.tree.clone()])
        .await
        .unwrap();
    for name in ["old-pipe", "new-pipe", "sock"] {
        let label = format!("unsupported: {name}");
        assert!(
            disclosed.iter().any(|d| d.ends_with(&label)),
            "{disclosed:?}"
        );
    }
    let preview = p
        .snapshots
        .preview_restore(&p.dir(), &before.tree, &after.tree, &after.tree)
        .await
        .unwrap();
    assert_eq!(
        preview.iter().map(|f| f.file.as_str()).collect::<Vec<_>>(),
        ["a.txt"]
    );
    let (pipe, sock) = (inode(&p.root.join("new-pipe")), inode(&p.root.join("sock")));
    let restored = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        p.snapshots.restore_reviewed(
            &p.dir(),
            &before.tree,
            &after.tree,
            &after.tree,
            Arc::new(|| Ok(())),
        ),
    )
    .await
    .expect("restore must never open a FIFO")
    .unwrap();
    assert_eq!(restored, ["a.txt"]);
    assert_eq!(std::fs::read(p.root.join("a.txt")).unwrap(), b"before\n");
    assert!(std::fs::symlink_metadata(p.root.join("old-pipe")).is_err());
    assert_eq!(inode(&p.root.join("new-pipe")), pipe);
    assert_eq!(inode(&p.root.join("sock")), sock);
    assert!(pipe.1.is_fifo() && sock.1.is_socket());
}

/// Parks the in-process restore after its first actual replacement, lets an editor
/// write a path the plan has not reached, then resumes the same restore.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_concurrent_editor_write_between_replacements_refuses_the_rest_and_keeps_the_edit() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    let p = Project::new();
    p.write("a.txt", "before a");
    p.write("b.txt", "before b");
    let before = p.track().await;
    p.write("a.txt", "after a");
    p.write("b.txt", "after b");
    let after = p.track().await;
    let (parked_tx, parked_rx) = mpsc::channel::<()>();
    let (resume_tx, resume_rx) = mpsc::channel::<()>();
    let (parked_tx, resume_rx) = (Mutex::new(parked_tx), Mutex::new(resume_rx));
    let first = p.root.join("a.txt");
    let parked = AtomicBool::new(false);
    let park: Arc<dyn Fn() -> Result<(), String> + Send + Sync> = Arc::new(move || {
        if std::fs::read(&first).ok().as_deref() == Some(b"before a")
            && !parked.swap(true, Ordering::SeqCst)
        {
            parked_tx.lock().unwrap().send(()).unwrap();
            resume_rx.lock().unwrap().recv().unwrap();
        }
        Ok(())
    });
    let (snapshots, dir) = (p.snapshots.clone(), p.dir());
    let (target, recorded) = (before.tree.clone(), after.tree.clone());
    let restore = tokio::spawn(async move {
        snapshots
            .restore_reviewed(&dir, &target, &recorded, &recorded, park)
            .await
    });
    tokio::task::spawn_blocking(move || {
        parked_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("restore did not reach the between-file cut")
    })
    .await
    .unwrap();
    p.write("b.txt", "editor b");
    resume_tx.send(()).unwrap();
    let result = restore.await.unwrap();
    assert!(
        matches!(&result, Err(RestoreError::Failed(message)) if message.contains("conflicts: b.txt")),
        "{result:?}"
    );
    assert_eq!(std::fs::read(p.root.join("a.txt")).unwrap(), b"before a");
    assert_eq!(std::fs::read(p.root.join("b.txt")).unwrap(), b"editor b");
    let recovery = p.snapshots.recovery(&p.dir()).await.unwrap();
    assert_eq!(recovery.len(), 1);
    assert!(!recovery[0].filesystem_complete);
    assert_eq!(recovery[0].applied, ["a.txt"]);
    let journal = namespace(&p).join("journals").join(&recovery[0].id);
    assert_eq!(std::fs::read(journal.join("stage_0")).unwrap(), b"after a");
    assert!(
        p.snapshots
            .track(&p.dir())
            .await
            .unwrap_err()
            .contains("requires recovery")
    );
    assert_eq!(std::fs::read(p.root.join("b.txt")).unwrap(), b"editor b");
}
