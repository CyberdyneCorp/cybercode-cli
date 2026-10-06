use std::ffi::OsString;
use std::future::Future;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Output;

use serde::{Deserialize, Serialize};

use super::{Name, RepositoryLock, Settings};

pub type GitFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Output>> + Send + 'a>>;

/// Runtime implementations must provide sandboxed, cancellation-owned Git execution
/// with ambient Git overrides removed. The core manager never spawns processes.
/// On Windows, long linked worktrees execute from their verified metadata directory
/// with an explicit checkout destination. Sandbox scope must follow that destination.
pub trait GitExecution: Send + Sync {
    fn run<'a>(&'a self, directory: &'a Path, args: &'a [OsString]) -> GitFuture<'a>;
}

#[derive(Debug, Clone)]
pub struct Repository {
    pub root: PathBuf,
    pub common_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Managed {
    /// Creation identity; older records require recovery before journaled setup.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub branch: String,
    pub base: String,
    pub common_dir: PathBuf,
    pub ready: bool,
    #[serde(default)]
    pub included: Vec<super::IncludedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListedWorktree {
    Ready(Managed),
    Pending(Managed),
    Invalid { name: String, error: String },
}

impl Repository {
    /// Inspect durable ownership under the shared lifecycle lock. Pending or
    /// invalid records remain visible for recovery; listing never repairs them.
    pub async fn list(&self, execution: &dyn GitExecution) -> io::Result<Vec<ListedWorktree>> {
        let _lock = RepositoryLock::try_acquire(&self.common_dir)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::WouldBlock, "Worktree repository is busy")
        })?;
        let records = self.common_dir.join("cyber-worktrees");
        let metadata = match std::fs::symlink_metadata(&records) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid(
                "Worktree ownership directory is not a regular directory",
            ));
        }
        let mut paths = std::fs::read_dir(records)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?;
        paths.retain(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        });
        paths.sort();
        let mut listed = Vec::with_capacity(paths.len());
        for path in paths {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let entry = match self.inspect_record(execution, &path, &name).await {
                Ok(entry) => entry,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
                Err(error) => ListedWorktree::Invalid {
                    name,
                    error: error.to_string(),
                },
            };
            listed.push(entry);
        }
        Ok(listed)
    }

    async fn inspect_record(
        &self,
        execution: &dyn GitExecution,
        record: &Path,
        name: &str,
    ) -> io::Result<ListedWorktree> {
        Name::parse(name).map_err(invalid)?;
        let metadata = std::fs::symlink_metadata(record)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(invalid("Worktree ownership record is not a regular file"));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(record)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 1024 * 1024 {
            return Err(invalid("Worktree ownership record exceeds 1 MiB"));
        }
        let managed: Managed =
            serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
        if managed.name != name
            || managed.common_dir != self.common_dir
            || !managed.path.is_absolute()
        {
            return Err(invalid("Managed worktree ownership does not match record"));
        }
        if !managed.ready {
            return Ok(ListedWorktree::Pending(managed));
        }
        self.verify(execution, &managed).await?;
        Ok(ListedWorktree::Ready(managed))
    }

    pub async fn discover(execution: &dyn GitExecution, location: &Path) -> io::Result<Self> {
        let root = git_path(execution, location, "--show-toplevel").await?;
        let common_dir = git_path(execution, location, "--git-common-dir").await?;
        Ok(Self { root, common_dir })
    }

    /// Create or verify a managed worktree. Contention returns WouldBlock so the
    /// runtime can retry with cancellation-aware waiting rather than block a thread.
    pub async fn create(
        &self,
        execution: &dyn GitExecution,
        settings: &Settings,
        data: &Path,
        project_id: &str,
        name: &Name,
    ) -> io::Result<Managed> {
        let _lock = RepositoryLock::try_acquire(&self.common_dir)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::WouldBlock, "Worktree repository is busy")
        })?;
        let target = self.target(settings, data, project_id, name)?;
        let git_target = git_path_argument(&target)?;
        let records = self.common_dir.join("cyber-worktrees");
        let record = records.join(format!("{}.json", name.as_str()));
        if record.try_exists()? {
            return self
                .reuse(execution, &record, &target, settings, name)
                .await;
        }
        match std::fs::symlink_metadata(&target) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Unmanaged worktree path exists",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let branch = format!("{}{}", settings.branch_prefix, name.as_str());
        git(
            execution,
            &self.root,
            &[
                "check-ref-format".into(),
                "--branch".into(),
                branch.clone().into(),
            ],
        )
        .await?;
        let base = commit(execution, &self.root, &settings.base).await?;
        let mut managed = Managed {
            id: crate::ids::new_id("wt"),
            name: name.as_str().into(),
            path: target,
            branch,
            base,
            common_dir: self.common_dir.clone(),
            ready: false,
            included: Vec::new(),
        };
        std::fs::create_dir_all(&records)?;
        write_new(&record, &managed)?;
        // Materialize only the reserved target so Linux can bind it into the sandbox.
        std::fs::create_dir(&managed.path)?;
        // A failed/cancelled creation leaves the pending record; do not delete user work.
        git(
            execution,
            &self.root,
            &[
                "worktree".into(),
                "add".into(),
                "--no-checkout".into(),
                "-b".into(),
                managed.branch.clone().into(),
                "--".into(),
                git_target,
                managed.base.clone().into(),
            ],
        )
        .await?;
        // Run from the new worktree without Git's long absolute GIT_DIR checkout environment.
        git(
            execution,
            &managed.path,
            &["read-tree".into(), managed.base.clone().into()],
        )
        .await?;
        // Never overwrite files written between registration and initial checkout.
        git(
            execution,
            &managed.path,
            &["checkout-index".into(), "--all".into()],
        )
        .await?;
        self.verify(execution, &managed).await?;
        managed.included = super::includes::copy(execution, &self.root, &managed.path).await?;
        self.verify(execution, &managed).await?;
        managed.ready = true;
        replace_record(&record, &managed)?;
        Ok(managed)
    }

    /// Resolve and create the storage root after runtime permission admission.
    /// The named target itself is reserved under the repository lock by `create`.
    pub fn target(
        &self,
        settings: &Settings,
        data: &Path,
        project_id: &str,
        name: &Name,
    ) -> io::Result<PathBuf> {
        if project_id.is_empty()
            || !project_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(invalid("Invalid project ID for worktree storage"));
        }
        let root = match &settings.root {
            Some(path) if path.is_absolute() => path.clone(),
            Some(path) => self.root.join(path),
            None if data.is_absolute() => data.join("worktrees").join(project_id),
            None => return Err(invalid("Worktree data directory must be absolute")),
        };
        std::fs::create_dir_all(&root)?;
        Ok(root.canonicalize()?.join(name.as_str()))
    }

    async fn reuse(
        &self,
        execution: &dyn GitExecution,
        record: &Path,
        target: &Path,
        settings: &Settings,
        name: &Name,
    ) -> io::Result<Managed> {
        let managed: Managed =
            serde_json::from_slice(&std::fs::read(record)?).map_err(|e| invalid(e.to_string()))?;
        if managed.path != target
            || managed.common_dir != self.common_dir
            || managed.name != name.as_str()
            || managed.branch != format!("{}{}", settings.branch_prefix, name.as_str())
        {
            return Err(invalid("Managed worktree ownership does not match request"));
        }
        if !managed.ready {
            return Err(io::Error::other(
                "Worktree creation is incomplete; recovery is required",
            ));
        }
        self.verify(execution, &managed).await?;
        Ok(managed)
    }

    pub(super) async fn verify(
        &self,
        execution: &dyn GitExecution,
        managed: &Managed,
    ) -> io::Result<()> {
        linked_git_directory(&managed.path)?;
        let found = Self::discover(execution, &managed.path).await?;
        let branch = line(
            git(
                execution,
                &managed.path,
                &["symbolic-ref".into(), "--short".into(), "HEAD".into()],
            )
            .await?,
        )?;
        if found.common_dir != self.common_dir
            || found.root != managed.path
            || branch != managed.branch
        {
            return Err(invalid("Managed worktree repository or branch changed"));
        }
        self.registration(execution, managed).await
    }

    async fn registration(
        &self,
        execution: &dyn GitExecution,
        managed: &Managed,
    ) -> io::Result<()> {
        let output = git(
            execution,
            &self.root,
            &[
                "worktree".into(),
                "list".into(),
                "--porcelain".into(),
                "-z".into(),
            ],
        )
        .await?;
        let expected = format!("branch refs/heads/{}", managed.branch);
        let mut matching_path = false;
        for field in output.split(|byte| *byte == 0) {
            if field.is_empty() {
                matching_path = false;
            } else if let Some(path) = field.strip_prefix(b"worktree ") {
                matching_path = std::str::from_utf8(path)
                    .ok()
                    .and_then(|value| Path::new(value).canonicalize().ok())
                    .is_some_and(|path| path == managed.path);
            } else if matching_path && field == expected.as_bytes() {
                return Ok(());
            }
        }
        Err(invalid("Managed worktree registration changed"))
    }
}

