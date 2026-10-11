//! Read-only git2 engine run only by the sandboxed `--git-review` helper.
//!
//! libgit2 never runs hooks, `diff.external`, textconv, external filter
//! drivers, fsmonitor or a pager, and revisions are resolved in-process, so no
//! user text reaches a process argument.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ErrorCode, Target, TargetError, escape};
use crate::worktrees::inspection::{read_only_config, relative};
use crate::worktrees::repository::{linked_git_directory, read_pointer};

/// Total patch text handed to the reviewer, spent by the host after it
/// withholds read-denied files (see [`ReviewDiff::limit_patch`]).
pub const MAX_PATCH: usize = 256 * 1024;
/// Total patch text the engine returns; each file is also capped at
/// [`MAX_PATCH`], so a withheld file cannot spend another file's share.
pub const MAX_ENGINE_PATCH: usize = 4 * MAX_PATCH;
/// Bytes of the user's global excludes file applied to untracked files.
pub const MAX_EXCLUDES: usize = 64 * 1024;
/// Changed files listed before the remainder is summarized in limitations.
pub const MAX_FILES: usize = 2000;
/// Upper bound on the helper response, allowing for JSON escaping of the patch.
pub const MAX_RESPONSE: usize = 8 * 1024 * 1024;
/// Upper bound on the encoded helper request.
pub const MAX_REQUEST: usize = 8192;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub location: PathBuf,
    pub target: Target,
    pub overrides: PathBuf,
    /// Diff a range from the merge base of its ends, as a pull request does.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub merge_base: bool,
    /// A host-made copy of the user's global `core.excludesFile`, which the
    /// isolated engine cannot discover, so untracked files match `git status`.
    /// Ignored when the repository sets its own `core.excludesFile`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excludes: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub additions: u64,
    pub deletions: u64,
    pub binary: bool,
    /// Unified patch for this file; kept per file so the host can withhold
    /// read-denied paths without re-running the engine.
    pub patch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReviewDiff {
    pub target: Target,
    /// Canonical repository work tree root.
    pub root: PathBuf,
    /// Base commit; `None` means the empty tree.
    pub base: Option<String>,
    /// Head commit; `None` means the working tree with its index.
    pub head: Option<String>,
    pub files: Vec<ChangedFile>,
    pub limitations: Vec<String>,
    /// SHA-256 of [`ReviewDiff::patch`].
    pub patch_sha256: String,
}

impl ReviewDiff {
    /// Concatenated per-file patches, in path order.
    pub fn patch(&self) -> String {
        self.files
            .iter()
            .filter_map(|file| file.patch.as_deref())
            .collect()
    }

