//! Real Git image preparation followed by synchronous native journal execution.
use super::*;
use cyber_server::runtime::{SnapshotRestoreRequest, Snapshots};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

struct Repo {
    _temp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    snapshots: Arc<crate::GitSnapshots>,
}

#[test]
fn unsupported_native_config_environment_keeps_ordinary_git_capture_available() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "copy::git::tests::multiline_environment_capture_child",
        ])
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "fixture.multiline")
        .env("GIT_CONFIG_VALUE_0", "first\nsecond")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "subprocess fixture with unsupported native configuration environment"]
async fn multiline_environment_capture_child() {
    assert!(crate::copy::config_inputs::environment_digest().is_err());
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), "canonical-readable").unwrap();
    let snapshot = repo
        .snapshots
        .track(repo.root.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(
        snapshot
            .skipped
            .iter()
            .any(|value| value.contains("native Git policy/raw-state proof unavailable"))
    );
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    assert_eq!(
        shadow
            .blob(&snapshot.tree, "file")
            .await
            .unwrap()
            .unwrap()
            .1,
        b"canonical-readable"
    );
}

#[tokio::test]
async fn non_utf8_user_excludes_preserve_canonical_capture_without_native_rule_authority() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), "legacy-readable").unwrap();
    std::fs::write(repo.root.join(".git/info/exclude"), b"\xff\n").unwrap();
    let snapshot = repo
        .snapshots
        .track(repo.root.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(
        snapshot
            .skipped
            .iter()
            .any(|value| value.contains("native Git policy/raw-state proof unavailable"))
    );
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    assert_eq!(
        shadow
            .blob(&snapshot.tree, "file")
            .await
            .unwrap()
            .unwrap()
            .1,
        b"legacy-readable"
    );
    assert!(
        std::fs::read(shadow.git_dir.join("info/exclude"))
            .unwrap()
            .starts_with(b"\xff\n")
    );
}

#[tokio::test]
async fn changed_user_excludes_after_preparation_refuses_native_source_effects() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), "before").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), "after").unwrap();
    let recorded = repo.track(&repo.root).await;
    assert_eq!(repo.track(&repo.root).await, recorded);
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    std::fs::write(repo.root.join(".git/info/exclude"), "new-policy\n").unwrap();
    let store = repo.store(&repo.root);
    assert!(store.restore_git_worker(prepared, None).is_err());
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
    assert!(store.recovery().unwrap().is_empty());
}

#[test]
fn git_config_command_override_cannot_redirect_snapshot_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("external-config");
    let original = b"[core]\nexcludesfile=never-read\n";
    std::fs::write(&path, original).unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "copy::git::tests::git_config_override_child",
        ])
        .env("GIT_CONFIG", &path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[tokio::test]
#[ignore = "subprocess fixture with isolated Git config command override"]
async fn git_config_override_child() {
    let (repo, target, recorded) = Repo::size_fixture().await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    assert!(prepared.expected_policy.is_some());
    assert_eq!(
        repo.store(&repo.root)
            .restore_git_worker(prepared, None)
            .unwrap(),
        ["file"]
    );
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"before");
}

#[tokio::test]
async fn frozen_classification_resolves_user_gitdir_and_branch_includes_in_original_context() {
    let mut repo = Repo::new();
    repo.snapshots = crate::GitSnapshots::new(
        repo.data.clone(),
        Arc::new(|_| serde_json::json!({"snapshots":{"max_file_bytes":4096}})),
    );
    let global = repo._temp.path().join("conditional-ignore");
    std::fs::write(&global, "ignored\n").unwrap();
    let include = repo._temp.path().join("conditional-config");
    std::fs::write(
        &include,
        format!("[core]\nexcludesfile={}\n", global.display()),
    )
    .unwrap();
    repo.git(&[
        "config",
        &format!("includeIf.gitdir:{}.path", repo.root.join(".git").display()),
        include.to_str().unwrap(),
    ]);
    let head = std::fs::read_to_string(repo.root.join(".git/HEAD")).unwrap();
    let branch = head.trim().strip_prefix("ref: refs/heads/").unwrap();
    let branch_config = repo._temp.path().join("branch-config");
    std::fs::write(
        &branch_config,
        "[custom]\nflag\nvalue=\"quoted \\\"value\\\"\"\n",
    )
    .unwrap();
    repo.git(&[
        "config",
        &format!("includeIf.onbranch:{branch}.path"),
        branch_config.to_str().unwrap(),
    ]);
    repo.git(&["config", "--get", "core.excludesfile"]);
    repo.git(&["config", "--bool", "--get", "custom.flag"]);
    std::fs::create_dir(repo.root.join("nested")).unwrap();
    std::fs::write(repo.root.join("nested/large[1]"), vec![b'x'; 5120]).unwrap();
    std::fs::write(repo.root.join("ignored"), vec![b'i'; 8192]).unwrap();
    std::fs::write(repo.root.join("file"), "before").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), "after").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    assert_eq!(prepared.ignore.size_excluded, ["nested/large[1]"]);
    assert!(prepared.expected.images.contains_key("ignored"));
    assert_eq!(
        repo.store(&repo.root)
            .restore_git_worker(prepared, None)
            .unwrap(),
        ["file"]
    );
    assert_eq!(
        std::fs::read(repo.root.join("ignored")).unwrap(),
        vec![b'i'; 8192]
    );
    assert!(!repo.root.join(".git/index").exists());
}

#[test]
fn classification_mutation_keeps_canonical_capture_without_native_proof() {
    let temporary = tempfile::tempdir().unwrap();
    let bin = temporary.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    std::fs::write(&wrapper, r#"#!/bin/sh
listing=0
for argument in "$@"; do
    if [ "$argument" = ls-files ]; then listing=1; fi
done
case "$GIT_DIR" in
    */.eligibility-*)
        if [ "$listing" = 1 ] && [ ! -f "$CYBER_CLASSIFY_CUT" ]; then
            "$CYBER_REAL_GIT" "$@" > "$GIT_DIR/listing-output" || exit "$?"
            touch "$CYBER_CLASSIFY_CUT"
            case "$CYBER_CLASSIFY_MUTATION" in
                source-index) env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE "$CYBER_REAL_GIT" -C "$GIT_WORK_TREE" add -- 'nested/large[1]' || exit "$?" ;;
                private-config) printf '\n# changed private config\n' >> "$GIT_DIR/config" ;;
                private-route) printf 'foreign-route\n' > "$GIT_DIR/commondir" ;;
            esac
            cat "$GIT_DIR/listing-output"
            result=$?
            rm "$GIT_DIR/listing-output"
            exit "$result"
        fi
        ;;
esac
exec "$CYBER_REAL_GIT" "$@"
"#).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let real = std::process::Command::new("which")
        .arg("git")
        .output()
        .unwrap();
    assert!(real.status.success());
    let real = std::str::from_utf8(&real.stdout).unwrap().trim();
    for kind in ["source-index", "private-config", "private-route"] {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "copy::git::tests::classification_index_change_child",
            ])
            .env("CYBER_REAL_GIT", real)
            .env("CYBER_CLASSIFY_MUTATION", kind)
            .env(
                "CYBER_CLASSIFY_CUT",
                temporary.path().join(format!("cut-{kind}")),
            )
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[tokio::test]
#[ignore = "subprocess fixture with isolated Git classification wrapper"]
async fn classification_index_change_child() {
    let repo = Repo::new();
    std::fs::create_dir(repo.root.join("nested")).unwrap();
    std::fs::write(repo.root.join("nested/large[1]"), vec![b'x'; 5120]).unwrap();
    std::fs::write(repo.root.join("file"), "before").unwrap();
    let mut repo = repo;
    repo.snapshots = crate::GitSnapshots::new(
        repo.data.clone(),
        Arc::new(|_| serde_json::json!({"snapshots":{"max_file_bytes":4096}})),
    );
    let snapshot = repo
        .snapshots
        .track(repo.root.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(std::path::Path::new(&std::env::var_os("CYBER_CLASSIFY_CUT").unwrap()).exists());
    assert!(
        snapshot
            .skipped
            .iter()
            .any(|reason| reason.contains("native Git policy/raw-state proof unavailable"))
    );
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    let blob = shadow
        .blob(&snapshot.tree, "nested/large[1]")
        .await
        .unwrap();
    if std::env::var("CYBER_CLASSIFY_MUTATION").unwrap() == "source-index" {
        assert_eq!(blob.unwrap().1, vec![b'x'; 5120]);
    } else {
        assert!(blob.is_none());
    }
    assert_eq!(
        std::fs::read(repo.root.join("nested/large[1]")).unwrap(),
        vec![b'x'; 5120]
    );
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"before");
    assert!(repo.store(&repo.root).recovery().unwrap().is_empty());
}

#[tokio::test]
async fn empty_directories_do_not_change_capture_or_classification_identity() {
    let (repo, _, recorded) = Repo::size_fixture().await;
    std::fs::create_dir_all(repo.root.join("empty/deep")).unwrap();
    assert_eq!(repo.track(&repo.root).await, recorded);
}

#[tokio::test]
async fn disabled_user_global_ignores_keep_bound_size_classification_available() {
    let (repo, target, recorded) = Repo::size_fixture().await;
    repo.git(&["config", "core.excludesfile", ""]);
    let expected = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &expected)
        .await;
    assert_eq!(prepared.ignore.size_excluded, ["nested/large[1]"]);
    assert_eq!(
        repo.store(&repo.root)
            .restore_git_worker(prepared, None)
            .unwrap(),
        ["file"]
    );
    assert_eq!(
        std::fs::read(repo.root.join("nested/large[1]")).unwrap(),
        vec![b'x'; 5120]
    );
}

#[tokio::test]
async fn changed_user_classification_config_after_preparation_refuses_native_restore() {
    let (repo, target, recorded) = Repo::size_fixture().await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let ignore = repo._temp.path().join("classification-ignore");
    std::fs::write(&ignore, "nested/large[1]\n").unwrap();
    repo.git(&["config", "core.excludesfile", ignore.to_str().unwrap()]);
    let store = repo.store(&repo.root);
    assert!(store.restore_git_worker(prepared, None).is_err());
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
    assert!(store.recovery().unwrap().is_empty());
}

#[tokio::test]
async fn shrinking_size_excluded_file_after_preparation_refuses_native_restore() {
    let mut repo = Repo::new();
    repo.snapshots = crate::GitSnapshots::new(
        repo.data.clone(),
        Arc::new(|_| serde_json::json!({"snapshots":{"max_file_bytes":4096}})),
    );
    std::fs::write(repo.root.join("large"), vec![b'x'; 5120]).unwrap();
    std::fs::write(repo.root.join("file"), "before").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), "after").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    std::fs::write(repo.root.join("large"), "now eligible").unwrap();
    let store = repo.store(&repo.root);
    assert!(store.restore_git_worker(prepared, None).is_err());
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
    assert_eq!(
        std::fs::read(repo.root.join("large")).unwrap(),
        b"now eligible"
    );
    assert!(store.recovery().unwrap().is_empty());
}

