//! Conflict-aware restore (`snapshots-checkpoints` → Conflict-aware code restore).
//!
//! Every path that differs between the recorded tree (the last state the system saw) and
//! the target is planned before anything is written: unchanged files are replaced, files
//! edited since are three-way merged, and anything ambiguous fails the whole restore.

use std::path::{Component, Path, PathBuf};

use cyber_server::runtime::RestoreError;

use crate::git::Shadow;

const SYMLINK: &str = "120000";

/// What a path should become.
#[derive(Debug, Clone, PartialEq)]
enum Content {
    Absent,
    File { bytes: Vec<u8>, executable: bool },
    Link(PathBuf),
}

struct Planned {
    path: String,
    /// The bytes seen at planning time, rechecked just before replacement.
    seen: Content,
    next: Content,
}

pub(crate) async fn restore(
    shadow: &Shadow,
    target: &str,
    recorded: &str,
    backups: &Path,
) -> Result<Vec<String>, RestoreError> {
    let paths = shadow
        .changed(recorded, target)
        .await
        .map_err(RestoreError::Failed)?;
    let mut plan = Vec::new();
    let mut conflicts = Vec::new();
    for path in paths {
        let want = content(shadow, target, &path).await?;
        let base = content(shadow, recorded, &path).await?;
        let current = on_disk(&shadow.worktree.join(&path));
        match decide(shadow, &path, &base, &current, &want).await? {
            Some(next) if next != current => plan.push(Planned {
                path,
                seen: current,
                next,
            }),
            Some(_) => {}
            None => conflicts.push(path),
        }
    }
    if !conflicts.is_empty() {
        return Err(RestoreError::Conflict(conflicts));
    }
    apply(shadow, plan, backups)
}

async fn content(shadow: &Shadow, tree: &str, path: &str) -> Result<Content, RestoreError> {
    Ok(
        match shadow
            .blob(tree, path)
            .await
            .map_err(RestoreError::Failed)?
        {
            None => Content::Absent,
            Some((mode, bytes)) if mode == SYMLINK => {
                Content::Link(PathBuf::from(String::from_utf8_lossy(&bytes).to_string()))
            }
            Some((mode, bytes)) => Content::File {
                bytes,
                executable: mode == "100755",
            },
        },
    )
}

fn on_disk(path: &Path) -> Content {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Content::Absent;
    };
    if meta.file_type().is_symlink() {
        return std::fs::read_link(path).map_or(Content::Absent, Content::Link);
    }
    match std::fs::read(path) {
        Ok(bytes) => Content::File {
            bytes,
            executable: is_executable(&meta),
        },
        Err(_) => Content::Absent,
    }
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

/// The content to write, or `None` for a conflict.
async fn decide(
    shadow: &Shadow,
    path: &str,
    base: &Content,
    current: &Content,
    want: &Content,
) -> Result<Option<Content>, RestoreError> {
    if same(current, want) {
        return Ok(Some(current.clone()));
    }
    if same(current, base) {
        return Ok(stays_inside(shadow, path, want).then(|| want.clone()));
    }
    // Edited since the system last saw it: merge when all three are regular files.
    let (
        Content::File {
            bytes: ours,
            executable,
        },
        Content::File { bytes: b, .. },
        Content::File { bytes: theirs, .. },
    ) = (current, base, want)
    else {
        return Ok(None);
    };
    Ok(merge(shadow, ours, b, theirs)
        .await?
        .map(|bytes| Content::File {
            bytes,
            executable: *executable,
        }))
}

/// Same content, ignoring the executable bit on the current file.
fn same(a: &Content, b: &Content) -> bool {
    match (a, b) {
        (Content::File { bytes: x, .. }, Content::File { bytes: y, .. }) => x == y,
        _ => a == b,
    }
}

/// A restored symlink must not point outside the worktree.
fn stays_inside(shadow: &Shadow, path: &str, want: &Content) -> bool {
    let Content::Link(target) = want else {
        return true;
    };
    let parent = shadow
        .worktree
        .join(path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut resolved = PathBuf::new();
    for part in parent.join(target).components() {
        match part {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    resolved.starts_with(&shadow.worktree)
}

async fn merge(
    shadow: &Shadow,
    ours: &[u8],
    base: &[u8],
    theirs: &[u8],
) -> Result<Option<Vec<u8>>, RestoreError> {
    let dir = shadow.git_dir.join("merge-tmp");
    let fail = |e: std::io::Error| RestoreError::Failed(e.to_string());
    std::fs::create_dir_all(&dir).map_err(fail)?;
    let files = [("ours", ours), ("base", base), ("theirs", theirs)]
        .map(|(name, bytes)| (dir.join(name), bytes));
    for (path, bytes) in &files {
        std::fs::write(path, bytes).map_err(fail)?;
    }
    let out = tokio::process::Command::new("git")
        .args(["merge-file", "-p", "--quiet"])
        .args(files.iter().map(|(p, _)| p))
        .output()
        .await
        .map_err(fail)?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok((out.status.code() == Some(0)).then_some(out.stdout))
}

fn apply(shadow: &Shadow, plan: Vec<Planned>, backups: &Path) -> Result<Vec<String>, RestoreError> {
    // Recheck immediately before replacing: a file that moved since planning is a race.
    let raced: Vec<String> = plan
        .iter()
        .filter(|p| on_disk(&shadow.worktree.join(&p.path)) != p.seen)
        .map(|p| p.path.clone())
        .collect();
    if !raced.is_empty() {
        return Err(RestoreError::Conflict(raced));
    }
    let mut restored = Vec::new();
    for item in plan {
        let path = shadow.worktree.join(&item.path);
        backup(&item.seen, &backups.join(&item.path))
            .map_err(|e| RestoreError::Failed(format!("backup of {}: {e}", item.path)))?;
        write(&path, &item.next)
            .map_err(|e| RestoreError::Failed(format!("{}: {e}", item.path)))?;
        restored.push(item.path);
    }
    Ok(restored)
}

fn backup(seen: &Content, to: &Path) -> std::io::Result<()> {
    if let Content::File { bytes, .. } = seen {
        std::fs::create_dir_all(to.parent().unwrap_or(to))?;
        std::fs::write(to, bytes)?;
    }
    Ok(())
}

fn write(path: &Path, next: &Content) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path).is_ok() {
        std::fs::remove_file(path)?;
    }
    match next {
        Content::Absent => Ok(()),
        Content::File { bytes, executable } => {
            std::fs::create_dir_all(path.parent().unwrap_or(path))?;
            std::fs::write(path, bytes)?;
            set_executable(path, *executable)
        }
        Content::Link(target) => {
            std::fs::create_dir_all(path.parent().unwrap_or(path))?;
            link(target, path)
        }
    }
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    let mode = perms.mode();
    perms.set_mode(if executable {
        mode | 0o111
    } else {
        mode & !0o111
    });
    std::fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn link(target: &Path, path: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, path)
}

#[cfg(not(unix))]
fn link(_target: &Path, _path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "symlinks are not supported on this platform",
    ))
}
