//! Native steps of an owner-confirmed discard of one unpublished construction stage.
//!
//! A stage is observed through a no-follow, non-blocking handle (regular file, one
//! link, at most [`STAGE_LIMIT`] bytes) and identified by device, inode, size,
//! modification time and SHA-256. A discard moves exactly that file with a
//! no-replace rename into `discarded/<key>/` beside its private namespace, flushes
//! both directories and only then unlinks it from the quarantine. Every step
//! compares the observed identity with the reviewed one first; a different file is
//! reported and never deleted. Nothing here decides who may discard: callers fence
//! the owner activity before [`quarantine`].
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io;

/// Largest stage a review digests; larger stages are refused.
pub const STAGE_LIMIT: u64 = 1024 * 1024;
/// The private quarantine directory created in a stage's namespace.
pub const QUARANTINE: &str = "discarded";

/// Native identity and contents of one stage at review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StageEvidence {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch.
    pub modified_ns: i64,
    /// Lowercase hex SHA-256 of the stage bytes.
    pub sha256: String,
}

/// Where a reviewed stage is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// Still at its original name with the reviewed identity.
    Source,
    /// Moved into its quarantine slot with the reviewed identity.
    Quarantined,
    /// Neither: the unlink already happened.
    Gone,
    /// A file with another identity holds the name; it is never deleted.
    Changed(String),
}

/// One quarantined entry that still awaits its unlink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub key: String,
    pub name: String,
    pub size: u64,
}

#[cfg(unix)]
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// A single plain path component, as every stage and slot name must be.
pub fn valid_component(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !matches!(name, "." | "..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
}

/// Quarantine slots are lowercase SHA-256 hex digests chosen by the caller.
pub fn valid_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(unix)]
fn require_component(name: &str) -> io::Result<()> {
    if valid_component(name) {
        Ok(())
    } else {
        Err(invalid("Invalid construction stage name"))
    }
}

#[cfg(unix)]
mod native {
    use super::*;
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::{DirBuilder, DirBuilderExt, OpenOptions, OpenOptionsExt};
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{FileExt, MetadataExt};

    /// Open `name` without following links or blocking on special files and
    /// record its evidence. Refuses anything but a single-link regular file.
    pub fn observe(dir: &Dir, name: &str) -> io::Result<(File, StageEvidence)> {
        require_component(name)?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
        let file = dir.open_with(name, &options)?.into_std();
        let evidence = evidence(&file)?;
        Ok((file, evidence))
    }

    /// Evidence of an already open handle; the bytes are read through it.
    pub fn evidence(file: &File) -> io::Result<StageEvidence> {
        let before = file.metadata()?;
        if !before.is_file() {
            return Err(invalid("Construction stage is not a regular file"));
        }
        if before.nlink() != 1 {
            return Err(invalid("Construction stage has aliases"));
        }
        if before.len() > STAGE_LIMIT {
            return Err(invalid(format!(
                "Construction stage exceeds the {STAGE_LIMIT}-byte review limit"
            )));
        }
        let sha256 = digest(file)?;
        let after = file.metadata()?;
        if (after.len(), after.mtime(), after.mtime_nsec())
            != (before.len(), before.mtime(), before.mtime_nsec())
        {
            return Err(invalid("Construction stage changed while it was read"));
        }
        Ok(StageEvidence {
            device: after.dev(),
            inode: after.ino(),
            size: after.len(),
            modified_ns: after
                .mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(after.mtime_nsec()),
            sha256,
        })
    }

    fn digest(file: &File) -> io::Result<String> {
        let mut hasher = Sha256::new();
        let mut chunk = [0u8; 8192];
        let mut offset = 0u64;
        loop {
            let read = file.read_at(&mut chunk, offset)?;
            if read == 0 {
                return Ok(format!("{:x}", hasher.finalize()));
            }
            offset += read as u64;
            if offset > STAGE_LIMIT {
                return Err(invalid("Construction stage grew beyond the review limit"));
            }
            hasher.update(&chunk[..read]);
        }
    }

