//! `team_merge` (`agent-teams` → Merging teammate work; design section 10): the lead
//! merges a worktree teammate's branch into its own checkout, and conflicts are reported,
//! never resolved.
//!
//! 1. Refused while the lead's checkout has a merge, rebase, cherry-pick or revert of
//!    its own in progress or a detached HEAD ([`BUSY`], [`DETACHED`]), has uncommitted
//!    changes to tracked files (`Commit or stash your changes before merging`), or when
//!    the branch has no commit the lead's HEAD lacks (`Nothing to merge from <name>`).
//! 2. `git merge-tree --write-tree` (git 2.38 or later, else `team_merge needs git 2.38
//!    or later`) decides the merge without touching the checkout; on conflicts the
//!    conflicted paths are returned and nothing else happens.
//! 3. A clean result lists every path it writes ([`plan`]). A path that exists untracked
//!    or ignored in the lead's checkout refuses the merge (`<path> exists untracked in
//!    your checkout`), since `git merge` would overwrite an ignored file silently; the
//!    caller authorizes the paths as edits of the lead.
//! 4. [`apply`] merges with `git merge --no-ff` of the exact commit that was checked,
//!    with hooks disabled, once the checkout is confirmed unchanged, and the merge commit
//!    is reported. A merge that fails part way is aborted, so the lead's checkout is
//!    never left mid-merge; only a merge of that commit, the one `team_merge` started,
//!    is ever aborted.
//!
//! Git runs through the caller's [`GitExecution`] (the tool host's sandboxed git, with
//! ambient and global configuration removed). The member's worktree and branch are kept.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Output;

use cyber_core::teams::Isolation;
use cyber_core::worktrees::GitExecution;
use serde::{Deserialize, Serialize};

use super::Role;
use crate::runtime::{Runtime, RuntimeError};

/// The oldest git whose `merge-tree --write-tree` decides a merge in memory.
const MIN_GIT: (u32, u32) = (2, 38);
pub const DIRTY: &str = "Commit or stash your changes before merging";
pub const OLD_GIT: &str = "team_merge needs git 2.38 or later";
pub const BUSY: &str = "A merge, rebase, cherry-pick or revert is in progress in your checkout; finish or abort it before merging";
pub const DETACHED: &str = "Your checkout has a detached HEAD; check out a branch before merging";

/// What an operation in progress leaves in the git directory: `team_merge` never runs
/// while one is there, so it never commits into it nor aborts it.
const IN_PROGRESS: [&str; 5] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "rebase-merge",
    "rebase-apply",
];

/// `team_merge` input.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TeamMerge {
    /// The teammate whose branch is merged.
    pub member: String,
    /// The merge commit message; default `Merge teammate <name> (<branch>)`.
    #[serde(default)]
    pub message: Option<String>,
}

/// What a merge works on, resolved by [`Runtime::team_merge_target`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeTarget {
    pub team_id: String,
    pub member: String,
    pub branch: String,
    /// The lead's Location, the checkout that receives the merge.
    pub directory: PathBuf,
}

/// The result of a merge that ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MergeOutcome {
    Merged {
        member: String,
        branch: String,
        commit: String,
    },
    /// Nothing changed in the lead's checkout.
    Conflicts {
        member: String,
        branch: String,
        paths: Vec<String>,
    },
}

impl Runtime {
    /// The branch `lead_id` may merge for `member`: refused unless the caller leads a
    /// team, `teams` is on in the team's directory, and `member` is a teammate in its own
    /// worktree.
    pub async fn team_merge_target(
        &self,
        lead_id: &str,
        member: &str,
    ) -> Result<MergeTarget, RuntimeError> {
        let (team, me) = self.team_membership(lead_id)?.ok_or_else(|| {
            RuntimeError::Invalid(format!("Session {lead_id} is not a member of a team"))
        })?;
        if me.role != Role::Lead {
            return Err(RuntimeError::Forbidden {
                reason: "team_lead_only",
                message: "Only the team lead can merge teammate work".into(),
            });
        }
        self.inner
            .tools
            .team_settings(&team.directory)
            .map_err(RuntimeError::Invalid)?
            .ok_or(RuntimeError::FeatureDisabled(cyber_core::teams::DISABLED))?;
        let teammate = self
            .team_members(&team.id)?
            .into_iter()
            .find(|m| m.name == member && m.role == Role::Teammate)
            .ok_or_else(|| {
                RuntimeError::Invalid(format!("No teammate named {member} in team {}", team.id))
            })?;
        let branch = match (&teammate.isolation, teammate.branch) {
            (Isolation::Worktree, Some(branch)) => branch,
            _ => {
                return Err(RuntimeError::Invalid(format!(
                    "{member} works in the shared Location; there is no branch to merge"
                )));
            }
        };
        let directory = PathBuf::from(self.state(lead_id).await?.info.directory);
        Ok(MergeTarget {
            team_id: team.id,
            member: teammate.name,
            branch,
            directory,
        })
    }
}

