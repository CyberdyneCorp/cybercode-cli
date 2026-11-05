//! Descriptor-relative private storage. Names are server-derived single components.
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, DirBuilder, DirBuilderExt, OpenOptions, OpenOptionsExt};
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub type Result<T> = std::result::Result<T, String>;
pub fn fail(error: impl std::fmt::Display) -> String {
    error.to_string()
}
pub fn identity(file: &File) -> Result<String> {
    let m = file.metadata().map_err(fail)?;
    let birth = m
        .created()
        .map_err(|_| "Filesystem does not expose stable directory creation identity")?;
    let birth = birth.duration_since(std::time::UNIX_EPOCH).map_err(fail)?;
    Ok(format!(
        "{}:{}:{}:{}",
        m.dev(),
        m.ino(),
        birth.as_secs(),
        birth.subsec_nanos()
    ))
}
pub fn dir_identity(dir: &Dir) -> Result<String> {
    identity(&dir.try_clone().map_err(fail)?.into_std_file())
}
pub fn sync(dir: &Dir) -> Result<()> {
    // cap-std can hold O_PATH directories on Linux, which fsync rejects.
    // Reopen the pinned directory itself for I/O, without resolving an ambient path.
    let fd = rustix::fs::openat(
        dir,
        ".",
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::DIRECTORY
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map_err(fail)?;
    File::from(fd).sync_all().map_err(fail)
}
pub fn private_child(parent: &Dir, name: &str) -> Result<Dir> {
    child(parent, name, 0o077)
}
pub fn shared_child(parent: &Dir, name: &str) -> Result<Dir> {
    child(parent, name, 0o022)
}
pub fn existing_child(parent: &Dir, name: &str, private: bool) -> Result<Dir> {
    child_mode(parent, name, if private { 0o077 } else { 0o022 }, false)
}
fn child(parent: &Dir, name: &str, forbidden: u32) -> Result<Dir> {
    child_mode(parent, name, forbidden, true)
}
fn child_mode(parent: &Dir, name: &str, forbidden: u32, create: bool) -> Result<Dir> {
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    if create {
        match parent.create_dir_with(name, &builder) {
            Ok(()) => sync(parent)?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(fail(e)),
        }
    }
    let dir = parent.open_dir_nofollow(name).map_err(fail)?;
    let m = dir
        .try_clone()
        .map_err(fail)?
        .into_std_file()
        .metadata()
        .map_err(fail)?;
    // Ownership must match the already trusted data-directory descriptor.
    let owner = parent
        .try_clone()
        .map_err(fail)?
        .into_std_file()
        .metadata()
        .map_err(fail)?
        .uid();
    if m.uid() != owner || m.mode() & forbidden != 0 {
        return Err("Snapshot storage is not private".into());
    }
    #[cfg(target_os = "macos")]
    if forbidden == 0o077 {
        cyber_core::binary::native::validate_private_directory_acl(
            &dir.try_clone().map_err(fail)?.into_std_file(),
        )
        .map_err(|_| "Snapshot private directory has unsupported ACL grants".to_string())?;
    }
    Ok(dir)
}
pub fn open(dir: &Dir, name: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .custom_flags(libc::O_NONBLOCK);
    let file = dir.open_with(name, &options).map_err(fail)?.into_std();
    let m = file.metadata().map_err(fail)?;
    if !m.is_file() || m.nlink() != 1 {
        return Err("Snapshot source is not a unique regular file".into());
    }
    Ok(file)
}
pub fn read(dir: &Dir, name: &str, limit: u64) -> Result<Vec<u8>> {
    let file = open(dir, Path::new(name))?;
    if file.metadata().map_err(fail)?.len() > limit {
        return Err("Snapshot record exceeds limit".into());
    }
    let mut bytes = vec![];
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(fail)?;
    if bytes.len() as u64 > limit {
        return Err("Snapshot record grew beyond limit".into());
    }
    Ok(bytes)
}
pub fn create(dir: &Dir, name: &str, bytes: &[u8]) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .read(true)
        .create_new(true)
        .mode(0o600)
        .follow(FollowSymlinks::No);
    let mut file = dir.open_with(name, &options).map_err(fail)?.into_std();
    file.write_all(bytes).map_err(fail)?;
    file.sync_all().map_err(fail)?;
    #[cfg(target_os = "macos")]
    rustix::fs::fcntl_fullfsync(&file).map_err(fail)?;
    Ok(file)
}
pub fn atomic(dir: &Dir, name: &str, bytes: &[u8]) -> Result<()> {
    let temp = format!("tmp_{}", ulid::Ulid::new());
    create(dir, &temp, bytes)?;
    dir.rename(&temp, dir, name).map_err(fail)?;
    sync(dir)
}
pub fn lock(dir: &Dir, create: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(create)
        .mode(0o600)
        .follow(FollowSymlinks::No);
    let file = dir.open_with("lock", &options).map_err(fail)?.into_std();
    let m = file.metadata().map_err(fail)?;
    if !m.is_file() || m.nlink() != 1 || m.mode() & 0o077 != 0 {
        return Err("Unsafe snapshot lock".into());
    }
    acquire_lock(&file)?;
    Ok(file)
}
fn acquire_lock(file: &File) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error))
                if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("Cannot lock snapshot storage: {error}")),
        }
        if std::time::Instant::now() >= deadline {
            return Err("Snapshot storage is busy".into());
        }
        // Bounded acquisition only: no restore operation has started or is replayed.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
pub fn names(dir: &Dir, max: usize) -> Result<Vec<String>> {
    let mut names = vec![];
    for entry in dir.entries().map_err(fail)? {
        if names.len() >= max {
            return Err("Snapshot storage entry limit exceeded".into());
        }
        names.push(
            entry
                .map_err(fail)?
                .file_name()
                .into_string()
                .map_err(|_| "Invalid snapshot storage name")?,
        );
    }
    names.sort();
    Ok(names)
}
pub fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}
pub fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_sync_uses_a_readable_descriptor() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        #[cfg(target_os = "linux")]
        let dir = {
            let fd = rustix::fs::openat(
                &dir,
                ".",
                rustix::fs::OFlags::PATH | rustix::fs::OFlags::DIRECTORY,
                rustix::fs::Mode::empty(),
            )
            .unwrap();
            Dir::from_std_file(File::from(fd))
        };
        sync(&dir).unwrap();
    }

    #[test]
    fn lock_acquisition_waits_for_a_short_handoff_but_keeps_a_live_owner_exclusive() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        let owner = lock(&dir, true).unwrap();
        assert!(matches!(lock(&dir, false), Err(error) if error.contains("busy")));
        let contender = dir.try_clone().unwrap();
        let waiting = std::thread::spawn(move || lock(&contender, false));
        std::thread::sleep(std::time::Duration::from_millis(25));
        drop(owner);
        let next = waiting.join().unwrap().unwrap();
        assert!(matches!(lock(&dir, false), Err(error) if error.contains("busy")));
        drop(next);
        assert!(lock(&dir, false).is_ok());
    }
}
