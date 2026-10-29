//! Content-bound package copies for a trusted, short-lived preparation worker.
//! This does not admit execution. The owner must protect the staging namespace,
//! check private-root provenance and generation authority, and apply confinement.
//! Never perform source-file IO here in the process that owns SQLite locks.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const MAX_REQUEST: usize = 64 * 1024;
pub const MAX_RESPONSE: usize = 32 * 1024;
pub const MAX_FILES: usize = 4096;
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
#[cfg(unix)]
const MAX_ENTRIES: usize = 8192;
#[cfg(unix)]
const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub source: PathBuf,
    /// A new child of an existing private owner directory. The owner retains
    /// that parent until the worker exits, including cancellation/failure.
    pub destination: PathBuf,
    pub expected_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedCopy {
    pub destination: PathBuf,
    /// Ephemeral Unix directory identity. The owner must open/retain the image
    /// beneath its protected parent and verify this binding before using it.
    pub root_identity: (u64, u64),
    pub digest: String,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid package copy request")]
    Request,
    #[error("package source is unavailable, linked or changed during capture")]
    Source,
    #[error("package contains a non-regular entry or unproven hard-link scope")]
    Entry,
    #[error("package copy exceeds a file, entry, depth, path or byte limit")]
    Limit,
    #[error("package copy does not match the approved content digest")]
    Digest,
    #[error("package destination is not a new child of a private owner directory")]
    Destination,
    #[error("package copy IO failed")]
    Io,
    #[error("package preparation is unavailable on this platform")]
    Unavailable,
}

/// Copy without executing anything from the package. The result is a prepared
/// copy, not an admitted/sealed image or a declaration of native OS compatibility.
pub fn prepare(request: &Request) -> Result<PreparedCopy, Error> {
    request.validate()?;
    #[cfg(unix)]
    return unix::prepare(request);
    #[cfg(not(unix))]
    Err(Error::Unavailable)
}