    /// Spends the reviewer's shared [`MAX_PATCH`] budget over the remaining
    /// patches in path order and refreshes [`ReviewDiff::patch_sha256`].
    pub fn limit_patch(&mut self) {
        let mut budget = MAX_PATCH;
        for file in &mut self.files {
            if let Some(patch) = file.patch.take() {
                file.patch = take_budget(&patch, &mut budget, &file.path, &mut self.limitations);
            }
        }
        self.patch_sha256 = format!("{:x}", Sha256::digest(self.patch()));
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error(transparent)]
    Target(#[from] TargetError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

fn git_error(error: git2::Error) -> io::Error {
    invalid(error)
}

fn not_a_repository() -> TargetError {
    TargetError::new(
        ErrorCode::NotARepository,
        "the location is not inside a Git work tree",
    )
}

pub fn resolve(request: &Request) -> Result<ReviewDiff, ResolveError> {
    let (repo, root) = open(request)?;
    let mut limitations = Vec::new();
    let (diff, base, head) = diff(&repo, request, &mut limitations)?;
    let files = files(&diff, &mut limitations)?;
    let mut result = ReviewDiff {
        target: request.target.clone(),
        root,
        base: base.map(|oid| oid.to_string()),
        head: head.map(|oid| oid.to_string()),
        files,
        limitations,
        patch_sha256: String::new(),
    };
    result.patch_sha256 = format!("{:x}", Sha256::digest(result.patch()));
    Ok(result)
}

fn open(request: &Request) -> Result<(git2::Repository, PathBuf), ResolveError> {
    let repo = git2::Repository::discover(&request.location).map_err(|_| not_a_repository())?;
    let root = repo
        .workdir()
        .ok_or_else(not_a_repository)?
        .canonicalize()?;
    let git_dir = repo.path().canonicalize()?;
    if marker_target(&root, &git_dir)? != git_dir {
        return Err(invalid("Review repository identity changed").into());
    }
    let config = read_only_config(
        &repo.commondir().canonicalize()?,
        &git_dir,
        &request.overrides,
    )?;
    repo.set_config(&config).map_err(git_error)?;
    // As in git, the repository's own `core.excludesFile` replaces the global one.
    let local_excludes = config
        .get_string("core.excludesfile")
        .is_ok_and(|value| !value.is_empty());
    if let Some(path) = request.excludes.as_ref().filter(|_| !local_excludes) {
        repo.add_ignore_rule(&excludes(path)?).map_err(git_error)?;
    }
    Ok((repo, root))
}

/// The user's global excludes file as `git status` finds it: `core.excludesFile`
/// from `$XDG_CONFIG_HOME/git/config` or `~/.gitconfig` (the latter wins, `~/`
/// expanded), else `$XDG_CONFIG_HOME/git/ignore`, where `XDG_CONFIG_HOME`
/// defaults to `~/.config`. Only configuration is parsed; nothing runs.
pub fn global_excludes(var: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |key: &str| {
        var(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let home = set("HOME").or_else(|| set("USERPROFILE"));
    let xdg = set("XDG_CONFIG_HOME").or_else(|| home.as_ref().map(|home| home.join(".config")));
    let configured = configured_excludes(
        xdg.as_ref().map(|xdg| xdg.join("git/config")),
        home.as_ref().map(|home| home.join(".gitconfig")),
    );
    let path = match configured {
        Some(value) => match (value.strip_prefix("~/"), &home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => PathBuf::from(value),
        },
        None => xdg?.join("git/ignore"),
    };
    (path.is_absolute() && path.is_file()).then_some(path)
}

fn configured_excludes(xdg: Option<PathBuf>, global: Option<PathBuf>) -> Option<String> {
    let mut config = git2::Config::new().ok()?;
    for (path, level) in [
        (xdg, git2::ConfigLevel::XDG),
        (global, git2::ConfigLevel::Global),
    ] {
        if let Some(path) = path.filter(|path| path.is_file()) {
            config.add_file(&path, level, false).ok()?;
        }
    }
    config
        .get_string("core.excludesfile")
        .ok()
        .filter(|value| !value.is_empty())
}

fn excludes(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_EXCLUDES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_EXCLUDES {
        return Err(invalid("Review excludes file exceeds supported size"));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Resolves the work tree's `.git` marker: a directory, a linked worktree file
/// (with its backpointer verified), or a submodule pointer file.
fn marker_target(root: &Path, git_dir: &Path) -> io::Result<PathBuf> {
    let marker = root.join(".git");
    if std::fs::symlink_metadata(&marker)?.is_dir() {
        return marker.canonicalize();
    }
    if git_dir.join("commondir").is_file() {
        return linked_git_directory(root);
    }
    let pointer = read_pointer(&marker)?;
    let pointer = pointer
        .strip_prefix("gitdir: ")
        .ok_or_else(|| invalid("Invalid Git marker"))?;
    root.join(pointer).canonicalize()
}

fn commit<'r>(repo: &'r git2::Repository, revision: &str) -> Result<git2::Commit<'r>, TargetError> {
    repo.revparse_single(revision)
        .and_then(|object| object.peel_to_commit())
        .map_err(|_| {
            TargetError::new(
                ErrorCode::UnresolvedRef,
                format!("cannot resolve {} to a commit", escape(revision)),
            )
        })
}

fn head_commit(repo: &git2::Repository) -> Result<Option<git2::Commit<'_>>, ResolveError> {
    match repo.head() {
        Ok(reference) => Ok(Some(reference.peel_to_commit().map_err(git_error)?)),
        Err(error)
            if matches!(
                error.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(git_error(error).into()),
    }
}

fn tree<'r>(commit: Option<&git2::Commit<'r>>) -> Result<Option<git2::Tree<'r>>, io::Error> {
    commit
        .map(|commit| commit.tree().map_err(git_error))
        .transpose()
}

type Resolved<'r> = (git2::Diff<'r>, Option<git2::Oid>, Option<git2::Oid>);

/// The commit a pull request forked from: the merge base of its ends.
fn fork_point<'r>(
    repo: &'r git2::Repository,
    base: &git2::Commit<'r>,
    head: &git2::Commit<'r>,
) -> Result<git2::Commit<'r>, TargetError> {
    repo.merge_base(base.id(), head.id())
        .and_then(|oid| repo.find_commit(oid))
        .map_err(|_| {
            TargetError::new(
                ErrorCode::UnresolvedRef,
                format!("{} and {} share no common ancestor", base.id(), head.id()),
            )
        })
}

