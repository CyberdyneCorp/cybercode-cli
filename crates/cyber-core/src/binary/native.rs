//! Same-filesystem native replacement primitives. The save journal must be durable
//! before exchange; these functions alone never authorize or acknowledge a save.
use super::{Error, MAX_EDIT_SIZE, Offset};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::File;
#[cfg(target_os = "macos")]
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

#[cfg(target_os = "macos")]
#[allow(unsafe_code)] // Bounded owned ACL/copyfile-state allocations, no borrowed raw pointers.
mod macos;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub evidence: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub root: PathBuf,
    pub relative: PathBuf,
    pub parents: Vec<String>,
    pub file: String,
    pub length: Offset,
    pub metadata: Metadata,
    pub observed: String,
}

pub(super) fn id(file: &File) -> Result<String, Error> {
    let m = file.metadata().map_err(|_| Error::Io)?;
    let mut hash = Sha256::new();
    hash.update(b"cyber-native-file-id-v1\0");
    hash.update(m.dev().to_le_bytes());
    hash.update(m.ino().to_le_bytes());
    Ok(format!("{:x}", hash.finalize()))
}
pub(super) fn attributes(file: &File) -> Result<Vec<(CString, Vec<u8>)>, Error> {
    let mut names = vec![0u8; 65536];
    let size = rustix::fs::flistxattr(file, names.as_mut_slice()).map_err(|_| Error::File)?;
    names.truncate(size);
    let mut result = vec![];
    let mut total = 0usize;
    for name in names.split(|b| *b == 0).filter(|part| !part.is_empty()) {
        let name = CString::new(name).map_err(|_| Error::File)?;
        // Privileged labels/capabilities are not silently recreated by ordinary edits.
        #[cfg(target_os = "linux")]
        if name.as_bytes().starts_with(b"security.") || name.as_bytes().starts_with(b"trusted.") {
            return Err(Error::File);
        }
        if result.len() >= 128 {
            return Err(Error::Limit);
        }
        let mut value = vec![0u8; 1024 * 1024];
        let size = rustix::fs::fgetxattr(file, name.as_c_str(), value.as_mut_slice())
            .map_err(|_| Error::File)?;
        total = total.checked_add(size).ok_or(Error::Limit)?;
        if total > 1024 * 1024 {
            return Err(Error::Limit);
        }
        value.truncate(size);
        value.shrink_to_fit();
        result.push((name, value));
    }
    result.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(result)
}
#[cfg(target_os = "macos")]
pub(super) fn private_acl(file: &File) -> Result<(), Error> {
    let acl = macos::acl(file)?;
    let text = std::str::from_utf8(&acl)
        .map_err(|_| Error::File)?
        .trim_end_matches('\0');
    if text
        .lines()
        .any(|line| !line.trim().is_empty() && !line.starts_with("!#acl "))
    {
        return Err(Error::File);
    }
    Ok(())
}

