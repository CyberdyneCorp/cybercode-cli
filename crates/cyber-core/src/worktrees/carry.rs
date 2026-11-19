//! Uncommitted work carried into an isolated child checkout (`worktrees` → Subagent
//! isolation): the parent checkout's staged, unstaged and untracked non-ignored changes
//! become one commit on the child's branch, which is the child's base.
use std::collections::HashSet;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use cap_std::fs::Dir;

use super::GitExecution;
use super::includes::{copy_file, paths};
use super::repository::git;

/// The default `worktrees.carry_limit_mb`.
pub const DEFAULT_CARRY_LIMIT_MB: u64 = 100;

/// Paths staged per `git add` call, well below any command-line limit.
const ADD_BATCH: usize = 200;

/// What a new child checkout takes over from its parent checkout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CarryPlan {
    /// The parent checkout's `HEAD`; nothing is carried onto another base.
    pub head: String,
    /// Changed, added or untracked regular files, relative to the checkout root.
    pub files: Vec<PathBuf>,
    /// Tracked files the parent checkout deleted (a rename is a deletion plus a file).
    pub deleted: Vec<PathBuf>,
    /// Files left behind: the Session may not read them, or they are not regular files.
    pub skipped: Vec<PathBuf>,
    /// The total size of `files`.
    pub bytes: u64,
}

impl CarryPlan {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.deleted.is_empty()
    }
}

/// Plan what the checkout at `source` would carry. `readable` admits a path the Session
/// may read; ignored files are never listed. Fails when the files exceed `limit_mb`.
/// `None` when the checkout has no commit yet.
pub async fn plan(
    execution: &dyn GitExecution,
    source: &Path,
    readable: &(dyn Fn(&Path) -> bool + Sync),
    limit_mb: u64,
) -> io::Result<Option<CarryPlan>> {
    let Ok(head) = git(execution, source, &args(&["rev-parse", "--verify", "HEAD"])).await else {
        return Ok(None);
    };
    let head = String::from_utf8(head)
        .map_err(|error| io::Error::other(error.to_string()))?
        .trim()
        .to_owned();
    let changed = |filter: &str| {
        args(&[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-relative",
            "--ignore-submodules=all",
            filter,
            "HEAD",
            "--",
        ])
    };
    let deleted = paths(git(execution, source, &changed("--diff-filter=D")).await?)?;
    let mut candidates = paths(git(execution, source, &changed("--diff-filter=d")).await?)?;
    candidates.extend(paths(
        git(
            execution,
            source,
            &args(&["ls-files", "--others", "--exclude-standard", "-z"]),
        )
        .await?,
    )?);
    let root = Dir::open_ambient_dir(source, cap_std::ambient_authority())?;
    let mut plan = CarryPlan {
        head,
        deleted,
        ..CarryPlan::default()
    };
    for path in candidates {
        let regular = root
            .symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.is_file());
        match regular {
            Some(metadata) if readable(&path) => {
                plan.bytes += metadata.len();
                plan.files.push(path);
            }
            _ => plan.skipped.push(path),
        }
    }
    let limit = limit_mb.saturating_mul(1024 * 1024);
    if plan.bytes > limit {
        return Err(io::Error::other(format!(
            "Uncommitted changes to carry into the isolated worktree total {} MiB, over the {limit_mb} MiB limit (worktrees.carry_limit_mb); commit, ignore or remove large files, or raise the limit",
            plan.bytes.div_ceil(1024 * 1024)
        )));
    }
    Ok(Some(plan))
}

/// Apply `plan` to the new checkout at `destination` on `base` and commit it there;
/// returns the commit, the checkout's new base. Paths `included` by `.worktreeinclude`
/// keep their included copy. Nothing happens when the checkout is not on the plan's `HEAD`.
pub(super) async fn apply(
    execution: &dyn GitExecution,
    plan: &CarryPlan,
    source: &Path,
    destination: &Path,
    base: &str,
    included: &HashSet<PathBuf>,
) -> io::Result<Option<String>> {
    if plan.head != base || plan.is_empty() {
        return Ok(None);
    }
    let source_dir = Dir::open_ambient_dir(source, cap_std::ambient_authority())?;
    let target = Dir::open_ambient_dir(destination, cap_std::ambient_authority())?;
    for path in &plan.deleted {
        remove(&target, path)?;
    }
    let mut staged = plan.deleted.clone();
    for path in plan.files.iter().filter(|path| !included.contains(*path)) {
        remove(&target, path)?;
        copy_file(&source_dir, &target, path)?;
        staged.push(path.clone());
    }
    for batch in staged.chunks(ADD_BATCH) {
        let mut add = args(&["--literal-pathspecs", "add", "--all", "--"]);
        add.extend(batch.iter().map(|path| path.as_os_str().to_owned()));
        git(execution, destination, &add).await?;
    }
    let tree = text(git(execution, destination, &args(&["write-tree"])).await?)?;
    let commit = text(
        git(
            execution,
            destination,
            &args(&[
                "-c",
                "user.name=Cyber",
                "-c",
                "user.email=cyber@localhost",
                "commit-tree",
                &tree,
                "-p",
                base,
                "-m",
                "Carry uncommitted changes from the parent checkout",
            ]),
        )
        .await?,
    )?;
    git(
        execution,
        destination,
        &args(&["update-ref", "-m", "carry", "HEAD", &commit, base]),
    )
    .await?;
    Ok(Some(commit))
}

/// Remove a file of the base checkout before it is replaced or deleted.
fn remove(target: &Dir, path: &Path) -> io::Result<()> {
    match target.symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Err(io::Error::other(format!(
            "Cannot carry {}: the new checkout has a directory there",
            path.display()
        ))),
        Ok(_) => target.remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn text(bytes: Vec<u8>) -> io::Result<String> {
    Ok(String::from_utf8(bytes)
        .map_err(|error| io::Error::other(error.to_string()))?
        .trim()
        .to_owned())
}