#[tokio::test]
async fn native_git_size_exclusions_reject_late_type_and_threshold_changes() {
    for kind in [
        "threshold",
        "deleted",
        "file-link",
        "parent-link",
        "directory",
        "fifo",
    ] {
        let (repo, target, recorded) = Repo::size_fixture().await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        let store = repo.store(&repo.root);
        let inventory = store.git_inventory(&prepared).unwrap();
        mutate_size_exclusion(&repo, kind);
        assert!(inventory.verify(&store).is_err(), "{kind}");
        assert!(store.preview_git(&prepared).is_err(), "{kind}");
        assert!(store.restore_git_worker(prepared, None).is_err(), "{kind}");
        assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
        assert_eq!(
            std::fs::read(repo._temp.path().join("outside/large[1]")).unwrap(),
            vec![b'z'; 8192]
        );
        assert!(store.recovery().unwrap().is_empty());
    }
}

fn mutate_size_exclusion(repo: &Repo, kind: &str) {
    let path = repo.root.join("nested/large[1]");
    let outside = repo._temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("large[1]"), vec![b'z'; 8192]).unwrap();
    match kind {
        "threshold" => std::fs::write(path, vec![b'x'; 4096]).unwrap(),
        "parent-link" => {
            std::fs::rename(
                repo.root.join("nested"),
                repo._temp.path().join("old-nested"),
            )
            .unwrap();
            std::os::unix::fs::symlink(outside, repo.root.join("nested")).unwrap();
        }
        kind => {
            std::fs::remove_file(&path).unwrap();
            match kind {
                "deleted" => {}
                "file-link" => std::os::unix::fs::symlink(outside.join("large[1]"), path).unwrap(),
                "directory" => std::fs::create_dir(path).unwrap(),
                "fifo" => assert!(
                    std::process::Command::new("mkfifo")
                        .arg(path)
                        .status()
                        .unwrap()
                        .success()
                ),
                _ => unreachable!(),
            }
        }
    }
}

#[tokio::test]
async fn native_git_size_exclusions_allow_still_oversized_edits_without_source_replay() {
    let (repo, target, recorded) = Repo::size_fixture().await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let store = repo.store(&repo.root);
    let inventory = store.git_inventory(&prepared).unwrap();
    std::fs::write(repo.root.join("nested/large[1]"), vec![b'y'; 8192]).unwrap();
    inventory.verify(&store).unwrap();
    assert_eq!(store.restore_git_worker(prepared, None).unwrap(), ["file"]);
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"before");
    assert_eq!(
        std::fs::read(repo.root.join("nested/large[1]")).unwrap(),
        vec![b'y'; 8192]
    );
}

#[tokio::test]
async fn fresh_git_capture_includes_newly_eligible_file_and_preserves_historical_omission() {
    let (repo, target, recorded) = Repo::size_fixture().await;
    std::fs::write(repo.root.join("nested/large[1]"), "now eligible").unwrap();
    let expected = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &expected)
        .await;
    assert_eq!(
        prepared.expected.images["nested/large[1]"].bytes,
        b"now eligible"
    );
    assert!(prepared.ignore.size_excluded.is_empty());
    let store = repo.store(&repo.root);
    assert_eq!(store.restore_git_worker(prepared, None).unwrap(), ["file"]);
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"before");
    assert_eq!(
        std::fs::read(repo.root.join("nested/large[1]")).unwrap(),
        b"now eligible"
    );
}

#[tokio::test]
async fn changed_user_index_after_preparation_refuses_native_source_effects() {
    let mut repo = Repo::new();
    repo.snapshots = crate::GitSnapshots::new(
        repo.data.clone(),
        Arc::new(|_| serde_json::json!({"snapshots":{"max_file_bytes":4096}})),
    );
    std::fs::write(repo.root.join("large"), vec![b'x'; 5120]).unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo.root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["add", "--", "large"]);
    std::fs::write(repo.root.join("file"), "before").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), "after").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    git(&["rm", "--cached", "--quiet", "--", "large"]);
    let store = repo.store(&repo.root);
    assert!(store.restore_git_worker(prepared, None).is_err());
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
    assert_eq!(
        std::fs::read(repo.root.join("large")).unwrap(),
        vec![b'x'; 5120]
    );
    assert!(store.recovery().unwrap().is_empty());
}

#[tokio::test]
async fn native_git_index_proof_rejects_creation_replacement_and_changed_split_dependency() {
    for kind in ["creation", "replacement", "split"] {
        let repo = Repo::new();
        std::fs::write(repo.root.join("file"), "before").unwrap();
        if kind != "creation" {
            repo.git(&["add", "--", "file"]);
        }
        if kind == "split" {
            repo.git(&["update-index", "--split-index"]);
        }
        let target = repo.track(&repo.root).await;
        std::fs::write(repo.root.join("file"), "after").unwrap();
        let recorded = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        match kind {
            "creation" => repo.git(&["add", "--", "file"]),
            "replacement" => {
                let index = repo.root.join(".git/index");
                let replacement = repo.root.join(".git/replacement");
                std::fs::write(&replacement, std::fs::read(&index).unwrap()).unwrap();
                std::fs::rename(replacement, index).unwrap();
            }
            "split" => {
                let shared = std::fs::read_dir(repo.root.join(".git"))
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| {
                        path.file_name()
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .starts_with("sharedindex.")
                    })
                    .unwrap();
                std::fs::write(shared, "changed split index").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(prepared.ignore.config.verify().is_err(), "{kind}");
        let store = repo.store(&repo.root);
        assert!(store.restore_git_worker(prepared, None).is_err(), "{kind}");
        assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
        assert!(store.recovery().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_git_split_index_capture_preparation_and_restore_preserve_user_index_metadata() {
    use std::os::unix::fs::MetadataExt;
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), "before").unwrap();
    repo.git(&["add", "--", "file"]);
    repo.git(&["update-index", "--split-index"]);
    let shared = std::fs::read_dir(repo.root.join(".git"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .as_encoded_bytes()
                .starts_with(b"sharedindex.")
        })
        .unwrap();
    let paths = [repo.root.join(".git/index"), shared];
    let observations = || {
        paths.each_ref().map(|path| {
            let metadata = std::fs::metadata(path).unwrap();
            (
                metadata.ino(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
                std::fs::read(path).unwrap(),
            )
        })
    };
    let before = observations();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), "after").unwrap();
    let recorded = repo.track(&repo.root).await;
    assert_eq!(observations(), before);
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    assert_eq!(observations(), before);
    repo.store(&repo.root)
        .restore_git_worker(prepared, None)
        .unwrap();
    assert_eq!(observations(), before);
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"before");
}

#[tokio::test]
async fn oversized_user_index_keeps_canonical_capture_without_native_authority() {
    use sha1::Digest;
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), "canonical-readable").unwrap();
    repo.git(&["add", "--", "file"]);
    let path = repo.root.join(".git/index");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.truncate(bytes.len() - 20);
    bytes.extend_from_slice(b"TEST");
    bytes.extend_from_slice(&(1024_u32 * 1024).to_be_bytes());
    bytes.resize(bytes.len() + 1024 * 1024, 0);
    let hash = sha1::Sha1::digest(&bytes);
    bytes.extend_from_slice(&hash);
    std::fs::write(&path, &bytes).unwrap();
    repo.git(&["ls-files", "--error-unmatch", "--", "file"]);
    let snapshot = repo
        .snapshots
        .track(repo.root.to_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(
        snapshot
            .skipped
            .iter()
            .any(|value| value.contains("native Git policy/raw-state proof unavailable"))
    );
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    assert_eq!(
        shadow
            .blob(&snapshot.tree, "file")
            .await
            .unwrap()
            .unwrap()
            .1,
        b"canonical-readable"
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test]
async fn changed_git_config_locator_after_preparation_refuses_native_source_effects() {
    let repo = Repo::new();
    repo.track(&repo.root).await;
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    let first = repo._temp.path().join("first-ignore");
    let second = repo._temp.path().join("second-ignore");
    std::fs::write(&first, "").unwrap();
    std::fs::write(&second, "").unwrap();
    let configure = |path: &Path| {
        assert!(
            std::process::Command::new("git")
                .arg("--git-dir")
                .arg(&shadow.git_dir)
                .args(["config", "core.excludesfile"])
                .arg(path)
                .status()
                .unwrap()
                .success()
        )
    };
    configure(&first);
    std::fs::write(repo.root.join("file"), "before").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), "after").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    configure(&second);
    let store = repo.store(&repo.root);
    assert!(store.restore_git_worker(prepared, None).is_err());
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
    assert!(store.recovery().unwrap().is_empty());
}

