//! Pinned root incarnation and private-state exclusions for metadata observation.
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

use cap_std::fs::Dir;
use same_file::Handle;
use sha2::{Digest, Sha256};

pub(super) struct Root {
    pub path: PathBuf,
    pub directory: Dir,
    handle: Handle,
    pub identity: String,
}

struct IdentityHash(Sha256);
impl Hasher for IdentityHash {
    fn finish(&self) -> u64 {
        0
    }
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}
#[cfg(not(windows))]
fn open_directory(path: &Path) -> io::Result<Dir> {
    Dir::open_ambient_dir(path, cap_std::ambient_authority())
}

#[cfg(windows)]
fn open_directory(path: &Path) -> io::Result<Dir> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    // An observer retains identity, not an exclusive pathname lease. In
    // particular it must not prevent a worktree/editor from renaming this root.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::other(
            "observation root is not a regular directory",
        ));
    }
    Ok(Dir::from_std_file(file))
}

impl Root {
    pub fn open(path: &Path) -> io::Result<Self> {
        let path = path.canonicalize()?;
        let directory = open_directory(&path)?;
        let handle = Handle::from_file(directory.try_clone()?.into_std_file())?;
        let mut hash = IdentityHash(Sha256::new());
        handle.hash(&mut hash);
        let identity = format!("file-id-v1:{:x}", hash.0.finalize());
        let root = Self {
            path,
            directory,
            handle,
            identity,
        };
        if !root.current() {
            return Err(io::Error::other("observation root changed while opening"));
        }
        Ok(root)
    }
    pub fn current(&self) -> bool {
        Handle::from_path(&self.path).is_ok_and(|current| current == self.handle)
    }
}

#[derive(Clone)]
pub(super) struct Exclusions {
    location: PathBuf,
    home: PathBuf,
    trees: Vec<PathBuf>,
    files: Vec<PathBuf>,
    protected: fn(&Path, &Path, &Path) -> bool,
}
impl Exclusions {
    pub fn new(
        location: &Path,
        home: &Path,
        trees: Vec<PathBuf>,
        files: Vec<PathBuf>,
        protected: fn(&Path, &Path, &Path) -> bool,
    ) -> Self {
        let aliases = |paths: Vec<PathBuf>| {
            paths
                .into_iter()
                .flat_map(|path| {
                    let canonical = path.canonicalize().ok();
                    std::iter::once(path).chain(canonical)
                })
                .collect()
        };
        Self {
            location: location.into(),
            home: home.into(),
            trees: aliases(trees),
            files: aliases(files),
            protected,
        }
    }
    pub fn excludes(&self, relative: &Path) -> bool {
        let path = self.location.join(relative);
        relative
            .components()
            .any(|part| part.as_os_str() == ".cyber")
            || (self.protected)(&path, &self.location, &self.home)
            || self.trees.iter().any(|tree| path.starts_with(tree))
            || self.files.contains(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_replacement_invalidates_pinned_observation_without_reading_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        std::fs::create_dir(&path).unwrap();
        let root = Root::open(&path).unwrap();
        let identity = root.identity.clone();
        assert!(root.directory.dir_metadata().unwrap().is_dir());
        assert_eq!(Root::open(&path).unwrap().identity, identity);
        std::fs::rename(&path, temp.path().join("old")).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(!root.current());
        assert_ne!(Root::open(&path).unwrap().identity, identity);
    }
    #[test]
    fn nested_private_storage_and_database_sidecars_are_not_observed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap();
        let exclusions = Exclusions::new(
            &path,
            &path.join("home"),
            vec![path.join("state")],
            vec![path.join("cyber.db"), path.join("cyber.db-wal")],
            |_, _, _| false,
        );
        for name in [
            "state/sessions/data",
            "cyber.db",
            "cyber.db-wal",
            ".cyber/hooks.json",
        ] {
            assert!(exclusions.excludes(Path::new(name)), "{name}");
        }
        assert!(!exclusions.excludes(Path::new("src/main.rs")));
    }
    #[cfg(unix)]
    #[test]
    fn private_root_aliases_are_excluded_at_their_physical_location() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap();
        std::fs::create_dir(path.join("private")).unwrap();
        std::os::unix::fs::symlink(path.join("private"), path.join("alias")).unwrap();
        let excluded = Exclusions::new(
            &path,
            &path.join("home"),
            vec![path.join("alias")],
            vec![],
            |_, _, _| false,
        );
        assert!(excluded.excludes(Path::new("private/secret")));
        assert!(excluded.excludes(Path::new("alias/secret")));
    }
}