pub fn metadata(file: &File) -> Result<Metadata, Error> {
    let m = file.metadata().map_err(|_| Error::Io)?;
    if !m.is_file() || m.nlink() != 1 || m.mode() & 0o7000 != 0 || m.permissions().readonly() {
        return Err(Error::File);
    }
    let mut hash = Sha256::new();
    hash.update(m.mode().to_le_bytes());
    hash.update(m.uid().to_le_bytes());
    hash.update(m.gid().to_le_bytes());
    #[cfg(target_os = "macos")]
    {
        hash.update(m.st_flags().to_le_bytes());
        hash.update(macos::acl(file)?);
    }
    for (name, value) in attributes(file)? {
        hash.update((name.as_bytes().len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value);
    }
    Ok(Metadata {
        mode: m.mode() & 0o777,
        uid: m.uid(),
        gid: m.gid(),
        evidence: format!("{:x}", hash.finalize()),
    })
}
pub fn copy_metadata(source: &File, destination: &File) -> Result<(), Error> {
    let before = metadata(source)?;
    #[cfg(target_os = "macos")]
    macos::copy_metadata(source, destination)?;
    #[cfg(target_os = "linux")]
    {
        rustix::fs::fchown(
            destination,
            Some(rustix::fs::Uid::from_raw(before.uid)),
            Some(rustix::fs::Gid::from_raw(before.gid)),
        )
        .map_err(|_| Error::Io)?;
        rustix::fs::fchmod(destination, rustix::fs::Mode::from_raw_mode(before.mode))
            .map_err(|_| Error::Io)?;
        for (name, value) in attributes(source)? {
            rustix::fs::fsetxattr(
                destination,
                name.as_c_str(),
                &value,
                rustix::fs::XattrFlags::empty(),
            )
            .map_err(|_| Error::Io)?;
        }
    }
    if metadata(source)? != before || metadata(destination)? != before {
        return Err(Error::Changed);
    }
    // A committed edit has a new modification time, including same-size edits.
    // This touches only the private replacement, never the original inode.
    destination
        .set_modified(std::time::SystemTime::now())
        .map_err(|_| Error::Io)?;
    Ok(())
}

/// Open canonical components without following aliases. Directory handles retain
/// routing authority; returned IDs are persisted so restart cannot rebind a draft.
fn open(root: &Path, relative: &Path, write: bool) -> Result<(Dir, File, Vec<String>), Error> {
    if !root.is_absolute() {
        return Err(Error::Invalid);
    }
    let mut dir =
        Dir::open_ambient_dir(root, cap_std::ambient_authority()).map_err(|_| Error::File)?;
    let mut ids = vec![id(&dir
        .try_clone()
        .map_err(|_| Error::Io)?
        .into_std_file())?];
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(Error::File);
        };
        if parts.peek().is_some() {
            dir = dir.open_dir_nofollow(name).map_err(|_| Error::File)?;
            ids.push(id(&dir
                .try_clone()
                .map_err(|_| Error::Io)?
                .into_std_file())?);
        } else {
            let observed = dir.symlink_metadata(name).map_err(|_| Error::File)?;
            if !observed.is_file() || observed.file_type().is_symlink() {
                return Err(Error::File);
            }
            let mut options = OpenOptions::new();
            options.read(true).write(write).follow(FollowSymlinks::No);
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
            let file = dir
                .open_with(name, &options)
                .map_err(|_| Error::File)?
                .into_std();
            metadata(&file)?;
            return Ok((dir, file, ids));
        }
    }
    Err(Error::File)
}
impl Target {
    pub fn capture(root: &Path, relative: &Path) -> Result<Self, Error> {
        let (_, file, parents) = open(root, relative, true)?;
        let m = file.metadata().map_err(|_| Error::Io)?;
        if m.len() > MAX_EDIT_SIZE {
            return Err(Error::Limit);
        }
        Ok(Self {
            root: root.into(),
            relative: relative.into(),
            parents,
            file: id(&file)?,
            length: Offset(m.len()),
            metadata: metadata(&file)?,
            observed: generation(&file)?,
        })
    }
    pub fn validate(&self) -> Result<(Dir, File), Error> {
        let (parent, file, ids) = open(&self.root, &self.relative, true)?;
        if ids != self.parents
            || id(&file)? != self.file
            || metadata(&file)? != self.metadata
            || file.metadata().map_err(|_| Error::Io)?.len() != self.length.0
            || generation(&file)? != self.observed
        {
            return Err(Error::Changed);
        }
        Ok((parent, file))
    }
    pub fn same_filesystem(&self, private: &Dir) -> Result<(), Error> {
        let (_, file) = self.validate()?;
        if file.metadata().map_err(|_| Error::Io)?.dev()
            != private
                .try_clone()
                .map_err(|_| Error::Io)?
                .into_std_file()
                .metadata()
                .map_err(|_| Error::Io)?
                .dev()
        {
            return Err(Error::File);
        }
        Ok(())
    }
    pub fn name(&self) -> Result<&std::ffi::OsStr, Error> {
        self.relative.file_name().ok_or(Error::Invalid)
    }
}

pub(super) fn generation(file: &File) -> Result<String, Error> {
    let m = file.metadata().map_err(|_| Error::Io)?;
    let mut hash = Sha256::new();
    hash.update(b"cyber-native-generation-v1\0");
    hash.update(m.len().to_le_bytes());
    hash.update(m.mtime().to_le_bytes());
    hash.update(m.mtime_nsec().to_le_bytes());
    hash.update(m.ctime().to_le_bytes());
    hash.update(m.ctime_nsec().to_le_bytes());
    Ok(format!("{:x}", hash.finalize()))
}

