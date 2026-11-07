//! Read-only Git rule inputs. These bindings grant no mutation or recovery authority.
use super::storage::fail;
use crate::git::prepared::PreparedRestore;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use cyber_server::runtime::RestoreError;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::io::Read;
use std::path::{Path, PathBuf};

const INPUT_LIMIT: u64 = 1024 * 1024;
const TOTAL_LIMIT: usize = 8 * 1024 * 1024;
struct Binding {
    path: PathBuf,
    source: bool,
    hash: Option<String>,
}
pub(super) struct Rules {
    root: Dir,
    path: PathBuf,
    case_insensitive: bool,
    bindings: Vec<Binding>,
    stack: Vec<Gitignore>,
    bytes: usize,
}
impl Rules {
    pub(super) fn new(prepared: &PreparedRestore) -> Result<Self, RestoreError> {
        let mut rules = Self {
            root: Dir::open_ambient_dir(&prepared.worktree, cap_std::ambient_authority())
                .map_err(failed)?,
            path: prepared.worktree.clone(),
            case_insensitive: prepared.ignore.case_insensitive,
            bindings: vec![],
            stack: vec![],
            bytes: 0,
        };
        let mut builder = rules.builder()?;
        for path in prepared
            .ignore
            .global
            .iter()
            .chain(std::iter::once(&prepared.ignore.info))
        {
            if let Some(bytes) = rules.bind(path, false)? {
                rules.lines(&mut builder, None, &bytes)?;
            }
        }
        rules.stack.push(
            builder
                .build()
                .map_err(|_| failed("Unsupported Git ignore rules"))?,
        );
        if !prepared.prefix.is_empty() {
            rules.enter("")?;
            let mut parent = Path::new(&prepared.prefix).parent();
            let mut ancestors = Vec::new();
            while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
                ancestors.push(path.to_path_buf());
                parent = path.parent();
            }
            for path in ancestors.into_iter().rev() {
                rules.enter(
                    path.to_str()
                        .ok_or_else(|| failed("Invalid Git rule scope"))?,
                )?;
            }
        }
        Ok(rules)
    }
    fn builder(&self) -> Result<GitignoreBuilder, RestoreError> {
        let mut builder = GitignoreBuilder::new(&self.path);
        builder
            .case_insensitive(self.case_insensitive)
            .map_err(|_| failed("Unsupported Git ignore case policy"))?;
        Ok(builder)
    }
    fn lines(
        &self,
        builder: &mut GitignoreBuilder,
        from: Option<&Path>,
        bytes: &[u8],
    ) -> Result<(), RestoreError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| failed("Non-UTF-8 Git rules are unsupported"))?;
        for line in text.lines() {
            builder
                .add_line(from.map(Path::to_path_buf), line)
                .map_err(|_| failed("Unsupported Git ignore pattern"))?;
        }
        Ok(())
    }
    pub(super) fn enter(&mut self, relative: &str) -> Result<(), RestoreError> {
        let path = Path::new(relative).join(".gitignore");
        let mut builder = self.builder()?;
        if let Some(bytes) = self.bind(&path, true)? {
            self.lines(&mut builder, Some(&self.path.join(&path)), &bytes)?;
        }
        self.stack.push(
            builder
                .build()
                .map_err(|_| failed("Unsupported Git ignore rules"))?,
        );
        Ok(())
    }
    pub(super) fn leave(&mut self) {
        self.stack.pop();
    }
    pub(super) fn ignored(&self, path: &str, directory: bool) -> bool {
        self.stack
            .iter()
            .rev()
            .map(|rules| rules.matched(path, directory))
            .find(|matched| !matched.is_none())
            .is_some_and(|matched| matched.is_ignore())
    }
    fn bind(&mut self, path: &Path, source: bool) -> Result<Option<Vec<u8>>, RestoreError> {
        let bytes = read(&self.root, path, source)?;
        self.bytes = self
            .bytes
            .checked_add(path.as_os_str().len() + bytes.as_ref().map_or(0, Vec::len))
            .ok_or_else(|| failed("Git rule input size overflow"))?;
        if self.bytes > TOTAL_LIMIT || self.bindings.len() >= 65536 {
            return Err(failed("Git rule inputs exceed limit"));
        }
        self.bindings.push(Binding {
            path: path.into(),
            source,
            hash: bytes
                .as_ref()
                .map(|bytes| blake3::hash(bytes).to_hex().to_string()),
        });
        Ok(bytes)
    }
    pub(super) fn verify(&self) -> Result<(), RestoreError> {
        for binding in &self.bindings {
            let bytes = read(&self.root, &binding.path, binding.source)?;
            if bytes
                .as_ref()
                .map(|bytes| blake3::hash(bytes).to_hex().to_string())
                != binding.hash
            {
                return Err(RestoreError::Stale);
            }
        }
        Ok(())
    }
}
fn read(root: &Dir, path: &Path, source: bool) -> Result<Option<Vec<u8>>, RestoreError> {
    let result = if source {
        let mut directory = root.try_clone().map_err(failed)?;
        for part in path
            .parent()
            .ok_or_else(|| failed("Invalid Git rule path"))?
            .components()
        {
            directory = directory
                .open_dir_nofollow(part.as_os_str())
                .map_err(|_| RestoreError::Stale)?;
        }
        let name = path
            .file_name()
            .ok_or_else(|| failed("Invalid Git rule file"))?;
        match directory.symlink_metadata(name) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(failed(error)),
            _ => {}
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_NONBLOCK);
        directory
            .open_with(name, &options)
            .map(|file| file.into_std())
    } else {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
    };
    let mut file = match result {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failed(error)),
    };
    let metadata = file.metadata().map_err(failed)?;
    if !metadata.is_file() || metadata.len() > INPUT_LIMIT {
        return Err(failed("Unsupported or over-limit Git rule input"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(INPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(failed)?;
    if bytes.len() as u64 > INPUT_LIMIT {
        return Err(failed("Git rule input grew beyond limit"));
    }
    Ok(Some(bytes))
}
fn failed(error: impl std::fmt::Display) -> RestoreError {
    RestoreError::Failed(fail(error))
}