#[tokio::test]
async fn native_git_config_proof_rejects_include_creation_changes_and_conditional_head_changes() {
    for kind in ["missing", "included", "head"] {
        let repo = Repo::new();
        repo.track(&repo.root).await;
        let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
        let include = shadow.git_dir.join("native-policy.include");
        let key = if kind == "head" {
            let head = std::fs::read_to_string(shadow.git_dir.join("HEAD")).unwrap();
            format!(
                "includeIf.onbranch:{}.path",
                head.trim().strip_prefix("ref: refs/heads/").unwrap()
            )
        } else {
            "include.path".into()
        };
        if kind != "missing" {
            std::fs::write(&include, "[core]\nignorecase=false\n").unwrap();
        }
        assert!(
            std::process::Command::new("git")
                .arg("--git-dir")
                .arg(&shadow.git_dir)
                .args(["config", &key, "native-policy.include"])
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(repo.root.join("file"), "before").unwrap();
        let target = repo.track(&repo.root).await;
        std::fs::write(repo.root.join("file"), "after").unwrap();
        let recorded = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        let store = repo.store(&repo.root);
        assert_eq!(store.preview_git(&prepared).unwrap().len(), 1);
        if kind == "head" {
            std::fs::write(
                shadow.git_dir.join("HEAD"),
                "ref: refs/heads/changed-private-branch\n",
            )
            .unwrap();
        } else {
            std::fs::write(&include, "[core]\nignorecase=true\n").unwrap();
        }
        assert!(store.restore_git_worker(prepared, None).is_err(), "{kind}");
        assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
        assert!(store.recovery().unwrap().is_empty());
    }
}

impl Repo {
    async fn size_fixture() -> (Self, String, String) {
        let mut repo = Self::new();
        repo.snapshots = crate::GitSnapshots::new(
            repo.data.clone(),
            Arc::new(|_| serde_json::json!({"snapshots":{"max_file_bytes":4096}})),
        );
        std::fs::create_dir(repo.root.join("nested")).unwrap();
        std::fs::write(repo.root.join("nested/large[1]"), vec![b'x'; 5120]).unwrap();
        std::fs::write(repo.root.join("file"), "before").unwrap();
        let target = repo.track(&repo.root).await;
        std::fs::write(repo.root.join("file"), "after").unwrap();
        let recorded = repo.track(&repo.root).await;
        (repo, target, recorded)
    }
    fn git(&self, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    fn new() -> Self {
        Self::with_format("sha1")
    }
    fn with_format(format: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("work");
        let data = base.join("data");
        std::fs::create_dir(&root).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet", &format!("--object-format={format}")])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        let snapshots = crate::GitSnapshots::new(data.clone(), Arc::new(|_| serde_json::json!({})));
        Self {
            _temp: temp,
            root,
            data,
            snapshots,
        }
    }
    async fn track(&self, directory: &Path) -> String {
        self.snapshots
            .track(directory.to_str().unwrap())
            .await
            .unwrap()
            .unwrap()
            .tree
    }
    fn store(&self, directory: &Path) -> Store {
        Store::open(
            &self.data,
            directory.to_str().unwrap(),
            super::super::Options {
                max_file: 65536,
                max_total: 8 * 1024 * 1024,
                ignore: vec![],
                private_roots: vec![],
            },
        )
        .unwrap()
    }
    async fn prepare(
        &self,
        directory: &Path,
        target: &str,
        recorded: &str,
        expected: &str,
    ) -> PreparedRestore {
        self.snapshots
            .shadow(directory.to_str().unwrap())
            .unwrap()
            .prepare_restore(
                &SnapshotRestoreRequest {
                    directory: directory.to_str().unwrap().into(),
                    target: target.into(),
                    recorded: recorded.into(),
                    expected: expected.into(),
                    paths: None,
                },
                65536,
                8 * 1024 * 1024,
            )
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn mixed_copy_git_images_keep_original_ids_scope_exclusions_and_new_journal_authority() {
    for (scoped, target_copy, recorded_copy) in [
        (false, true, true),
        (true, true, true),
        (false, true, false),
        (true, true, false),
        (false, false, true),
        (true, false, true),
    ] {
        let repo = Repo::with_format("sha256");
        let location = if scoped {
            repo.root.join("scope")
        } else {
            repo.root.clone()
        };
        std::fs::create_dir_all(&location).unwrap();
        let mut store = repo.store(&location);
        store.options.ignore = vec!["protected".into()];
        std::fs::write(location.join("file"), "before\n").unwrap();
        std::fs::write(location.join("protected"), "excluded before").unwrap();
        let target = if target_copy {
            store.track().unwrap().tree
        } else {
            repo.track(&location).await
        };
        std::fs::write(location.join("file"), "after\n").unwrap();
        let recorded = if recorded_copy {
            store.track().unwrap().tree
        } else {
            repo.track(&location).await
        };
        store.options.ignore.clear();
        std::fs::write(location.join("protected"), "new eligible owner bytes").unwrap();
        let expected = repo.track(&location).await;
        let prepared = repo.prepare(&location, &target, &recorded, &expected).await;
        let preview = store.preview_git(&prepared).unwrap();
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0].file, if scoped { "scope/file" } else { "file" });
        assert_eq!(store.restore_git_worker(prepared, None).unwrap(), ["file"]);
        assert_eq!(std::fs::read(location.join("file")).unwrap(), b"before\n");
        assert_eq!(
            std::fs::read(location.join("protected")).unwrap(),
            b"new eligible owner bytes"
        );
        let id = store.recovery().unwrap()[0].id.clone();
        let (dir, journal) = store.journal(&id).unwrap();
        assert_eq!(journal.version, 6);
        let context = journal.context.as_ref().unwrap();
        assert_eq!(
            (&context.target, &context.recorded, &context.expected),
            (&target, &recorded, &expected)
        );
        assert_eq!(
            serde_json::to_value(context).unwrap()["git"]["sources"],
            serde_json::json!({"target":if target_copy { "copy" } else { "git" },"recorded":if recorded_copy { "copy" } else { "git" },"expected":"git"})
        );
        assert!(
            context
                .exclusions
                .iter()
                .any(|value| value.contains("ignore: protected"))
        );
        assert!(context.validate(&location, 5).is_err());
        let mut downgraded = journal.clone();
        downgraded.version = 5;
        Store::record(&dir, &downgraded).unwrap();
        assert!(store.journal(&id).is_err());
        Store::record(&dir, &journal).unwrap();
        let mut mismatched = context.clone();
        mismatched
            .git
            .as_mut()
            .unwrap()
            .sources
            .as_mut()
            .unwrap()
            .expected = ImageSource::Copy;
        assert!(mismatched.validate(&location, 6).is_err());
        assert!(
            !repo
                .snapshots
                .owned_restore_available(location.to_str().unwrap())
        );
        assert!(!repo.root.join(".git/index").exists());
    }
}

#[tokio::test]
async fn mixed_image_import_refuses_corruption_foreign_manifests_stale_source_and_combined_budget()
{
    for fault in ["corrupt", "foreign", "stale", "budget", "packet"] {
        let repo = Repo::new();
        let mut store = repo.store(&repo.root);
        std::fs::write(repo.root.join("file"), "before\n").unwrap();
        let mut target = store.track().unwrap().tree;
        std::fs::write(repo.root.join("file"), "after\n").unwrap();
        let recorded = store.track().unwrap().tree;
        if fault == "corrupt" {
            let tree = store.load(&target).unwrap();
            store
                .objects
                .write(&tree.files["file"].object, b"corrupt")
                .unwrap();
        }
        if fault == "foreign" {
            let other = repo.root.join("foreign");
            std::fs::create_dir(&other).unwrap();
            let foreign = repo.store(&other);
            let snapshot = foreign.track().unwrap().tree;
            let digest = snapshot.strip_prefix("copy:").unwrap();
            let bytes =
                storage::read(&foreign.manifests, digest, super::super::RECORD_LIMIT).unwrap();
            storage::atomic(&store.manifests, digest, &bytes).unwrap();
            target = snapshot;
        }
        let expected = repo.track(&repo.root).await;
        let mut prepared = repo
            .prepare(&repo.root, &target, &recorded, &expected)
            .await;
        if fault == "packet" {
            let bytes = b"caller-supplied image".to_vec();
            prepared.target.images.insert(
                "file".into(),
                Image {
                    source: ImageSource::Copy,
                    mode: "100644".into(),
                    object: storage::digest(&bytes),
                    bytes,
                },
            );
        }
        if fault == "stale" {
            std::fs::write(repo.root.join("file"), "later owner").unwrap();
        }
        if fault == "budget" {
            store.options.max_total = 18;
        }
        let error = store.restore_git_worker(prepared, None).unwrap_err();
        if fault == "budget" {
            assert!(error.to_string().contains("handoff exceeds"), "{error}");
        }
        assert_eq!(
            std::fs::read(repo.root.join("file")).unwrap(),
            if fault == "stale" {
                b"later owner".as_slice()
            } else {
                b"after\n".as_slice()
            }
        );
        assert!(store.recovery().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_git_import_restores_files_modes_creations_deletions_and_retains_original_ids() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), b"before").unwrap();
    std::fs::set_permissions(
        repo.root.join("file"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::create_dir(repo.root.join("deleted")).unwrap();
    std::fs::write(repo.root.join("deleted/file"), b"restore parent").unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo.root)
            .args(["add", "file"])
            .status()
            .unwrap()
            .success()
    );
    let user_index = std::fs::read(repo.root.join(".git/index")).unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"after").unwrap();
    std::fs::set_permissions(
        repo.root.join("file"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    std::fs::remove_file(repo.root.join("deleted/file")).unwrap();
    std::fs::remove_dir(repo.root.join("deleted")).unwrap();
    std::fs::write(repo.root.join("added"), b"added").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    // Native execution must not ask Git for object bytes or use its index again.
    let objects = shadow.git_dir.join("objects");
    std::fs::rename(&objects, objects.with_extension("retained")).unwrap();
    let store = repo.store(&repo.root);
    assert_eq!(
        store.restore_git_worker(prepared, None).unwrap(),
        ["added", "deleted/file", "file"]
    );
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"before");
    assert_ne!(
        std::fs::metadata(repo.root.join("file"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    assert_eq!(
        std::fs::read(repo.root.join("deleted/file")).unwrap(),
        b"restore parent"
    );
    assert!(!repo.root.join("added").exists());
    assert_eq!(
        std::fs::read(repo.root.join(".git/index")).unwrap(),
        user_index
    );
    let journals = store.recovery().unwrap();
    assert_eq!(journals.len(), 1);
    let (dir, journal) = store.journal(&journals[0].id).unwrap();
    assert_eq!(journal.version, 5);
    let context = journal.context.as_ref().unwrap();
    assert_eq!(
        (&context.target, &context.recorded, &context.expected),
        (&target, &recorded, &recorded)
    );
    assert_eq!(context.git.as_ref().unwrap().worktree, repo.root);
    assert!(journal.completed);
    assert!(journal.ownership.is_some());
    assert_eq!(storage::read(&dir, "stage_2", 65536).unwrap(), b"after");
    assert!(
        !repo
            .snapshots
            .owned_restore_available(repo.root.to_str().unwrap())
    );
    let mut legacy_git = journal.clone();
    legacy_git.version = 4;
    legacy_git.backend = None;
    Store::record(&dir, &legacy_git).unwrap();
    assert!(store.journal(&journals[0].id).is_ok());
    let mut downgraded = journal.clone();
    downgraded.version = 3;
    Store::record(&dir, &downgraded).unwrap();
    assert!(store.journal(&journals[0].id).is_err());
    let mut missing_context = journal.clone();
    missing_context.context = None;
    Store::record(&dir, &missing_context).unwrap();
    assert!(store.journal(&journals[0].id).is_err());
    let mut missing_ownership = journal;
    missing_ownership.ownership = None;
    Store::record(&dir, &missing_ownership).unwrap();
    assert!(store.journal(&journals[0].id).is_err());
}

#[tokio::test]
async fn native_git_import_merges_owner_edits_and_rejects_stale_handoffs() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), b"one\ntwo\nthree\n").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"ONE\ntwo\nthree\n").unwrap();
    let recorded = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"ONE\ntwo\nTHREE\n").unwrap();
    let expected = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &expected)
        .await;
    let store = repo.store(&repo.root);
    store.restore_git_worker(prepared, None).unwrap();
    assert_eq!(
        std::fs::read(repo.root.join("file")).unwrap(),
        b"one\ntwo\nTHREE\n"
    );
    drop(store);
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &expected)
        .await;
    std::fs::write(repo.root.join("file"), b"later owner").unwrap();
    let store = repo.store(&repo.root);
    let before = store.recovery().unwrap().len();
    assert!(matches!(
        store.restore_git_worker(prepared, None),
        Err(RestoreError::Stale)
    ));
    assert_eq!(store.recovery().unwrap().len(), before);
    assert_eq!(
        std::fs::read(repo.root.join("file")).unwrap(),
        b"later owner"
    );
}

