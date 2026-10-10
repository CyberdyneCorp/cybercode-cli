//! Directory-bound memory reads and exclusive per-scope ownership.
mod transaction;
use super::{IndexSnapshot, MemoryDocument, MemoryError, MemoryMetadata, directory, validate_name};
#[cfg(not(windows))]
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
#[cfg(not(windows))]
use cap_std::fs::OpenOptions;
use cap_std::fs::{Dir, DirBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fs::{File, TryLockError};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
pub use transaction::{
    MemoryEditReview, MemoryJournalIdentity, MemoryMutation, MemoryRecoveryReview, PreparedMemory,
    ReviewedMemory,
};

const NOTE_LIMIT: u64 = 1_048_576;
const ENTRY_LIMIT: usize = 4096;
const TRANSACTION: &str = ".memory-transaction";

#[derive(Debug, thiserror::Error)]
pub enum MemoryStorageError {
    #[error(transparent)]
    Format(#[from] MemoryError),
    #[error("Memory storage unavailable")]
    Io(#[source] io::Error),
    #[error("Memory storage is busy")]
    Busy,
    #[error("Memory recovery required")]
    RecoveryRequired,
    #[error("Unsafe memory storage: {0}")]
    Unsafe(&'static str),
    #[error("Memory not found")]
    NotFound,
    #[error("Memory changed during mutation; transaction evidence retained")]
    Conflict,
    #[error("Memory changed since review")]
    ReviewConflict,
    #[error("Memory file exceeds 1 MiB")]
    TooLarge,
}
impl From<io::Error> for MemoryStorageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct MemoryStore {
    dir: Dir,
    path: PathBuf,
}

pub struct MemoryScope<'a> {
    store: &'a MemoryStore,
    lock: File,
}
impl Drop for MemoryScope<'_> {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InvalidMemory {
    pub filename: String,
    pub diagnostic: String,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryCatalog {
    pub memories: Vec<MemoryMetadata>,
    pub invalid: Vec<InvalidMemory>,
}

impl MemoryStore {
    /// Explicit storage admission creates private directories, never note/index files.
    pub fn open(data: &Path, project_id: &str) -> Result<Self, MemoryStorageError> {
        let path = directory(data, project_id)?;
        let data = Dir::open_ambient_dir(data, cap_std::ambient_authority())?;
        let root = private_directory(&data, "memory")?;
        let dir = private_directory(&root, project_id)?;
        Ok(Self { dir, path })
    }

    #[cfg(windows)]
    pub(crate) fn ensure_root(data: &Path) -> Result<(), MemoryStorageError> {
        let data = Dir::open_ambient_dir(data, cap_std::ambient_authority())?;
        private_directory(&data, "memory")?;
        Ok(())
    }

    /// Review existing storage without creating directories or changing permissions.
    pub fn existing(data: &Path, project_id: &str) -> Result<Option<Self>, MemoryStorageError> {
        let path = directory(data, project_id)?;
        #[cfg(windows)]
        {
            let data = match Dir::open_ambient_dir(data, cap_std::ambient_authority()) {
                Ok(data) => data,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            let Some(root) = existing_private_directory(&data, "memory")? else {
                return Ok(None);
            };
            let Some(dir) = existing_private_directory(&root, project_id)? else {
                return Ok(None);
            };
            Ok(Some(Self { dir, path }))
        }
        #[cfg(not(windows))]
        {
            let open = || -> io::Result<Dir> {
                let data = Dir::open_ambient_dir(data, cap_std::ambient_authority())?;
                data.open_dir_nofollow("memory")?
                    .open_dir_nofollow(project_id)
            };
            let dir = match open() {
                Ok(dir) => dir,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            verify_private_directory(&dir)?;
            Ok(Some(Self { dir, path }))
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Callers retain this guard through their complete read or mutation/recovery.
    /// Contention is explicit so async consumers can wait/cancel without blocking.
    pub fn claim(&self) -> Result<MemoryScope<'_>, MemoryStorageError> {
        verify_private_directory(&self.dir)?;
        match self.dir.symlink_metadata(".memory.lock") {
            Ok(metadata) if !metadata.is_file() => {
                return Err(MemoryStorageError::Unsafe("expected a regular lock file"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        #[cfg(windows)]
        let lock = {
            let parent = self.dir.try_clone()?.into_std_file();
            match super::windows::create_private_file(&parent, ".memory.lock") {
                Ok(file) => file,
                Err(MemoryStorageError::Io(error))
                    if error.kind() == io::ErrorKind::AlreadyExists =>
                {
                    super::windows::open_private_file(
                        &parent,
                        ".memory.lock",
                        super::windows::Access::Write,
                    )?
                }
                Err(error) => return Err(error),
            }
        };
        #[cfg(not(windows))]
        let lock = {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .write(true)
                .create(true)
                .follow(FollowSymlinks::No);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            self.dir.open_with(".memory.lock", &options)?.into_std()
        };
        verify_regular(&lock)?;
        make_private_file(&lock)?;
        match lock.try_lock() {
            Ok(()) => Ok(MemoryScope { store: self, lock }),
            Err(TryLockError::WouldBlock) => Err(MemoryStorageError::Busy),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }
}

impl MemoryScope<'_> {
    pub fn path(&self) -> &Path {
        &self.store.path
    }
    fn ready(&self) -> Result<(), MemoryStorageError> {
        match self.store.dir.symlink_metadata(TRANSACTION) {
            Ok(_) => Err(MemoryStorageError::RecoveryRequired),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn read(&self, name: &str) -> Result<MemoryDocument, MemoryStorageError> {
        self.ready()?;
        self.read_unchecked(name)
    }

    fn read_unchecked(&self, name: &str) -> Result<MemoryDocument, MemoryStorageError> {
        validate_name(name)?;
        let bytes = self.read_file(&format!("{name}.md"), NOTE_LIMIT)?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| MemoryStorageError::Unsafe("invalid UTF-8 memory"))?;
        let document = MemoryDocument::parse(text)?;
        if document.metadata.name != name {
            return Err(MemoryStorageError::Unsafe(
                "filename and memory name differ",
            ));
        }
        Ok(document)
    }

    pub fn list(&self) -> Result<MemoryCatalog, MemoryStorageError> {
        self.ready()?;
        let catalog = self.catalog_unchecked()?;
        self.ready()?;
        Ok(catalog)
    }

    fn catalog_unchecked(&self) -> Result<MemoryCatalog, MemoryStorageError> {
        let mut catalog = MemoryCatalog::default();
        let mut captured = 0usize;
        for (index, entry) in self.store.dir.entries()?.enumerate() {
            if index >= ENTRY_LIMIT {
                return Err(MemoryStorageError::Unsafe(
                    "memory directory contains too many entries",
                ));
            }
            let filename = entry?.file_name().to_string_lossy().into_owned();
            if filename == "MEMORY.md" || !filename.ends_with(".md") {
                continue;
            }
            let name = filename.strip_suffix(".md").expect("checked memory suffix");
            match self.read_unchecked(name) {
                Ok(document) => {
                    captured += document.metadata.name.len() + document.metadata.description.len();
                    if captured > 4 * 1_048_576 {
                        return Err(MemoryStorageError::Unsafe("memory catalog exceeds 4 MiB"));
                    }
                    catalog.memories.push(document.metadata);
                }
                Err(MemoryStorageError::RecoveryRequired) => {
                    return Err(MemoryStorageError::RecoveryRequired);
                }
                Err(error) => catalog.invalid.push(InvalidMemory {
                    filename: display_filename(&filename),
                    diagnostic: error.to_string(),
                }),
            }
        }
        catalog.memories.sort_by(|a, b| a.name.cmp(&b.name));
        catalog.invalid.sort_by(|a, b| a.filename.cmp(&b.filename));
        Ok(catalog)
    }

    /// Baseline reads only the index prefix, never individual memory bodies.
    pub fn index(&self) -> Result<IndexSnapshot, MemoryStorageError> {
        self.ready()?;
        let file = match self.open_file("MEMORY.md") {
            Ok(file) => file,
            Err(MemoryStorageError::NotFound) => return Ok(IndexSnapshot::from_prefix("", false)),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(25_004).read_to_end(&mut bytes)?;
        let line_end = bytes
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .nth(199)
            .map_or(bytes.len(), |(index, _)| index + 1);
        let limit = line_end.min(25_000);
        let prefix = match std::str::from_utf8(&bytes[..limit]) {
            Ok(text) => text,
            Err(error) if error.error_len().is_none() => {
                std::str::from_utf8(&bytes[..error.valid_up_to()]).expect("validated UTF-8 prefix")
            }
            Err(_) => return Err(MemoryStorageError::Unsafe("invalid UTF-8 memory index")),
        };
        Ok(IndexSnapshot::from_prefix(
            prefix,
            prefix.len() < bytes.len(),
        ))
    }

    fn open_file(&self, name: &str) -> Result<File, MemoryStorageError> {
        let metadata = self.store.dir.symlink_metadata(name).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                MemoryStorageError::NotFound
            } else {
                error.into()
            }
        })?;
        if !metadata.is_file() {
            return Err(MemoryStorageError::Unsafe("expected a regular file"));
        }
        #[cfg(windows)]
        let file = match super::windows::open_private_file(
            &self.store.dir.try_clone()?.into_std_file(),
            name,
            super::windows::Access::Read,
        ) {
            Ok(file) => file,
            Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Err(MemoryStorageError::NotFound);
            }
            Err(error) => return Err(error),
        };
        #[cfg(not(windows))]
        let file = {
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            match self.store.dir.open_with(name, &options) {
                Ok(file) => file.into_std(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(MemoryStorageError::NotFound);
                }
                Err(error) => return Err(error.into()),
            }
        };
        verify_regular(&file)?;
        verify_private_file(&file)?;
        Ok(file)
    }

    fn read_file(&self, name: &str, limit: u64) -> Result<Vec<u8>, MemoryStorageError> {
        let mut bytes = Vec::new();
        self.open_file(name)?
            .take(limit + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(MemoryStorageError::TooLarge);
        }
        Ok(bytes)
    }
}

fn display_filename(filename: &str) -> String {
    if super::secrets::check(filename).is_err() {
        "[redacted]".into()
    } else {
        filename.into()
    }
}

#[cfg(not(windows))]
fn private_directory(parent: &Dir, name: &str) -> Result<Dir, MemoryStorageError> {
    let builder = private_builder();
    match parent.create_dir_with(name, &builder) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let dir = parent.open_dir_nofollow(name)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        directory_file(&dir)?.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

#[cfg(windows)]
fn private_directory(parent: &Dir, name: &str) -> Result<Dir, MemoryStorageError> {
    let parent = parent.try_clone()?.into_std_file();
    let file = match super::windows::create_private_directory(&parent, name) {
        Ok(file) => file,
        Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
            super::windows::open_private_directory(&parent, name, super::windows::Access::Write)?
        }
        Err(error) => return Err(error),
    };
    Ok(Dir::from_std_file(file))
}
#[cfg(windows)]
fn existing_private_directory(parent: &Dir, name: &str) -> Result<Option<Dir>, MemoryStorageError> {
    match super::windows::open_private_directory(
        &parent.try_clone()?.into_std_file(),
        name,
        super::windows::Access::Write,
    ) {
        Ok(file) => Ok(Some(Dir::from_std_file(file))),
        Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn private_builder() -> DirBuilder {
    #[cfg(unix)]
    {
        use cap_std::fs::DirBuilderExt;
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        builder
    }
    #[cfg(not(unix))]
    {
        DirBuilder::new()
    }
}

pub(super) fn verify_regular(file: &File) -> Result<(), MemoryStorageError> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(MemoryStorageError::Unsafe("expected a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(MemoryStorageError::Unsafe("hard-linked memory file"));
        }
    }
    #[cfg(windows)]
    super::windows::verify_regular(file)?;
    Ok(())
}
fn make_private_file(file: &File) -> Result<(), MemoryStorageError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    super::windows::verify_private(file)?;
    #[cfg(not(any(unix, windows)))]
    let _ = file;
    Ok(())
}
pub(super) fn verify_private_file(file: &File) -> Result<(), MemoryStorageError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if file.metadata()?.permissions().mode() & 0o077 != 0 {
            return Err(MemoryStorageError::Unsafe(
                "memory file permissions are not private",
            ));
        }
    }
    #[cfg(windows)]
    super::windows::verify_private(file)?;
    #[cfg(not(any(unix, windows)))]
    let _ = file;
    Ok(())
}
pub(super) fn verify_private_directory(dir: &Dir) -> Result<(), MemoryStorageError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if dir
            .try_clone()?
            .into_std_file()
            .metadata()?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err(MemoryStorageError::Unsafe(
                "memory directory permissions are not private",
            ));
        }
    }
    #[cfg(windows)]
    super::windows::verify_private(&dir.try_clone()?.into_std_file())?;
    #[cfg(not(any(unix, windows)))]
    let _ = dir;
    Ok(())
}

#[cfg(unix)]
pub(super) fn directory_file(dir: &Dir) -> Result<File, MemoryStorageError> {
    use cap_fs_ext::OpenOptionsMaybeDirExt;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .maybe_dir(true)
        .follow(FollowSymlinks::No);
    let file = dir.open_with(".", &options)?.into_std();
    if !file.metadata()?.is_dir() {
        return Err(MemoryStorageError::Unsafe("expected a directory handle"));
    }
    Ok(file)
}

#[cfg(all(test, unix))]
mod directory_handle_tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[test]
    fn readable_handle_supports_chmod_and_sync_on_the_retained_directory_after_namespace_changes() {
        let data = tempfile::tempdir().unwrap();
        let original = data.path().join("original");
        std::fs::create_dir(&original).unwrap();
        let dir = Dir::open_ambient_dir(&original, cap_std::ambient_authority()).unwrap();
        let identity = dir.try_clone().unwrap().into_std_file().metadata().unwrap();
        std::fs::rename(&original, data.path().join("moved")).unwrap();
        std::fs::create_dir(&original).unwrap();
        std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = directory_file(&dir).unwrap();
        assert_eq!(
            (
                file.metadata().unwrap().dev(),
                file.metadata().unwrap().ino()
            ),
            (identity.dev(), identity.ino())
        );
        file.set_permissions(std::fs::Permissions::from_mode(0o700))
            .unwrap();
        file.sync_all().unwrap();
        assert_eq!(
            std::fs::metadata(&original).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            std::fs::metadata(data.path().join("moved"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
