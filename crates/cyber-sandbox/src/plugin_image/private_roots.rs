//! Metadata-only alias inventory, not an execution permit or namespace seal.
//! Callers must supply every protected root and exclude untrusted writers before
//! capture/recheck and throughout launch. No regular file is opened by this API.
use std::path::PathBuf;

#[cfg(unix)]
const MAX_ROOTS: usize = 64;
#[cfg(unix)]
const MAX_ENTRIES: usize = 8192;
#[cfg(unix)]
const MAX_DEPTH: usize = 32;
#[cfg(unix)]
const MAX_PATH: usize = 4096;
#[cfg(unix)]
const MAX_PATH_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("private-root inventory is unavailable on this platform")]
    Unavailable,
    #[error("invalid private-root inventory request")]
    Request,
    #[error("private-root metadata could not be inspected")]
    Io,
    #[error("private-root inventory contains an alias or unsupported object")]
    Unsafe,
    #[error("private-root inventory changed")]
    Changed,
    #[error("private-root inventory exceeded its metadata limit")]
    Limit,
}

/// Retains only directory handles and bounded metadata. It deliberately contains
/// no private contents and supplies no way to reopen a private regular file.
#[derive(Debug)]
pub struct Inventory {
    #[cfg(unix)]
    captured: unix::Captured,
}
impl Inventory {
    pub fn capture(roots: &[PathBuf]) -> Result<Self, Error> {
        #[cfg(unix)]
        return Ok(Self {
            captured: unix::Captured::read(roots)?,
        });
        #[cfg(not(unix))]
        {
            let _ = roots;
            Err(Error::Unavailable)
        }
    }
    /// Compare a fresh metadata inventory with the captured incarnation. This is
    /// a check, not exclusion against a writer racing after it returns.
    pub fn recheck(&self) -> Result<(), Error> {
        #[cfg(unix)]
        return self.captured.recheck();
        #[cfg(not(unix))]
        Err(Error::Unavailable)
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use cap_fs_ext::DirExt;
    use cap_std::fs::{Dir, Metadata, MetadataExt};
    use std::collections::BTreeMap;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Component, Path};
    use std::time::SystemTime;

    #[derive(Debug, PartialEq, Eq)]
    struct State {
        identity: (u64, u64),
        directory: bool,
        links: u64,
        length: u64,
        mode: u32,
        owner: (u32, u32),
        modified: SystemTime,
        changed: (i64, i64),
    }
    impl State {
        fn read(metadata: &Metadata) -> Result<Self, Error> {
            if !(metadata.is_dir() || metadata.is_file())
                || (metadata.is_file() && metadata.nlink() != 1)
            {
                return Err(Error::Unsafe);
            }
            Ok(Self {
                identity: (metadata.dev(), metadata.ino()),
                directory: metadata.is_dir(),
                links: metadata.nlink(),
                length: metadata.len(),
                mode: metadata.mode(),
                owner: (metadata.uid(), metadata.gid()),
                modified: metadata.modified().map_err(|_| Error::Io)?.into_std(),
                changed: (metadata.ctime(), metadata.ctime_nsec()),
            })
        }
    }

    #[derive(Debug)]
    pub(super) struct Captured {
        roots: Vec<PathBuf>,
        _parents: Vec<Dir>,
        records: BTreeMap<PathBuf, State>,
        path_bytes: usize,
    }
    impl Captured {
        pub(super) fn read(roots: &[PathBuf]) -> Result<Self, Error> {
            if roots.is_empty() || roots.len() > MAX_ROOTS {
                return Err(Error::Request);
            }
            for root in roots {
                validate(root)?;
                if root.file_name().is_none() {
                    return Err(Error::Request);
                }
            }
            let mut captured = Self {
                roots: roots.to_vec(),
                _parents: Vec::new(),
                records: BTreeMap::new(),
                path_bytes: 0,
            };
            for root in roots {
                let name = root.file_name().ok_or(Error::Request)?;
                let parent = directory(root.parent().ok_or(Error::Request)?)?;
                captured.visit(&parent, Path::new(name), root, 0)?;
                captured._parents.push(parent);
            }
            Ok(captured)
        }
        pub(super) fn recheck(&self) -> Result<(), Error> {
            let current = Self::read(&self.roots)?;
            if current.records != self.records {
                return Err(Error::Changed);
            }
            for (old, new) in self._parents.iter().zip(&current._parents) {
                let old = old.dir_metadata().map_err(|_| Error::Io)?;
                let new = new.dir_metadata().map_err(|_| Error::Io)?;
                if (old.dev(), old.ino()) != (new.dev(), new.ino()) {
                    return Err(Error::Changed);
                }
            }
            if current._parents.len() != self._parents.len() {
                return Err(Error::Changed);
            }
            Ok(())
        }
        fn insert(&mut self, path: &Path, state: State) -> Result<(), Error> {
            if let Some(previous) = self.records.get(path) {
                return if previous == &state {
                    Ok(())
                } else {
                    Err(Error::Changed)
                };
            }
            let bytes = path.as_os_str().as_bytes().len();
            if bytes > MAX_PATH
                || self.records.len() >= MAX_ENTRIES
                || self.path_bytes + bytes > MAX_PATH_BYTES
            {
                return Err(Error::Limit);
            }
            self.path_bytes += bytes;
            self.records.insert(path.into(), state);
            Ok(())
        }
        fn visit(
            &mut self,
            parent: &Dir,
            name: &Path,
            path: &Path,
            depth: usize,
        ) -> Result<(), Error> {
            if depth > MAX_DEPTH {
                return Err(Error::Limit);
            }
            let state = State::read(&parent.symlink_metadata(name).map_err(|_| Error::Io)?)?;
            let is_directory = state.directory;
            let identity = state.identity;
            self.insert(path, state)?;
            if is_directory {
                let child = parent.open_dir_nofollow(name).map_err(|_| Error::Io)?;
                let opened = State::read(&child.dir_metadata().map_err(|_| Error::Io)?)?;
                if opened.identity != identity || self.records.get(path) != Some(&opened) {
                    return Err(Error::Changed);
                }
                for entry in child.entries().map_err(|_| Error::Io)? {
                    let name = entry.map_err(|_| Error::Io)?.file_name();
                    self.visit(&child, Path::new(&name), &path.join(&name), depth + 1)?;
                }
                let settled = State::read(&child.dir_metadata().map_err(|_| Error::Io)?)?;
                if self.records.get(path) != Some(&settled) {
                    return Err(Error::Changed);
                }
            }
            let settled = State::read(&parent.symlink_metadata(name).map_err(|_| Error::Io)?)?;
            if self.records.get(path) != Some(&settled) {
                return Err(Error::Changed);
            }
            Ok(())
        }
    }
    fn validate(path: &Path) -> Result<(), Error> {
        if !path.is_absolute()
            || path.as_os_str().as_bytes().len() > MAX_PATH
            || path
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return Err(Error::Request);
        }
        Ok(())
    }
    fn directory(path: &Path) -> Result<Dir, Error> {
        let mut current =
            Dir::open_ambient_dir("/", cap_std::ambient_authority()).map_err(|_| Error::Io)?;
        for part in path.components() {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => {
                    current = current.open_dir_nofollow(name).map_err(|_| Error::Io)?
                }
                _ => return Err(Error::Request),
            }
        }
        Ok(current)
    }
}

#[cfg(test)]
mod tests;