#[tokio::test]
async fn native_git_import_keeps_subdirectory_and_historical_exclusion_boundaries() {
    let repo = Repo::new();
    let directory = repo.root.join("[ab]");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("file"), b"before").unwrap();
    std::fs::write(directory.join("ignored"), b"owner ignored before").unwrap();
    std::fs::write(directory.join(".gitignore"), b"ignored\n").unwrap();
    let target = repo.track(&directory).await;
    std::fs::write(directory.join("file"), b"after").unwrap();
    std::fs::write(directory.join("ignored"), b"later ignored owner").unwrap();
    std::fs::write(repo.root.join("sibling"), b"sibling owner").unwrap();
    let recorded = repo.track(&directory).await;
    let prepared = repo
        .prepare(&directory, &target, &recorded, &recorded)
        .await;
    let store = repo.store(&directory);
    assert_eq!(store.restore_git_worker(prepared, None).unwrap(), ["file"]);
    assert_eq!(std::fs::read(directory.join("file")).unwrap(), b"before");
    assert_eq!(
        std::fs::read(directory.join("ignored")).unwrap(),
        b"later ignored owner"
    );
    assert_eq!(
        std::fs::read(repo.root.join("sibling")).unwrap(),
        b"sibling owner"
    );
    let recovery = store.recovery().unwrap();
    let (_, journal) = store.journal(&recovery[0].id).unwrap();
    let context = journal.context.unwrap();
    assert_eq!(context.git.as_ref().unwrap().prefix, "[ab]");
    assert!(
        context
            .exclusions
            .iter()
            .any(|path| path.contains("ignored"))
    );
    let mut invalid = context.clone();
    invalid.git.as_mut().unwrap().prefix = "../foreign".into();
    assert!(invalid.validate(&directory, 4).is_err());
    invalid = context;
    invalid.target = format!("copy:{}", "0".repeat(64));
    assert!(invalid.validate(&directory, 4).is_err());
}

#[tokio::test]
async fn native_git_import_refuses_escaping_links_before_any_journal_or_source_effect() {
    let repo = Repo::new();
    std::os::unix::fs::symlink("../destination", repo.root.join("link")).unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::remove_file(repo.root.join("link")).unwrap();
    std::fs::write(repo.root.join("link"), b"current owner").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let store = repo.store(&repo.root);
    assert!(
        matches!(store.restore_git_worker(prepared, None), Err(RestoreError::Failed(error)) if error.contains("outside the worktree"))
    );
    assert!(store.recovery().unwrap().is_empty());
    assert_eq!(
        std::fs::read(repo.root.join("link")).unwrap(),
        b"current owner"
    );
}

#[test]
fn legacy_copy_context_keeps_its_encoding_and_cannot_be_reinterpreted_as_git() {
    let value = serde_json::json!({"target": format!("copy:{}", "a".repeat(64)),
        "recorded": format!("copy:{}", "b".repeat(64)), "expected": format!("copy:{}", "c".repeat(64)), "exclusions": []});
    let context: RestoreContext = serde_json::from_value(value.clone()).unwrap();
    context.validate(Path::new("/fixture"), 3).unwrap();
    assert_eq!(serde_json::to_value(&context).unwrap(), value);
    let mut invalid = context;
    invalid.git = Some(GitContext {
        sources: None,
        worktree: "/fixture".into(),
        prefix: String::new(),
    });
    assert!(invalid.validate(Path::new("/fixture"), 4).is_err());
}

#[tokio::test]
async fn imported_git_preimages_support_native_keep_and_rollback_with_retained_git_context() {
    use cyber_server::runtime::{
        SnapshotBinding, SnapshotOrigin, SnapshotRecoveryAction as Action,
        SnapshotRecoveryConfirmation,
    };
    for (action, mixed) in [
        (Action::Keep, false),
        (Action::Rollback, false),
        (Action::Keep, true),
        (Action::Rollback, true),
    ] {
        let repo = Repo::new();
        let store = repo.store(&repo.root);
        std::fs::write(repo.root.join("file"), b"target before").unwrap();
        let target = if mixed {
            store.track().unwrap().tree
        } else {
            repo.track(&repo.root).await
        };
        std::fs::write(repo.root.join("file"), b"recorded after").unwrap();
        let recorded = if mixed {
            store.track().unwrap().tree
        } else {
            repo.track(&repo.root).await
        };
        let expected = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &expected)
            .await;
        store.restore_git_worker(prepared, None).unwrap();
        let id = store.recovery().unwrap()[0].id.clone();
        let (dir, mut journal) = store.journal(&id).unwrap();
        // Synthetic fixture origin exercises the native backend only. It grants no
        // runtime/Store acknowledgement authority or worker lifetime proof.
        journal.origin = Some(SnapshotOrigin {
            version: 1,
            session_id: "ses_fixture".into(),
            session_created_id: "evt_fixture".into(),
            activity_id: "op_fixture".into(),
            worktree_id: None,
            binding: SnapshotBinding {
                directory: store.root.clone(),
                root_identity: store.root_id.clone(),
                namespace_identity: storage::dir_identity(&store.journals).unwrap(),
                journal_id: id.clone(),
                journal_identity: storage::dir_identity(&dir).unwrap(),
                lock_identity: Some(store.lock_identity().unwrap()),
            },
        });
        Store::record(&dir, &journal).unwrap();
        std::fs::write(repo.root.join("owner-file"), b"later owner").unwrap();
        let review = store.recovery_review(&id, action).unwrap();
        assert!(review.conflicts.is_empty(), "{:?}", review.conflicts);
        let confirmation = SnapshotRecoveryConfirmation {
            journal_id: id,
            action,
            confirmation: review.confirmation.unwrap(),
            request_id: "git-native-fixture".into(),
        };
        let receipt = store
            .resolve_recovery(&confirmation, &|| Ok(()), None)
            .unwrap();
        let (_, resolution) = store.journal(&receipt.resolution_id).unwrap();
        assert_eq!(resolution.version, if mixed { 6 } else { 5 });
        assert_eq!(resolution.context.as_ref().unwrap().target, target);
        assert_eq!(
            std::fs::read(repo.root.join("file")).unwrap(),
            if action == Action::Keep {
                b"target before".as_slice()
            } else {
                b"recorded after".as_slice()
            }
        );
        assert_eq!(
            std::fs::read(repo.root.join("owner-file")).unwrap(),
            b"later owner"
        );
        std::fs::write(repo.root.join("file"), b"owner after resolution").unwrap();
        assert_eq!(
            store
                .resolve_recovery(&confirmation, &|| Ok(()), None)
                .unwrap(),
            receipt
        );
        assert_eq!(
            std::fs::read(repo.root.join("file")).unwrap(),
            b"owner after resolution"
        );
    }
}

#[tokio::test]
async fn git_image_handoff_enforces_byte_bounds_without_source_effects() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), b"bounded image").unwrap();
    let snapshot = repo.track(&repo.root).await;
    let request = SnapshotRestoreRequest {
        directory: repo.root.to_str().unwrap().into(),
        target: snapshot.clone(),
        recorded: snapshot.clone(),
        expected: snapshot,
        paths: None,
    };
    let shadow = repo.snapshots.shadow(&request.directory).unwrap();
    assert!(shadow.prepare_restore(&request, 65536, 1).await.is_err());
    assert_eq!(
        std::fs::read(repo.root.join("file")).unwrap(),
        b"bounded image"
    );
    assert!(repo.store(&repo.root).recovery().unwrap().is_empty());
}

#[tokio::test]
async fn native_git_import_rechecks_the_handed_off_blob_identity_before_reading_source() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), b"before").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"after").unwrap();
    let recorded = repo.track(&repo.root).await;
    let mut prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    prepared.target.images.get_mut("file").unwrap().bytes = b"substituted".to_vec();
    let store = repo.store(&repo.root);
    assert!(
        matches!(store.restore_git_worker(prepared, None), Err(RestoreError::Failed(error)) if error.contains("object identity"))
    );
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
    assert!(store.recovery().unwrap().is_empty());
}

#[tokio::test]
async fn git_handoff_limits_changes_to_exact_reviewed_paths_inside_the_location() {
    let repo = Repo::new();
    let directory = repo.root.join("[ab]");
    std::fs::create_dir(&directory).unwrap();
    for name in ["first", "second"] {
        std::fs::write(directory.join(name), b"before").unwrap();
    }
    let target = repo.track(&directory).await;
    for name in ["first", "second"] {
        std::fs::write(directory.join(name), b"after").unwrap();
    }
    let recorded = repo.track(&directory).await;
    let mut request = SnapshotRestoreRequest {
        directory: directory.to_str().unwrap().into(),
        target,
        recorded: recorded.clone(),
        expected: recorded,
        paths: Some(vec!["[ab]/first".into()]),
    };
    let shadow = repo.snapshots.shadow(&request.directory).unwrap();
    let prepared = shadow
        .prepare_restore(&request, 65536, 8 * 1024 * 1024)
        .await
        .unwrap();
    let store = repo.store(&directory);
    assert_eq!(store.restore_git_worker(prepared, None).unwrap(), ["first"]);
    assert_eq!(std::fs::read(directory.join("first")).unwrap(), b"before");
    assert_eq!(std::fs::read(directory.join("second")).unwrap(), b"after");
    request.paths = Some(vec!["[absolutely]/first".into()]);
    assert!(
        shadow
            .prepare_restore(&request, 65536, 8 * 1024 * 1024)
            .await
            .is_err()
    );
}

/// Synthetic native fixture only; no runtime acknowledgement authority is granted.
fn own_native_journal(store: &Store, id: &str) {
    use cyber_server::runtime::{SnapshotBinding, SnapshotOrigin};
    let (dir, mut journal) = store.journal(id).unwrap();
    journal.origin = Some(SnapshotOrigin {
        version: 1,
        session_id: "ses_fixture".into(),
        session_created_id: "evt_fixture".into(),
        activity_id: "op_fixture".into(),
        worktree_id: None,
        binding: SnapshotBinding {
            directory: store.root.clone(),
            root_identity: store.root_id.clone(),
            namespace_identity: storage::dir_identity(&store.journals).unwrap(),
            journal_id: id.into(),
            journal_identity: storage::dir_identity(&dir).unwrap(),
            lock_identity: Some(store.lock_identity().unwrap()),
        },
    });
    Store::record(&dir, &journal).unwrap();
}
fn put_node(root: &Path, node: &str) {
    let path = root.join("entry");
    if std::fs::symlink_metadata(&path).is_ok() {
        std::fs::remove_file(&path).unwrap();
    }
    match node {
        "absent" => {}
        "file" => std::fs::write(path, b"regular owner bytes").unwrap(),
        target => std::os::unix::fs::symlink(target, path).unwrap(),
    }
}
fn assert_node(root: &Path, node: &str) {
    let path = root.join("entry");
    match node {
        "absent" => assert!(std::fs::symlink_metadata(path).is_err()),
        "file" => assert_eq!(std::fs::read(path).unwrap(), b"regular owner bytes"),
        target => assert_eq!(std::fs::read_link(path).unwrap(), Path::new(target)),
    }
}

