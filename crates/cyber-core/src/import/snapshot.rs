use super::{DiscoveryError, SourceFile, SourceLayer, SourceRoots, SourceTool, discover_sources};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    time::SystemTime,
};

#[cfg(windows)]
#[path = "snapshot_windows.rs"]
mod windows;

const LIMIT: u64 = 1024 * 1024;
fn error(path: &Path, reason: &'static str) -> DiscoveryError {
    DiscoveryError {
        path: path.into(),
        reason,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Identity {
    #[cfg(unix)]
    Unix(u64, u64),
    #[cfg(windows)]
    Windows(crate::memory::windows::FileIdentity),
}
fn identity(file: &File, path: &Path) -> Result<Identity, DiscoveryError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = file
            .metadata()
            .map_err(|_| error(path, "source identity is unavailable"))?;
        if m.is_file() && m.nlink() != 1 {
            return Err(error(path, "hard-linked source file is unsupported"));
        }
        Ok(Identity::Unix(m.dev(), m.ino()))
    }
    #[cfg(windows)]
    {
        crate::memory::windows::identity(file)
            .map(Identity::Windows)
            .map_err(|_| error(path, "source native identity or reparse check failed"))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(error(path, "source native identity is unsupported"))
    }
}
fn directory_identity(dir: &Dir, path: &Path) -> Result<Identity, DiscoveryError> {
    let file = dir
        .try_clone()
        .map_err(|_| error(path, "source directory handle is unavailable"))?
        .into_std_file();
    identity(&file, path)
}

struct OpenSource {
    _directories: Vec<Dir>,
    #[cfg(windows)]
    _identity_handles: Vec<File>,
    bindings: Vec<Identity>,
    file: File,
    state: FileState,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileState {
    identity: Identity,
    len: u64,
    modified: SystemTime,
}
fn state(file: &File, path: &Path) -> Result<FileState, DiscoveryError> {
    let metadata = file
        .metadata()
        .map_err(|_| error(path, "source file metadata is unavailable"))?;
    if !metadata.is_file() {
        return Err(error(path, "source is not a regular file"));
    }
    Ok(FileState {
        identity: identity(file, path)?,
        len: metadata.len(),
        modified: metadata
            .modified()
            .map_err(|_| error(path, "source modification time is unavailable"))?,
    })
}
fn directory_chain(path: &Path) -> Result<(Vec<Dir>, Vec<Identity>), DiscoveryError> {
    let root = path
        .ancestors()
        .last()
        .filter(|root| root.is_absolute())
        .ok_or_else(|| error(path, "source path is not absolute"))?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| error(path, "source path root is invalid"))?;
    let mut directories = vec![
        Dir::open_ambient_dir(root, cap_std::ambient_authority())
            .map_err(|_| error(path, "source volume root is unavailable"))?,
    ];
    let mut bindings = vec![directory_identity(&directories[0], path)?];
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(error(path, "source path contains traversal"));
        };
        let dir = directories
            .last()
            .unwrap()
            .open_dir_nofollow(name)
            .map_err(|_| error(path, "source directory binding is unavailable or linked"))?;
        bindings.push(directory_identity(&dir, path)?);
        directories.push(dir);
    }
    Ok((directories, bindings))
}

/// Internal read-time command discovery keeps every directory binding live during enumeration.
pub(crate) fn command_directory(path: &Path) -> Result<Vec<Dir>, DiscoveryError> {
    directory_chain(path).map(|(directories, _)| directories)
}

pub(crate) fn verify_command_directory(dir: &Dir, path: &Path) -> Result<(), DiscoveryError> {
    directory_identity(dir, path).map(|_| ())
}

