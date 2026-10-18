//! The `path` source of `monitor`: a file tail read by this (unsandboxed) process.
//!
//! Permissions and `deny_read` are checked once against the file the path names at start,
//! so every open re-checks that the path still names that same visible file. A symbolic
//! link or hard link swapped in later (a sandboxed command may create one) ends the tail
//! instead of reading what it points to.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use cyber_sandbox::DenyRead;

/// The validated file a tail may read.
pub(super) struct Target {
    path: PathBuf,
    /// The canonical file the checks were made for.
    expected: PathBuf,
    hidden: DenyRead,
}

/// The open file and the offset read so far.
#[derive(Default)]
pub(super) struct State {
    file: Option<File>,
    offset: u64,
}

impl State {
    pub(super) fn from(offset: u64) -> Self {
        Self { file: None, offset }
    }
}

impl Target {
    /// The canonical file `path` names now, or will name once created in its existing
    /// directory.
    pub(super) fn new(path: PathBuf, hidden: DenyRead) -> Result<Self, String> {
        let expected = match std::fs::canonicalize(&path) {
            Ok(canonical) => canonical,
            Err(_) => {
                let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                    return Err(format!("Path not found: {}", path.display()));
                };
                std::fs::canonicalize(parent)
                    .map_err(|_| format!("Path not found: {}", parent.display()))?
                    .join(name)
            }
        };
        Ok(Self {
            path,
            expected,
            hidden,
        })
    }

    /// The canonical file every permission check must also cover.
    pub(super) fn expected(&self) -> &Path {
        &self.expected
    }

    /// Bytes appended since the last poll (at most `limit`). `Err` ends the tail: the path
    /// no longer names the validated file.
    pub(super) fn poll(&self, mut state: State, limit: u64) -> Result<(State, Vec<u8>), String> {
        if state.file.as_ref().is_some_and(|file| !self.current(file)) {
            // Replaced (rotated): read the new file from its start once it validates.
            state = State::default();
        }
        if state.file.is_none() {
            state.file = self.open()?;
        }
        let Some(file) = state.file.as_mut() else {
            return Ok((state, Vec::new()));
        };
        let mut bytes = Vec::new();
        let len = file.metadata().map_or(state.offset, |meta| meta.len());
        if len < state.offset {
            state.offset = 0;
        }
        if file.seek(SeekFrom::Start(state.offset)).is_ok() {
            let _ = file.take(limit).read_to_end(&mut bytes);
        }
        state.offset += bytes.len() as u64;
        Ok((state, bytes))
    }

    /// The file, opened only while the path resolves to the validated, visible file
    /// before and after the open; `None` while it does not exist yet.
    fn open(&self) -> Result<Option<File>, String> {
        let Ok(before) = std::fs::canonicalize(&self.path) else {
            return Ok(None);
        };
        self.verify(&before)?;
        let file = match open_no_follow(&self.expected) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("Could not open {}: {error}", self.path.display())),
        };
        let after = std::fs::canonicalize(&self.path).map_err(|_| self.moved())?;
        self.verify(&after)?;
        if !same_file(&file, &self.expected) {
            return Err(self.moved());
        }
        Ok(Some(file))
    }

    fn verify(&self, canonical: &Path) -> Result<(), String> {
        if canonical != self.expected {
            return Err(self.moved());
        }
        if self.hidden.denies(canonical) {
            return Err(format!("Sandbox denied: read {}", canonical.display()));
        }
        Ok(())
    }

    fn moved(&self) -> String {
        format!("{} no longer names the monitored file", self.path.display())
    }

    /// Whether the validated file is still the open one (not rotated or replaced).
    fn current(&self, file: &File) -> bool {
        same_file(file, &self.expected)
    }
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

/// `file` is the single-link file `path` names, without following a final symbolic link.
#[cfg(unix)]
fn same_file(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let (Ok(open), Ok(named)) = (file.metadata(), std::fs::symlink_metadata(path)) else {
        return false;
    };
    open.dev() == named.dev() && open.ino() == named.ino() && open.nlink() == 1
}

/// Windows has no stable file identity in std; the canonical path checks stand alone.
#[cfg(not(unix))]
fn same_file(_file: &File, path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(path: &Path) -> Target {
        Target::new(path.to_path_buf(), DenyRead::default()).unwrap()
    }

    #[test]
    fn a_truncated_file_is_read_from_its_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let tail = target(&path);
        let (state, bytes) = tail.poll(State::from(4), 1024).unwrap();
        assert_eq!((bytes.as_slice(), state.offset), (&b"two\n"[..], 8));
        std::fs::write(&path, "x\n").unwrap();
        let (state, bytes) = tail.poll(state, 1024).unwrap();
        assert_eq!((bytes.as_slice(), state.offset), (&b"x\n"[..], 2));
    }

    /// Regression: a symbolic link created where a missing monitored file was expected
    /// ends the tail instead of reading the link's target.
    #[cfg(unix)]
    #[test]
    fn a_symlink_swapped_in_ends_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        std::fs::write(&secret, "PRIVATE\n").unwrap();
        let path = dir.path().join("new.log");
        let tail = target(&path);
        let (state, bytes) = tail.poll(State::default(), 1024).unwrap();
        assert!(bytes.is_empty());
        std::os::unix::fs::symlink(&secret, &path).unwrap();
        let refused = tail.poll(state, 1024).err().expect("refused");
        assert!(refused.contains("no longer names"), "{refused}");
    }

    /// Regression: a hard link to another file is refused, and a rotated file is checked
    /// again before it is read.
    #[cfg(unix)]
    #[test]
    fn a_hard_link_is_refused_even_after_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("secret");
        std::fs::write(&secret, "PRIVATE\n").unwrap();
        let path = dir.path().join("app.log");
        std::fs::write(&path, "one\n").unwrap();
        let tail = target(&path);
        let (state, bytes) = tail.poll(State::default(), 1024).unwrap();
        assert_eq!(bytes, b"one\n");
        std::fs::remove_file(&path).unwrap();
        std::fs::hard_link(&secret, &path).unwrap();
        assert!(tail.poll(state, 1024).is_err());
    }

    /// Regression: a `deny_read` file is never opened, even when created after start.
    #[test]
    fn a_hidden_file_is_never_opened() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let hidden = DenyRead::new(&["**/*.pem".to_string()], &root, &root);
        let path = root.join("late.pem");
        let tail = Target::new(path.clone(), hidden).unwrap();
        let (state, _) = tail.poll(State::default(), 1024).unwrap();
        std::fs::write(&path, "PRIVATE\n").unwrap();
        let refused = tail.poll(state, 1024).err().expect("refused");
        assert!(refused.starts_with("Sandbox denied: read"), "{refused}");
    }
}