    /// Exclusive, non-blocking advisory lock on the stage. A live constructor
    /// holds it from creation through publication.
    pub fn try_lock(file: &File) -> io::Result<bool> {
        match file.try_lock() {
            Ok(()) => Ok(true),
            Err(std::fs::TryLockError::WouldBlock) => Ok(false),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    }

    pub fn sync(dir: &Dir) -> io::Result<()> {
        let fd = rustix::fs::openat(
            dir,
            ".",
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::CLOEXEC
                | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )?;
        File::from(fd).sync_all()
    }

    fn owner_uid(dir: &Dir) -> io::Result<u32> {
        Ok(dir.try_clone()?.into_std_file().metadata()?.uid())
    }

    /// The private `name` directory in `parent`, created 0700 when `create`.
    /// Returns `None` when it is missing and `create` is false.
    fn private_dir(parent: &Dir, name: &str, create: bool) -> io::Result<Option<Dir>> {
        if create {
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            match parent.create_dir_with(name, &builder) {
                Ok(()) => sync(parent)?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        let dir = match parent.open_dir_nofollow(name) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
            Err(error) => return Err(error),
        };
        let metadata = dir.try_clone()?.into_std_file().metadata()?;
        if metadata.uid() != owner_uid(parent)? || metadata.mode() & 0o077 != 0 {
            return Err(invalid("Construction quarantine is not private"));
        }
        Ok(Some(dir))
    }

    fn slot(namespace: &Dir, key: &str, create: bool) -> io::Result<Option<(Dir, Dir)>> {
        if !valid_key(key) {
            return Err(invalid("Invalid construction quarantine key"));
        }
        let Some(quarantine) = private_dir(namespace, QUARANTINE, create)? else {
            return Ok(None);
        };
        Ok(private_dir(&quarantine, key, create)?.map(|slot| (quarantine, slot)))
    }

    /// Move the held, reviewed stage into `discarded/<key>/` with no replacement.
    /// `file` must be the handle the caller fenced; the name must still route to it.
    pub fn quarantine(
        source: &Dir,
        name: &str,
        namespace: &Dir,
        key: &str,
        file: &File,
        expected: &StageEvidence,
    ) -> io::Result<()> {
        require_component(name)?;
        if evidence(file)? != *expected {
            return Err(invalid("Construction stage changed since review"));
        }
        let (_, routed) = observe(source, name)?;
        if routed != *expected {
            return Err(invalid(
                "Construction stage name no longer routes to the reviewed file",
            ));
        }
        let (quarantine, slot) = slot(namespace, key, true)?.expect("created quarantine slot");
        rustix::fs::renameat_with(
            source,
            name,
            &slot,
            name,
            rustix::fs::RenameFlags::NOREPLACE,
        )?;
        sync(&slot)?;
        sync(source)?;
        sync(&quarantine)?;
        let (_, moved) = observe(&slot, name)?;
        if moved != *expected {
            // Never keep or delete a file nobody reviewed: return it, no replacement.
            let _ = rustix::fs::renameat_with(
                &slot,
                name,
                source,
                name,
                rustix::fs::RenameFlags::NOREPLACE,
            );
            let _ = sync(source);
            return Err(invalid(
                "A different file was moved; it was returned unchanged",
            ));
        }
        Ok(())
    }

    /// Unlink the quarantined stage when it still has the reviewed identity.
    pub fn unlink(
        namespace: &Dir,
        key: &str,
        name: &str,
        expected: &StageEvidence,
    ) -> io::Result<Place> {
        require_component(name)?;
        let Some((quarantine, slot)) = slot(namespace, key, false)? else {
            return Ok(Place::Gone);
        };
        match observe(&slot, name) {
            Ok((_, observed)) if observed != *expected => {
                return Ok(Place::Changed(
                    "The quarantined file differs from the reviewed stage".into(),
                ));
            }
            Ok(_) => {
                slot.remove_file(name)?;
                sync(&slot)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        // Another entry or a concurrent retry may keep the slot; the stage is gone.
        if quarantine.remove_dir(key).is_ok() {
            sync(&quarantine)?;
        }
        Ok(Place::Gone)
    }

    /// Where the reviewed stage is now, without changing anything.
    pub fn locate(
        source: Option<&Dir>,
        name: &str,
        namespace: &Dir,
        key: &str,
        expected: &StageEvidence,
    ) -> io::Result<Place> {
        require_component(name)?;
        if let Some(source) = source {
            match observe(source, name) {
                Ok((_, observed)) if observed == *expected => return Ok(Place::Source),
                Ok(_) => {
                    return Ok(Place::Changed(
                        "The stage name now holds a different file".into(),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let Some((_, slot)) = slot(namespace, key, false)? else {
            return Ok(Place::Gone);
        };
        match observe(&slot, name) {
            Ok((_, observed)) if observed == *expected => Ok(Place::Quarantined),
            Ok(_) => Ok(Place::Changed(
                "The quarantined file differs from the reviewed stage".into(),
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Place::Gone),
            Err(error) => Err(error),
        }
    }

    /// Entries still in `discarded/` under `namespace`, bounded by `limit`.
    pub fn pending(namespace: &Dir, limit: usize) -> io::Result<Vec<Pending>> {
        let Some(quarantine) = private_dir(namespace, QUARANTINE, false)? else {
            return Ok(vec![]);
        };
        let mut found = vec![];
        for entry in quarantine.entries()? {
            let key = entry?.file_name().to_string_lossy().into_owned();
            let slot = quarantine.open_dir_nofollow(&key)?;
            for entry in slot.entries()? {
                let entry = entry?;
                if found.len() >= limit {
                    return Err(invalid("Construction quarantine exceeds its listing limit"));
                }
                found.push(Pending {
                    key: key.clone(),
                    name: entry.file_name().to_string_lossy().into_owned(),
                    size: entry.metadata()?.len(),
                });
            }
        }
        Ok(found)
    }
}

#[cfg(unix)]
pub use native::{evidence, locate, observe, pending, quarantine, sync, try_lock, unlink};

#[cfg(not(unix))]
mod unsupported {
    use super::*;
    fn unsupported<T>() -> io::Result<T> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Construction evidence discard is unavailable on this platform",
        ))
    }
    pub fn observe(_: &Dir, _: &str) -> io::Result<(File, StageEvidence)> {
        unsupported()
    }
    pub fn evidence(_: &File) -> io::Result<StageEvidence> {
        unsupported()
    }
    pub fn try_lock(_: &File) -> io::Result<bool> {
        unsupported()
    }
    pub fn sync(_: &Dir) -> io::Result<()> {
        unsupported()
    }
    pub fn quarantine(
        _: &Dir,
        _: &str,
        _: &Dir,
        _: &str,
        _: &File,
        _: &StageEvidence,
    ) -> io::Result<()> {
        unsupported()
    }
    pub fn unlink(_: &Dir, _: &str, _: &str, _: &StageEvidence) -> io::Result<Place> {
        unsupported()
    }
    pub fn locate(
        _: Option<&Dir>,
        _: &str,
        _: &Dir,
        _: &str,
        _: &StageEvidence,
    ) -> io::Result<Place> {
        unsupported()
    }
    pub fn pending(_: &Dir, _: usize) -> io::Result<Vec<Pending>> {
        unsupported()
    }
}
#[cfg(not(unix))]
pub use unsupported::{evidence, locate, observe, pending, quarantine, sync, try_lock, unlink};

fn open_optional(path: &std::path::Path) -> io::Result<Option<Dir>> {
    match Dir::open_ambient_dir(path, cap_std::ambient_authority()) {
        Ok(dir) => Ok(Some(dir)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// [`locate`] by recorded paths. The source lies inside its namespace, so a
/// missing namespace means the stage is gone.
pub fn locate_at(
    source: &std::path::Path,
    namespace: &std::path::Path,
    name: &str,
    key: &str,
    expected: &StageEvidence,
) -> io::Result<Place> {
    let Some(namespace) = open_optional(namespace)? else {
        return Ok(Place::Gone);
    };
    locate(
        open_optional(source)?.as_ref(),
        name,
        &namespace,
        key,
        expected,
    )
}

/// [`unlink`] by the recorded namespace path.
pub fn unlink_at(
    namespace: &std::path::Path,
    key: &str,
    name: &str,
    expected: &StageEvidence,
) -> io::Result<Place> {
    match open_optional(namespace)? {
        Some(namespace) => unlink(&namespace, key, name, expected),
        None => Ok(Place::Gone),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn dirs(root: &std::path::Path) -> (Dir, Dir) {
        std::fs::create_dir_all(root.join("ns/source")).unwrap();
        let namespace =
            Dir::open_ambient_dir(root.join("ns"), cap_std::ambient_authority()).unwrap();
        let source = namespace.open_dir("source").unwrap();
        (namespace, source)
    }
    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn reviewed_stage_moves_into_its_slot_and_is_unlinked_from_there_only() {
        let temp = tempfile::tempdir().unwrap();
        let (namespace, source) = dirs(temp.path());
        std::fs::write(temp.path().join("ns/source/tmp_stage"), b"stage").unwrap();
        let (file, evidence) = observe(&source, "tmp_stage").unwrap();
        assert_eq!(evidence.size, 5);
        assert!(try_lock(&file).unwrap());
        quarantine(&source, "tmp_stage", &namespace, KEY, &file, &evidence).unwrap();
        assert!(!temp.path().join("ns/source/tmp_stage").exists());
        let slot = temp.path().join("ns").join(QUARANTINE).join(KEY);
        assert_eq!(
            std::fs::metadata(&slot).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            locate(Some(&source), "tmp_stage", &namespace, KEY, &evidence).unwrap(),
            Place::Quarantined
        );
        assert_eq!(pending(&namespace, 10).unwrap().len(), 1);
        assert_eq!(
            unlink(&namespace, KEY, "tmp_stage", &evidence).unwrap(),
            Place::Gone
        );
        assert!(!slot.exists());
        assert_eq!(
            locate(Some(&source), "tmp_stage", &namespace, KEY, &evidence).unwrap(),
            Place::Gone
        );
        assert_eq!(
            unlink(&namespace, KEY, "tmp_stage", &evidence).unwrap(),
            Place::Gone
        );
        assert!(pending(&namespace, 10).unwrap().is_empty());
    }

    #[test]
    fn links_special_files_aliases_and_oversized_stages_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let (_, source) = dirs(temp.path());
        let base = temp.path().join("ns/source");
        std::fs::write(temp.path().join("outside"), b"outside").unwrap();
        std::os::unix::fs::symlink(temp.path().join("outside"), base.join("link")).unwrap();
        std::fs::create_dir(base.join("dir")).unwrap();
        std::fs::write(base.join("aliased"), b"a").unwrap();
        std::fs::hard_link(base.join("aliased"), base.join("alias")).unwrap();
        std::fs::write(base.join("large"), vec![0u8; STAGE_LIMIT as usize + 1]).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(base.join("fifo"))
                .status()
                .unwrap()
                .success()
        );
        for name in [
            "link",
            "dir",
            "aliased",
            "large",
            "fifo",
            "../outside",
            "..",
        ] {
            assert!(observe(&source, name).is_err(), "{name}");
        }
        assert_eq!(
            std::fs::read(temp.path().join("outside")).unwrap(),
            b"outside"
        );
    }

    #[test]
    fn a_changed_stage_is_neither_moved_nor_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let (namespace, source) = dirs(temp.path());
        let path = temp.path().join("ns/source/tmp_stage");
        std::fs::write(&path, b"stage").unwrap();
        let (file, evidence) = observe(&source, "tmp_stage").unwrap();
        std::fs::write(&path, b"other").unwrap();
        assert!(quarantine(&source, "tmp_stage", &namespace, KEY, &file, &evidence).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"other");
        assert!(matches!(
            locate(Some(&source), "tmp_stage", &namespace, KEY, &evidence).unwrap(),
            Place::Changed(_)
        ));
        // A replaced name (new inode) is refused as well.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"stage").unwrap();
        assert!(quarantine(&source, "tmp_stage", &namespace, KEY, &file, &evidence).is_err());
        assert!(path.exists());
    }

    #[test]
    fn a_foreign_quarantined_file_is_reported_and_kept() {
        let temp = tempfile::tempdir().unwrap();
        let (namespace, source) = dirs(temp.path());
        std::fs::write(temp.path().join("ns/source/tmp_stage"), b"stage").unwrap();
        let (file, evidence) = observe(&source, "tmp_stage").unwrap();
        quarantine(&source, "tmp_stage", &namespace, KEY, &file, &evidence).unwrap();
        let moved = temp
            .path()
            .join("ns")
            .join(QUARANTINE)
            .join(KEY)
            .join("tmp_stage");
        std::fs::remove_file(&moved).unwrap();
        std::fs::write(&moved, b"foreign").unwrap();
        assert!(matches!(
            unlink(&namespace, KEY, "tmp_stage", &evidence).unwrap(),
            Place::Changed(_)
        ));
        assert_eq!(std::fs::read(&moved).unwrap(), b"foreign");
        assert!(!valid_key("../escape") && !valid_component("a/b"));
    }
}