/// A merge decided clean by [`plan`], to be authorized and then applied by [`apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanMerge {
    /// The top of the lead's checkout, which the paths are relative to.
    pub top: PathBuf,
    /// Every path the merge adds, changes or removes in the lead's checkout,
    /// `/`-separated and relative to `top`.
    pub paths: Vec<String>,
    head: String,
    tip: String,
}

/// What [`plan`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergePlan {
    /// Nothing changed in the lead's checkout.
    Conflicts(MergeOutcome),
    Clean(CleanMerge),
}

/// Decide the merge of `target` without touching the lead's checkout: refused while the
/// checkout is mid-operation, detached or dirty, when there is nothing to merge, and when a path the merge writes
/// exists untracked (ignored files included) in the checkout, which `git merge` would
/// otherwise overwrite.
pub async fn plan(git: &dyn GitExecution, target: &MergeTarget) -> Result<MergePlan, RuntimeError> {
    let run = Git {
        execution: git,
        directory: &target.directory,
    };
    check_version(&run.stdout(&["version"]).await?)?;
    let top = PathBuf::from(run.stdout(&["rev-parse", "--show-toplevel"]).await?.trim());
    let run = Git {
        execution: git,
        directory: &top,
    };
    run.ensure_settled().await?;
    run.ensure_clean().await?;
    let head = run.commit("HEAD").await?;
    let tip = run
        .commit(&format!("refs/heads/{}", target.branch))
        .await
        .map_err(|_| {
            RuntimeError::Invalid(format!(
                "Branch {} of {} is missing",
                target.branch, target.member
            ))
        })?;
    let ahead = run
        .stdout(&["rev-list", "--count", &format!("{head}..{tip}")])
        .await?;
    if ahead.trim() == "0" {
        return Err(RuntimeError::Invalid(format!(
            "Nothing to merge from {}",
            target.member
        )));
    }
    let tree = match run.merge_tree(&head, &tip).await? {
        Ok(tree) => tree,
        Err(paths) => {
            return Ok(MergePlan::Conflicts(MergeOutcome::Conflicts {
                member: target.member.clone(),
                branch: target.branch.clone(),
                paths,
            }));
        }
    };
    let paths = run.changed(&head, &tree).await?;
    run.ensure_untouched(&paths).await?;
    Ok(MergePlan::Clean(CleanMerge {
        top,
        paths,
        head,
        tip,
    }))
}

/// Apply a planned merge: `git merge --no-ff` of the exact commit that was checked, with
/// hooks disabled. Refused when the checkout changed since [`plan`] (an operation
/// started, its HEAD moved, it became dirty, or a written path appeared untracked).
pub async fn apply(
    git: &dyn GitExecution,
    target: &MergeTarget,
    planned: &CleanMerge,
    message: Option<&str>,
    home: &Path,
) -> Result<MergeOutcome, RuntimeError> {
    let run = Git {
        execution: git,
        directory: &planned.top,
    };
    run.ensure_settled().await?;
    run.ensure_clean().await?;
    if run.commit("HEAD").await? != planned.head {
        return Err(RuntimeError::Conflict(
            "Your checkout changed while the merge was prepared; run team_merge again".into(),
        ));
    }
    run.ensure_untouched(&planned.paths).await?;
    let message = message
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map_or_else(
            || format!("Merge teammate {} ({})", target.member, target.branch),
            str::to_owned,
        );
    run.merge(&planned.tip, &message, home).await?;
    Ok(MergeOutcome::Merged {
        member: target.member.clone(),
        branch: target.branch.clone(),
        commit: run.commit("HEAD").await?,
    })
}