fn diff<'r>(
    repo: &'r git2::Repository,
    request: &Request,
    limitations: &mut Vec<String>,
) -> Result<Resolved<'r>, ResolveError> {
    let mut options = git2::DiffOptions::new();
    options.include_typechange(true);
    let (base, head) = match &request.target {
        Target::Uncommitted => {
            let base = head_commit(repo)?;
            options
                .include_untracked(true)
                .recurse_untracked_dirs(true)
                .show_untracked_content(true);
            let diff = repo
                .diff_tree_to_workdir_with_index(tree(base.as_ref())?.as_ref(), Some(&mut options))
                .map_err(git_error)?;
            return finish(diff, base.map(|commit| commit.id()), None);
        }
        Target::Range { base, head } => {
            let (base, head) = (commit(repo, base)?, commit(repo, head)?);
            let base = match request.merge_base {
                true => fork_point(repo, &base, &head)?,
                false => base,
            };
            (Some(base), head)
        }
        Target::Commit(revision) => {
            let head = commit(repo, revision)?;
            if head.parent_count() > 1 {
                limitations.push(format!(
                    "merge commit {} reviewed against its first parent only",
                    head.id()
                ));
            }
            (head.parents().next(), head)
        }
        Target::Pr(_) => {
            return Err(TargetError::new(
                ErrorCode::InvalidTarget,
                "pull request targets are resolved to commits by the host",
            )
            .into());
        }
    };
    let diff = repo
        .diff_tree_to_tree(
            tree(base.as_ref())?.as_ref(),
            tree(Some(&head))?.as_ref(),
            Some(&mut options),
        )
        .map_err(git_error)?;
    finish(diff, base.map(|commit| commit.id()), Some(head.id()))
}

fn finish(
    mut diff: git2::Diff<'_>,
    base: Option<git2::Oid>,
    head: Option<git2::Oid>,
) -> Result<Resolved<'_>, ResolveError> {
    diff.find_similar(None).map_err(git_error)?;
    Ok((diff, base, head))
}

fn files(diff: &git2::Diff<'_>, limitations: &mut Vec<String>) -> io::Result<Vec<ChangedFile>> {
    let mut budget = MAX_ENGINE_PATCH;
    let mut files = Vec::new();
    let total = diff.deltas().len();
    for index in 0..total.min(MAX_FILES) {
        files.push(file(diff, index, &mut budget, limitations)?);
    }
    if total > MAX_FILES {
        limitations.push(format!("{} more changed files omitted", total - MAX_FILES));
    }
    Ok(files)
}

fn repository_path(file: git2::DiffFile<'_>) -> io::Result<Option<String>> {
    file.path_bytes()
        .map(|bytes| {
            let path = relative(bytes)?;
            path.to_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid("Review path is not Unicode"))
        })
        .transpose()
}