impl OpenSource {
    fn open(path: &Path) -> Result<Self, DiscoveryError> {
        let name = path
            .file_name()
            .ok_or_else(|| error(path, "source file name is missing"))?;
        let parent_path = path
            .parent()
            .ok_or_else(|| error(path, "source directory is missing"))?;
        let (directories, bindings) = directory_chain(parent_path)?;
        let parent = directories.last().unwrap();
        let metadata = parent
            .symlink_metadata(name)
            .map_err(|_| error(path, "source file is unavailable"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(error(path, "source is not an unlinked regular file"));
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = parent
            .open_with(name, &options)
            .map_err(|_| error(path, "source file cannot be opened without links"))?
            .into_std();
        let state = state(&file, path)?;
        Ok(Self {
            _directories: directories,
            #[cfg(windows)]
            _identity_handles: Vec::new(),
            bindings,
            file,
            state,
        })
    }
    #[cfg(windows)]
    fn retain_review_identities(&mut self, path: &Path) -> Result<(), DiscoveryError> {
        let mut paths: Vec<_> = path.ancestors().skip(1).collect();
        paths.reverse();
        if paths.len() != self._directories.len() {
            return Err(error(path, "source directory identity chain is incomplete"));
        }
        let mut handles = Vec::new();
        for (binding, expected) in paths.iter().zip(&self.bindings) {
            let retained = windows::retain_identity(binding).map_err(|e| {
                let reason = match e.raw_os_error() {
                    Some(5) => "source review identity handoff denied access",
                    Some(6) => "source review identity handoff rejected the directory handle",
                    Some(32) => "source review identity handoff encountered incompatible sharing",
                    Some(87) => "source review identity handoff rejected native parameters",
                    _ => "source directory identity cannot be retained for review",
                };
                error(path, reason)
            })?;
            if identity(&retained, path)? != *expected {
                return Err(error(
                    path,
                    "source directory identity changed during review handoff",
                ));
            }
            handles.push(retained);
        }
        let file = windows::retain_identity(path)
            .map_err(|_| error(path, "source file identity cannot be retained for review"))?;
        if state(&file, path)? != self.state {
            return Err(error(path, "source file changed during review handoff"));
        }
        // Keep the object alive without retaining data access that can prevent
        // NTFS ancestor renames. All content verification uses fresh fenced reads.
        self.file = file;
        self._identity_handles = handles;
        self._directories.clear();
        Ok(())
    }
    fn bytes(&mut self, path: &Path) -> Result<Vec<u8>, DiscoveryError> {
        if self.state.len > LIMIT {
            return Err(error(path, "source exceeds the one MiB read limit"));
        }
        let mut bytes = Vec::new();
        self.file
            .by_ref()
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| error(path, "source read failed"))?;
        if bytes.len() as u64 > LIMIT {
            return Err(error(path, "source exceeds the one MiB read limit"));
        }
        if state(&self.file, path)? != self.state {
            return Err(error(path, "source changed during snapshot read"));
        }
        Ok(bytes)
    }
}

/// A bounded source review; private bytes never appear in Debug or error messages.
pub struct SourceSnapshot {
    path: PathBuf,
    opened: OpenSource,
    bytes: Vec<u8>,
    digest: [u8; 32],
}
impl std::fmt::Debug for SourceSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceSnapshot")
            .field("path", &self.path)
            .field("length", &self.bytes.len())
            .finish_non_exhaustive()
    }
}
impl SourceSnapshot {
    pub fn read(roots: &SourceRoots, source: &SourceFile) -> Result<Self, DiscoveryError> {
        let inventory = discover_sources(roots)?;
        Self::read_admitted(roots, source, &inventory.files)
    }
    pub(super) fn read_admitted(
        roots: &SourceRoots,
        source: &SourceFile,
        files: &[SourceFile],
    ) -> Result<Self, DiscoveryError> {
        if !files.contains(source) {
            return Err(error(
                &source.path,
                "source is not admitted by the current inventory",
            ));
        }
        let base = match source.layer {
            SourceLayer::Project => &roots.project_root,
            SourceLayer::Global => &roots.home,
            SourceLayer::CodexHome if source.tool == SourceTool::Codex => roots
                .codex_home
                .as_ref()
                .ok_or_else(|| error(&source.path, "explicit Codex home is missing"))?,
            _ => return Err(error(&source.path, "source tool and layer do not match")),
        }
        .canonicalize()
        .map_err(|_| error(&source.path, "source root is unavailable"))?;
        if !source.path.starts_with(&base) {
            return Err(error(&source.path, "source is outside its declared root"));
        }
        Self::capture(&source.path)
    }
    pub(crate) fn read_command(path: &Path) -> Result<Self, DiscoveryError> {
        if !path.is_absolute() || path.extension().is_none_or(|e| e != "md") {
            return Err(error(path, "invalid native command source"));
        }
        Self::capture(path)
    }
    pub(super) fn read_native_config(path: &Path) -> Result<Option<Self>, DiscoveryError> {
        let path = std::path::absolute(path)
            .map_err(|_| error(path, "native configuration path is unavailable"))?;
        if path.components().any(|c| matches!(c, Component::ParentDir))
            || !path.file_name().is_some_and(|n| {
                n == "cyber.json" || n == "cyber.jsonc" || n == "cyber.local.jsonc"
            })
        {
            return Err(error(&path, "invalid native configuration target"));
        }
        let mut exists = false;
        for ancestor in path.ancestors() {
            match std::fs::symlink_metadata(ancestor) {
                Ok(metadata) => {
                    if super::discovery::is_link(&metadata) {
                        return Err(error(
                            ancestor,
                            "linked native configuration binding is unsupported",
                        ));
                    }
                    if ancestor == path {
                        exists = true;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => {
                    return Err(error(
                        ancestor,
                        "native configuration metadata is unavailable",
                    ));
                }
            }
        }
        if !exists {
            return Ok(None);
        }
        Self::capture(&path).map(Some)
    }
    fn capture(path: &Path) -> Result<Self, DiscoveryError> {
        let mut opened = OpenSource::open(path)?;
        let bytes = opened.bytes(path)?;
        let snapshot = Self {
            path: path.into(),
            opened,
            digest: Sha256::digest(&bytes).into(),
            bytes,
        };
        snapshot.verify()?;
        snapshot.into_review()
    }
    fn into_review(self) -> Result<Self, DiscoveryError> {
        #[cfg(windows)]
        {
            let mut snapshot = self;
            snapshot.opened.retain_review_identities(&snapshot.path)?;
            snapshot.verify()?;
            Ok(snapshot)
        }
        #[cfg(not(windows))]
        {
            Ok(self)
        }
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn text(&self) -> Result<&str, DiscoveryError> {
        std::str::from_utf8(&self.bytes).map_err(|_| error(&self.path, "source text is not UTF-8"))
    }
    pub fn verify(&self) -> Result<(), DiscoveryError> {
        let mut current = OpenSource::open(&self.path)?;
        if current.bindings != self.opened.bindings || current.state != self.opened.state {
            return Err(error(
                &self.path,
                "source bindings or file changed since snapshot",
            ));
        }
        let bytes = current.bytes(&self.path)?;
        if <[u8; 32]>::from(Sha256::digest(&bytes)) != self.digest {
            return Err(error(&self.path, "source content changed since snapshot"));
        }
        let after = OpenSource::open(&self.path)?;
        if after.bindings != self.opened.bindings || after.state != self.opened.state {
            return Err(error(
                &self.path,
                "source changed during snapshot verification",
            ));
        }
        Ok(())
    }
}