#[tokio::test]
async fn native_git_link_changes_and_rollback_never_follow_target_files() {
    use cyber_server::runtime::{SnapshotRecoveryAction as Action, SnapshotRecoveryConfirmation};
    for (want, recorded_node) in [
        ("one", "two"),
        ("one", "file"),
        ("file", "two"),
        ("one", "absent"),
        ("absent", "two"),
        ("missing/child", "file"),
    ] {
        let repo = Repo::new();
        std::fs::write(repo.root.join("one"), b"first target owner data").unwrap();
        std::fs::write(repo.root.join("two"), b"second target owner data").unwrap();
        put_node(&repo.root, want);
        let target = repo.track(&repo.root).await;
        put_node(&repo.root, recorded_node);
        let recorded = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        let store = repo.store(&repo.root);
        assert_eq!(
            store.restore_git_worker(prepared, None).unwrap(),
            ["entry"],
            "{want}/{recorded_node}"
        );
        assert_node(&repo.root, want);
        let id = store.recovery().unwrap()[0].id.clone();
        own_native_journal(&store, &id);
        let review = store.recovery_review(&id, Action::Rollback).unwrap();
        assert!(review.conflicts.is_empty(), "{:?}", review.conflicts);
        let desired = review
            .paths
            .iter()
            .find(|path| path.path == "entry")
            .unwrap();
        if want != "file" && want != "absent" {
            assert_eq!(
                desired.current.as_ref().unwrap().kind,
                super::super::ImageKind::Symlink
            );
        }
        let confirmation = SnapshotRecoveryConfirmation {
            journal_id: id,
            action: Action::Rollback,
            confirmation: review.confirmation.unwrap(),
            request_id: "link-rollback".into(),
        };
        let receipt = store
            .resolve_recovery(&confirmation, &|| Ok(()), None)
            .unwrap();
        assert_node(&repo.root, recorded_node);
        assert_eq!(
            std::fs::read(repo.root.join("one")).unwrap(),
            b"first target owner data"
        );
        assert_eq!(
            std::fs::read(repo.root.join("two")).unwrap(),
            b"second target owner data"
        );
        let (_, journal) = store.journal(&receipt.resolution_id).unwrap();
        assert_eq!(journal.version, 5);
        std::fs::write(repo.root.join("later-owner"), b"owner after resolution").unwrap();
        assert_eq!(
            store
                .resolve_recovery(&confirmation, &|| Ok(()), None)
                .unwrap(),
            receipt
        );
    }
}

#[tokio::test]
async fn native_git_link_target_aliases_cannot_escape_or_enter_protected_namespaces() {
    for target_name in [
        "alias/secret",
        "alias/../secret",
        ".git/config",
        ".GIT/config",
        "../outside/secret",
    ] {
        let repo = Repo::new();
        if target_name == ".GIT/config" && !repo.root.join(target_name).exists() {
            continue;
        }
        let outside = repo.root.parent().unwrap().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"private target data").unwrap();
        std::fs::create_dir(outside.join("child")).unwrap();
        std::os::unix::fs::symlink(outside.join("child"), repo.root.join("alias")).unwrap();
        put_node(&repo.root, target_name);
        let target = repo.track(&repo.root).await;
        put_node(&repo.root, "file");
        let recorded = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        let store = repo.store(&repo.root);
        assert!(store.restore_git_worker(prepared, None).is_err());
        assert_node(&repo.root, "file");
        assert!(store.recovery().unwrap().is_empty());
        assert_eq!(
            std::fs::read(outside.join("secret")).unwrap(),
            b"private target data"
        );
    }
}

#[tokio::test]
async fn native_link_targets_cannot_use_case_aliases_of_configured_private_roots() {
    let repo = Repo::new();
    let private = repo.root.join("Private");
    std::fs::create_dir(&private).unwrap();
    if !repo.root.join("PRIVATE").exists() {
        return;
    }
    put_node(&repo.root, "PRIVATE/missing");
    let target = repo.track(&repo.root).await;
    put_node(&repo.root, "file");
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let mut store = repo.store(&repo.root);
    store.options.private_roots.push(private);
    let result = store.restore_git_worker(prepared, None);
    assert!(
        matches!(&result, Err(RestoreError::Failed(error)) if error.contains("private root") || error.contains("protected")),
        "{result:?}"
    );
    assert_node(&repo.root, "file");
    assert!(store.recovery().unwrap().is_empty());
}

#[tokio::test]
async fn native_inventory_refuses_new_files_nested_files_and_links_before_journaling() {
    for kind in ["file", "nested", "link"] {
        let repo = Repo::new();
        std::fs::write(repo.root.join("file"), b"before").unwrap();
        let target = repo.track(&repo.root).await;
        std::fs::write(repo.root.join("file"), b"after").unwrap();
        let recorded = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        match kind {
            "file" => std::fs::write(repo.root.join("appeared"), b"new owner").unwrap(),
            "nested" => {
                std::fs::create_dir(repo.root.join("appeared")).unwrap();
                std::fs::write(repo.root.join("appeared/child"), b"new owner").unwrap();
            }
            _ => std::os::unix::fs::symlink("../never-opened", repo.root.join("appeared")).unwrap(),
        }
        let store = repo.store(&repo.root);
        assert!(
            matches!(
                store.restore_git_worker(prepared, None),
                Err(RestoreError::Stale)
            ),
            "{kind}"
        );
        assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
        assert!(store.recovery().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_inventory_respects_live_git_rules_in_mixed_directories_and_empty_directories() {
    let repo = Repo::new();
    std::fs::write(repo.root.join(".gitignore"), b"*.log\nignored/\n").unwrap();
    std::fs::create_dir(repo.root.join("mixed")).unwrap();
    std::fs::create_dir(repo.root.join("ignored")).unwrap();
    std::fs::write(repo.root.join("mixed/owner.log"), b"ignored owner").unwrap();
    std::fs::write(repo.root.join("mixed/kept"), b"kept owner").unwrap();
    std::fs::write(repo.root.join("ignored/seed"), b"ignored seed").unwrap();
    std::fs::write(repo.root.join("file"), b"before").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"after").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    std::fs::write(repo.root.join("mixed/owner.log"), b"later ignored owner").unwrap();
    std::fs::write(repo.root.join("ignored/new"), b"later ignored owner").unwrap();
    std::fs::create_dir_all(repo.root.join("empty/deep")).unwrap();
    let store = repo.store(&repo.root);
    assert_eq!(store.restore_git_worker(prepared, None).unwrap(), ["file"]);
    assert_eq!(
        std::fs::read(repo.root.join("mixed/owner.log")).unwrap(),
        b"later ignored owner"
    );
    assert_eq!(
        std::fs::read(repo.root.join("mixed/kept")).unwrap(),
        b"kept owner"
    );
    assert!(repo.root.join("empty/deep").is_dir());
}

#[tokio::test]
async fn namespace_and_rule_changes_after_inventory_are_stale_without_source_effects() {
    for change in ["namespace", "rule"] {
        let repo = Repo::new();
        std::fs::write(repo.root.join(".gitignore"), b"*.log\n").unwrap();
        std::fs::write(repo.root.join("file"), b"before").unwrap();
        let target = repo.track(&repo.root).await;
        std::fs::write(repo.root.join("file"), b"after").unwrap();
        let recorded = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        let store = repo.store(&repo.root);
        let inventory = store.git_inventory(&prepared).unwrap();
        if change == "namespace" {
            std::fs::write(repo.root.join("appeared"), b"new owner").unwrap();
        } else {
            std::fs::write(repo.root.join(".gitignore"), b"file\n").unwrap();
        }
        assert!(
            matches!(inventory.verify(&store), Err(RestoreError::Stale)),
            "{change}"
        );
        assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
        assert!(store.recovery().unwrap().is_empty());
    }
}

#[tokio::test]
async fn historical_exclusions_protect_mutation_without_hiding_current_state_changes() {
    for owner_changes in [false, true] {
        let repo = Repo::new();
        std::fs::write(repo.root.join(".gitignore"), b"kept\n").unwrap();
        std::fs::write(repo.root.join("kept"), b"owner before").unwrap();
        std::fs::write(repo.root.join("file"), b"before").unwrap();
        let target = repo.track(&repo.root).await;
        std::fs::write(repo.root.join(".gitignore"), b"").unwrap();
        std::fs::write(repo.root.join("file"), b"after").unwrap();
        let recorded = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        if owner_changes {
            std::fs::write(repo.root.join("kept"), b"later owner").unwrap();
        }
        let store = repo.store(&repo.root);
        let result = store.restore_git_worker(prepared, None);
        if owner_changes {
            assert!(matches!(result, Err(RestoreError::Stale)));
            assert!(store.recovery().unwrap().is_empty());
        } else {
            assert!(result.is_ok(), "{result:?}");
        }
        assert_eq!(
            std::fs::read(repo.root.join("kept")).unwrap(),
            if owner_changes {
                b"later owner".as_slice()
            } else {
                b"owner before".as_slice()
            }
        );
    }
}

#[tokio::test]
async fn native_inventory_keeps_literal_backslashes_in_locations_and_file_names() {
    let repo = Repo::new();
    let directory = repo.root.join("back\\slash");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("literal\\file"), b"before").unwrap();
    let target = repo.track(&directory).await;
    std::fs::write(directory.join("literal\\file"), b"after").unwrap();
    let recorded = repo.track(&directory).await;
    let prepared = repo
        .prepare(&directory, &target, &recorded, &recorded)
        .await;
    let store = repo.store(&directory);
    assert_eq!(
        store.restore_git_worker(prepared, None).unwrap(),
        ["literal\\file"]
    );
    assert_eq!(
        std::fs::read(directory.join("literal\\file")).unwrap(),
        b"before"
    );
    for invalid in ["../escape", "a//b", "a/./b", "/absolute"] {
        assert!(super::super::tree::safe_path(invalid).is_err());
    }
}

#[tokio::test]
async fn native_inventory_applies_ancestor_rules_and_nested_negations_only_inside_its_location() {
    let repo = Repo::new();
    std::fs::write(repo.root.join(".gitignore"), b"*.tmp\n").unwrap();
    let directory = repo.root.join("scope");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join(".gitignore"), b"!keep.tmp\n").unwrap();
    std::fs::write(directory.join("keep.tmp"), b"included owner").unwrap();
    std::fs::write(directory.join("owner.tmp"), b"ignored owner").unwrap();
    std::fs::write(directory.join("file"), b"before").unwrap();
    let target = repo.track(&directory).await;
    std::fs::write(directory.join("file"), b"after").unwrap();
    let recorded = repo.track(&directory).await;
    let prepared = repo
        .prepare(&directory, &target, &recorded, &recorded)
        .await;
    std::fs::write(repo.root.join("sibling"), b"outside Location owner").unwrap();
    std::fs::write(directory.join("owner.tmp"), b"later ignored owner").unwrap();
    let store = repo.store(&directory);
    assert_eq!(store.restore_git_worker(prepared, None).unwrap(), ["file"]);
    assert_eq!(
        std::fs::read(directory.join("keep.tmp")).unwrap(),
        b"included owner"
    );
    assert_eq!(
        std::fs::read(repo.root.join("sibling")).unwrap(),
        b"outside Location owner"
    );
}

