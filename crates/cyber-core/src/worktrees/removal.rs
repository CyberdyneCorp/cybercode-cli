//! Durable deletion admission and read-only reconciliation of unknown outcomes.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::repository::{git, git_path_argument, replace_record, write_new};
use super::{GitExecution, ListedWorktree, Managed, Name, Repository, RepositoryLock};

/// The runtime must atomically refuse active Sessions and fence new Location use
/// across this guard's lifetime, including other embedded/server processes.
/// A force request never bypasses activity admission. No default idle port exists.
pub trait RemovalActivity: Send + Sync {
    fn reserve(&self, managed: &Managed) -> io::Result<Box<dyn Send>>;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemovalPhase {
    TreeRemovalIntent,
    TreeRemoved,
    BranchRemovalIntent,
    BranchRemoved,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemovalRecord {
    pub managed: Managed,
    pub head: String,
    pub force: bool,
    pub phase: RemovalPhase,
}

impl Repository {
    /// Delete exactly this creation identity. Unknown command outcomes are never
    /// redispatched: retries reconcile missing artifacts or require recovery.
    pub async fn remove(
        &self,
        execution: &dyn GitExecution,
        activity: &dyn RemovalActivity,
        managed: &Managed,
        force: bool,
    ) -> io::Result<RemovalRecord> {
        let _lock = RepositoryLock::try_acquire(&self.common_dir)?.ok_or_else(busy)?;
        validate_identity(self, managed)?;
        let _activity = activity.reserve(managed)?;
        if let Some(mut record) = read_removal(self, &Name::parse(&managed.name).map_err(invalid)?)?
            && record.managed.id == managed.id
        {
            if record.managed != *managed || record.force != force {
                return Err(invalid("Removal request does not match durable intent"));
            }
            return self.resume_removal(execution, &mut record).await;
        }
        check_admission(self, &Name::parse(&managed.name).map_err(invalid)?)?;
        let owned: Managed = read_json(&ownership_path(self, &managed.name))?;
        if owned != *managed {
            return Err(invalid("Removal requires matching ready ownership"));
        }
        self.verify(execution, managed).await?;
        let head = branch_head(self, execution, &managed.branch)
            .await?
            .ok_or_else(|| invalid("Owned branch is missing"))?;
        if !force {
            let status = self.git_status(execution, managed).await?;
            if status.dirty || status.ahead != 0 || ignored_changes(execution, managed).await? {
                return Err(io::Error::other(
                    "Worktree has uncommitted changes or commits beyond its base; use --force",
                ));
            }
        }
        let mut record = RemovalRecord {
            managed: managed.clone(),
            head,
            force,
            phase: RemovalPhase::TreeRemovalIntent,
        };
        save(self, &record)?;
        let mut args = vec!["worktree".into(), "remove".into()];
        if force {
            args.push("--force".into());
        }
        args.extend(["--".into(), git_path_argument(&managed.path)?]);
        repository_git(self, execution, &args).await?;
        confirm_tree_removed(self, execution, managed).await?;
        record.phase = RemovalPhase::TreeRemoved;
        save(self, &record)?;
        self.resume_removal(execution, &mut record).await
    }

    /// Recreate only an acknowledged clean removal, with original base and new incarnation.
    pub async fn recreate_removed(
        &self,
        execution: &dyn GitExecution,
        activity: &dyn RemovalActivity,
        settings: &super::Settings,
        data: &Path,
        project_id: &str,
        managed: &Managed,
    ) -> io::Result<Managed> {
        let _lock = RepositoryLock::try_acquire(&self.common_dir)?.ok_or_else(busy)?;
        validate_identity(self, managed)?;
        let _activity = activity.reserve(managed)?;
        let name = Name::parse(&managed.name).map_err(invalid)?;
        let removed = read_removal(self, &name)?
            .ok_or_else(|| invalid("Clean removal evidence is missing; recovery is required"))?;
        if removed.managed != *managed
            || removed.force
            || removed.phase != RemovalPhase::Completed
            || removed.head != managed.base
        {
            return Err(invalid(
                "Clean removal evidence does not match the original checkout; recovery is required",
            ));
        }
        confirm_tree_removed(self, execution, managed).await?;
        if branch_head(self, execution, &managed.branch)
            .await?
            .is_some()
            || ownership_path(self, &managed.name).try_exists()?
        {
            return Err(invalid(
                "Removed branch or ownership was replaced; recovery is required",
            ));
        }
        let mut settings = settings.clone();
        settings.base = managed.base.clone();
        self.create_locked(
            execution,
            &settings,
            data,
            project_id,
            super::Branch {
                name: &name,
                reference: &managed.branch,
            },
        )
        .await
    }

    async fn resume_removal(
        &self,
        execution: &dyn GitExecution,
        record: &mut RemovalRecord,
    ) -> io::Result<RemovalRecord> {
        if record.phase == RemovalPhase::Completed {
            return Ok(record.clone());
        }
        confirm_tree_removed(self, execution, &record.managed).await?;
        if record.phase == RemovalPhase::TreeRemovalIntent {
            record.phase = RemovalPhase::TreeRemoved;
            save(self, record)?;
        }
        self.settle_branch(execution, record).await?;
        let path = ownership_path(self, &record.managed.name);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                let owned: Managed = read_json(&path)?;
                if owned != record.managed {
                    return Err(invalid(
                        "Ownership changed during removal; recovery is required",
                    ));
                }
                std::fs::remove_file(&path)?;
                sync_parent(&path)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        record.phase = RemovalPhase::Completed;
        save(self, record)?;
        Ok(record.clone())
    }

    async fn settle_branch(
        &self,
        execution: &dyn GitExecution,
        record: &mut RemovalRecord,
    ) -> io::Result<()> {
        if record.phase == RemovalPhase::BranchRemoved {
            return Ok(());
        }
        let head = branch_head(self, execution, &record.managed.branch).await?;
        if record.phase == RemovalPhase::BranchRemovalIntent && head.is_some() {
            return Err(io::Error::other(
                "Branch removal outcome is unknown; recovery is required",
            ));
        }
        if let Some(head) = head {
            if head != record.head {
                return Err(invalid(
                    "Owned branch changed during removal; recovery is required",
                ));
            }
            record.phase = RemovalPhase::BranchRemovalIntent;
            save(self, record)?;
            repository_git(
                self,
                execution,
                &[
                    "update-ref".into(),
                    "--no-deref".into(),
                    "-d".into(),
                    format!("refs/heads/{}", record.managed.branch).into(),
                    record.head.clone().into(),
                ],
            )
            .await?;
            if branch_head(self, execution, &record.managed.branch)
                .await?
                .is_some()
            {
                return Err(invalid("Branch exists after removal; recovery is required"));
            }
        }
        record.phase = RemovalPhase::BranchRemoved;
        save(self, record)
    }
}

pub(super) fn check_admission(repo: &Repository, name: &Name) -> io::Result<()> {
    if read_removal(repo, name)?.is_some_and(|record| record.phase != RemovalPhase::Completed) {
        return Err(io::Error::other(
            "Worktree removal is incomplete; recovery is required",
        ));
    }
    Ok(())
}

pub(super) fn check_record_admission(repo: &Repository, managed: &Managed) -> io::Result<()> {
    if let Some(record) = read_removal(repo, &Name::parse(&managed.name).map_err(invalid)?)?
        && (record.phase != RemovalPhase::Completed || record.managed.id == managed.id)
    {
        return Err(io::Error::other(
            "Worktree was removed or removal is incomplete; recovery is required",
        ));
    }
    Ok(())
}

fn validate_identity(repo: &Repository, managed: &Managed) -> io::Result<()> {
    Name::parse(&managed.name).map_err(invalid)?;
    if !managed.ready
        || managed.id.is_empty()
        || managed.common_dir != repo.common_dir
        || !managed.path.is_absolute()
    {
        return Err(invalid(
            "Removal requires ready creation identity in this repository",
        ));
    }
    Ok(())
}

async fn confirm_tree_removed(
    repo: &Repository,
    execution: &dyn GitExecution,
    managed: &Managed,
) -> io::Result<()> {
    match std::fs::symlink_metadata(&managed.path) {
        Ok(_) => {
            return Err(io::Error::other(
                "Worktree removal outcome is unknown or path was replaced; recovery is required",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let output = repository_git(
        repo,
        execution,
        &[
            "worktree".into(),
            "list".into(),
            "--porcelain".into(),
            "-z".into(),
        ],
    )
    .await?;
    let target = git_path_argument(&managed.path)?
        .to_string_lossy()
        .into_owned();
    let branch = format!("branch refs/heads/{}", managed.branch);
    for field in output.split(|byte| *byte == 0) {
        if field == branch.as_bytes() {
            return Err(invalid(
                "Owned branch is still checked out; recovery is required",
            ));
        }
        if let Some(path) = field.strip_prefix(b"worktree ")
            && same_git_path(path, &target)
        {
            return Err(invalid(
                "Worktree registration remains; recovery is required",
            ));
        }
    }
    Ok(())
}

fn same_git_path(path: &[u8], expected: &str) -> bool {
    #[cfg(windows)]
    {
        String::from_utf8_lossy(path)
            .replace('\\', "/")
            .eq_ignore_ascii_case(&expected.replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        path == expected.as_bytes()
    }
}

async fn branch_head(
    repo: &Repository,
    execution: &dyn GitExecution,
    branch: &str,
) -> io::Result<Option<String>> {
    let reference = format!("refs/heads/{branch}");
    let bytes = repository_git(
        repo,
        execution,
        &[
            "for-each-ref".into(),
            "--format=%(refname)%00%(objectname)%00%(symref)".into(),
            reference.clone().into(),
        ],
    )
    .await?;
    let output = String::from_utf8(bytes).map_err(invalid)?;
    for line in output.lines() {
        let fields = line.split('\0').collect::<Vec<_>>();
        if fields.first().copied() != Some(reference.as_str()) {
            continue;
        }
        if fields.len() != 3
            || !fields[2].is_empty()
            || !matches!(fields[1].len(), 40 | 64)
            || !fields[1].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid("Owned branch is not a direct object reference"));
        }
        return Ok(Some(fields[1].into()));
    }
    Ok(None)
}

async fn repository_git(
    repo: &Repository,
    execution: &dyn GitExecution,
    args: &[std::ffi::OsString],
) -> io::Result<Vec<u8>> {
    let mut arguments = vec!["--git-dir".into(), git_path_argument(&repo.common_dir)?];
    arguments.extend_from_slice(args);
    git(execution, &repo.common_dir, &arguments).await
}

async fn ignored_changes(execution: &dyn GitExecution, managed: &Managed) -> io::Result<bool> {
    Ok(!changed_ignored_files(execution, managed).await?.is_empty())
}

pub(super) async fn changed_ignored_files(
    execution: &dyn GitExecution,
    managed: &Managed,
) -> io::Result<Vec<PathBuf>> {
    if super::inspection::required(managed) {
        let report =
            super::inspection::inspect(execution, managed, super::inspection::Operation::Ignored)
                .await?;
        return filter_ignored_files(managed, report.ignored);
    }
    let paths = git(
        execution,
        &managed.path,
        &[
            "ls-files".into(),
            "--others".into(),
            "--ignored".into(),
            "--exclude-standard".into(),
            "-z".into(),
        ],
    )
    .await?;
    let paths = paths
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::str::from_utf8(path)
                .map(PathBuf::from)
                .map_err(invalid)
        })
        .collect::<io::Result<Vec<_>>>()?;
    filter_ignored_files(managed, paths)
}

pub(super) fn filter_ignored_files(
    managed: &Managed,
    paths: Vec<PathBuf>,
) -> io::Result<Vec<PathBuf>> {
    let root = Dir::open_ambient_dir(&managed.path, cap_std::ambient_authority())?;
    let mut changed = Vec::new();
    for path in &paths {
        let path = path.as_path();
        let Some(included) = managed.included.iter().find(|file| file.path == path) else {
            changed.push(path.to_owned());
            continue;
        };
        if !root.symlink_metadata(path)?.is_file() {
            changed.push(path.to_owned());
            continue;
        }
        let mut file = root.open(path)?;
        let mut digest = Sha256::new();
        let mut buffer = [0; 16384];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        if format!("{:x}", digest.finalize()) != included.sha256 {
            changed.push(path.to_owned());
        }
    }
    Ok(changed)
}

fn ownership_path(repo: &Repository, name: &str) -> PathBuf {
    repo.common_dir
        .join("cyber-worktrees")
        .join(format!("{name}.json"))
}
fn removal_path(repo: &Repository, name: &str) -> PathBuf {
    repo.common_dir
        .join("cyber-worktree-removals")
        .join(format!("{name}.json"))
}

fn read_removal(repo: &Repository, name: &Name) -> io::Result<Option<RemovalRecord>> {
    record_directory(repo, false)?;
    let path = removal_path(repo, name.as_str());
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            let record: RemovalRecord = read_json(&path)?;
            validate_identity(repo, &record.managed)?;
            if record.managed.name != name.as_str()
                || !matches!(record.head.len(), 40 | 64)
                || !record.head.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(invalid(
                    "Removal ownership does not match record name or commit",
                ));
            }
            Ok(Some(record))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("Ownership record is not a regular file"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(1048577)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1048576 {
        return Err(invalid("Ownership record exceeds 1 MiB"));
    }
    serde_json::from_slice(&bytes).map_err(invalid)
}

fn save(repo: &Repository, record: &RemovalRecord) -> io::Result<()> {
    record_directory(repo, true)?;
    let path = removal_path(repo, &record.managed.name);
    let parent = path.parent().expect("journal parent");
    sync_parent(parent)?;
    if path.try_exists()? {
        replace_record(&path, record)?;
    } else {
        write_new(&path, record)?;
    }
    sync_parent(&path)
}

fn record_directory(repo: &Repository, create: bool) -> io::Result<bool> {
    let path = repo.common_dir.join("cyber-worktree-removals");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(invalid(
            "Removal journal directory is not a regular directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if create {
                std::fs::create_dir(&path)?;
            }
            Ok(create)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn pending_listing(repo: &Repository) -> io::Result<Vec<ListedWorktree>> {
    if !record_directory(repo, false)? {
        return Ok(Vec::new());
    }
    let mut paths = std::fs::read_dir(repo.common_dir.join("cyber-worktree-removals"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    paths.sort();
    let mut entries = Vec::new();
    for path in paths {
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let error = match Name::parse(&name)
            .map_err(invalid)
            .and_then(|name| read_removal(repo, &name))
        {
            Ok(Some(record)) if record.phase == RemovalPhase::Completed => continue,
            Ok(Some(record)) => format!(
                "Removal {:?} at {}; recovery is required",
                record.phase,
                record.managed.path.display()
            ),
            Ok(None) => continue,
            Err(error) => error.to_string(),
        };
        entries.push(ListedWorktree::Invalid { name, error });
    }
    Ok(entries)
}

pub(super) fn listing_name(entry: &ListedWorktree) -> &str {
    match entry {
        ListedWorktree::Ready(managed) | ListedWorktree::Pending(managed) => &managed.name,
        ListedWorktree::Invalid { name, .. } => name,
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(
            path.parent()
                .ok_or_else(|| invalid("Missing journal parent"))?,
        )?
        .sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn busy() -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, "Worktree repository is busy")
}
fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
