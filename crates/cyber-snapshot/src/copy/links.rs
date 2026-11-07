//! Link-inode observations. No file descriptor here follows a link target.
use super::storage::{self, Result, fail};
use super::{Entry, ImageKind, Store};
use cap_std::fs::Dir;
use std::ffi::OsStr;
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "macos")]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

pub(super) const TARGET_LIMIT: usize = 4096;

pub(super) fn pin_image(dir: &Dir, name: &OsStr) -> Result<File> {
    if dir
        .symlink_metadata(name)
        .map_err(fail)?
        .file_type()
        .is_symlink()
    {
        pin_link(dir, name)
    } else {
        storage::open(dir, Path::new(name))
    }
}
fn pin_link(dir: &Dir, name: &OsStr) -> Result<File> {
    if Path::new(name).file_name() != Some(name) {
        return Err("Invalid symbolic-link leaf".into());
    }
    // macOS O_SYMLINK opens the link itself; combining it with O_NOFOLLOW
    // would reject the link instead. Linux O_PATH|O_NOFOLLOW pins its inode.
    #[cfg(target_os = "macos")]
    let flags = rustix::fs::OFlags::SYMLINK | rustix::fs::OFlags::NONBLOCK;
    #[cfg(target_os = "linux")]
    let flags = rustix::fs::OFlags::PATH | rustix::fs::OFlags::NOFOLLOW;
    let file = File::from(
        rustix::fs::openat(
            dir,
            name,
            flags | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(fail)?,
    );
    let metadata = file.metadata().map_err(fail)?;
    if !metadata.file_type().is_symlink() || metadata.nlink() != 1 {
        return Err("Snapshot source is not a unique symbolic link".into());
    }
    Ok(file)
}

/// Bound the read independently of the file's reported target length.
pub(super) fn read(dir: &Dir, name: &OsStr, limit: u64) -> Result<(File, Vec<u8>)> {
    let pin = pin_link(dir, name)?;
    let before = super::ownership::generation(&pin)?;
    let parent = dir.try_clone().map_err(fail)?.into_std_file();
    let parent_before = super::ownership::generation(&parent)?;
    plain_metadata(dir, name, &pin)?;
    let mut bytes = [0u8; TARGET_LIMIT + 1];
    let size = rustix::fs::readlinkat_raw(dir, name, &mut bytes[..]).map_err(fail)?;
    if size == 0 || size > TARGET_LIMIT || size as u64 > limit {
        return Err("Symbolic-link target exceeds snapshot bounds".into());
    }
    let current = pin_link(dir, name)?;
    if storage::identity(&current)? != storage::identity(&pin)?
        || super::ownership::generation(&pin)? != before
        || super::ownership::generation(&current)? != before
        || super::ownership::generation(&parent)? != parent_before
    {
        return Err("Symbolic-link route or metadata changed while inspecting".into());
    }
    Ok((pin, bytes[..size].to_vec()))
}

#[cfg(target_os = "macos")]
fn plain_metadata(_dir: &Dir, _name: &OsStr, pin: &File) -> Result<()> {
    cyber_core::binary::native::plain_snapshot_symlink(pin)
        .map(|_| ())
        .map_err(|_| "Unsupported symbolic-link metadata".into())
}
#[cfg(target_os = "linux")]
fn plain_metadata(dir: &Dir, name: &OsStr, pin: &File) -> Result<()> {
    use std::os::fd::AsRawFd;
    let metadata = pin.metadata().map_err(fail)?;
    if metadata.mode() & 0o777 != 0o777 {
        return Err("Unsupported symbolic-link mode".into());
    }
    // O_PATH descriptors do not support fgetxattr. The kernel proc route keeps
    // the already pinned parent, and llistxattr never follows the leaf. Both the
    // parent generation and pinned link generation are checked around this read.
    let parent_route = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
    let routed = File::open(&parent_route).map_err(fail)?;
    if storage::identity(&routed)? != storage::dir_identity(dir)? {
        return Err("Symbolic-link metadata route changed".into());
    }
    let mut attributes = [0u8; 65536];
    if rustix::fs::llistxattr(parent_route.join(name), &mut attributes[..]).map_err(fail)? != 0 {
        return Err("Unsupported symbolic-link attributes".into());
    }
    Ok(())
}

impl Store {
    pub(super) fn link_entry(&self, dir: &Dir, name: &OsStr, save: bool) -> Result<Entry> {
        let (pin, bytes) = read(dir, name, self.options.max_file)?;
        let metadata = pin.metadata().map_err(fail)?;
        let mut entry = if save {
            self.object(&bytes)?
        } else {
            Entry {
                kind: ImageKind::Symlink,
                object: storage::digest(&bytes),
                length: bytes.len() as u64,
                mode: 0,
                uid: 0,
                gid: 0,
            }
        };
        entry.kind = ImageKind::Symlink;
        entry.mode = metadata.mode() & 0o777;
        entry.uid = metadata.uid();
        entry.gid = metadata.gid();
        Ok(entry)
    }

    pub(super) fn stage_image(
        &self,
        dir: &Dir,
        name: &str,
        entry: &Entry,
        bytes: &[u8],
    ) -> Result<()> {
        if entry.kind == ImageKind::File {
            let file = storage::create(dir, name, bytes)?;
            return Self::stage_metadata(&file, entry);
        }
        if bytes.is_empty() || bytes.len() > TARGET_LIMIT || bytes.contains(&0) {
            return Err("Invalid symbolic-link target".into());
        }
        rustix::fs::symlinkat(OsStr::from_bytes(bytes), dir, name).map_err(fail)?;
        let pin = pin_link(dir, OsStr::new(name))?;
        #[cfg(target_os = "macos")]
        {
            rustix::fs::fchown(
                &pin,
                Some(rustix::fs::Uid::from_raw(entry.uid)),
                Some(rustix::fs::Gid::from_raw(entry.gid)),
            )
            .map_err(fail)?;
            pin.set_permissions(std::fs::Permissions::from_mode(entry.mode))
                .map_err(fail)?;
            storage::sync_file(&pin)?;
        }
        #[cfg(target_os = "linux")]
        {
            if entry.mode != 0o777 {
                return Err("Unsupported symbolic-link mode".into());
            }
            rustix::fs::chownat(
                dir,
                name,
                Some(rustix::fs::Uid::from_raw(entry.uid)),
                Some(rustix::fs::Gid::from_raw(entry.gid)),
                rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
            )
            .map_err(fail)?;
        }
        if storage::identity(&pin_image(dir, OsStr::new(name))?)? != storage::identity(&pin)? {
            return Err("Prepared symbolic-link inode changed".into());
        }
        storage::sync(dir)
    }

    /// Rechecked immediately before publication. Canonicalize only an existing
    /// prefix so safe dangling targets remain representable without reading data.
    pub(super) fn check_link_target(
        &self,
        path: &str,
        bytes: &[u8],
        boundary: &Path,
    ) -> Result<()> {
        if bytes.is_empty() || bytes.len() > TARGET_LIMIT || bytes.contains(&0) {
            return Err("Invalid symbolic-link target".into());
        }
        let target = self
            .root
            .join(path)
            .parent()
            .ok_or("Invalid link parent")?
            .join(OsStr::from_bytes(bytes));
        let mut normalized = PathBuf::new();
        for part in target.components() {
            match part {
                Component::ParentDir => {
                    if !normalized.pop() {
                        return Err("Unsafe symbolic-link target".into());
                    }
                }
                Component::CurDir => {}
                other => normalized.push(other),
            }
        }
        self.check_link_boundary(&normalized, boundary)?;
        // Keep the original component order: resolving alias/../file can escape
        // even when a lexical collapse of the same spelling stays inside.
        let mut prefix = target.as_path();
        let mut missing = Vec::new();
        loop {
            match std::fs::symlink_metadata(prefix) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(prefix.file_name().ok_or("Invalid symbolic-link prefix")?);
                    prefix = prefix.parent().ok_or("Invalid symbolic-link prefix")?;
                }
                Err(error) => return Err(fail(error)),
            }
        }
        let mut physical = prefix.canonicalize().map_err(fail)?;
        for part in missing.into_iter().rev() {
            physical.push(part);
        }
        self.check_link_boundary(&physical, boundary)?;
        self.check_link_private_identities(&physical, boundary)
    }

    fn check_link_private_identities(&self, target: &Path, boundary: &Path) -> Result<()> {
        let mut private = std::collections::BTreeSet::new();
        for path in self
            .options
            .private_roots
            .iter()
            .chain(std::iter::once(&self.data_path))
        {
            if let Some(metadata) = existing_metadata(path)? {
                private.insert((metadata.dev(), metadata.ino()));
            }
        }
        for cursor in target
            .ancestors()
            .take_while(|cursor| cursor.starts_with(boundary))
        {
            let Some(metadata) = existing_metadata(cursor)? else {
                continue;
            };
            let identity = (metadata.dev(), metadata.ino());
            if private.contains(&identity) {
                return Err("Symbolic-link target aliases a private root".into());
            }
            if let Some(parent) = cursor.parent() {
                // Do not depend on a resolver normalizing filesystem aliases.
                // Compare actual nodes to reserved siblings instead of folding names.
                for name in [".git", ".cyber", ".claude"] {
                    if existing_metadata(&parent.join(name))?
                        .is_some_and(|reserved| (reserved.dev(), reserved.ino()) == identity)
                    {
                        return Err("Symbolic-link target aliases a protected namespace".into());
                    }
                }
            }
        }
        Ok(())
    }
    fn check_link_boundary(&self, target: &Path, boundary: &Path) -> Result<()> {
        let relative = target
            .strip_prefix(boundary)
            .map_err(|_| "Symbolic-link target is outside the worktree")?;
        if super::tree::protected(relative)
            || target.starts_with(&self.data_path)
            || self
                .options
                .private_roots
                .iter()
                .any(|root| target.starts_with(root))
        {
            return Err("Symbolic-link target is protected".into());
        }
        Ok(())
    }
}

