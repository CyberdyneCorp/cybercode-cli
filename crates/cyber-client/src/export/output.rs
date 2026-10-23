//! Writing an export to a file: atomically, through a temporary file in the target's
//! directory renamed over it, and never through a symlink. An existing file keeps its
//! permissions; a new one is readable by its owner only, since it holds the whole
//! conversation.

use std::io::{self, Write};
use std::path::Path;

/// Write `bytes` to `path`. A symlink or anything but a regular file at `path` is
/// refused, so the export never lands somewhere the name does not show.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let existing = match std::fs::symlink_metadata(path) {
        Ok(meta) => Some(meta),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    if let Some(meta) = &existing {
        if meta.file_type().is_symlink() {
            return Err(refused("it is a symlink"));
        }
        if !meta.is_file() {
            return Err(refused("it is not a regular file"));
        }
    }
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let mut file = tempfile::Builder::new()
        .prefix(".cyber-export-")
        .tempfile_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    if let Some(meta) = existing {
        file.as_file().set_permissions(meta.permissions())?;
    }
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn refused(why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, why.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_a_file_and_keeps_its_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.html");
        std::fs::write(&path, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o640);
        }
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(left.len(), 1, "no temporary file is left behind");
    }

    #[test]
    fn refuses_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let error = write_atomic(dir.path(), b"x").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
