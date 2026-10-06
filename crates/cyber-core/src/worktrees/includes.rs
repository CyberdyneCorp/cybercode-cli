use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use cap_std::fs::{Dir, OpenOptions};
use ignore::gitignore::GitignoreBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::GitExecution;
use super::repository::git;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IncludedFile {
    pub path: PathBuf,
    pub sha256: String,
}

pub(super) async fn copy(
    execution: &dyn GitExecution,
    source: &Path,
    destination: &Path,
) -> io::Result<Vec<IncludedFile>> {
    let Some(source) = main_checkout(execution, source).await? else {
        return Ok(Vec::new());
    };
    let source = source.as_path();
    let source_dir = Dir::open_ambient_dir(source, cap_std::ambient_authority())?;
    let mut policy = String::new();
    match source_dir.open(".worktreeinclude") {
        Ok(file) => {
            file.take(1_048_577).read_to_string(&mut policy)?;
            if policy.len() > 1_048_576 {
                return Err(invalid("Worktree inclusion policy exceeds supported size"));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }
    let mut builder = GitignoreBuilder::new(source);
    for line in policy.lines() {
        builder
            .add_line(None, line)
            .map_err(|error| invalid(error.to_string()))?;
    }
    let matcher = builder
        .build()
        .map_err(|error| invalid(error.to_string()))?;
    let tracked: HashSet<_> = paths(
        git(
            execution,
            destination,
            &["ls-files".into(), "--cached".into(), "-z".into()],
        )
        .await?,
    )?
    .into_iter()
    .collect();
    let untracked = paths(
        git(
            execution,
            source,
            &["ls-files".into(), "--others".into(), "-z".into()],
        )
        .await?,
    )?;
    let destination_dir = Dir::open_ambient_dir(destination, cap_std::ambient_authority())?;
    let mut included = Vec::new();
    for path in untracked {
        if !tracked.contains(&path)
            && matcher
                .matched_path_or_any_parents(&path, false)
                .is_ignore()
        {
            let sha256 = copy_file(&source_dir, &destination_dir, &path)?;
            included.push(IncludedFile { path, sha256 });
        }
    }
    Ok(included)
}

async fn main_checkout(
    execution: &dyn GitExecution,
    location: &Path,
) -> io::Result<Option<PathBuf>> {
    let output = git(
        execution,
        location,
        &[
            "worktree".into(),
            "list".into(),
            "--porcelain".into(),
            "-z".into(),
        ],
    )
    .await?;
    let first: Vec<_> = output
        .split(|byte| *byte == 0)
        .take_while(|field| !field.is_empty())
        .collect();
    if first.contains(&b"bare".as_slice()) {
        return Ok(None);
    }
    let value = first
        .first()
        .and_then(|field| field.strip_prefix(b"worktree "))
        .ok_or_else(|| invalid("Git returned no primary checkout"))?;
    Ok(Some(git_path(value)?.canonicalize()?))
}

fn copy_file(source: &Dir, destination: &Dir, path: &Path) -> io::Result<String> {
    if !source.symlink_metadata(path)?.is_file() {
        return Err(invalid("Worktree inclusion requires regular files"));
    }
    let mut input = source.open(path)?;
    let metadata = input.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("Included file type changed"));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        destination.create_dir_all(parent)?;
    }
    let mut output =
        destination.open_with(path, OpenOptions::new().write(true).create_new(true))?;
    output.set_permissions(metadata.permissions())?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 16384];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        hash.update(&buffer[..count]);
    }
    output.sync_all()?;
    Ok(format!("{:x}", hash.finalize()))
}

fn paths(bytes: Vec<u8>) -> io::Result<Vec<PathBuf>> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let path = git_path(entry)?;
            if !path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
                || path.components().any(|part| {
                    part.as_os_str()
                        .as_encoded_bytes()
                        .eq_ignore_ascii_case(b".git")
                })
            {
                return Err(invalid("Unsafe Git inclusion path"));
            }
            #[cfg(windows)]
            for component in path.components() {
                super::repository::windows_component(component.as_os_str())?;
            }
            Ok(path)
        })
        .collect()
}

#[cfg(unix)]
fn git_path(bytes: &[u8]) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn git_path(bytes: &[u8]) -> io::Result<PathBuf> {
    Ok(PathBuf::from(
        std::str::from_utf8(bytes).map_err(|error| invalid(error.to_string()))?,
    ))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