#[tokio::test]
async fn native_inventory_reads_git_external_ignore_inputs_and_detects_rule_only_changes() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), b"before").unwrap();
    repo.track(&repo.root).await;
    let global = repo.root.parent().unwrap().join("global-ignore");
    std::fs::write(&global, b"*.global\n").unwrap();
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&shadow.git_dir)
            .args(["config", "core.excludesfile"])
            .arg(&global)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(repo.root.join(".git/info/exclude"), b"*.info\n").unwrap();
    std::fs::write(repo.root.join("owner.global"), b"global ignored owner").unwrap();
    std::fs::write(repo.root.join("owner.info"), b"info ignored owner").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"after").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let store = repo.store(&repo.root);
    let inventory = store.git_inventory(&prepared).unwrap();
    std::fs::write(&global, b"file\n").unwrap();
    assert!(matches!(inventory.verify(&store), Err(RestoreError::Stale)));
    std::fs::write(&global, b"*.global\n").unwrap();
    assert!(matches!(
        store.restore_git_worker(prepared, None),
        Err(RestoreError::Stale)
    ));
    let expected = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &expected)
        .await;
    assert_eq!(store.restore_git_worker(prepared, None).unwrap(), ["file"]);
    assert_eq!(
        std::fs::read(repo.root.join("owner.info")).unwrap(),
        b"info ignored owner"
    );
    assert_eq!(
        std::fs::read(repo.root.join("owner.global")).unwrap(),
        b"global ignored owner"
    );
}

#[tokio::test]
async fn expected_exclusion_disappearance_and_type_changes_are_stale() {
    for change in ["missing", "directory", "file"] {
        let repo = Repo::new();
        std::fs::write(repo.root.join(".gitignore"), b"ignored/\n*.log\n").unwrap();
        std::fs::create_dir(repo.root.join("ignored")).unwrap();
        std::fs::write(repo.root.join("ignored/seed"), b"ignored owner").unwrap();
        std::fs::write(repo.root.join("old.log"), b"ignored owner").unwrap();
        std::fs::write(repo.root.join("file"), b"before").unwrap();
        let target = repo.track(&repo.root).await;
        std::fs::write(repo.root.join("file"), b"after").unwrap();
        let recorded = repo.track(&repo.root).await;
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        match change {
            "missing" => std::fs::remove_file(repo.root.join("old.log")).unwrap(),
            "directory" => {
                std::fs::remove_file(repo.root.join("old.log")).unwrap();
                std::fs::create_dir(repo.root.join("old.log")).unwrap();
            }
            _ => {
                std::fs::remove_file(repo.root.join("ignored/seed")).unwrap();
                std::fs::remove_dir(repo.root.join("ignored")).unwrap();
                std::fs::write(repo.root.join("ignored"), b"owner file").unwrap();
            }
        }
        let store = repo.store(&repo.root);
        assert!(
            matches!(
                store.restore_git_worker(prepared, None),
                Err(RestoreError::Stale)
            ),
            "{change}"
        );
        assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
        assert!(store.recovery().unwrap().is_empty());
    }
}

#[tokio::test]
async fn non_git_snapshots_capture_and_restore_literal_backslash_names() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("work");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("literal\\name"), b"before").unwrap();
    let snapshots =
        crate::GitSnapshots::new(base.join("data"), Arc::new(|_| serde_json::json!({})));
    let directory = root.to_str().unwrap();
    let target = snapshots.track(directory).await.unwrap().unwrap().tree;
    std::fs::write(root.join("literal\\name"), b"after").unwrap();
    let recorded = snapshots.track(directory).await.unwrap().unwrap().tree;
    assert_eq!(
        snapshots
            .restore_reviewed(
                directory,
                &target,
                &recorded,
                &recorded,
                Arc::new(|| Ok(()))
            )
            .await
            .unwrap(),
        ["literal\\name"]
    );
    assert_eq!(
        std::fs::read(root.join("literal\\name")).unwrap(),
        b"before"
    );
}

#[tokio::test]
async fn capture_bound_policy_rejects_ignored_inventory_changes_before_planning() {
    let repo = Repo::new();
    std::fs::write(repo.root.join(".gitignore"), b"*.log\n").unwrap();
    std::fs::create_dir(repo.root.join("mixed")).unwrap();
    std::fs::write(repo.root.join("mixed/kept"), b"owner").unwrap();
    std::fs::write(repo.root.join("mixed/old.log"), b"ignored owner").unwrap();
    std::fs::write(repo.root.join("file"), b"before").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"after").unwrap();
    let recorded = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("mixed/new.log"), b"new ignored owner").unwrap();
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    assert!(prepared.expected_policy.is_some());
    let store = repo.store(&repo.root);
    assert!(matches!(
        store.restore_git_worker(prepared, None),
        Err(RestoreError::Stale)
    ));
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
    assert!(store.recovery().unwrap().is_empty());
}

#[tokio::test]
async fn capture_bound_policy_rejects_excluded_rule_changes_that_do_not_change_the_tree() {
    let repo = Repo::new();
    std::fs::write(repo.root.join(".git/info/exclude"), b".gitignore\n").unwrap();
    std::fs::write(repo.root.join(".gitignore"), b"*.log\n").unwrap();
    std::fs::write(repo.root.join("file"), b"before").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"after").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    std::fs::write(
        repo.root.join(".gitignore"),
        b"*.log\n# changed ignored rule input\n",
    )
    .unwrap();
    assert!(!prepared.expected.images.contains_key(".gitignore"));
    let store = repo.store(&repo.root);
    assert!(matches!(
        store.restore_git_worker(prepared, None),
        Err(RestoreError::Stale)
    ));
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
    assert!(store.recovery().unwrap().is_empty());
}

#[tokio::test]
async fn legacy_expected_policy_absence_never_authorizes_native_import() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), b"before").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file"), b"after").unwrap();
    let recorded = repo.track(&repo.root).await;
    let mut prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    prepared.expected_policy = None;
    let store = repo.store(&repo.root);
    assert!(
        matches!(store.restore_git_worker(prepared, None), Err(RestoreError::Failed(error)) if error.contains("capture-bound policy"))
    );
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
    assert!(store.recovery().unwrap().is_empty());
}

#[tokio::test]
async fn capture_bound_policy_rejects_external_setting_changes_before_preparation() {
    for change in ["content", "locator", "case"] {
        let repo = Repo::new();
        std::fs::write(repo.root.join("file"), b"before").unwrap();
        repo.track(&repo.root).await;
        let global = repo.root.parent().unwrap().join("ignore-a");
        let other = repo.root.parent().unwrap().join("ignore-b");
        std::fs::write(&global, b"*.log\n").unwrap();
        std::fs::write(&other, b"*.log\n").unwrap();
        let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
        let config = |key: &str, value: &str| {
            assert!(
                std::process::Command::new("git")
                    .arg("--git-dir")
                    .arg(&shadow.git_dir)
                    .args(["config", key, value])
                    .status()
                    .unwrap()
                    .success()
            );
        };
        config("core.excludesfile", global.to_str().unwrap());
        config("core.ignorecase", "false");
        let target = repo.track(&repo.root).await;
        std::fs::write(repo.root.join("file"), b"after").unwrap();
        let recorded = repo.track(&repo.root).await;
        match change {
            "content" => std::fs::write(&global, b"*.log\n# changed input\n").unwrap(),
            "locator" => config("core.excludesfile", other.to_str().unwrap()),
            _ => config("core.ignorecase", "true"),
        }
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        let store = repo.store(&repo.root);
        assert!(
            matches!(
                store.restore_git_worker(prepared, None),
                Err(RestoreError::Stale)
            ),
            "{change}"
        );
        assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
        assert!(store.recovery().unwrap().is_empty());
    }
}

#[tokio::test]
async fn capture_bound_policy_rejects_application_ignore_and_size_setting_changes() {
    use std::sync::atomic::{AtomicBool, Ordering};
    for choice in ["ignore", "size"] {
        let mut repo = Repo::new();
        let changed = Arc::new(AtomicBool::new(false));
        let config_changed = changed.clone();
        repo.snapshots = crate::GitSnapshots::new(
            repo.data.clone(),
            Arc::new(move |_| {
                if !config_changed.load(Ordering::SeqCst) {
                    return serde_json::json!({});
                }
                if choice == "ignore" {
                    serde_json::json!({"snapshots":{"ignore":["never-matched-name"]}})
                } else {
                    serde_json::json!({"snapshots":{"max_file_bytes":1048576}})
                }
            }),
        );
        std::fs::write(repo.root.join("file"), b"before").unwrap();
        let target = repo.track(&repo.root).await;
        std::fs::write(repo.root.join("file"), b"after").unwrap();
        let recorded = repo.track(&repo.root).await;
        changed.store(true, Ordering::SeqCst);
        let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
        assert!(shadow.ignore_inputs().await.is_err());
        // A fresh capture regenerates rules for the new settings. The old
        // checkpoint still cannot authorize native restore under this policy.
        assert_ne!(repo.track(&repo.root).await, recorded);
        let prepared = repo
            .prepare(&repo.root, &target, &recorded, &recorded)
            .await;
        let store = repo.store(&repo.root);
        assert!(
            matches!(
                store.restore_git_worker(prepared, None),
                Err(RestoreError::Stale)
            ),
            "{choice}"
        );
        assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after");
        assert!(store.recovery().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_git_uses_exact_captured_crlf_bytes_and_keeps_both_graphs_reachable() {
    let repo = Repo::new();
    std::fs::write(repo.root.join(".gitattributes"), b"*.txt text eol=crlf\n").unwrap();
    std::fs::write(repo.root.join("file.txt"), b"before\r\n").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file.txt"), b"after\r\n").unwrap();
    let recorded = repo.track(&repo.root).await;
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    // Canonical public Git consumers still see the normalized object.
    assert_eq!(
        shadow.blob(&target, "file.txt").await.unwrap().unwrap().1,
        b"before\n"
    );
    // Aggressive ordinary Git GC must retain the raw graph through the tag ref.
    assert!(
        std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&shadow.git_dir)
            .args(["gc", "--prune=now"])
            .status()
            .unwrap()
            .success()
    );
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    assert_eq!(prepared.target.images["file.txt"].bytes, b"before\r\n");
    assert_eq!(prepared.expected.images["file.txt"].bytes, b"after\r\n");
    let store = repo.store(&repo.root);
    assert_eq!(
        store.restore_git_worker(prepared, None).unwrap(),
        ["file.txt"]
    );
    assert_eq!(
        std::fs::read(repo.root.join("file.txt")).unwrap(),
        b"before\r\n"
    );
}

#[tokio::test]
async fn native_git_rejects_later_normalization_equivalent_owner_bytes() {
    let repo = Repo::new();
    std::fs::write(repo.root.join(".gitattributes"), b"*.txt text eol=lf\n").unwrap();
    std::fs::write(repo.root.join("file.txt"), b"before\n").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file.txt"), b"after\n").unwrap();
    let recorded = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file.txt"), b"after\r\n").unwrap();
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let store = repo.store(&repo.root);
    assert!(matches!(
        store.restore_git_worker(prepared, None),
        Err(RestoreError::Stale)
    ));
    assert_eq!(
        std::fs::read(repo.root.join("file.txt")).unwrap(),
        b"after\r\n"
    );
    assert!(store.recovery().unwrap().is_empty());
}

#[tokio::test]
async fn native_git_raw_capture_supports_sha256_user_repositories_and_encoded_bytes() {
    let repo = Repo::with_format("sha256");
    std::fs::write(
        repo.root.join(".gitattributes"),
        b"*.txt working-tree-encoding=UTF-16LE\n",
    )
    .unwrap();
    let before: Vec<u8> = "before\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let after: Vec<u8> = "after\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    std::fs::write(repo.root.join("file.txt"), &before).unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo.root)
            .args(["add", "--all"])
            .status()
            .unwrap()
            .success()
    );
    let user_index = std::fs::read(repo.root.join(".git/index")).unwrap();
    let user_config = std::fs::read(repo.root.join(".git/config")).unwrap();
    let target = repo.track(&repo.root).await;
    assert_eq!(target.len(), 64);
    assert!(
        !repo
            .snapshots
            .owned_restore_available(repo.root.to_str().unwrap())
    );
    std::fs::write(repo.root.join("file.txt"), &after).unwrap();
    let recorded = repo.track(&repo.root).await;
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    assert_eq!(
        shadow.blob(&target, "file.txt").await.unwrap().unwrap().1,
        b"before\n"
    );
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    assert_eq!(prepared.expected.images["file.txt"].bytes, after);
    let store = repo.store(&repo.root);
    assert_eq!(
        store.restore_git_worker(prepared, None).unwrap(),
        ["file.txt"]
    );
    assert_eq!(std::fs::read(repo.root.join("file.txt")).unwrap(), before);
    assert_eq!(
        std::fs::read(repo.root.join(".git/index")).unwrap(),
        user_index
    );
    assert_eq!(
        std::fs::read(repo.root.join(".git/config")).unwrap(),
        user_config
    );
}

