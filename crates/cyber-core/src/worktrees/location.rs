//! Filesystem-only identification; actual admission revalidates ownership under RepoLock.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use super::{Managed, Name, Repository};

impl Repository {
    /// Identify an owned linked checkout containing this Location. Ordinary roots,
    /// primary checkouts and submodules do not require Git execution or a lease.
    pub fn managed_at(directory: &Path) -> io::Result<Option<(Self, Managed)>> {
        Ok(Self::managed_locations_at(directory)?.into_iter().next())
    }

    /// Include every enclosing managed checkout, nearest first. A nested checkout
    /// also needs its parents fenced because their removal deletes its Location.
    pub fn managed_locations_at(directory: &Path) -> io::Result<Vec<(Self, Managed)>> {
        let directory = directory.canonicalize()?;
        let mut found = Vec::new();
        for root in directory.ancestors() {
            let marker = root.join(".git");
            let metadata = match std::fs::symlink_metadata(&marker) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                continue;
            }
            regular_file(&marker)?;
            let pointer = super::repository::read_pointer(&marker)?;
            let git_dir = root
                .join(
                    pointer
                        .strip_prefix("gitdir: ")
                        .ok_or_else(|| invalid("Invalid Git marker"))?,
                )
                .canonicalize()?;
            let common = git_dir.join("commondir");
            if !optional_regular_file(&common)? {
                continue;
            }
            regular_file(&common)?;
            let common_dir = git_dir
                .join(super::repository::read_pointer(&common)?)
                .canonicalize()?;
            super::repository::linked_git_directory(root)?;
            let Some(name) = root
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| Name::parse(name).ok())
            else {
                continue;
            };
            let records = common_dir.join("cyber-worktrees");
            match std::fs::symlink_metadata(&records) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(invalid(
                        "Worktree ownership directory is not a regular directory",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }
            let path = records.join(format!("{}.json", name.as_str()));
            if !optional_regular_file(&path)? {
                continue;
            }
            regular_file(&path)?;
            let mut bytes = Vec::new();
            File::open(&path)?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 1024 * 1024 {
                return Err(invalid("Worktree ownership record exceeds 1 MiB"));
            }
            let managed: Managed = serde_json::from_slice(&bytes).map_err(invalid)?;
            // An unrelated linked checkout may share a managed checkout's basename.
            if managed.path != root {
                continue;
            }
            if managed.common_dir != common_dir || managed.name != name.as_str() {
                return Err(invalid(
                    "Managed worktree ownership does not match Location",
                ));
            }
            found.push((
                Self {
                    root: root.to_path_buf(),
                    common_dir,
                },
                managed,
            ));
        }
        Ok(found)
    }
}

fn regular_file(path: &Path) -> io::Result<()> {
    if !optional_regular_file(path)? {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Worktree pointer or ownership is missing",
        ));
    }
    Ok(())
}

fn optional_regular_file(path: &Path) -> io::Result<bool> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid(
            "Worktree pointer or ownership is not a regular file",
        ));
    }
    Ok(true)
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
