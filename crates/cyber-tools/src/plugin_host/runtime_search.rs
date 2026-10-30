//! Prevent Location/package PATH entries from choosing the initial runtime.
//! This is a search guard, not immutable image or dependency admission.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub(super) fn find(names: &[&str], location: &Path, package: &Path) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    search(names, &path, location, package)
}

fn search(names: &[&str], path: &OsStr, location: &Path, package: &Path) -> Option<PathBuf> {
    let excluded = [location.canonicalize().ok()?, package.canonicalize().ok()?];
    find_in(names, path, &excluded)
}

fn find_in(names: &[&str], path: &OsStr, excluded: &[PathBuf]) -> Option<PathBuf> {
    // Preserve runtime-name preference (python3 before python), then PATH order.
    names.iter().find_map(|name| {
        std::env::split_paths(path)
            .filter(|directory| directory.is_absolute())
            .filter_map(|directory| directory.canonicalize().ok())
            .filter(|directory| !excluded.iter().any(|root| directory.starts_with(root)))
            .find_map(|directory| find_at(name, &directory, excluded))
    })
}

fn find_at(name: &str, directory: &Path, excluded: &[PathBuf]) -> Option<PathBuf> {
    #[cfg(windows)]
    let names = [format!("{name}.exe"), format!("{name}.cmd")];
    #[cfg(not(windows))]
    let names = [name.to_owned()];
    names.iter().find_map(|name| {
        let candidate = directory.join(name).canonicalize().ok()?;
        if candidate.to_str().is_none()
            || excluded.iter().any(|root| candidate.starts_with(root))
            || !executable(&candidate)
        {
            return None;
        }
        Some(candidate)
    })
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

#[cfg(test)]
mod tests;
