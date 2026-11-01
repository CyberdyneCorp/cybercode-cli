//! Private, pinned draft storage. Atomic publication failures fence the store;
//! reopening requires recovery instead of assuming an interrupted write rolled back.
use super::{DEFAULT_PAGE, Error, MAX_EDIT_SIZE, draft::ImmutableBase};
use cap_fs_ext::DirExt;
#[cfg(not(windows))]
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::Dir;
#[cfg(not(windows))]
use cap_std::fs::OpenOptions;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const MAX_STORAGE: u64 = 2 * 1024 * 1024 * 1024;
const MAX_RECORD: u64 = 256 * 1024 * 1024;

fn open_root(data: &Path) -> Result<(PathBuf, Dir, Dir), Error> {
    if !data.is_absolute() {
        return Err(Error::Invalid);
    }
    let data_path = data.canonicalize().map_err(|_| Error::File)?;
    let data =
        Dir::open_ambient_dir(&data_path, cap_std::ambient_authority()).map_err(|_| Error::File)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if data
            .try_clone()
            .map_err(|_| Error::Io)?
            .into_std_file()
            .metadata()
            .map_err(|_| Error::Io)?
            .mode()
            & 0o022
            != 0
        {
            return Err(Error::File);
        }
    }
    let root = child(&data, "binary")?;
    flush_dir(&root)?;
    flush_dir(&data)?;
    Ok((data_path, data, root))
}

/// Prepared before untrusted child launch so its existing native directory can be
/// hidden by the sandbox. A later document open cannot silently bind a replacement.
pub struct StorageRoot {
    data: PathBuf,
    root: Dir,
}
impl StorageRoot {
    pub fn prepare(data: &Path) -> Result<Self, Error> {
        let (data, _parent, root) = open_root(data)?;
        Ok(Self { data, root })
    }
    pub fn path(&self) -> PathBuf {
        self.data.join("binary")
    }
    pub fn open(&self, id: &str) -> Result<Store, Error> {
        let store = Store::open(&self.data, id)?;
        if identity(&store.root)? != identity(&self.root)? {
            return Err(Error::Changed);
        }
        Ok(store)
    }
    /// A writable project containing private storage could rename or inspect the
    /// staging directory under its filesystem grant. Such editing is unsupported.
    pub fn admit_project(&self, project: &Path) -> Result<(), Error> {
        let project = project.canonicalize().map_err(|_| Error::File)?;
        if self.path().starts_with(&project) || project.starts_with(self.path()) {
            return Err(Error::File);
        }
        // Validate the root binding without capturing any source content.
        let root = Dir::open_ambient_dir(self.path(), cap_std::ambient_authority())
            .map_err(|_| Error::File)?;
        if identity(&root)? != identity(&self.root)? {
            return Err(Error::Changed);
        }
        private(
            &root,
            &root.try_clone().map_err(|_| Error::Io)?.into_std_file(),
            true,
        )
    }
}

pub struct Store {
    data_path: PathBuf,
    data: Dir,
    root: Dir,
    dir: Arc<Dir>,
    id: String,
    _lock: File,
    fenced: bool,
    #[cfg(test)]
    pub(super) fail_after_publication: Option<u64>,
    #[cfg(test)]
    pub(super) publications: u64,
}