/// Exact bounded content evidence through a retained regular-file descriptor.
pub fn content(file: &File, cancelled: impl Fn() -> bool) -> Result<String, Error> {
    use std::os::unix::fs::FileExt;
    if cancelled() {
        return Err(Error::Cancelled);
    }
    let before = file.metadata().map_err(|_| Error::Io)?;
    if !before.is_file() || before.len() > MAX_EDIT_SIZE {
        return Err(Error::File);
    }
    let mut hash = Sha256::new();
    let mut at = 0;
    let mut bytes = vec![0u8; super::DEFAULT_PAGE];
    while at < before.len() {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let size = (before.len() - at).min(bytes.len() as u64) as usize;
        file.read_exact_at(&mut bytes[..size], at)
            .map_err(|_| Error::Changed)?;
        hash.update(&bytes[..size]);
        at += size as u64;
    }
    let after = file.metadata().map_err(|_| Error::Io)?;
    if (
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) != (
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    ) {
        return Err(Error::Changed);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) fn sync(file: &File) -> Result<(), Error> {
    file.sync_all().map_err(|_| Error::Io)?;
    #[cfg(target_os = "macos")]
    rustix::fs::fcntl_fullfsync(file).map_err(|_| Error::Io)?;
    Ok(())
}

pub fn exchange(private: &Dir, stage: &str, target: &Target) -> Result<(), Error> {
    if stage.is_empty()
        || stage.len() > 100
        || stage == "."
        || stage == ".."
        || !stage
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'.')
    {
        return Err(Error::Invalid);
    }
    let (parent, _file) = target.validate()?;
    target.same_filesystem(private)?;
    rustix::fs::renameat_with(
        private,
        stage,
        &parent,
        target.name()?,
        rustix::fs::RenameFlags::EXCHANGE,
    )
    .map_err(|_| Error::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn setup() -> (tempfile::TempDir, Target, Dir) {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("target"), b"before").unwrap();
        std::fs::set_permissions(
            root.path().join("target"),
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        std::fs::create_dir(root.path().join("private")).unwrap();
        std::fs::set_permissions(
            root.path().join("private"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let private =
            Dir::open_ambient_dir(root.path().join("private"), cap_std::ambient_authority())
                .unwrap();
        let target = Target::capture(root.path(), Path::new("target")).unwrap();
        (root, target, private)
    }
    #[test]
    fn exchange_installs_exact_new_inode_preserves_displaced_bytes_and_metadata() {
        let (root, target, private) = setup();
        let (_, original) = target.validate().unwrap();
        #[cfg(target_os = "macos")]
        let attribute = "com.cybercode.binary-test";
        #[cfg(target_os = "linux")]
        let attribute = "user.cybercode-binary-test";
        rustix::fs::fsetxattr(
            &original,
            attribute,
            b"preserved",
            rustix::fs::XattrFlags::empty(),
        )
        .unwrap();
        let target = Target::capture(root.path(), Path::new("target")).unwrap();
        std::fs::write(root.path().join("private/replacement"), b"after!").unwrap();
        let staged = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.path().join("private/replacement"))
            .unwrap();
        copy_metadata(&original, &staged).unwrap();
        staged.sync_all().unwrap();
        let installed = id(&staged).unwrap();
        exchange(&private, "replacement", &target).unwrap();
        let (parent, now, _) = open(root.path(), Path::new("target"), false).unwrap();
        assert_eq!(id(&now).unwrap(), installed);
        assert_eq!(metadata(&now).unwrap(), target.metadata);
        assert_eq!(parent.read("target").unwrap(), b"after!");
        assert_eq!(private.read("replacement").unwrap(), b"before");
        assert!(target.validate().is_err());
        assert_eq!(id(&original).unwrap(), target.file);
    }
    #[test]
    fn symlinks_hardlinks_readonly_and_parent_replacement_do_not_widen_mutation() {
        let (root, target, private) = setup();
        symlink(root.path().join("target"), root.path().join("alias")).unwrap();
        assert!(Target::capture(root.path(), Path::new("alias")).is_err());
        std::fs::hard_link(root.path().join("target"), root.path().join("hardlink")).unwrap();
        assert!(Target::capture(root.path(), Path::new("target")).is_err());
        std::fs::remove_file(root.path().join("hardlink")).unwrap();
        std::fs::set_permissions(
            root.path().join("target"),
            std::fs::Permissions::from_mode(0o444),
        )
        .unwrap();
        assert!(Target::capture(root.path(), Path::new("target")).is_err());
        std::fs::set_permissions(
            root.path().join("target"),
            std::fs::Permissions::from_mode(0o750),
        )
        .unwrap();
        std::fs::write(root.path().join("private/replacement"), b"after!").unwrap();
        std::fs::rename(
            root.path().join("target"),
            root.path().join("external-original"),
        )
        .unwrap();
        std::fs::write(root.path().join("target"), b"external").unwrap();
        assert!(exchange(&private, "replacement", &target).is_err());
        assert_eq!(
            std::fs::read(root.path().join("target")).unwrap(),
            b"external"
        );
        assert_eq!(private.read("replacement").unwrap(), b"after!");
    }
    #[test]
    fn moved_parent_is_refused_instead_of_reopening_the_new_route() {
        let (root, _, private) = setup();
        std::fs::create_dir(root.path().join("nested")).unwrap();
        std::fs::write(root.path().join("nested/target"), b"before").unwrap();
        let target = Target::capture(root.path(), Path::new("nested/target")).unwrap();
        std::fs::write(root.path().join("private/replacement"), b"after!").unwrap();
        std::fs::rename(root.path().join("nested"), root.path().join("moved")).unwrap();
        std::fs::create_dir(root.path().join("nested")).unwrap();
        std::fs::write(root.path().join("nested/target"), b"external").unwrap();
        assert!(exchange(&private, "replacement", &target).is_err());
        assert_eq!(
            std::fs::read(root.path().join("moved/target")).unwrap(),
            b"before"
        );
        assert_eq!(
            std::fs::read(root.path().join("nested/target")).unwrap(),
            b"external"
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn native_linux_posix_acl_is_fingerprinted_and_preserved_without_data_copy() {
        let (root, target, _private) = setup();
        let before = target.metadata;
        let source = File::open(root.path().join("target")).unwrap();
        // Linux POSIX ACL xattr, version 2: owner, named user, group, mask, other.
        let mut acl = 2u32.to_le_bytes().to_vec();
        let named = if source.metadata().unwrap().uid() == 65534 {
            65533
        } else {
            65534
        };
        for (tag, permissions, uid) in [
            (1u16, 6u16, u32::MAX),
            (2, 4, named),
            (4, 0, u32::MAX),
            (16, 4, u32::MAX),
            (32, 0, u32::MAX),
        ] {
            acl.extend(tag.to_le_bytes());
            acl.extend(permissions.to_le_bytes());
            acl.extend(uid.to_le_bytes());
        }
        rustix::fs::fsetxattr(
            &source,
            "system.posix_acl_access",
            &acl,
            rustix::fs::XattrFlags::empty(),
        )
        .unwrap();
        let target = Target::capture(root.path(), Path::new("target")).unwrap();
        assert_ne!(target.metadata.evidence, before.evidence);
        let (_, source) = target.validate().unwrap();
        std::fs::write(root.path().join("private/replacement"), b"changed data").unwrap();
        let destination = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.path().join("private/replacement"))
            .unwrap();
        copy_metadata(&source, &destination).unwrap();
        assert_eq!(metadata(&destination).unwrap(), target.metadata);
        let mut actual = vec![0; acl.len()];
        let size = rustix::fs::fgetxattr(&destination, "system.posix_acl_access", &mut actual[..])
            .unwrap();
        assert_eq!(&actual[..size], acl);
        assert_eq!(
            std::fs::read(root.path().join("private/replacement")).unwrap(),
            b"changed data"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_mac_extended_acl_is_fingerprinted_and_preserved_without_data_copy() {
        let (root, target, _private) = setup();
        let before = target.metadata;
        let status = std::process::Command::new("/bin/chmod")
            .args(["+a", "everyone allow read"])
            .arg(root.path().join("target"))
            .status()
            .unwrap();
        assert!(status.success());
        let target = Target::capture(root.path(), Path::new("target")).unwrap();
        assert_ne!(target.metadata.evidence, before.evidence);
        let (_, source) = target.validate().unwrap();
        std::fs::write(root.path().join("private/replacement"), b"changed data").unwrap();
        let destination = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.path().join("private/replacement"))
            .unwrap();
        copy_metadata(&source, &destination).unwrap();
        assert_eq!(metadata(&destination).unwrap(), target.metadata);
        assert_eq!(
            std::fs::read(root.path().join("private/replacement")).unwrap(),
            b"changed data"
        );
    }
}