#[tokio::test]
async fn git_markers_refuse_non_git_admission_without_creating_identity_or_snapshot_state() {
    let repo = Repo::with_format("sha256");
    let nested = repo.root.join("nested/deep");
    std::fs::create_dir_all(&nested).unwrap();
    for path in [&repo.root, &nested] {
        assert!(!repo.snapshots.non_git(path.to_str().unwrap()));
        assert!(
            !repo
                .snapshots
                .owned_restore_available(path.to_str().unwrap())
        );
    }
    assert!(!repo.root.join(".git/cyber-project-id").exists());
    assert!(!repo.data.exists());
    let missing = format!("copy:{}", "0".repeat(64));
    assert!(
        repo.snapshots
            .changed(repo.root.to_str().unwrap(), &missing, &missing)
            .await
            .is_err()
    );
    assert!(
        repo.snapshots
            .diff(repo.root.to_str().unwrap(), &missing, &missing)
            .await
            .is_err()
    );
    assert!(!repo.data.exists());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    std::fs::write(root.join(".git"), "malformed gitfile\n").unwrap();
    assert!(!repo.snapshots.non_git(root.to_str().unwrap()));
    assert!(
        repo.snapshots
            .track(root.to_str().unwrap())
            .await
            .unwrap_err()
            .contains("fallback is disabled")
    );
    std::fs::remove_file(root.join(".git")).unwrap();
    std::os::unix::fs::symlink("missing", root.join(".git")).unwrap();
    assert!(!repo.snapshots.non_git(root.to_str().unwrap()));
    std::fs::remove_file(root.join(".git")).unwrap();
    assert!(repo.snapshots.non_git(root.to_str().unwrap()));
    assert!(!repo.data.exists());
}

#[tokio::test]
async fn mismatched_git_shadow_format_refuses_capture_without_touching_source_or_index() {
    let repo = Repo::new();
    std::fs::write(repo.root.join("file"), "source").unwrap();
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    std::fs::create_dir_all(&shadow.git_dir).unwrap();
    assert!(
        std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&shadow.git_dir)
            .args(["init", "--bare", "--quiet", "--object-format=sha256"])
            .status()
            .unwrap()
            .success()
    );
    let config = std::fs::read(shadow.git_dir.join("config")).unwrap();
    let error = repo
        .snapshots
        .track(repo.root.to_str().unwrap())
        .await
        .unwrap_err();
    assert!(error.contains("object format differs"), "{error}");
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"source");
    assert_eq!(
        std::fs::read(shadow.git_dir.join("config")).unwrap(),
        config
    );
    assert!(!shadow.git_dir.join("index").exists());
    assert!(!repo.root.join(".git/index").exists());
    assert!(!repo.data.join("snapshot/global").exists());
}

#[tokio::test]
async fn sha256_linked_worktree_uses_its_common_repository_format_and_scoped_git_images() {
    let repo = Repo::with_format("sha256");
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo.root)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "root"
            ])
            .status()
            .unwrap()
            .success()
    );
    let linked = repo._temp.path().canonicalize().unwrap().join("linked");
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo.root)
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(&linked)
            .arg("HEAD")
            .status()
            .unwrap()
            .success()
    );
    let marker = std::fs::read(linked.join(".git")).unwrap();
    let location = linked.join("nested");
    std::fs::create_dir(&location).unwrap();
    std::fs::write(location.join("file"), "before").unwrap();
    let target = repo.track(&location).await;
    assert_eq!(target.len(), 64);
    std::fs::write(location.join("file"), "after").unwrap();
    let recorded = repo.track(&location).await;
    let prepared = repo.prepare(&location, &target, &recorded, &recorded).await;
    assert_eq!(prepared.prefix, "nested");
    assert_eq!(prepared.target.images["file"].bytes, b"before");
    let policy = prepared.ignore.config.clone();
    repo.store(&location)
        .restore_git_worker(prepared, None)
        .unwrap();
    policy.verify().unwrap();
    assert_eq!(std::fs::read(location.join("file")).unwrap(), b"before");
    std::fs::write(location.join("file"), "after").unwrap();
    let paths = repo
        .snapshots
        .restore_reviewed(
            location.to_str().unwrap(),
            &target,
            &recorded,
            &recorded,
            Arc::new(|| Ok(())),
        )
        .await
        .unwrap();
    assert_eq!(paths, ["nested/file"]);
    assert_eq!(std::fs::read(location.join("file")).unwrap(), b"before");
    let expected = repo.track(&location).await;
    let prepared = repo
        .prepare(&location, &recorded, &expected, &expected)
        .await;
    let store = repo.store(&location);
    assert_eq!(store.preview_git(&prepared).unwrap().len(), 1);
    std::fs::write(
        repo.root.join(".git/info/exclude"),
        "changed-common-policy\n",
    )
    .unwrap();
    assert!(store.restore_git_worker(prepared, None).is_err());
    assert_eq!(std::fs::read(location.join("file")).unwrap(), b"before");
    assert_eq!(std::fs::read(linked.join(".git")).unwrap(), marker);
    std::fs::remove_file(repo.root.join(".git/info/exclude")).unwrap();
    let expected = repo.track(&location).await;
    let prepared = repo
        .prepare(&location, &recorded, &expected, &expected)
        .await;
    let common_index = std::fs::read(repo.root.join(".git/index")).ok();
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&linked)
            .args(["add", "--", "nested/file"])
            .status()
            .unwrap()
            .success()
    );
    assert!(prepared.ignore.config.verify().is_err());
    assert!(store.restore_git_worker(prepared, None).is_err());
    assert_eq!(
        std::fs::read(repo.root.join(".git/index")).ok(),
        common_index
    );
    assert_eq!(std::fs::read(location.join("file")).unwrap(), b"before");
    assert!(
        !repo
            .snapshots
            .owned_restore_available(location.to_str().unwrap())
    );
}

#[tokio::test]
async fn retained_copy_history_stays_readable_after_sha256_repository_routing_is_corrected() {
    let repo = Repo::with_format("sha256");
    let directory = repo.root.to_str().unwrap();
    std::fs::write(repo.root.join("file"), "before\n").unwrap();
    std::fs::write(repo.root.join("huge"), vec![b'x'; 65537]).unwrap();
    // Reproduce checkpoints captured by the prior erroneous fallback routing.
    let store = repo.store(&repo.root);
    let target = store.track().unwrap().tree;
    std::fs::write(repo.root.join("file"), "after\n").unwrap();
    let recorded = store.track().unwrap().tree;
    let excluded = store
        .exclusions(&[target.clone(), recorded.clone()])
        .unwrap();
    assert!(!excluded.is_empty());
    assert!(
        repo.snapshots
            .changed(directory, &target, &recorded)
            .await
            .unwrap_err()
            .contains("storage is busy")
    );
    drop(store);
    let git = repo.track(&repo.root).await;
    assert_eq!(git.len(), 64);
    assert_eq!(
        repo.snapshots
            .changed(directory, &target, &recorded)
            .await
            .unwrap(),
        ["file"]
    );
    assert_eq!(
        repo.snapshots
            .diff(directory, &target, &recorded)
            .await
            .unwrap()
            .len(),
        1
    );
    let observed = repo
        .snapshots
        .exclusions(directory, vec![target.clone(), recorded, git.clone()])
        .await
        .unwrap();
    for path in excluded {
        assert!(observed.contains(&path));
    }
    assert!(
        repo.snapshots
            .changed(directory, &target, &git)
            .await
            .unwrap_err()
            .contains("different backends")
    );
    assert!(
        repo.snapshots
            .diff(directory, &git, &target)
            .await
            .unwrap_err()
            .contains("different backends")
    );
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"after\n");
}

#[tokio::test]
async fn corrupt_raw_git_blob_cannot_be_promoted_to_native_restore_evidence() {
    let repo = Repo::new();
    std::fs::write(repo.root.join(".gitattributes"), b"*.txt text eol=crlf\n").unwrap();
    std::fs::write(repo.root.join("file.txt"), b"before\r\n").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file.txt"), b"after\r\n").unwrap();
    let recorded = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    let object = |id: &str| shadow.git_dir.join("objects").join(&id[..2]).join(&id[2..]);
    let original = object(&prepared.target.images["file.txt"].object);
    let replacement = std::fs::read(object(&prepared.recorded.images["file.txt"].object)).unwrap();
    std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(original, replacement).unwrap();
    assert!(
        shadow
            .prepare_restore(
                &SnapshotRestoreRequest {
                    directory: repo.root.to_str().unwrap().into(),
                    target,
                    recorded: recorded.clone(),
                    expected: recorded,
                    paths: None,
                },
                65536,
                8 * 1024 * 1024
            )
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(repo.root.join("file.txt")).unwrap(),
        b"after\r\n"
    );
    assert!(repo.store(&repo.root).recovery().unwrap().is_empty());
}