fn status(delta: git2::Delta) -> FileStatus {
    match delta {
        git2::Delta::Added | git2::Delta::Untracked => FileStatus::Added,
        git2::Delta::Deleted => FileStatus::Deleted,
        git2::Delta::Renamed => FileStatus::Renamed,
        git2::Delta::Copied => FileStatus::Copied,
        git2::Delta::Typechange => FileStatus::TypeChanged,
        _ => FileStatus::Modified,
    }
}

fn file(
    diff: &git2::Diff<'_>,
    index: usize,
    budget: &mut usize,
    limitations: &mut Vec<String>,
) -> io::Result<ChangedFile> {
    let delta = diff
        .get_delta(index)
        .ok_or_else(|| invalid("Review diff delta disappeared"))?;
    let new = repository_path(delta.new_file())?;
    let old = repository_path(delta.old_file())?;
    let path = new
        .clone()
        .or_else(|| old.clone())
        .ok_or_else(|| invalid("Review diff has no path"))?;
    let status = status(delta.status());
    let mut file = ChangedFile {
        old_path: old.filter(|old| *old != path),
        path,
        status,
        additions: 0,
        deletions: 0,
        binary: true,
        patch: None,
    };
    let patch = git2::Patch::from_diff(diff, index).map_err(git_error)?;
    match patch {
        Some(mut patch) if !patch.delta().flags().is_binary() => {
            let (_, additions, deletions) = patch.line_stats().map_err(git_error)?;
            let text = patch.to_buf().map_err(git_error)?;
            file.binary = false;
            file.additions = additions as u64;
            file.deletions = deletions as u64;
            file.patch = engine_patch(
                &String::from_utf8_lossy(&text),
                budget,
                &file.path,
                limitations,
            );
        }
        _ => limitations.push(format!("binary file {} not shown", escape(&file.path))),
    }
    Ok(file)
}

/// Caps one file at [`MAX_PATCH`] and spends that from the engine's total.
fn engine_patch(
    text: &str,
    budget: &mut usize,
    path: &str,
    limitations: &mut Vec<String>,
) -> Option<String> {
    let granted = (*budget).min(MAX_PATCH);
    let mut left = granted;
    let patch = take_budget(text, &mut left, path, limitations);
    *budget -= granted - left;
    patch
}