impl Request {
    /// Validate the bounded wire values without opening any source file.
    pub fn validate(&self) -> Result<(), Error> {
        let valid_path = |path: &std::path::Path| {
            path.is_absolute()
                && path
                    .to_str()
                    .is_some_and(|text| text.len() <= 4096 && !text.contains('\0'))
                && path.components().all(|part| {
                    matches!(
                        part,
                        std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                            | std::path::Component::Normal(_)
                    )
                })
        };
        let digest = self
            .expected_digest
            .strip_prefix("sha256:")
            .ok_or(Error::Request)?;
        if !valid_path(&self.source)
            || !valid_path(&self.destination)
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || self.source.starts_with(&self.destination)
            || self.destination.starts_with(&self.source)
        {
            return Err(Error::Request);
        }
        Ok(())
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::{
        Dir, DirBuilder, DirBuilderExt, Metadata, MetadataExt, OpenOptions, OpenOptionsExt,
        Permissions, PermissionsExt,
    };
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::path::{Component, Path};
    use std::time::SystemTime;

    fn directory(path: &Path) -> Result<Dir, Error> {
        let root = path
            .ancestors()
            .last()
            .filter(|root| root.is_absolute())
            .ok_or(Error::Request)?;
        let mut current =
            Dir::open_ambient_dir(root, cap_std::ambient_authority()).map_err(|_| Error::Source)?;
        for part in path
            .strip_prefix(root)
            .map_err(|_| Error::Request)?
            .components()
        {
            let Component::Normal(name) = part else {
                return Err(Error::Request);
            };
            current = current.open_dir_nofollow(name).map_err(|_| Error::Source)?;
        }
        Ok(current)
    }
    fn identity(metadata: &Metadata) -> (u64, u64) {
        (metadata.dev(), metadata.ino())
    }
    fn bound(parent: &Dir, name: &std::ffi::OsStr, expected: (u64, u64)) -> bool {
        parent
            .symlink_metadata(name)
            .is_ok_and(|metadata| metadata.is_dir() && identity(&metadata) == expected)
    }
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct State {
        identity: (u64, u64),
        links: u64,
        length: u64,
        modified: SystemTime,
        mode: u32,
    }
    fn state(metadata: &Metadata) -> Result<State, Error> {
        if !metadata.is_file() {
            return Err(Error::Entry);
        }
        Ok(State {
            identity: identity(metadata),
            links: metadata.nlink(),
            length: metadata.len(),
            modified: metadata.modified().map_err(|_| Error::Source)?.into_std(),
            mode: metadata.mode() & 0o777,
        })
    }
    #[derive(Debug, PartialEq, Eq, Default)]
    struct Inventory {
        files: BTreeMap<String, (PathBuf, State)>,
        directories: BTreeMap<PathBuf, (u64, u64)>,
        entries: usize,
        bytes: u64,
    }
    impl Inventory {
        fn read(root: &Dir) -> Result<Self, Error> {
            let mut inventory = Self::default();
            inventory.walk(root, Path::new(""), 0)?;
            let mut groups = BTreeMap::<(u64, u64), (u64, u64)>::new();
            for (_, state) in inventory.files.values() {
                let group = groups.entry(state.identity).or_insert((state.links, 0));
                if group.0 != state.links || state.links == 0 {
                    return Err(Error::Entry);
                }
                group.1 += 1;
            }
            if groups.values().any(|(links, count)| links != count) {
                return Err(Error::Entry);
            }
            Ok(inventory)
        }
        fn walk(&mut self, directory: &Dir, relative: &Path, depth: usize) -> Result<(), Error> {
            if depth > MAX_DEPTH {
                return Err(Error::Limit);
            }
            self.directories.insert(
                relative.into(),
                identity(&directory.dir_metadata().map_err(|_| Error::Source)?),
            );
            for entry in directory.entries().map_err(|_| Error::Source)? {
                let entry = entry.map_err(|_| Error::Source)?;
                if depth == 0 && entry.file_name() == ".git" {
                    continue;
                }
                self.entries += 1;
                if self.entries > MAX_ENTRIES {
                    return Err(Error::Limit);
                }
                let path = relative.join(entry.file_name());
                let name = path
                    .components()
                    .map(|part| part.as_os_str().to_str().ok_or(Error::Entry))
                    .collect::<Result<Vec<_>, _>>()?
                    .join("/");
                if name.len() > 4096 {
                    return Err(Error::Limit);
                }
                let metadata = directory
                    .symlink_metadata(entry.file_name())
                    .map_err(|_| Error::Source)?;
                if metadata.is_dir() {
                    let child = directory
                        .open_dir_nofollow(entry.file_name())
                        .map_err(|_| Error::Source)?;
                    if identity(&child.dir_metadata().map_err(|_| Error::Source)?)
                        != identity(&metadata)
                    {
                        return Err(Error::Source);
                    }
                    self.walk(&child, &path, depth + 1)?;
                } else {
                    let state = state(&metadata)?;
                    self.bytes = self.bytes.checked_add(state.length).ok_or(Error::Limit)?;
                    if self.files.len() >= MAX_FILES || self.bytes > MAX_BYTES {
                        return Err(Error::Limit);
                    }
                    self.files.insert(name, (path, state));
                }
            }
            Ok(())
        }
    }
    fn source_file(root: &Dir, relative: &Path) -> Result<cap_std::fs::File, Error> {
        let mut parent = root.try_clone().map_err(|_| Error::Io)?;
        for part in relative.parent().ok_or(Error::Entry)?.components() {
            let Component::Normal(name) = part else {
                return Err(Error::Entry);
            };
            parent = parent.open_dir_nofollow(name).map_err(|_| Error::Source)?;
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
        parent
            .open_with(relative.file_name().ok_or(Error::Entry)?, &options)
            .map_err(|_| Error::Source)
    }
    fn copy_file(
        root: &Dir,
        output: &Dir,
        name: &str,
        relative: &Path,
        before: &State,
        hash: &mut Sha256,
    ) -> Result<(), Error> {
        let mut input = source_file(root, relative)?;
        if state(&input.metadata().map_err(|_| Error::Source)?)? != *before {
            return Err(Error::Source);
        }
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(0o600)
            .follow(FollowSymlinks::No);
        let mut file = output
            .open_with(relative, &options)
            .map_err(|_| Error::Destination)?;
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(before.length.to_le_bytes());
        let mut copied = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let limit =
                (before.length.saturating_sub(copied) + 1).min(buffer.len() as u64) as usize;
            let count = match input.read(&mut buffer[..limit]) {
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Error::Io),
            };
            if count == 0 {
                break;
            }
            copied += count as u64;
            if copied > before.length {
                return Err(Error::Source);
            }
            file.write_all(&buffer[..count]).map_err(|_| Error::Io)?;
            hash.update(&buffer[..count]);
        }
        if copied != before.length
            || state(&input.metadata().map_err(|_| Error::Source)?)? != *before
        {
            return Err(Error::Source);
        }
        file.set_permissions(Permissions::from_mode(before.mode))
            .map_err(|_| Error::Io)?;
        Ok(())
    }
    pub(super) fn prepare(request: &Request) -> Result<PreparedCopy, Error> {
        let root = directory(&request.source)?;
        let inventory = Inventory::read(&root)?;
        copy(request, &root, &inventory)
    }
    #[cfg(test)]
    pub(super) fn prepare_after_capture(
        request: &Request,
        change: impl FnOnce(),
    ) -> Result<PreparedCopy, Error> {
        let root = directory(&request.source)?;
        let inventory = Inventory::read(&root)?;
        change();
        copy(request, &root, &inventory)
    }
    fn copy(request: &Request, root: &Dir, inventory: &Inventory) -> Result<PreparedCopy, Error> {
        let parent = directory(request.destination.parent().ok_or(Error::Request)?)?;
        if parent
            .dir_metadata()
            .map_err(|_| Error::Destination)?
            .mode()
            & 0o077
            != 0
        {
            return Err(Error::Destination);
        }
        let name = request.destination.file_name().ok_or(Error::Request)?;
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        parent
            .create_dir_with(name, &builder)
            .map_err(|_| Error::Destination)?;
        let created = parent
            .symlink_metadata(name)
            .map_err(|_| Error::Destination)?;
        if !created.is_dir() {
            return Err(Error::Destination);
        }
        let created_identity = identity(&created);
        let result = (|| {
            let output = parent
                .open_dir_nofollow(name)
                .map_err(|_| Error::Destination)?;
            if identity(&output.dir_metadata().map_err(|_| Error::Destination)?) != created_identity
                || output
                    .entries()
                    .map_err(|_| Error::Destination)?
                    .next()
                    .is_some()
            {
                return Err(Error::Destination);
            }
            for path in inventory
                .directories
                .keys()
                .filter(|path| !path.as_os_str().is_empty())
            {
                output
                    .create_dir_with(path, &builder)
                    .map_err(|_| Error::Destination)?;
            }
            let mut hash = Sha256::new();
            for (name, (path, state)) in &inventory.files {
                copy_file(root, &output, name, path, state, &mut hash)?;
            }
            let digest = format!("sha256:{:x}", hash.finalize());
            if digest != request.expected_digest {
                return Err(Error::Digest);
            }
            let current = directory(&request.source)?;
            if identity(&current.dir_metadata().map_err(|_| Error::Source)?)
                != identity(&root.dir_metadata().map_err(|_| Error::Source)?)
                || &Inventory::read(&current)? != inventory
            {
                return Err(Error::Source);
            }
            if !bound(&parent, name, created_identity) {
                return Err(Error::Destination);
            }
            Ok(PreparedCopy {
                destination: request.destination.clone(),
                root_identity: created_identity,
                digest,
                files: inventory.files.len(),
                bytes: inventory.bytes,
            })
        })();
        if result.is_err() && bound(&parent, name, created_identity) {
            let _ = parent.remove_dir_all(name);
        }
        result
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    struct Fixture {
        _temp: tempfile::TempDir,
        source: PathBuf,
        parent: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let source = root.join("source");
            let parent = root.join("private");
            std::fs::create_dir(&source).unwrap();
            std::fs::create_dir(&parent).unwrap();
            std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                _temp: temp,
                source,
                parent,
            }
        }
        fn request(&self) -> Request {
            Request {
                source: self.source.clone(),
                destination: self.parent.join("image"),
                expected_digest: crate::plugins::tree_digest(&self.source).unwrap(),
            }
        }
        fn invalid_digest(&self) -> Request {
            Request {
                source: self.source.clone(),
                destination: self.parent.join("image"),
                expected_digest: format!("sha256:{}", "0".repeat(64)),
            }
        }
    }
    #[test]
    fn copied_tree_matches_existing_trust_digest_and_breaks_owned_source_aliases() {
        let f = Fixture::new();
        std::fs::create_dir_all(f.source.join("nested/empty")).unwrap();
        let main = f.source.join("nested/main");
        std::fs::write(&main, b"approved bytes").unwrap();
        std::fs::set_permissions(&main, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::hard_link(&main, f.source.join("alias")).unwrap();
        symlink("/not-followed", f.source.join(".git")).unwrap();
        let request = f.request();
        let report = prepare(&request).unwrap();
        assert_eq!(report.files, 2);
        assert_eq!(report.bytes, 28);
        assert_eq!(
            crate::plugins::tree_digest(&report.destination).unwrap(),
            request.expected_digest
        );
        assert!(report.destination.join("nested/empty").is_dir());
        assert!(!report.destination.join(".git").exists());
        let copied = report.destination.join("nested/main");
        assert_ne!(
            std::fs::metadata(&copied).unwrap().ino(),
            std::fs::metadata(&main).unwrap().ino()
        );
        assert_eq!(std::fs::metadata(&copied).unwrap().nlink(), 1);
        assert_eq!(
            std::fs::metadata(report.destination.join("alias"))
                .unwrap()
                .nlink(),
            1
        );
        assert_eq!(
            std::fs::metadata(&copied).unwrap().permissions().mode() & 0o777,
            0o755
        );
        std::fs::write(f.source.join("alias"), "changed source").unwrap();
        assert_eq!(std::fs::read(&copied).unwrap(), b"approved bytes");
        assert_eq!(
            crate::plugins::tree_digest(&report.destination).unwrap(),
            request.expected_digest
        );
    }
    #[test]
    fn unknown_digest_removes_partial_output_but_never_overwrites_existing_destination() {
        let f = Fixture::new();
        std::fs::write(f.source.join("data"), "approved").unwrap();
        let request = f.invalid_digest();
        assert_eq!(prepare(&request), Err(Error::Digest));
        assert!(!request.destination.exists());
        std::fs::create_dir(&request.destination).unwrap();
        std::fs::write(request.destination.join("owned"), "must remain").unwrap();
        assert_eq!(prepare(&f.request()), Err(Error::Destination));
        assert_eq!(
            std::fs::read_to_string(request.destination.join("owned")).unwrap(),
            "must remain"
        );
    }
    #[test]
    fn links_to_outside_files_and_source_symlinks_are_refused_before_output_creation() {
        let f = Fixture::new();
        let outside = f.parent.join("outside");
        std::fs::write(&outside, "not package authority").unwrap();
        std::fs::hard_link(&outside, f.source.join("alias")).unwrap();
        let request = f.request();
        assert_eq!(prepare(&request), Err(Error::Entry));
        assert!(!request.destination.exists());
        std::fs::remove_file(f.source.join("alias")).unwrap();
        symlink(&outside, f.source.join("alias")).unwrap();
        assert_eq!(prepare(&f.invalid_digest()), Err(Error::Entry));
    }
    #[test]
    fn linked_directory_bindings_and_overlapping_output_are_refused() {
        let f = Fixture::new();
        let mut request = f.request();
        let link = f.parent.join("linked-source");
        symlink(&f.source, &link).unwrap();
        request.source = link;
        assert_eq!(prepare(&request), Err(Error::Source));
        request = f.request();
        request.destination = f.source.join("image");
        assert_eq!(prepare(&request), Err(Error::Request));
        std::fs::set_permissions(&f.parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(prepare(&f.request()), Err(Error::Destination));
    }
    #[test]
    fn oversized_sparse_source_is_refused_before_allocating_or_copying_its_contents() {
        let f = Fixture::new();
        std::fs::File::create(f.source.join("large"))
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        let request = f.invalid_digest();
        assert_eq!(prepare(&request), Err(Error::Limit));
        assert!(!request.destination.exists());
    }
    #[test]
    fn nonregular_and_excess_depth_sources_are_refused() {
        let f = Fixture::new();
        let _socket = std::os::unix::net::UnixListener::bind(f.source.join("socket")).unwrap();
        assert_eq!(prepare(&f.invalid_digest()), Err(Error::Entry));
        std::fs::remove_file(f.source.join("socket")).unwrap();
        let mut path = f.source.clone();
        for _ in 0..33 {
            path.push("nested");
            std::fs::create_dir(&path).unwrap();
        }
        assert_eq!(prepare(&f.invalid_digest()), Err(Error::Limit));
    }
    #[test]
    fn replacements_between_inventory_and_copy_are_refused_and_cleaned() {
        for replace in ["file", "directory", "root", "fifo", "symlink"] {
            let f = Fixture::new();
            std::fs::create_dir(f.source.join("nested")).unwrap();
            let file = f.source.join("nested/main");
            std::fs::write(&file, "approved bytes").unwrap();
            let request = f.request();
            let result = unix::prepare_after_capture(&request, || {
                match replace {
                    "file" => {
                        // Equal bytes must not disguise a different file identity.
                        std::fs::rename(&file, f.parent.join("old-file")).unwrap();
                        std::fs::write(&file, "approved bytes").unwrap();
                    }
                    "directory" => {
                        std::fs::rename(f.source.join("nested"), f.parent.join("old-dir")).unwrap();
                        std::fs::create_dir(f.source.join("nested")).unwrap();
                        std::fs::write(&file, "approved bytes").unwrap();
                    }
                    "root" => {
                        // The pinned old tree still hashes correctly. Only a
                        // final source-path binding check catches this swap.
                        std::fs::rename(&f.source, f.parent.join("old-root")).unwrap();
                        std::fs::create_dir_all(f.source.join("nested")).unwrap();
                        std::fs::write(&file, "approved bytes").unwrap();
                    }
                    "fifo" => {
                        std::fs::remove_file(&file).unwrap();
                        // A blocking source open would hang on this controlled FIFO.
                        assert!(
                            std::process::Command::new("mkfifo")
                                .arg(&file)
                                .status()
                                .unwrap()
                                .success()
                        );
                    }
                    "symlink" => {
                        std::fs::rename(&file, f.parent.join("outside")).unwrap();
                        symlink(f.parent.join("outside"), &file).unwrap();
                    }
                    _ => unreachable!(),
                }
            });
            assert!(
                matches!(result, Err(Error::Source | Error::Entry)),
                "{replace}: {result:?}"
            );
            assert!(
                !request.destination.exists(),
                "{replace}: partial copy survived"
            );
        }
    }
    #[test]
    fn file_and_directory_entry_budgets_are_enforced_before_output_creation() {
        let f = Fixture::new();
        for n in 0..=MAX_FILES {
            std::fs::write(f.source.join(format!("f{n:05}")), []).unwrap();
        }
        let request = f.invalid_digest();
        assert_eq!(prepare(&request), Err(Error::Limit));
        assert!(!request.destination.exists());
        std::fs::remove_dir_all(&f.source).unwrap();
        std::fs::create_dir(&f.source).unwrap();
        for n in 0..=8192 {
            std::fs::create_dir(f.source.join(format!("d{n:05}"))).unwrap();
        }
        assert_eq!(prepare(&request), Err(Error::Limit));
        assert!(!request.destination.exists());
    }
}
