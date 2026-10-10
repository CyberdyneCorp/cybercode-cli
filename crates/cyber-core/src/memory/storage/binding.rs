//! Retained path and ownership evidence; comparisons never repair replacement objects.
use super::*;
use crate::memory::identity::verify_identity;

pub(super) struct Binding {
    data: Dir,
    root: Dir,
    path: PathBuf,
    scope: String,
}
impl Binding {
    pub(super) fn new(path: &Path, data: Dir, root: Dir, scope: &str) -> Self {
        Self {
            data,
            root,
            path: path.into(),
            scope: scope.into(),
        }
    }
    fn verify(&self, scope: &Dir) -> Result<(), MemoryStorageError> {
        let data = data_directory(&self.path)?;
        compare(&data, &self.data)?;
        verify_private_directory(&self.root)?;
        compare(&child_directory(&self.data, "memory")?, &self.root)?;
        verify_private_directory(scope)?;
        compare(&child_directory(&self.root, &self.scope)?, scope)
    }
}
fn compare(current: &Dir, retained: &Dir) -> Result<(), MemoryStorageError> {
    verify_identity(
        &current.try_clone()?.into_std_file(),
        &retained.try_clone()?.into_std_file(),
    )
}
pub(super) fn data_directory(path: &Path) -> Result<Dir, MemoryStorageError> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_GENERIC_READ)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        super::super::windows::identity(&file)?;
        if !file.metadata()?.is_dir() {
            return Err(MemoryStorageError::Unsafe(
                "expected a memory data directory",
            ));
        }
        Ok(Dir::from_std_file(file))
    }
    #[cfg(not(windows))]
    {
        Ok(Dir::open_ambient_dir(path, cap_std::ambient_authority())?)
    }
}
fn child_directory(parent: &Dir, name: &str) -> Result<Dir, MemoryStorageError> {
    #[cfg(windows)]
    {
        Ok(Dir::from_std_file(
            super::super::windows::open_private_directory(
                &parent.try_clone()?.into_std_file(),
                name,
                super::super::windows::Access::Read,
            )?,
        ))
    }
    #[cfg(not(windows))]
    {
        Ok(parent.open_dir_nofollow(name)?)
    }
}
impl MemoryStore {
    pub(super) fn verify_binding(&self) -> Result<(), MemoryStorageError> {
        self.binding.verify(&self.dir)
    }
}
impl MemoryScope<'_> {
    pub(super) fn verify_binding(&self) -> Result<(), MemoryStorageError> {
        self.store.verify_binding()?;
        verify_regular(&self.lock)?;
        verify_private_file(&self.lock)?;
        #[cfg(windows)]
        let current = super::super::windows::open_private_file(
            &self.store.dir.try_clone()?.into_std_file(),
            ".memory.lock",
            super::super::windows::Access::Read,
        )?;
        #[cfg(not(windows))]
        let current = {
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            self.store
                .dir
                .open_with(".memory.lock", &options)?
                .into_std()
        };
        verify_regular(&current)?;
        verify_private_file(&current)?;
        verify_identity(&current, &self.lock)
    }
}
