//! Informational completion summaries; removal still checks its own admission under lock.
use super::repository::git;
use super::{GitExecution, Managed, Repository, RepositoryLock};
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChangedFile {
    pub file: String,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Changes {
    pub files: Vec<ChangedFile>,
    pub additions: u64,
    pub deletions: u64,
    pub unknown_stats: bool,
    pub dirty: bool,
    pub ahead: u64,
    pub behind: u64,
}

impl Repository {
    pub async fn changes(
        &self,
        execution: &dyn GitExecution,
        managed: &Managed,
    ) -> io::Result<Changes> {
        let _lock = RepositoryLock::try_acquire(&self.common_dir)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::WouldBlock, "Worktree repository is busy")
        })?;
        self.verify_owned(execution, managed).await?;
        let (status, mut files, untracked, ignored) = if super::inspection::required(managed) {
            let report = super::inspection::inspect(
                execution,
                managed,
                super::inspection::Operation::Changes,
            )
            .await?;
            let ignored = super::removal::filter_ignored_files(managed, report.ignored)?;
            (report.status, report.files, report.untracked, ignored)
        } else {
            self.git_changes(execution, managed).await?
        };
        let root = Dir::open_ambient_dir(&managed.path, cap_std::ambient_authority())?;
        for path in untracked {
            files.push(untracked_stats(&root, &path)?);
        }
        let dirty_ignored = !ignored.is_empty();
        for path in ignored {
            files.push(untracked_stats(&root, &path)?);
        }
        files.sort_by(|a, b| a.file.cmp(&b.file));
        Ok(Changes {
            additions: files.iter().filter_map(|file| file.additions).sum(),
            deletions: files.iter().filter_map(|file| file.deletions).sum(),
            unknown_stats: files
                .iter()
                .any(|file| file.additions.is_none() || file.deletions.is_none()),
            files,
            dirty: status.dirty || dirty_ignored,
            ahead: status.ahead,
            behind: status.behind,
        })
    }
    async fn git_changes(
        &self,
        execution: &dyn GitExecution,
        managed: &Managed,
    ) -> io::Result<(
        super::WorktreeStatus,
        Vec<ChangedFile>,
        Vec<PathBuf>,
        Vec<PathBuf>,
    )> {
        let status = self.git_status(execution, managed).await?;
        let diff = git(
            execution,
            &managed.path,
            &[
                "--no-optional-locks".into(),
                "diff".into(),
                "--no-ext-diff".into(),
                "--no-textconv".into(),
                "--no-renames".into(),
                "--numstat".into(),
                "-z".into(),
                managed.base.clone().into(),
                "--".into(),
            ],
        )
        .await?;
        let files = numstat(&diff)?;
        let untracked = git(
            execution,
            &managed.path,
            &[
                "ls-files".into(),
                "--others".into(),
                "--exclude-standard".into(),
                "-z".into(),
            ],
        )
        .await?;
        let untracked = untracked
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| {
                std::str::from_utf8(path)
                    .map(PathBuf::from)
                    .map_err(io::Error::other)
            })
            .collect::<io::Result<Vec<_>>>()?;
        let ignored = super::removal::changed_ignored_files(execution, managed).await?;
        Ok((status, files, untracked, ignored))
    }
}

fn numstat(bytes: &[u8]) -> io::Result<Vec<ChangedFile>> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let text = std::str::from_utf8(record).map_err(io::Error::other)?;
            let mut fields = text.splitn(3, '\t');
            let additions = count(fields.next())?;
            let deletions = count(fields.next())?;
            let file = fields
                .next()
                .filter(|file| !file.is_empty())
                .ok_or_else(|| io::Error::other("Invalid Git numstat path"))?
                .to_owned();
            Ok(ChangedFile {
                file,
                additions,
                deletions,
            })
        })
        .collect()
}

fn count(field: Option<&str>) -> io::Result<Option<u64>> {
    match field {
        Some("-") => Ok(None),
        Some(value) => value.parse().map(Some).map_err(io::Error::other),
        None => Err(io::Error::other("Invalid Git numstat count")),
    }
}

fn untracked_stats(root: &Dir, path: &Path) -> io::Result<ChangedFile> {
    let file = path
        .to_str()
        .ok_or_else(|| io::Error::other("Non-Unicode Git path"))?
        .into();
    let mut result = ChangedFile {
        file,
        additions: None,
        deletions: Some(0),
    };
    if !root.symlink_metadata(path)?.is_file() {
        return Ok(result);
    }
    let mut bytes = Vec::new();
    root.open(path)?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 || bytes.contains(&0) {
        return Ok(result);
    }
    let lines = bytes.iter().filter(|byte| **byte == b'\n').count()
        + usize::from(bytes.last().is_some_and(|byte| *byte != b'\n'));
    result.additions = Some(lines as u64);
    Ok(result)
}
