//! The SHA-256 content digest of an installed plugin (`plugins-marketplace` → Plugin
//! sources, Trust and signing). Every regular file under the root except `.git` counts,
//! in sorted `/`-separated relative path order, so the digest is the same on every OS.
//! A link anywhere in the tree fails the digest: a plugin cannot point outside itself.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const MAX_FILES: usize = 4096;
const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// `sha256:<hex>` of the tree at `root`.
pub fn tree(root: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    for (relative, path) in files(root)? {
        let bytes =
            std::fs::read(&path).map_err(|e| format!("{}: cannot be read: {e}", path.display()))?;
        total += bytes.len() as u64;
        if total > MAX_BYTES {
            return Err("plugin exceeds 64 MiB".into());
        }
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// The regular files of `root`, as `(relative path, path)`, refusing links.
pub(super) fn files(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut files = Vec::new();
    walk(root, root, &mut files, 0)?;
    files.sort();
    Ok(files)
}

fn walk(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, PathBuf)>,
    depth: usize,
) -> Result<(), String> {
    if depth > 32 {
        return Err(format!("{}: directory depth exceeds 32", dir.display()));
    }
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("{}: cannot be listed: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        if depth == 0 && entry.file_name() == ".git" {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if kind.is_symlink() {
            return Err(format!("{}: linked files are not allowed", path.display()));
        }
        if kind.is_dir() {
            walk(root, &path, files, depth + 1)?;
            continue;
        }
        let relative = relative(root, &path)?;
        files.push((relative, path));
        if files.len() > MAX_FILES {
            return Err("plugin has more than 4096 files".into());
        }
    }
    Ok(())
}

fn relative(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("{}: outside the plugin", path.display()))?;
    relative
        .components()
        .map(|part| {
            part.as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{}: the name is not UTF-8", path.display()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_covers_paths_and_contents_but_not_git_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("hooks")).unwrap();
        std::fs::write(root.join("hooks/hooks.json"), "{}").unwrap();
        let first = tree(root).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/HEAD"), "x").unwrap();
        assert_eq!(tree(root).unwrap(), first);
        std::fs::write(root.join("hooks/hooks.json"), "{ }").unwrap();
        assert_ne!(tree(root).unwrap(), first);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_fails_the_digest() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/etc/hosts", dir.path().join("hosts")).unwrap();
        assert!(tree(dir.path()).unwrap_err().contains("linked"));
    }
}