pub(super) async fn git(
    execution: &dyn GitExecution,
    directory: &Path,
    args: &[OsString],
) -> io::Result<Vec<u8>> {
    #[cfg(windows)]
    let (launch_directory, windows_args) = windows_git_launch(directory, args)?;
    #[cfg(windows)]
    let directory = launch_directory.as_path();
    #[cfg(windows)]
    let args = windows_args.as_slice();
    let output = execution.run(directory, args).await?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

#[cfg(windows)]
fn windows_git_launch(directory: &Path, args: &[OsString]) -> io::Result<(PathBuf, Vec<OsString>)> {
    use std::os::windows::ffi::OsStrExt;

    let target = git_path_argument(directory)?;
    if target.encode_wide().count() >= 240 {
        let metadata = git_path_argument(&linked_git_directory(directory)?)?;
        if metadata.encode_wide().count() >= 240 {
            return Err(invalid(
                "Linked Git metadata exceeds supported launch length",
            ));
        }
        let mut prepared = vec![
            "-c".into(),
            "core.longpaths=true".into(),
            "--git-dir".into(),
            metadata.clone(),
        ];
        prepared.extend_from_slice(args);
        if args.first().is_some_and(|value| value == "checkout-index") {
            let mut prefix = OsString::from("--prefix=");
            prefix.push(target);
            prefix.push("/");
            prepared.push(prefix);
        }
        return Ok((PathBuf::from(metadata), prepared));
    }
    for ancestor in directory.ancestors() {
        let launch = git_path_argument(ancestor)?;
        // CreateProcess cannot use a long current directory, even with long-path opt-in.
        if launch.encode_wide().count() < 240 {
            let args = [
                "-c".into(),
                "core.longpaths=true".into(),
                "-C".into(),
                target,
            ]
            .into_iter()
            .chain(args.iter().cloned())
            .collect();
            return Ok((PathBuf::from(launch), args));
        }
    }
    Err(invalid("No supported short Git launch directory"))
}

async fn git_path(
    execution: &dyn GitExecution,
    directory: &Path,
    flag: &str,
) -> io::Result<PathBuf> {
    #[cfg(windows)]
    if flag == "--show-toplevel" {
        use std::os::windows::ffi::OsStrExt;
        if git_path_argument(directory)?.encode_wide().count() >= 240 {
            linked_git_directory(directory)?;
            return directory.canonicalize();
        }
    }
    let path = PathBuf::from(line(
        git(
            execution,
            directory,
            &[
                "rev-parse".into(),
                "--path-format=absolute".into(),
                flag.into(),
            ],
        )
        .await?,
    )?);
    if !path.is_absolute() {
        return Err(invalid("Git returned a relative repository path"));
    }
    path.canonicalize()
}

async fn commit(
    execution: &dyn GitExecution,
    directory: &Path,
    revision: &str,
) -> io::Result<String> {
    let result = line(
        git(
            execution,
            directory,
            &[
                "rev-parse".into(),
                "--verify".into(),
                "--end-of-options".into(),
                format!("{revision}^{{commit}}").into(),
            ],
        )
        .await?,
    )?;
    if ![40, 64].contains(&result.len()) || !result.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("Git returned an invalid commit ID"));
    }
    Ok(result)
}

