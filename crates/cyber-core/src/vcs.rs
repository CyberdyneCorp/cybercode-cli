//! A Location's git state (`vcs-integration` → Git awareness), read with libgit2 so no
//! `git` process or hook runs.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;

/// Branch, upstream and working-tree state of the repository a Location is in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct GitState {
    /// The worktree root.
    pub root: PathBuf,
    /// The checked-out branch; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// The short commit ID of HEAD; `None` before the first commit.
    pub head: Option<String>,
    /// The upstream branch (`origin/main`), when one is configured and exists.
    pub upstream: Option<String>,
    /// Commits on the branch that the upstream lacks, and the other way round; 0 without
    /// an upstream.
    pub ahead: usize,
    pub behind: usize,
    /// The linked worktree's name; `None` in the main worktree.
    pub worktree: Option<String>,
    /// Staged, unstaged or untracked (not ignored) changes exist.
    pub dirty: bool,
}

/// The git state of `location`, or `None` outside a git repository (or in a bare one).
pub fn status(location: &Path) -> Option<GitState> {
    let repo = git2::Repository::discover(location).ok()?;
    let root = repo.workdir()?.to_path_buf();
    let head = repo.head().ok();
    let branch = head
        .as_ref()
        .filter(|h| h.is_branch())
        .and_then(|h| h.shorthand().ok().map(str::to_owned))
        .or_else(|| unborn_branch(&repo));
    let head_id = head.as_ref().and_then(|h| h.target());
    let upstream = branch.as_deref().and_then(|name| {
        let local = repo.find_branch(name, git2::BranchType::Local).ok()?;
        let upstream = local.upstream().ok()?;
        let id = upstream.get().target()?;
        Some((upstream.name().ok()??.to_owned(), id))
    });
    let (ahead, behind) = match (head_id, &upstream) {
        (Some(local), Some((_, remote))) => repo.graph_ahead_behind(local, *remote).ok()?,
        _ => (0, 0),
    };
    let worktree = repo
        .is_worktree()
        .then(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
        .flatten();
    Some(GitState {
        root,
        branch,
        head: head_id.map(|id| id.to_string()[..7].to_owned()),
        upstream: upstream.map(|(name, _)| name),
        ahead,
        behind,
        worktree,
        dirty: dirty(&repo),
    })
}

/// The branch HEAD names before its first commit.
fn unborn_branch(repo: &git2::Repository) -> Option<String> {
    let head = repo.find_reference("HEAD").ok()?;
    let target = head.symbolic_target().ok()??;
    target.strip_prefix("refs/heads/").map(str::to_owned)
}

fn dirty(repo: &git2::Repository) -> bool {
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .include_ignored(false)
        .exclude_submodules(true);
    repo.statuses(Some(&mut options))
        .is_ok_and(|statuses| !statuses.is_empty())
}

/// The git status letter of each changed path under `root`, keyed by its
/// `/`-separated path relative to `root`: `M` modified, renamed or retyped, `A` staged
/// as new, `?` untracked, `D` deleted. Empty outside git.
pub fn file_statuses(root: &Path) -> std::collections::BTreeMap<String, char> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(repo) = git2::Repository::discover(root) else {
        return out;
    };
    let Some(workdir) = repo.workdir().and_then(|w| std::fs::canonicalize(w).ok()) else {
        return out;
    };
    let Ok(root) = std::fs::canonicalize(root) else {
        return out;
    };
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let Ok(statuses) = repo.statuses(Some(&mut options)) else {
        return out;
    };
    for entry in statuses.iter() {
        let (Ok(path), Some(letter)) = (entry.path(), letter(entry.status())) else {
            continue;
        };
        let absolute = workdir.join(path);
        if let Ok(relative) = absolute.strip_prefix(&root) {
            out.insert(slash_path(relative), letter);
        }
    }
    out
}

fn letter(status: git2::Status) -> Option<char> {
    if status.intersects(git2::Status::INDEX_DELETED | git2::Status::WT_DELETED) {
        Some('D')
    } else if status.contains(git2::Status::INDEX_NEW) {
        Some('A')
    } else if status.contains(git2::Status::WT_NEW) {
        Some('?')
    } else if status.is_empty() || status.contains(git2::Status::IGNORED) {
        None
    } else {
        Some('M')
    }
}

/// A relative path with `/` between its components, on every platform.
pub fn slash_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn outside_git_there_is_no_state() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(status(dir.path()), None);
    }

    #[test]
    fn branch_upstream_ahead_behind_worktree_and_dirty() {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin");
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(root.path(), &["clone", "-q", "origin", "repo"]);
        let fresh = status(&repo).unwrap();
        assert_eq!(fresh.branch.as_deref(), Some("main"));
        assert_eq!(fresh.upstream.as_deref(), Some("origin/main"));
        assert_eq!((fresh.ahead, fresh.behind, fresh.dirty), (0, 0, false));
        assert_eq!(fresh.worktree, None);
        assert_eq!(fresh.head.as_ref().map(String::len), Some(7));

        git(&repo, &["commit", "-q", "--allow-empty", "-m", "two"]);
        git(&origin, &["commit", "-q", "--allow-empty", "-m", "three"]);
        git(&repo, &["fetch", "-q"]);
        std::fs::write(repo.join("new.txt"), "x").unwrap();
        let moved = status(&repo.join(".")).unwrap();
        assert_eq!((moved.ahead, moved.behind, moved.dirty), (1, 1, true));

        let linked = root.path().join("feature-x");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                linked.to_str().unwrap(),
            ],
        );
        let worktree = status(&linked).unwrap();
        assert_eq!(worktree.branch.as_deref(), Some("feature"));
        assert_eq!(worktree.worktree.as_deref(), Some("feature-x"));
        assert_eq!(worktree.upstream, None);
    }

    #[test]
    fn an_unborn_branch_and_a_detached_head() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "trunk"]);
        let unborn = status(dir.path()).unwrap();
        assert_eq!(unborn.branch.as_deref(), Some("trunk"));
        assert_eq!(unborn.head, None);
        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "one"]);
        git(dir.path(), &["checkout", "-q", "--detach"]);
        let detached = status(dir.path()).unwrap();
        assert_eq!(detached.branch, None);
        assert!(detached.head.is_some());
    }
}
