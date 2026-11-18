//! Native filesystem privacy of non-Git snapshot storage through the real binary:
//! every object, manifest, journal, displaced image and lock the service writes
//! is reachable only by its owner, whatever the process umask and the source
//! modes, and storage widened by someone else is refused rather than used.
use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

const PUBLIC: &str = "world-readable owner bytes\n";

/// Every entry below `root`, without following symbolic links.
fn entries(root: &Path) -> Vec<(PathBuf, std::fs::Metadata)> {
    let mut found = vec![];
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                pending.push(path.clone());
            }
            found.push((path, metadata));
        }
    }
    found
}

/// No macOS ACL entry can grant another principal what the mode bits deny.
#[cfg(target_os = "macos")]
fn assert_no_acl(path: &Path) {
    let listing = Command::new("ls").arg("-led").arg(path).output().unwrap();
    let text = String::from_utf8_lossy(&listing.stdout);
    assert!(
        listing.status.success() && text.lines().count() == 1,
        "{}: {text}",
        path.display()
    );
}

#[cfg(not(target_os = "macos"))]
fn assert_no_acl(_: &Path) {}

/// `journals/<restore>/stage_<n>`: a source file the restore moved aside.
fn is_displaced_image(path: &Path) -> bool {
    let name = path.file_name().unwrap().to_string_lossy();
    let journals = path.parent().and_then(Path::parent).and_then(Path::file_name);
    name.starts_with("stage_") && journals.is_some_and(|j| j == "journals")
}

#[track_caller]
fn assert_private_storage(env: &Env) -> usize {
    let data = env.root.join("cyber-home/data");
    assert_eq!(std::fs::metadata(&data).unwrap().mode() & 0o022, 0);
    let global = data.join("snapshot/global");
    let owner = std::fs::metadata(&env.root).unwrap().uid();
    let top = std::fs::metadata(&global).unwrap();
    assert_eq!(top.mode() & 0o777, 0o700, "{}", global.display());
    assert_no_acl(&global);
    let mut copies = 0;
    for (path, metadata) in entries(&global) {
        let shown = path.display();
        assert_eq!(metadata.uid(), owner, "{shown}");
        assert!(!metadata.file_type().is_symlink(), "{shown}");
        if metadata.is_dir() {
            assert_eq!(metadata.mode() & 0o077, 0, "{shown} is not private");
            assert_no_acl(&path);
            continue;
        }
        assert!(metadata.is_file(), "{shown}");
        assert_eq!(metadata.nlink(), 1, "{shown} shares an inode");
        if is_displaced_image(&path) {
            // A displaced source keeps its own inode and mode for an exact
            // rollback; only the owner-only directories above it make it private.
            assert_eq!(metadata.mode() & 0o022, 0, "{shown} is writable by others");
        } else {
            assert_eq!(metadata.mode() & 0o077, 0, "{shown} is not private");
        }
        copies += usize::from(std::fs::read(&path).unwrap() == PUBLIC.as_bytes());
    }
    copies
}

/// The test process keeps the usual 022 umask, the Location and a captured owner
/// file are world-readable, and the agent's write and the reviewed restore move
/// bytes in and out of storage; nothing in it may become readable by others.
#[test]
fn non_git_snapshot_storage_is_owner_only_after_capture_and_restore() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    std::fs::remove_dir_all(env.directory.join(".git")).unwrap();
    std::fs::set_permissions(&env.directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    let public = env.directory.join("public.txt");
    std::fs::write(&public, PUBLIC).unwrap();
    std::fs::set_permissions(&public, std::fs::Permissions::from_mode(0o644)).unwrap();
    let id = env.exec(None, "WRITE-A");
    let first = env.user_ids(&id)[0].clone();
    assert!(
        assert_private_storage(&env) >= 1,
        "the world-readable owner file was captured"
    );
    let preview = rewind_preview(&env, &id, &first, "both");
    json(&confirm_rewind(&env, &id, &first, "both", &preview));
    assert_eq!(env.file("a.txt"), None);
    assert_private_storage(&env);
    // Restored owner bytes keep their own recorded mode; storage privacy never
    // narrows or widens a source file.
    assert_eq!(std::fs::metadata(&public).unwrap().mode() & 0o777, 0o644);
    assert_eq!(env.file("public.txt").as_deref(), Some(PUBLIC));
}

/// Storage that someone widened is refused, never written to or read from: code
/// rewind becomes visibly unavailable while conversation work and conversation
/// rewind continue, and no new snapshot bytes appear in the widened directory.
#[test]
fn widened_snapshot_storage_is_refused_and_conversation_work_continues() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    std::fs::remove_dir_all(env.directory.join(".git")).unwrap();
    let id = env.exec(None, "WRITE-A");
    let global = env.root.join("cyber-home/data/snapshot/global");
    let before: Vec<PathBuf> = entries(&global).into_iter().map(|(p, _)| p).collect();
    std::fs::set_permissions(&global, std::fs::Permissions::from_mode(0o755)).unwrap();
    env.exec(Some(&id), "WRITE-B");
    assert_eq!(env.file("b.txt").as_deref(), Some("beta\n"));
    let after: Vec<PathBuf> = entries(&global).into_iter().map(|(p, _)| p).collect();
    assert_eq!(after, before, "nothing was written to widened storage");
    let users = env.user_ids(&id);
    let refused = exits(
        &env.cyber(&["sessions", "rewind", &id, "--to", &users[1], "--target", "code"]),
        2,
    );
    assert!(refused.contains("not private"), "{refused}");
    let requests = model.requests().len();
    let preview = rewind_preview(&env, &id, &users[1], "conversation");
    assert_eq!(preview["code"], Value::Null);
    json(&confirm_rewind(&env, &id, &users[1], "conversation", &preview));
    assert_eq!(env.user_ids(&id), users[..1]);
    assert_eq!(env.file("b.txt").as_deref(), Some("beta\n"));
    assert_eq!(model.requests().len(), requests);
    std::fs::set_permissions(&global, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_private_storage(&env);
}
