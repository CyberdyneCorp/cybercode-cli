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
impl OpenSource {
    fn open(path: &Path) -> Result<Self, DiscoveryError> {
        let root = path
            .ancestors()
            .last()
            .filter(|root| root.is_absolute())
            .ok_or_else(|| error(path, "source path is not absolute"))?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| error(path, "source path root is invalid"))?;
        let components: Vec<_> = relative
            .components()
            .map(|c| match c {
                Component::Normal(name) => Ok(name.to_owned()),
                _ => Err(error(path, "source path contains traversal")),
            })
            .collect::<Result<_, _>>()?;
        let (name, parents) = components
            .split_last()
            .ok_or_else(|| error(path, "source file name is missing"))?;
        let mut directories = vec![
            Dir::open_ambient_dir(root, cap_std::ambient_authority())
                .map_err(|_| error(path, "source volume root is unavailable"))?,
        ];
        let mut bindings = vec![directory_identity(&directories[0], path)?];
        for component in parents {
            let dir = directories
                .last()
                .unwrap()
                .open_dir_nofollow(component)
                .map_err(|_| error(path, "source directory binding is unavailable or linked"))?;
            bindings.push(directory_identity(&dir, path)?);
            directories.push(dir);
        }
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
            bindings,
            file,
            state,
        })
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
        if !inventory.files.contains(source) {
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
        let mut opened = OpenSource::open(&source.path)?;
        let bytes = opened.bytes(&source.path)?;
        let snapshot = Self {
            path: source.path.clone(),
            opened,
            digest: Sha256::digest(&bytes).into(),
            bytes,
        };
        snapshot.verify()?;
        Ok(snapshot)
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