fn existing_metadata(path: &Path) -> Result<Option<std::fs::Metadata>> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(fail(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinning_and_reading_a_dangling_link_never_opens_its_target_and_rejects_hardlinks() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        std::os::unix::fs::symlink("missing", temp.path().join("link")).unwrap();
        let (pin, bytes) = read(&dir, OsStr::new("link"), TARGET_LIMIT as u64).unwrap();
        assert!(pin.metadata().unwrap().file_type().is_symlink());
        assert_eq!(bytes, b"missing");
        assert!(!temp.path().join("missing").exists());
        assert!(read(&dir, OsStr::new("link"), 1).is_err());
        std::fs::hard_link(temp.path().join("link"), temp.path().join("alias")).unwrap();
        assert!(pin_image(&dir, OsStr::new("link")).is_err());
    }
    #[test]
    fn same_target_link_substitution_has_a_distinct_native_identity() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        std::os::unix::fs::symlink("missing", temp.path().join("link")).unwrap();
        let old = pin_image(&dir, OsStr::new("link")).unwrap();
        std::fs::rename(temp.path().join("link"), temp.path().join("retained")).unwrap();
        std::os::unix::fs::symlink("missing", temp.path().join("link")).unwrap();
        assert_ne!(
            storage::identity(&old).unwrap(),
            storage::identity(&pin_image(&dir, OsStr::new("link")).unwrap()).unwrap()
        );
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn link_attributes_are_inspected_on_the_pinned_link_and_refuse_before_recreation() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        std::os::unix::fs::symlink("missing", temp.path().join("link")).unwrap();
        let pin = pin_image(&dir, OsStr::new("link")).unwrap();
        rustix::fs::fsetxattr(
            &pin,
            "user.snapshot-test",
            b"retained metadata",
            rustix::fs::XattrFlags::empty(),
        )
        .unwrap();
        assert!(read(&dir, OsStr::new("link"), 4096).is_err());
        assert_eq!(
            std::fs::read_link(temp.path().join("link")).unwrap(),
            Path::new("missing")
        );
        assert!(!temp.path().join("missing").exists());
    }
    #[test]
    fn protected_and_private_node_checks_do_not_depend_on_alias_spelling() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), b"fixture").unwrap();
        std::fs::create_dir(root.join("Private")).unwrap();
        if !root.join(".GIT").exists() {
            return;
        }
        let mut store = Store::open(
            &temp.path().join("data"),
            root.to_str().unwrap(),
            super::super::Options {
                max_file: 65536,
                max_total: 8 * 1024 * 1024,
                ignore: vec![],
                private_roots: vec![root.join("Private")],
            },
        )
        .unwrap();
        // Store canonicalizes its Location; preserve aliases only in the target.
        let boundary = store.root.clone();
        assert!(
            store
                .check_link_private_identities(&boundary.join(".GIT/config"), &boundary)
                .is_err()
        );
        assert!(
            store
                .check_link_private_identities(&boundary.join("PRIVATE/missing"), &boundary)
                .is_err()
        );
        store.options.private_roots.clear();
        assert!(
            store
                .check_link_private_identities(&boundary.join("PRIVATE/missing"), &boundary)
                .is_ok()
        );
    }
}