/// Spends the shared patch budget, cutting at a line boundary when exhausted.
fn take_budget(
    text: &str,
    budget: &mut usize,
    path: &str,
    limitations: &mut Vec<String>,
) -> Option<String> {
    if text.len() <= *budget {
        *budget -= text.len();
        return Some(text.to_owned());
    }
    let mut cut = (*budget).min(text.len());
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let cut = text[..cut].rfind('\n').map_or(0, |end| end + 1);
    *budget = 0;
    limitations.push(format!(
        "patch for {} truncated at the {} KiB review limit",
        escape(path),
        MAX_PATCH / 1024
    ));
    (cut > 0).then(|| text[..cut].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_cuts_at_line_boundaries_and_names_the_file() {
        let mut limitations = Vec::new();
        let mut budget = 10;
        assert_eq!(
            take_budget("abc\n", &mut budget, "a", &mut limitations).as_deref(),
            Some("abc\n")
        );
        assert_eq!(
            take_budget("1234\n5678\n", &mut budget, "b", &mut limitations).as_deref(),
            Some("1234\n")
        );
        assert_eq!(budget, 0);
        assert_eq!(take_budget("x\n", &mut budget, "c", &mut limitations), None);
        assert_eq!(limitations.len(), 2);
        assert!(limitations[0].contains(" b "), "{limitations:?}");
        assert!(limitations[1].contains(" c "), "{limitations:?}");
    }

    fn changed(path: &str, patch: Option<&str>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            old_path: None,
            status: FileStatus::Modified,
            additions: 1,
            deletions: 0,
            binary: patch.is_none(),
            patch: patch.map(str::to_owned),
        }
    }

    #[test]
    fn engine_caps_each_file_so_one_cannot_spend_the_whole_total() {
        let mut limitations = Vec::new();
        let mut budget = MAX_ENGINE_PATCH;
        let big = "x\n".repeat(MAX_PATCH);
        let first = engine_patch(&big, &mut budget, "big", &mut limitations).unwrap();
        assert!(first.len() <= MAX_PATCH);
        assert_eq!(budget, MAX_ENGINE_PATCH - first.len());
        assert_eq!(
            engine_patch("small\n", &mut budget, "small", &mut limitations).as_deref(),
            Some("small\n")
        );
        assert_eq!(limitations.len(), 1, "{limitations:?}");
    }

    /// Regression: the shared budget was spent in the engine, before the host
    /// withheld files, so a large withheld file starved every allowed one.
    #[test]
    fn limit_patch_spends_the_budget_only_on_remaining_patches() {
        let big = "y\n".repeat(MAX_PATCH / 2 + 1);
        let mut diff = ReviewDiff {
            target: Target::Uncommitted,
            root: PathBuf::from("/r"),
            base: None,
            head: None,
            files: vec![
                changed("assets/data.json", None),
                changed("src/main.rs", Some("+main\n")),
                changed("z.txt", Some(&big)),
            ],
            limitations: Vec::new(),
            patch_sha256: String::new(),
        };
        diff.limit_patch();
        assert_eq!(diff.files[1].patch.as_deref(), Some("+main\n"));
        assert!(diff.patch().len() <= MAX_PATCH);
        assert!(diff.limitations.iter().all(|item| item.contains("z.txt")));
        assert_eq!(
            diff.patch_sha256,
            format!("{:x}", Sha256::digest(diff.patch()))
        );
    }

    #[test]
    fn global_excludes_follow_git_lookup_order() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        std::fs::create_dir_all(home.join(".config/git")).unwrap();
        let env = |home: &Path, xdg: Option<&Path>| {
            let home = home.display().to_string();
            let xdg = xdg.map(|xdg| xdg.display().to_string());
            move |key: &str| match key {
                "HOME" => Some(home.clone()),
                "XDG_CONFIG_HOME" => xdg.clone(),
                _ => None,
            }
        };
        assert_eq!(global_excludes(env(&home, None)), None);
        let default = home.join(".config/git/ignore");
        std::fs::write(&default, "*.pem\n").unwrap();
        assert_eq!(global_excludes(env(&home, None)), Some(default.clone()));
        std::fs::write(home.join("custom-ignore"), "venv/\n").unwrap();
        std::fs::write(
            home.join(".gitconfig"),
            "[core]\n\texcludesFile = ~/custom-ignore\n",
        )
        .unwrap();
        assert_eq!(
            global_excludes(env(&home, None)),
            Some(home.join("custom-ignore"))
        );
        std::fs::remove_file(home.join(".gitconfig")).unwrap();
        let xdg = temp.path().join("xdg");
        std::fs::create_dir_all(xdg.join("git")).unwrap();
        std::fs::write(xdg.join("git/ignore"), "*.key\n").unwrap();
        assert_eq!(
            global_excludes(env(&home, Some(&xdg))),
            Some(xdg.join("git/ignore"))
        );
    }

    #[test]
    fn request_rejects_unknown_fields_and_invalid_targets() {
        let valid = r#"{"location":"/r","target":"main..HEAD","overrides":"/o"}"#;
        assert!(serde_json::from_str::<Request>(valid).is_ok());
        for invalid in [
            r#"{"location":"/r","target":"","overrides":"/o","extra":1}"#,
            r#"{"location":"/r","target":"--output=/tmp/x","overrides":"/o"}"#,
            r#"{"location":"/r","overrides":"/o"}"#,
        ] {
            assert!(
                serde_json::from_str::<Request>(invalid).is_err(),
                "{invalid}"
            );
        }
    }
}