#[tokio::test]
async fn git_capture_does_not_publish_a_checkpoint_if_a_fixture_filter_changes_raw_state() {
    let repo = Repo::new();
    repo.track(&repo.root).await;
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    let refs = || {
        let output = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&shadow.git_dir)
            .args([
                "for-each-ref",
                "--format=%(objectname)",
                "refs/cyber/snapshots",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        output.stdout
    };
    let before = refs();
    // This explicitly configured fixture filter changes only its own temp repo.
    // Canonical capture already executes Git filters; raw proof must not hide it.
    assert!(
        std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&shadow.git_dir)
            .args([
                "config",
                "filter.fixture.clean",
                "printf changed > capture-marker; cat"
            ])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(repo.root.join(".gitattributes"), b"file filter=fixture\n").unwrap();
    std::fs::write(repo.root.join("file"), b"owner").unwrap();
    let result = repo.snapshots.track(repo.root.to_str().unwrap()).await;
    assert!(matches!(result, Err(error) if error.contains("raw state changed")));
    assert_eq!(
        refs(),
        before,
        "failed capture must not publish a checkpoint ref"
    );
    assert_eq!(std::fs::read(repo.root.join("file")).unwrap(), b"owner");
    assert_eq!(
        std::fs::read(repo.root.join("capture-marker")).unwrap(),
        b"changed"
    );
}

#[tokio::test]
async fn native_git_preview_and_path_selection_include_normalization_only_changes() {
    let repo = Repo::new();
    std::fs::write(repo.root.join(".gitattributes"), b"*.txt text eol=lf\n").unwrap();
    std::fs::write(repo.root.join("file.txt"), b"same\r\n").unwrap();
    let target = repo.track(&repo.root).await;
    std::fs::write(repo.root.join("file.txt"), b"same\n").unwrap();
    let recorded = repo.track(&repo.root).await;
    let shadow = repo.snapshots.shadow(repo.root.to_str().unwrap()).unwrap();
    assert!(shadow.changed(&target, &recorded).await.unwrap().is_empty());
    assert_eq!(
        shadow.changed_native(&target, &recorded).await.unwrap(),
        ["file.txt"]
    );
    assert!(
        !repo
            .snapshots
            .owned_restore_available(repo.root.to_str().unwrap())
    );
    let preview = repo
        .snapshots
        .git_preview(SnapshotRestoreRequest {
            directory: repo.root.to_str().unwrap().into(),
            target: target.clone(),
            recorded: recorded.clone(),
            expected: recorded.clone(),
            paths: None,
        })
        .await
        .unwrap();
    assert_eq!(preview.len(), 1);
    assert_eq!(preview[0].file, "file.txt");
    assert!(!preview[0].patch.is_empty());
    assert_eq!(
        std::fs::read(repo.root.join("file.txt")).unwrap(),
        b"same\n"
    );
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &recorded)
        .await;
    let store = repo.store(&repo.root);
    assert!(store.recovery().unwrap().is_empty());
    assert_eq!(
        store.restore_git_worker(prepared, None).unwrap(),
        ["file.txt"]
    );
    assert_eq!(
        std::fs::read(repo.root.join("file.txt")).unwrap(),
        b"same\r\n"
    );
}

#[tokio::test]
async fn native_git_scoped_preview_matches_restore_selection_and_refuses_stale_state() {
    let repo = Repo::new();
    let directory = repo.root.join("scope");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("one"), b"before one").unwrap();
    std::fs::write(directory.join("two"), b"before two").unwrap();
    let target = repo.track(&directory).await;
    std::fs::write(directory.join("one"), b"after one").unwrap();
    std::fs::write(directory.join("two"), b"after two").unwrap();
    let recorded = repo.track(&directory).await;
    let request = || SnapshotRestoreRequest {
        directory: directory.to_str().unwrap().into(),
        target: target.clone(),
        recorded: recorded.clone(),
        expected: recorded.clone(),
        paths: Some(vec!["scope/one".into()]),
    };
    let preview = repo.snapshots.git_preview(request()).await.unwrap();
    assert_eq!(preview.len(), 1);
    assert_eq!(preview[0].file, "scope/one");
    assert!(preview[0].patch.contains("a/scope/one"));
    assert_eq!(std::fs::read(directory.join("one")).unwrap(), b"after one");
    assert!(repo.store(&directory).recovery().unwrap().is_empty());
    std::fs::write(directory.join("two"), b"later owner edit outside selection").unwrap();
    assert!(matches!(
        repo.snapshots.git_preview(request()).await,
        Err(RestoreError::Stale)
    ));
    assert_eq!(std::fs::read(directory.join("one")).unwrap(), b"after one");
    assert!(repo.store(&directory).recovery().unwrap().is_empty());
}

#[tokio::test]
async fn git_lifetime_port_retains_exact_owner_lock_without_enabling_source_admission() {
    use cyber_server::runtime::SnapshotLifetimeRequest;
    let repo = Repo::new();
    let request = SnapshotLifetimeRequest {
        activity_id: "op_git_worker".into(),
        session_id: "ses_git_owner".into(),
        session_created_id: "evt_git_created".into(),
        directory: repo.root.clone(),
        worktree_id: None,
    };
    let lifetime = repo
        .snapshots
        .reserve_worker(request.clone())
        .await
        .unwrap()
        .unwrap();
    let binding = lifetime.binding().clone();
    assert_eq!(binding.version, 2);
    assert_eq!(binding.owner, request);
    lifetime.verify().unwrap();
    lifetime.verify_preparation(None).unwrap();
    assert_eq!(lifetime.checkout_bindings().unwrap(), None);
    assert!(
        repo.snapshots
            .reserve_worker(request.clone())
            .await
            .is_err()
    );
    assert!(repo.snapshots.lock_worker(binding.clone()).await.is_err());
    assert!(
        !repo
            .snapshots
            .owned_restore_available(repo.root.to_str().unwrap())
    );
    drop(lifetime);
    let lifetime = repo.snapshots.lock_worker(binding.clone()).await.unwrap();
    assert_eq!(lifetime.binding(), &binding);
    lifetime.bind_checkouts(&[]).unwrap();
    assert_eq!(lifetime.checkout_bindings().unwrap(), Some(vec![]));
    drop(lifetime);
    let mut foreign = binding.clone();
    foreign.owner.session_created_id = "evt_other_creation".into();
    assert!(repo.snapshots.lock_worker(foreign).await.is_err());
    let lifetime = repo
        .snapshots
        .reserve_worker(request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lifetime.binding(), &binding);
    assert_eq!(lifetime.checkout_bindings().unwrap(), Some(vec![]));
    assert!(repo.store(&repo.root).recovery().unwrap().is_empty());
    assert!(
        !repo
            .snapshots
            .owned_restore_available(repo.root.to_str().unwrap())
    );
}

#[tokio::test]
async fn git_lifetime_port_keeps_scoped_locations_and_managed_checkout_completeness() {
    use cyber_server::runtime::SnapshotLifetimeRequest;
    let repo = Repo::new();
    let scope = repo.root.join("scope");
    std::fs::create_dir(&scope).unwrap();
    std::fs::write(scope.join("owner"), b"owner bytes").unwrap();
    let request = SnapshotLifetimeRequest {
        activity_id: "op_git_scoped".into(),
        session_id: "ses_git_owner".into(),
        session_created_id: "evt_git_created".into(),
        directory: scope.clone(),
        worktree_id: Some("wt_required".into()),
    };
    let lifetime = repo
        .snapshots
        .reserve_worker(request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lifetime.binding().owner.directory, scope);
    assert!(lifetime.bind_checkouts(&[]).is_err());
    assert_eq!(lifetime.checkout_bindings().unwrap(), None);
    assert!(repo.store(&scope).recovery().unwrap().is_empty());
    assert_eq!(std::fs::read(scope.join("owner")).unwrap(), b"owner bytes");
    assert!(
        !repo
            .snapshots
            .owned_restore_available(scope.to_str().unwrap())
    );
}

#[tokio::test]
async fn interrupted_mixed_native_replacement_reopens_and_rolls_back_only_retained_effects() {
    use cyber_server::runtime::{
        SnapshotBinding, SnapshotOrigin, SnapshotRecoveryAction, SnapshotRecoveryConfirmation,
    };
    let repo = Repo::new();
    let store = repo.store(&repo.root);
    for file in ["a", "b"] {
        std::fs::write(repo.root.join(file), "before").unwrap();
    }
    let target = store.track().unwrap().tree;
    for file in ["a", "b"] {
        std::fs::write(repo.root.join(file), "after").unwrap();
    }
    let recorded = store.track().unwrap().tree;
    let expected = repo.track(&repo.root).await;
    let prepared = repo
        .prepare(&repo.root, &target, &recorded, &expected)
        .await;
    let (changes, context) = store.plan_git(&prepared).unwrap();
    let id = "restore_mixed_partial";
    let mut journal = store
        .prepare_journal(changes, id, None, Some(context), None)
        .unwrap();
    let stop = || {
        if std::fs::read(repo.root.join("a")).unwrap() == b"before" {
            Err("fixture stopped after first actual replacement".into())
        } else {
            Ok(())
        }
    };
    assert!(
        store
            .execute_journal(id, &mut journal, &stop, None)
            .is_err()
    );
    assert_eq!(std::fs::read(repo.root.join("a")).unwrap(), b"before");
    assert_eq!(std::fs::read(repo.root.join("b")).unwrap(), b"after");
    let (directory, mut journal) = store.journal(id).unwrap();
    assert_eq!(journal.version, 6);
    assert!(!journal.completed);
    // Native backend fixture only: no runtime/Store or checkout admission proof.
    journal.origin = Some(SnapshotOrigin {
        version: 1,
        session_id: "ses_fixture".into(),
        session_created_id: "evt_fixture".into(),
        activity_id: "op_fixture".into(),
        worktree_id: None,
        binding: SnapshotBinding {
            directory: store.root.clone(),
            root_identity: store.root_id.clone(),
            namespace_identity: storage::dir_identity(&store.journals).unwrap(),
            journal_id: id.into(),
            journal_identity: storage::dir_identity(&directory).unwrap(),
            lock_identity: Some(store.lock_identity().unwrap()),
        },
    });
    Store::record(&directory, &journal).unwrap();
    drop(directory);
    drop(store);
    let reopened = repo.store(&repo.root);
    std::fs::write(repo.root.join("owner-file"), "later owner").unwrap();
    let action = SnapshotRecoveryAction::Rollback;
    let review = reopened.recovery_review(id, action).unwrap();
    assert!(review.conflicts.is_empty());
    let confirmation = SnapshotRecoveryConfirmation {
        journal_id: id.into(),
        action,
        confirmation: review.confirmation.unwrap(),
        request_id: "mixed-partial-reopened".into(),
    };
    let receipt = reopened
        .resolve_recovery(&confirmation, &|| Ok(()), None)
        .unwrap();
    assert_eq!(
        reopened.journal(&receipt.resolution_id).unwrap().1.version,
        6
    );
    for file in ["a", "b"] {
        assert_eq!(std::fs::read(repo.root.join(file)).unwrap(), b"after");
    }
    assert_eq!(
        std::fs::read(repo.root.join("owner-file")).unwrap(),
        b"later owner"
    );
    std::fs::write(repo.root.join("a"), "owner after resolution").unwrap();
    assert_eq!(
        reopened
            .resolve_recovery(&confirmation, &|| Ok(()), None)
            .unwrap(),
        receipt
    );
    assert_eq!(
        std::fs::read(repo.root.join("a")).unwrap(),
        b"owner after resolution"
    );
}