struct Git<'a> {
    execution: &'a dyn GitExecution,
    directory: &'a Path,
}

impl Git<'_> {
    async fn output(&self, args: &[&str]) -> Result<Output, RuntimeError> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        self.execution
            .run(self.directory, &args)
            .await
            .map_err(|error| RuntimeError::Invalid(format!("git failed: {error}")))
    }

    /// The standard output of a git command that must succeed.
    async fn stdout(&self, args: &[&str]) -> Result<String, RuntimeError> {
        let output = self.output(args).await?;
        if !output.status.success() {
            return Err(RuntimeError::Invalid(format!(
                "git {} failed: {}",
                args.first().copied().unwrap_or_default(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// The commit ID `revision` names.
    async fn commit(&self, revision: &str) -> Result<String, RuntimeError> {
        let id = self
            .stdout(&[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{revision}^{{commit}}"),
            ])
            .await?;
        Ok(id.trim().to_owned())
    }

    /// Refused while a merge, rebase, cherry-pick or revert is in progress, or HEAD is
    /// detached (a stopped rebase among others): a merge would commit into it.
    async fn ensure_settled(&self) -> Result<(), RuntimeError> {
        if self.in_progress().await? {
            return Err(RuntimeError::Conflict(BUSY.into()));
        }
        let branch = self.output(&["symbolic-ref", "--quiet", "HEAD"]).await?;
        if branch.status.success() {
            Ok(())
        } else {
            Err(RuntimeError::Conflict(DETACHED.into()))
        }
    }

    /// Whether the git directory holds an operation in progress ([`IN_PROGRESS`]).
    async fn in_progress(&self) -> Result<bool, RuntimeError> {
        let mut args = vec!["rev-parse"];
        for marker in IN_PROGRESS {
            args.extend(["--git-path", marker]);
        }
        let paths = self.stdout(&args).await?;
        Ok(paths
            .lines()
            .filter(|line| !line.is_empty())
            .any(|path| self.directory.join(path).exists()))
    }

    /// The commit `MERGE_HEAD` names, while a merge is in progress.
    async fn merge_head(&self) -> Result<Option<String>, RuntimeError> {
        let output = self
            .output(&["rev-parse", "--verify", "--quiet", "MERGE_HEAD"])
            .await?;
        let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok((output.status.success() && !id.is_empty()).then_some(id))
    }

    /// Refused while tracked files have uncommitted changes.
    async fn ensure_clean(&self) -> Result<(), RuntimeError> {
        let status = self
            .stdout(&["status", "--porcelain", "--untracked-files=no"])
            .await?;
        if status.trim().is_empty() {
            Ok(())
        } else {
            Err(RuntimeError::Conflict(DIRTY.into()))
        }
    }

    /// The merge of `tip` into `head`, decided in memory: the merged tree's ID, or the
    /// conflicted paths.
    async fn merge_tree(
        &self,
        head: &str,
        tip: &str,
    ) -> Result<Result<String, Vec<String>>, RuntimeError> {
        let output = self
            .output(&[
                "merge-tree",
                "--write-tree",
                "--name-only",
                "--no-messages",
                "-z",
                head,
                tip,
            ])
            .await?;
        match output.status.code() {
            Some(0) => Ok(Ok(tree_id(&output.stdout))),
            Some(1) => Ok(Err(conflicted_paths(&output.stdout))),
            _ => Err(RuntimeError::Invalid(format!(
                "git merge-tree failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))),
        }
    }

    /// Every path that differs between `head` and the merged `tree`: what the merge
    /// writes or removes in the checkout (renames as both of their paths).
    async fn changed(&self, head: &str, tree: &str) -> Result<Vec<String>, RuntimeError> {
        let output = self
            .stdout(&["diff", "--name-only", "--no-renames", "-z", head, tree])
            .await?;
        Ok(nul_separated(output.as_bytes()))
    }

    /// Refused when a path the merge writes exists untracked or ignored in the checkout.
    async fn ensure_untouched(&self, paths: &[String]) -> Result<(), RuntimeError> {
        let untracked = self
            .stdout(&["ls-files", "--others", "--directory", "--full-name", "-z"])
            .await?;
        let untracked = nul_separated(untracked.as_bytes());
        let occupied = |path: &str| occupied(self.directory, path);
        match untracked_collision(paths, &untracked, occupied) {
            Some(path) => Err(RuntimeError::Conflict(format!(
                "{path} exists untracked in your checkout; move it away before merging"
            ))),
            None => Ok(()),
        }
    }

    /// `git merge --no-ff` of `tip` with hooks disabled, aborted when it fails part way:
    /// only when the merge in progress is of `tip`, the one this call started (an
    /// operation of the user's is refused before, and never aborted).
    async fn merge(&self, tip: &str, message: &str, home: &Path) -> Result<(), RuntimeError> {
        let mut args: Vec<String> = vec!["-c".into(), "core.hooksPath=".into()];
        args.extend(self.identity(home).await?);
        args.extend(
            [
                "merge",
                "--no-ff",
                "--no-verify",
                "--no-edit",
                "-m",
                message,
                tip,
            ]
            .map(str::to_owned),
        );
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = self.output(&args).await?;
        if output.status.success() {
            return Ok(());
        }
        if self.merge_head().await.ok().flatten().as_deref() == Some(tip) {
            let _ = self.output(&["merge", "--abort"]).await;
        }
        Err(RuntimeError::Invalid(format!(
            "git merge failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }

    /// `-c user.name=… -c user.email=…` from the user's git configuration files when the
    /// repository sets no identity; nothing when the repository does or none is found
    /// (git then reports the missing identity and the merge is aborted).
    async fn identity(&self, home: &Path) -> Result<Vec<String>, RuntimeError> {
        if self.config(None, "user.email").await?.is_some() {
            return Ok(Vec::new());
        }
        let files = [home.join(".gitconfig"), home.join(".config/git/config")];
        for file in files.iter().filter(|file| file.is_file()) {
            let file = file.display().to_string();
            let email = self.config(Some(&file), "user.email").await?;
            let name = self.config(Some(&file), "user.name").await?;
            if let (Some(email), Some(name)) = (email, name) {
                return Ok(vec![
                    "-c".into(),
                    format!("user.name={name}"),
                    "-c".into(),
                    format!("user.email={email}"),
                ]);
            }
        }
        Ok(Vec::new())
    }

    async fn config(&self, file: Option<&str>, key: &str) -> Result<Option<String>, RuntimeError> {
        let mut args = vec!["config"];
        if let Some(file) = file {
            args.extend(["--file", file]);
        }
        args.extend(["--get", key]);
        let output = self.output(&args).await?;
        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok((output.status.success() && !value.is_empty()).then_some(value))
    }
}

/// `git version 2.39.3 (Apple Git-146)` is at least 2.38.
fn check_version(version: &str) -> Result<(), RuntimeError> {
    let mut numbers = version
        .split_whitespace()
        .find(|word| word.starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or_default()
        .split('.')
        .map(|part| part.parse::<u32>().ok());
    match (numbers.next().flatten(), numbers.next().flatten()) {
        (Some(major), Some(minor)) if (major, minor) >= MIN_GIT => Ok(()),
        _ => Err(RuntimeError::Invalid(OLD_GIT.into())),
    }
}

/// The paths of `merge-tree --write-tree --name-only --no-messages -z` output: the tree
/// ID, then each conflicted path, NUL-terminated.
fn conflicted_paths(stdout: &[u8]) -> Vec<String> {
    let mut paths: Vec<String> = nul_separated(stdout).into_iter().skip(1).collect();
    paths.dedup();
    paths
}

/// The tree ID that `merge-tree --write-tree -z` output starts with.
fn tree_id(stdout: &[u8]) -> String {
    nul_separated(stdout).into_iter().next().unwrap_or_default()
}

fn nul_separated(stdout: &[u8]) -> Vec<String> {
    stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            String::from_utf8_lossy(path)
                .trim_end_matches('\n')
                .to_owned()
        })
        .filter(|path| !path.is_empty())
        .collect()
}

/// Whether `path` (relative to `top`), or an ancestor of it standing in for a directory,
/// exists on disk.
fn occupied(top: &Path, path: &str) -> bool {
    let mut current = top.to_path_buf();
    let parts: Vec<&str> = path.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if index + 1 == parts.len() || !meta.is_dir() => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

/// The first of `paths` the merge would write over something untracked: `untracked` is
/// `ls-files --others --directory` output (a trailing `/` marks an untracked directory
/// listed whole). A path that is, or lies under, or contains an untracked file
/// collides; inside an untracked directory, a path collides when something occupies it
/// on disk.
fn untracked_collision<'a>(
    paths: &'a [String],
    untracked: &[String],
    occupied: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    let under = |path: &str, dir: &str| {
        path.strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
    };
    paths
        .iter()
        .find(|path| {
            untracked.iter().any(|entry| match entry.strip_suffix('/') {
                Some(dir) => {
                    path.as_str() == dir || under(dir, path) || (under(path, dir) && occupied(path))
                }
                None => *path == entry || under(entry, path) || under(path, entry),
            })
        })
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_before_2_38_is_refused() {
        assert!(check_version("git version 2.39.3 (Apple Git-146)\n").is_ok());
        assert!(check_version("git version 2.38.0").is_ok());
        assert!(check_version("git version 3.0.0").is_ok());
        assert!(check_version("git version 2.45.1.windows.1").is_ok());
        for old in ["git version 2.37.9", "git version 1.99.0", "nonsense", ""] {
            assert_eq!(
                check_version(old).unwrap_err().to_string(),
                format!("InvalidRequestError: {OLD_GIT}"),
                "{old}"
            );
        }
    }

    #[test]
    fn conflicted_paths_follow_the_tree_id() {
        assert_eq!(
            conflicted_paths(b"4b825dc642cb6eb9a060e54bf8d69288fbee4904\0shared.txt\0src/a b.rs\0"),
            ["shared.txt", "src/a b.rs"]
        );
        assert!(conflicted_paths(b"4b825dc642cb6eb9a060e54bf8d69288fbee4904\0").is_empty());
    }

    #[test]
    fn a_merge_never_writes_over_untracked_or_ignored_files() {
        let paths: Vec<String> = [".env", "src/new.rs", "build/out.bin", "logs"]
            .map(str::to_owned)
            .to_vec();
        let collide = |untracked: &[&str], occupied: &[&str]| {
            let untracked: Vec<String> = untracked.iter().map(|e| (*e).to_owned()).collect();
            untracked_collision(&paths, &untracked, |p| occupied.contains(&p)).map(str::to_owned)
        };
        assert_eq!(collide(&[".env"], &[]), Some(".env".into()));
        assert_eq!(collide(&["notes.txt", "target/"], &[]), None);
        // Inside an untracked directory only an occupied path collides.
        assert_eq!(collide(&["build/"], &[]), None);
        assert_eq!(
            collide(&["build/"], &["build/out.bin"]),
            Some("build/out.bin".into())
        );
        // An untracked file where the merge needs a directory, and the reverse.
        assert_eq!(collide(&["src"], &[]), Some("src/new.rs".into()));
        assert_eq!(collide(&["logs/today.log"], &[]), Some("logs".into()));
        assert_eq!(collide(&["logs/"], &[]), Some("logs".into()));
        assert_eq!(collide(&["src/new.rs.bak"], &[]), None);
    }

    #[test]
    fn occupied_paths_are_found_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("build")).unwrap();
        std::fs::write(dir.path().join("build/out.bin"), "x").unwrap();
        std::fs::write(dir.path().join("file"), "x").unwrap();
        assert!(occupied(dir.path(), "build/out.bin"));
        assert!(
            occupied(dir.path(), "file/inner"),
            "a file stands in a directory"
        );
        assert!(!occupied(dir.path(), "build/other.bin"));
        assert!(!occupied(dir.path(), "missing/x"));
    }
}