fn line(bytes: Vec<u8>) -> io::Result<String> {
    let value = String::from_utf8(bytes).map_err(|e| invalid(e.to_string()))?;
    Ok(value.strip_suffix('\n').unwrap_or(&value).to_string())
}

fn write_new(path: &Path, value: &Managed) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| invalid(e.to_string()))?;
    let mut file = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()
}

fn replace_record(path: &Path, value: &Managed) -> io::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", ulid::Ulid::new()));
    write_new(&temporary, value)?;
    std::fs::rename(&temporary, path)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn linked_git_directory(directory: &Path) -> io::Result<PathBuf> {
    let marker = directory.join(".git");
    if !std::fs::symlink_metadata(&marker)?.is_file() {
        return Err(invalid("Managed worktree Git marker is not a regular file"));
    }
    let pointer = read_pointer(&marker)?;
    let metadata = directory
        .join(
            pointer
                .strip_prefix("gitdir: ")
                .ok_or_else(|| invalid("Invalid Git marker"))?,
        )
        .canonicalize()?;
    let backpointer = read_pointer(&metadata.join("gitdir"))?;
    if metadata.join(backpointer).canonicalize()? != marker.canonicalize()? {
        return Err(invalid("Managed worktree Git backpointer changed"));
    }
    Ok(metadata)
}