fn component(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.')
        || value == "."
        || value == ".."
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
#[cfg(not(windows))]
fn identity(dir: &Dir) -> Result<same_file::Handle, Error> {
    same_file::Handle::from_file(dir.try_clone().map_err(|_| Error::Io)?.into_std_file())
        .map_err(|_| Error::Io)
}

#[cfg(windows)]
fn identity(dir: &Dir) -> Result<crate::memory::windows::FileIdentity, Error> {
    crate::memory::windows::identity(&dir.try_clone().map_err(|_| Error::Io)?.into_std_file())
        .map_err(|_| Error::File)
}

#[cfg(unix)]
fn private(dir: &Dir, file: &File, directory: bool) -> Result<(), Error> {
    use std::os::unix::fs::MetadataExt;
    let owner = dir
        .try_clone()
        .map_err(|_| Error::Io)?
        .into_std_file()
        .metadata()
        .map_err(|_| Error::Io)?;
    let m = file.metadata().map_err(|_| Error::Io)?;
    #[cfg(target_os = "macos")]
    super::native::private_acl(file)?;
    if m.uid() != owner.uid()
        || m.mode() & 0o077 != 0
        || m.is_dir() != directory
        || (!directory && (!m.is_file() || m.nlink() != 1))
    {
        return Err(Error::File);
    }
    Ok(())
}
#[cfg(windows)]
fn private(_dir: &Dir, file: &File, directory: bool) -> Result<(), Error> {
    crate::memory::windows::verify_private(file).map_err(|_| Error::File)?;
    if file.metadata().map_err(|_| Error::Io)?.is_dir() != directory {
        return Err(Error::File);
    }
    Ok(())
}
#[cfg(not(any(unix, windows)))]
fn private(_dir: &Dir, _file: &File, _directory: bool) -> Result<(), Error> {
    Err(Error::File)
}

fn child(parent: &Dir, name: &str) -> Result<Dir, Error> {
    component(name)?;
    #[cfg(windows)]
    {
        use crate::memory::{MemoryStorageError, windows as native};
        let owner = parent.try_clone().map_err(|_| Error::Io)?.into_std_file();
        match native::open_private_directory(&owner, name, native::Access::DataWrite) {
            Ok(file) => Ok(Dir::from_std_file(file)),
            Err(MemoryStorageError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                let created =
                    native::create_private_directory(&owner, name).map_err(|_| Error::File)?;
                native::verify_private(&created).map_err(|_| Error::File)?;
                let expected = native::identity(&created).map_err(|_| Error::File)?;
                drop(created);
                let file = native::open_private_directory(&owner, name, native::Access::DataWrite)
                    .map_err(|_| Error::File)?;
                if native::identity(&file).map_err(|_| Error::File)? != expected {
                    return Err(Error::Changed);
                }
                Ok(Dir::from_std_file(file))
            }
            Err(_) => Err(Error::File),
        }
    }
    #[cfg(not(windows))]
    {
        let mut builder = cap_std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use cap_std::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match parent.create_dir_with(name, &builder) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(Error::Io),
        }
        let dir = parent.open_dir_nofollow(name).map_err(|_| Error::File)?;
        private(
            parent,
            &dir.try_clone().map_err(|_| Error::Io)?.into_std_file(),
            true,
        )?;
        Ok(dir)
    }
}
fn create(dir: &Dir, name: &str) -> Result<File, Error> {
    component(name)?;
    #[cfg(windows)]
    {
        crate::memory::windows::create_private_file(
            &dir.try_clone().map_err(|_| Error::Io)?.into_std_file(),
            name,
        )
        .map_err(|_| Error::Io)
    }
    #[cfg(not(windows))]
    {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = dir
            .open_with(name, &options)
            .map_err(|_| Error::Io)?
            .into_std();
        private(dir, &file, false)?;
        Ok(file)
    }
}
fn read_file(dir: &Dir, name: &str) -> Result<File, Error> {
    component(name)?;
    #[cfg(windows)]
    {
        crate::memory::windows::open_private_file(
            &dir.try_clone().map_err(|_| Error::Io)?.into_std_file(),
            name,
            crate::memory::windows::Access::Read,
        )
        .map_err(|_| Error::File)
    }
    #[cfg(not(windows))]
    {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = dir
            .open_with(name, &options)
            .map_err(|_| Error::File)?
            .into_std();
        private(dir, &file, false)?;
        Ok(file)
    }
}
fn flush_file(file: &File) -> Result<(), Error> {
    file.sync_all().map_err(|_| Error::Io)?;
    #[cfg(target_os = "macos")]
    rustix::fs::fcntl_fullfsync(file).map_err(|_| Error::Io)?;
    Ok(())
}
pub(super) fn flush_dir(dir: &Dir) -> Result<(), Error> {
    #[cfg(windows)]
    {
        let file = dir.try_clone().map_err(|_| Error::Io)?.into_std_file();
        crate::memory::windows::sync_private(&file).map_err(|_| Error::Io)
    }
    #[cfg(unix)]
    {
        // cap-std directory handles can be O_PATH on Linux. They retain routing
        // identity but cannot be fsynced. Open the same directory through that
        // retained handle with read access, without consulting its ambient path.
        let fd = rustix::fs::openat(
            dir,
            ".",
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| Error::Io)?;
        File::from(fd).sync_all().map_err(|_| Error::Io)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(Error::File)
    }
}

