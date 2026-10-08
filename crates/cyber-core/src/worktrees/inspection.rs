//! Read-only engine for the owned inspection subprocess; never called by the manager.

use std::io::{self, Read};
use std::path::{Component, PathBuf};

use serde::{Deserialize, Serialize};

use super::repository::linked_git_directory;
use super::{ChangedFile, WorktreeStatus};

pub const OVERRIDES: &str = "[core]\nlongpaths = true\nhooksPath =\nfsmonitor = false\n";
pub const MAX_RESPONSE: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Status,
    Changes,
    Ignored,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub metadata: PathBuf,
    pub common_dir: PathBuf,
    pub worktree: PathBuf,
    pub branch: String,
    pub base: String,
    pub operation: Operation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub target: Target,
    pub overrides: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Inspection {
    pub target: Target,
    pub head: String,
    pub status: WorktreeStatus,
    pub files: Vec<ChangedFile>,
    pub untracked: Vec<PathBuf>,
    pub ignored: Vec<PathBuf>,
}

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
fn git_error(error: git2::Error) -> io::Error {
    invalid(error)
}

fn open(request: &Request) -> io::Result<git2::Repository> {
    let target = &request.target;
    if target.worktree.canonicalize()? != target.worktree
        || linked_git_directory(&target.worktree)? != target.metadata
    {
        return Err(invalid("Inspection worktree identity changed"));
    }
    let repo = git2::Repository::open_bare(&target.metadata).map_err(git_error)?;
    if repo.path().canonicalize()? != target.metadata
        || repo.commondir().canonicalize()? != target.common_dir
    {
        return Err(invalid("Inspection repository identity changed"));
    }
    let mut contents = String::new();
    std::fs::File::open(&request.overrides)?
        .take(4096)
        .read_to_string(&mut contents)?;
    if contents != OVERRIDES {
        return Err(invalid(
            "Inspection overrides are not the fixed read-only policy",
        ));
    }
    let mut config = git2::Config::new().map_err(git_error)?;
    config
        .add_file(
            &target.common_dir.join("config"),
            git2::ConfigLevel::Local,
            false,
        )
        .map_err(git_error)?;
    let worktree_config = match config.get_bool("extensions.worktreeconfig") {
        Ok(enabled) => enabled,
        Err(error) if error.code() == git2::ErrorCode::NotFound => false,
        Err(error) => return Err(git_error(error)),
    };
    if worktree_config {
        let path = target.metadata.join("config.worktree");
        if path.exists() {
            config
                .add_file(&path, git2::ConfigLevel::Worktree, false)
                .map_err(git_error)?;
        }
    }
    config
        .add_file(&request.overrides, git2::ConfigLevel::App, false)
        .map_err(git_error)?;
    repo.set_config(&config).map_err(git_error)?;
    repo.set_workdir(&target.worktree, false)
        .map_err(git_error)?;
    Ok(repo)
}

fn head(repo: &git2::Repository, branch: &str) -> io::Result<git2::Oid> {
    let reference = repo.head().map_err(git_error)?;
    if reference.name().map_err(git_error)? != format!("refs/heads/{branch}") {
        return Err(invalid("Inspection branch identity changed"));
    }
    reference
        .target()
        .ok_or_else(|| invalid("Inspection HEAD is unresolved"))
}

fn relative(bytes: &[u8]) -> io::Result<PathBuf> {
    let path = PathBuf::from(std::str::from_utf8(bytes).map_err(invalid)?);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid("Inspection returned an unsafe repository path"));
    }
    Ok(path)
}

fn changes(repo: &git2::Repository, base: git2::Oid) -> io::Result<Vec<ChangedFile>> {
    let tree = repo
        .find_commit(base)
        .map_err(git_error)?
        .tree()
        .map_err(git_error)?;
    let mut options = git2::DiffOptions::new();
    options.include_typechange(true);
    let diff = repo
        .diff_tree_to_workdir_with_index(Some(&tree), Some(&mut options))
        .map_err(git_error)?;
    let mut files = Vec::new();
    for (index, delta) in diff.deltas().enumerate() {
        let bytes = delta
            .new_file()
            .path_bytes()
            .or_else(|| delta.old_file().path_bytes())
            .ok_or_else(|| invalid("Inspection diff has no path"))?;
        let path = relative(bytes)?;
        let stats = git2::Patch::from_diff(&diff, index)
            .map_err(git_error)?
            .filter(|_| !delta.new_file().is_binary() && !delta.old_file().is_binary())
            .map(|patch| patch.line_stats())
            .transpose()
            .map_err(git_error)?;
        files.push(ChangedFile {
            file: path
                .to_str()
                .ok_or_else(|| invalid("Inspection diff path is not Unicode"))?
                .into(),
            additions: stats.map(|(_, added, _)| added as u64),
            deletions: stats.map(|(_, _, deleted)| deleted as u64),
        });
    }
    Ok(files)
}

pub fn read(request: &Request) -> io::Result<Inspection> {
    let target = &request.target;
    let repo = open(request)?;
    let initial = head(&repo, &target.branch)?;
    let base = git2::Oid::from_str(&target.base).map_err(git_error)?;
    let (ahead, behind) = repo.graph_ahead_behind(initial, base).map_err(git_error)?;
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .update_index(false)
        .no_refresh(true)
        .include_ignored(target.operation != Operation::Status)
        .recurse_ignored_dirs(true);
    let statuses = repo.statuses(Some(&mut options)).map_err(git_error)?;
    let mut result = Inspection {
        target: target.clone(),
        head: initial.to_string(),
        status: WorktreeStatus {
            dirty: false,
            ahead: ahead as u64,
            behind: behind as u64,
        },
        files: Vec::new(),
        untracked: Vec::new(),
        ignored: Vec::new(),
    };
    for entry in statuses.iter() {
        let status = entry.status();
        let path = relative(entry.path_bytes())?;
        if status.is_ignored() {
            result.ignored.push(path);
        } else if !status.is_empty() {
            result.status.dirty = true;
            if status.is_wt_new() {
                result.untracked.push(path);
            }
        }
    }
    if target.operation == Operation::Changes {
        result.files = changes(&repo, base)?;
    }
    if target.worktree.canonicalize()? != target.worktree
        || linked_git_directory(&target.worktree)? != target.metadata
    {
        return Err(invalid("Inspection worktree identity changed during read"));
    }
    if head(&repo, &target.branch)? != initial {
        return Err(invalid("Inspection HEAD changed during read"));
    }
    Ok(result)
}

pub(super) fn required(managed: &super::Managed) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        super::repository::git_path_argument(&managed.path)
            .as_os_str()
            .encode_wide()
            .count()
            >= 240
    }
    #[cfg(not(windows))]
    {
        let _ = managed;
        false
    }
}

pub(super) async fn inspect(
    execution: &dyn super::GitExecution,
    managed: &super::Managed,
    operation: Operation,
) -> io::Result<Inspection> {
    let target = Target {
        metadata: linked_git_directory(&managed.path)?,
        common_dir: managed.common_dir.clone(),
        worktree: managed.path.clone(),
        branch: managed.branch.clone(),
        base: managed.base.clone(),
        operation,
    };
    let report = execution.inspect(&target).await?;
    if report.target != target {
        return Err(invalid("Owned inspection returned a foreign repository"));
    }
    Ok(report)
}