fn read_pointer(path: &Path) -> io::Result<String> {
    let mut value = String::new();
    std::fs::File::open(path)?
        .take(65537)
        .read_to_string(&mut value)?;
    if value.len() > 65536 {
        return Err(invalid("Git pointer exceeds supported size"));
    }
    let value = value.strip_suffix('\n').unwrap_or(&value);
    Ok(value.strip_suffix('\r').unwrap_or(value).to_string())
}

#[cfg(not(windows))]
fn git_path_argument(path: &Path) -> io::Result<OsString> {
    Ok(path.as_os_str().into())
}

#[cfg(windows)]
fn git_path_argument(path: &Path) -> io::Result<OsString> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Component, Prefix};
    for component in path.components() {
        if let Component::Normal(part) = component {
            windows_component(part)?;
        }
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::VerbatimDisk(_) => Ok(OsString::from_wide(&wide[4..])),
            Prefix::VerbatimUNC(_, _) => {
                let mut normal = vec![b'\\' as u16; 2];
                normal.extend_from_slice(&wide[8..]);
                Ok(OsString::from_wide(&normal))
            }
            Prefix::Verbatim(_) | Prefix::DeviceNS(_) => {
                Err(invalid("Unsupported Git path namespace"))
            }
            _ => Ok(path.as_os_str().into()),
        },
        _ => Ok(path.as_os_str().into()),
    }
}

#[cfg(windows)]
pub(super) fn windows_component(part: &std::ffi::OsStr) -> io::Result<()> {
    let value = part
        .to_str()
        .ok_or_else(|| invalid("Git path is not valid Unicode"))?;
    let base = value
        .split('.')
        .next()
        .unwrap_or(value)
        .to_ascii_uppercase();
    let device = ["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
        || base
            .strip_prefix("COM")
            .or_else(|| base.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&suffix)
            });
    if value.ends_with('.') || value.ends_with(' ') || value.contains(':') || device {
        return Err(invalid(
            "Git path changes meaning outside the Windows verbatim namespace",
        ));
    }
    Ok(())
}