impl Store {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(super) fn directory(&self) -> &Dir {
        &self.dir
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(super) fn stage(&self, length: u64) -> Result<(String, File), Error> {
        self.validate()?;
        if length > MAX_EDIT_SIZE {
            return Err(Error::Limit);
        }
        self.capacity(length)?;
        let name = crate::ids::new_id("stage");
        let file = create(&self.dir, &name)?;
        Ok((name, file))
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub(super) fn sync(&self) -> Result<(), Error> {
        self.validate()?;
        flush_dir(&self.dir)
    }

    pub fn open(data: &Path, id: &str) -> Result<Self, Error> {
        component(id)?;
        let (data_path, data, root) = open_root(data)?;
        let dir = child(&root, id)?;
        #[cfg(windows)]
        let lock = {
            let owner = dir.try_clone().map_err(|_| Error::Io)?.into_std_file();
            match crate::memory::windows::create_private_file(&owner, "lock") {
                Ok(created) => drop(created),
                Err(crate::memory::MemoryStorageError::Io(e))
                    if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(Error::File),
            }
            crate::memory::windows::open_pinned_private_file(&owner, "lock")
                .map_err(|_| Error::File)?
        };
        #[cfg(not(windows))]
        let lock = match create(&dir, "lock") {
            Ok(f) => f,
            Err(_) => {
                let mut options = OpenOptions::new();
                options.read(true).write(true).follow(FollowSymlinks::No);
                let file = dir
                    .open_with("lock", &options)
                    .map_err(|_| Error::File)?
                    .into_std();
                private(&dir, &file, false)?;
                file
            }
        };
        lock.try_lock().map_err(|_| Error::Limit)?;
        flush_dir(&dir)?;
        flush_dir(&root)?;
        flush_dir(&data)?;
        let store = Self {
            data_path,
            data,
            root,
            dir: Arc::new(dir),
            id: id.into(),
            _lock: lock,
            fenced: false,
            #[cfg(test)]
            fail_after_publication: None,
            #[cfg(test)]
            publications: 0,
        };
        store.validate()?;
        Ok(store)
    }
    pub fn validate(&self) -> Result<(), Error> {
        if self.fenced {
            return Err(Error::Changed);
        }
        let now = Dir::open_ambient_dir(&self.data_path, cap_std::ambient_authority())
            .map_err(|_| Error::Changed)?;
        if identity(&now)? != identity(&self.data)? {
            return Err(Error::Changed);
        }
        let root = now
            .open_dir_nofollow("binary")
            .map_err(|_| Error::Changed)?;
        let dir = root
            .open_dir_nofollow(&self.id)
            .map_err(|_| Error::Changed)?;
        if identity(&root)? != identity(&self.root)? || identity(&dir)? != identity(&self.dir)? {
            return Err(Error::Changed);
        }
        private(
            &self.data,
            &root.try_clone().map_err(|_| Error::Io)?.into_std_file(),
            true,
        )?;
        private(
            &root,
            &dir.try_clone().map_err(|_| Error::Io)?.into_std_file(),
            true,
        )?;
        Ok(())
    }
    fn capacity(&self, additional: u64) -> Result<(), Error> {
        let mut used = 0u64;
        for (count, entry) in self.dir.entries().map_err(|_| Error::Io)?.enumerate() {
            if count >= 4096 {
                return Err(Error::Limit);
            }
            let entry = entry.map_err(|_| Error::Io)?;
            let metadata = self
                .dir
                .symlink_metadata(entry.file_name())
                .map_err(|_| Error::Io)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(Error::File);
            }
            used = used.checked_add(metadata.len()).ok_or(Error::Limit)?;
        }
        if used.checked_add(additional).is_none_or(|n| n > MAX_STORAGE) {
            return Err(Error::Limit);
        }
        #[cfg(unix)]
        {
            let stat = rustix::fs::fstatvfs(self.dir.as_ref()).map_err(|_| Error::Io)?;
            let available = stat.f_bavail.saturating_mul(stat.f_frsize);
            if available < additional.saturating_add(1024 * 1024) {
                return Err(Error::Limit);
            }
        }
        Ok(())
    }
    /// Publishes one private record. An error after rename fences further writes;
    /// callers must reopen/reconcile the durable record before continuing.
    pub fn record(
        &mut self,
        name: &str,
        limit: u64,
        write: impl FnOnce(&mut dyn Write) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.validate()?;
        component(name)?;
        if !name.ends_with(".json") {
            return Err(Error::Invalid);
        }
        if let Ok(metadata) = self.dir.symlink_metadata(name) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(Error::File);
            }
            let _existing = read_file(&self.dir, name)?;
        }
        if limit > MAX_RECORD {
            return Err(Error::Limit);
        }
        self.capacity(limit)?;
        let temporary = format!("record-{}", crate::ids::new_id("tmp"));
        let mut file = create(&self.dir, &temporary)?;
        write(&mut Limited {
            writer: &mut file,
            remaining: limit,
        })?;
        if file.metadata().map_err(|_| Error::Io)?.len() > MAX_RECORD {
            return Err(Error::Limit);
        }
        flush_file(&file)?;
        self.validate()?;
        self.dir
            .rename(&temporary, &self.dir, name)
            .map_err(|_| Error::Io)?;
        // The namespace effect may already be durable even if the next flush fails.
        self.fenced = true;
        #[cfg(test)]
        {
            self.publications += 1;
            if self.fail_after_publication == Some(self.publications) {
                return Err(Error::Io);
            }
        }
        flush_dir(&self.dir)?;
        self.fenced = false;
        self.validate()
    }
    pub fn read_record(&self, name: &str) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let file = read_file(&self.dir, name)?;
        if file.metadata().map_err(|_| Error::Io)?.len() > MAX_RECORD {
            return Err(Error::Limit);
        }
        let mut bytes = Vec::new();
        file.take(MAX_RECORD + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Io)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(Error::Limit);
        }
        self.validate()?;
        Ok(bytes)
    }
    /// Captures into a newly created private object, then retains read-only access.
    /// The producer must check source authority/identity before and after streaming.
    pub fn base(
        &self,
        length: u64,
        copy: impl FnOnce(&mut dyn Write) -> Result<String, Error>,
    ) -> Result<Base, Error> {
        self.base_cancellable(length, copy, || false)
    }
    pub fn base_cancellable(
        &self,
        length: u64,
        copy: impl FnOnce(&mut dyn Write) -> Result<String, Error>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Base, Error> {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        self.validate()?;
        if length > MAX_EDIT_SIZE {
            return Err(Error::Limit);
        }
        self.capacity(length)?;
        let name = crate::ids::new_id("base");
        let mut file = create(&self.dir, &name)?;
        let digest = copy(&mut Limited {
            writer: &mut file,
            remaining: length,
        })?;
        if file.metadata().map_err(|_| Error::Io)?.len() != length {
            return Err(Error::Changed);
        }
        flush_file(&file)?;
        flush_dir(&self.dir)?;
        drop(file);
        let base = self.open_base_cancellable(&name, length, &digest, cancelled)?;
        self.validate()?;
        Ok(base)
    }
    pub fn open_base(&self, name: &str, length: u64, digest: &str) -> Result<Base, Error> {
        self.open_base_cancellable(name, length, digest, || false)
    }
    pub fn open_base_cancellable(
        &self,
        name: &str,
        length: u64,
        digest: &str,
        cancelled: impl Fn() -> bool,
    ) -> Result<Base, Error> {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        self.validate()?;
        if length > MAX_EDIT_SIZE {
            return Err(Error::Limit);
        }
        let file = read_file(&self.dir, name)?;
        if file.metadata().map_err(|_| Error::Io)?.len() != length {
            return Err(Error::Changed);
        }
        #[cfg(windows)]
        let file = {
            let expected = crate::memory::windows::identity(&file).map_err(|_| Error::File)?;
            drop(file);
            crate::memory::windows::freeze_private_file(
                &self.dir.try_clone().map_err(|_| Error::Io)?.into_std_file(),
                name,
                expected,
            )
            .map_err(|_| Error::File)?
        };
        let before = generation(&file)?;
        let mut observed = Sha256::new();
        let mut at = 0;
        while at < length {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let take = (length - at).min(DEFAULT_PAGE as u64) as usize;
            let bytes = positioned(&file, at, take)?;
            observed.update(&bytes);
            at += take as u64;
        }
        if format!("{:x}", observed.finalize()) != digest {
            return Err(Error::Changed);
        }
        let observed = generation(&file)?;
        if before != observed {
            return Err(Error::Changed);
        }
        Ok(Base {
            inner: Arc::new(BaseInner {
                file,
                _dir: self.dir.clone(),
                name: name.into(),
                length,
                digest: digest.into(),
                generation: observed,
            }),
        })
    }
}
fn positioned(file: &File, offset: u64, length: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![0; length];
    let mut done = 0;
    while done < length {
        #[cfg(unix)]
        let count = {
            use std::os::unix::fs::FileExt;
            file.read_at(&mut bytes[done..], offset + done as u64)
        };
        #[cfg(windows)]
        let count = {
            use std::os::windows::fs::FileExt;
            file.seek_read(&mut bytes[done..], offset + done as u64)
        };
        #[cfg(not(any(unix, windows)))]
        let count: std::io::Result<usize> = Err(std::io::ErrorKind::Unsupported.into());
        let count = count.map_err(|_| Error::Io)?;
        if count == 0 {
            return Err(Error::Changed);
        }
        done += count;
    }
    Ok(bytes)
}
#[derive(Clone)]
pub struct Base {
    inner: Arc<BaseInner>,
}
struct BaseInner {
    file: File,
    _dir: Arc<Dir>,
    name: String,
    length: u64,
    digest: String,
    generation: String,
}
impl Base {
    pub fn name(&self) -> &str {
        &self.inner.name
    }
}
impl ImmutableBase for Base {
    fn length(&self) -> u64 {
        self.inner.length
    }
    fn sha256(&self) -> &str {
        &self.inner.digest
    }
    fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>, Error> {
        if length > super::MAX_PAGE
            || offset
                .checked_add(length as u64)
                .is_none_or(|n| n > self.length())
        {
            return Err(Error::Limit);
        }
        if generation(&self.inner.file)? != self.inner.generation {
            return Err(Error::Changed);
        }
        let bytes = positioned(&self.inner.file, offset, length)?;
        if generation(&self.inner.file)? != self.inner.generation {
            return Err(Error::Changed);
        }
        Ok(bytes)
    }
}

fn generation(file: &File) -> Result<String, Error> {
    let m = file.metadata().map_err(|_| Error::Io)?;
    let mut hash = Sha256::new();
    hash.update(m.len().to_le_bytes());
    hash.update(format!("{:?}", m.modified().map_err(|_| Error::Io)?));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(m.ctime().to_le_bytes());
        hash.update(m.ctime_nsec().to_le_bytes());
        hash.update(m.mode().to_le_bytes());
        hash.update(m.nlink().to_le_bytes());
    }
    Ok(format!("{:x}", hash.finalize()))
}
struct Limited<'a> {
    writer: &'a mut File,
    remaining: u64,
}
impl Write for Limited<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(std::io::Error::other("binary storage limit"));
        }
        let written = self.writer.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capture(store: &Store, bytes: &[u8]) -> Base {
        store
            .base(bytes.len() as u64, |writer| {
                writer.write_all(bytes).map_err(|_| Error::Io)?;
                Ok(format!("{:x}", Sha256::digest(bytes)))
            })
            .unwrap()
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn directory_flush_uses_a_readable_descriptor_from_a_retained_path_handle() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pinned");
        std::fs::create_dir(&path).unwrap();
        let fd = rustix::fs::open(
            &path,
            rustix::fs::OFlags::PATH | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .unwrap();
        let file = File::from(fd);
        assert_eq!(
            file.sync_all().unwrap_err().raw_os_error(),
            Some(libc::EBADF)
        );
        let pinned = Dir::from_std_file(file);
        std::fs::rename(&path, root.path().join("moved")).unwrap();
        std::fs::write(&path, b"replacement is not a directory").unwrap();
        flush_dir(&pinned).unwrap();
        assert_eq!(
            std::fs::read(path).unwrap(),
            b"replacement is not a directory"
        );
    }

    #[test]
    fn prepared_root_refuses_project_overlap_and_root_replacement() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let project = root.path().join("project");
        std::fs::create_dir(&data).unwrap();
        std::fs::create_dir(&project).unwrap();
        let storage = StorageRoot::prepare(&data).unwrap();
        storage.admit_project(&project).unwrap();
        assert!(storage.admit_project(root.path()).is_err());
        assert!(storage.admit_project(&storage.path()).is_err());
        storage.open("bin_positive").unwrap();
        #[cfg(not(windows))]
        {
            std::fs::rename(storage.path(), data.join("moved")).unwrap();
            let replacement = StorageRoot::prepare(&data).unwrap();
            assert!(storage.admit_project(&project).is_err());
            assert!(storage.open("bin_rebound").is_err());
            replacement.admit_project(&project).unwrap();
            replacement.open("bin_rebound").unwrap();
        }
        #[cfg(windows)]
        {
            // Retained Windows directory handles deny rename rather than allowing
            // replacement followed by a later identity refusal.
            assert!(std::fs::rename(storage.path(), data.join("moved")).is_err());
            storage.open("bin_still_bound").unwrap();
            drop(storage);
            std::fs::rename(data.join("binary"), data.join("moved")).unwrap();
            StorageRoot::prepare(&data)
                .unwrap()
                .open("bin_new_root")
                .unwrap();
        }
    }

    #[test]
    fn private_base_survives_store_reopen_and_feeds_a_recoverable_draft() {
        let data = tempfile::tempdir().unwrap();
        let mut store = Store::open(data.path(), "bin_test").unwrap();
        let base = capture(&store, &(0..=255).collect::<Vec<u8>>());
        let name = base.name().to_owned();
        let digest = base.sha256().to_owned();
        let mut draft = crate::binary::draft::Draft::new(base).unwrap();
        draft
            .patch(&crate::binary::draft::PatchBatch {
                revision: draft.revision(),
                splices: vec![crate::binary::draft::Splice {
                    offset: super::super::Offset(0),
                    delete_length: super::super::Offset(2),
                    expected_hex: "0001".into(),
                    insert_hex: "ff0080".into(),
                }],
            })
            .unwrap();
        let state = draft.snapshot();
        let revision = draft.revision();
        store
            .record("state.json", 4096, |writer| {
                serde_json::to_writer(writer, &state).map_err(|_| Error::Io)
            })
            .unwrap();
        drop(draft);
        drop(store);
        let store = Store::open(data.path(), "bin_test").unwrap();
        let base = store.open_base(&name, 256, &digest).unwrap();
        let restored = crate::binary::draft::Draft::resume(
            base,
            serde_json::from_slice(&store.read_record("state.json").unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(restored.revision(), revision);
        assert_eq!(restored.read(0, 4).unwrap(), [255, 0, 128, 2]);
        assert!(store.open_base(&name, 256, &"00".repeat(32)).is_err());
    }
    #[test]
    fn cancellation_interrupts_private_base_verification_between_chunks() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path(), "bin_cancel").unwrap();
        let base = capture(&store, &vec![42; DEFAULT_PAGE * 3]);
        let checks = std::cell::Cell::new(0);
        let result = store.open_base_cancellable(base.name(), base.length(), base.sha256(), || {
            checks.set(checks.get() + 1);
            checks.get() == 3
        });
        assert!(matches!(result, Err(Error::Cancelled)));
        assert_eq!(checks.get(), 3);
        // Cancellation does not poison the stored base or lose its evidence.
        let reopened = store
            .open_base(base.name(), base.length(), base.sha256())
            .unwrap();
        assert_eq!(reopened.read(DEFAULT_PAGE as u64, 4).unwrap(), [42; 4]);
    }

    #[test]
    fn exclusive_owner_and_bounded_records_keep_the_last_durable_state() {
        let data = tempfile::tempdir().unwrap();
        let mut store = Store::open(data.path(), "bin_test").unwrap();
        assert!(Store::open(data.path(), "bin_test").is_err());
        store
            .record("state.json", 3, |w| {
                w.write_all(b"one").map_err(|_| Error::Io)
            })
            .unwrap();
        assert!(
            store
                .record("state.json", 2, |w| w
                    .write_all(b"too much")
                    .map_err(|_| Error::Io))
                .is_err()
        );
        assert_eq!(store.read_record("state.json").unwrap(), b"one");
        store
            .record("state.json", 3, |w| {
                w.write_all(b"two").map_err(|_| Error::Io)
            })
            .unwrap();
        assert_eq!(store.read_record("state.json").unwrap(), b"two");
        drop(store);
        assert!(Store::open(data.path(), "bin_test").is_ok());
    }
    #[test]
    fn invalid_names_and_capture_evidence_are_refused_before_admission() {
        let data = tempfile::tempdir().unwrap();
        for name in ["..", "", "nested/path", "/outside"] {
            assert!(Store::open(data.path(), name).is_err());
        }
        let store = Store::open(data.path(), "bin_test").unwrap();
        assert!(
            store
                .base(MAX_EDIT_SIZE + 1, |_| panic!("oversized capture started"))
                .is_err()
        );
        assert!(
            store
                .base(1, |w| {
                    w.write_all(b"oversized").map_err(|_| Error::Io)?;
                    Ok("00".repeat(32))
                })
                .is_err()
        );
        assert!(
            store
                .base(1, |w| {
                    w.write_all(b"x").map_err(|_| Error::Io)?;
                    Ok("00".repeat(32))
                })
                .is_err()
        );
    }
    #[cfg(unix)]
    #[test]
    fn private_namespace_links_permissions_and_base_mutation_are_not_repaired() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("binary");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Store::open(data.path(), "bin_test").is_err());
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o755
        );
        std::fs::remove_dir(&root).unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), &root).unwrap();
        assert!(Store::open(data.path(), "bin_test").is_err());
        std::fs::remove_file(&root).unwrap();
        let store = Store::open(data.path(), "bin_test").unwrap();
        let base = capture(&store, b"original");
        let path = root.join("bin_test").join(base.name());
        std::fs::write(&path, b"changed!").unwrap();
        assert!(matches!(base.read(0, 8), Err(Error::Changed)));
        let retained = data.path().join("retained");
        std::fs::rename(&root, &retained).unwrap();
        std::fs::create_dir(&root).unwrap();
        assert!(store.validate().is_err());
        assert_eq!(
            std::fs::read(retained.join("bin_test").join(base.name())).unwrap(),
            b"changed!"
        );
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_mac_extended_private_directory_acl_is_refused_without_repair() {
        use std::os::unix::fs::PermissionsExt;
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("binary");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            std::process::Command::new("/bin/chmod")
                .args(["+a", "everyone allow list,search"])
                .arg(&root)
                .status()
                .unwrap()
                .success()
        );
        assert!(Store::open(data.path(), "bin_test").is_err());
        assert!(super::super::native::private_acl(&File::open(&root).unwrap()).is_err());
        assert!(!root.join("bin_test").exists());
    }
    #[cfg(windows)]
    #[test]
    fn native_windows_private_base_excludes_writers_until_owner_release() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path(), "bin_test").unwrap();
        let base = capture(&store, b"original");
        let path = data
            .path()
            .join("binary")
            .join("bin_test")
            .join(base.name());
        assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
        assert_eq!(base.read(0, 8).unwrap(), b"original");
        drop(base);
        std::fs::write(&path, b"changed!").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"changed!");
    }
}
