//! Read-only Git rule inputs. These bindings grant no mutation or recovery authority.
use super::storage::fail;
use crate::git::prepared::{IgnoreInputs, PreparedRestore};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt};
use cyber_server::runtime::RestoreError;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::io::Read;
use std::path::{Path, PathBuf};

const INPUT_LIMIT: u64 = 1024 * 1024;
const TOTAL_LIMIT: usize = 8 * 1024 * 1024;
#[derive(Clone, serde::Serialize)]
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
    inputs: IgnoreInputs,
}
impl Rules {
    pub(super) fn new(prepared: &PreparedRestore) -> Result<Self, RestoreError> {
        Self::open(&prepared.worktree, &prepared.prefix, &prepared.ignore)
    }
    fn open(worktree: &Path, prefix: &str, inputs: &IgnoreInputs) -> Result<Self, RestoreError> {
        let mut rules = Self {
            root: Dir::open_ambient_dir(worktree, cap_std::ambient_authority()).map_err(failed)?,
            path: worktree.to_path_buf(),
            case_insensitive: inputs.case_insensitive,
            bindings: vec![],
            stack: vec![],
            bytes: 0,
            inputs: inputs.clone(),
        };
        let mut builder = rules.builder()?;
        for path in inputs.global.iter().chain(std::iter::once(&inputs.info)) {
            if let Some(bytes) = rules.bind(path, false)? {
                rules.lines(&mut builder, None, &bytes)?;
            }
        }
        rules.stack.push(
            builder
                .build()
                .map_err(|_| failed("Unsupported Git ignore rules"))?,
        );
        if !prefix.is_empty() {
            rules.enter("")?;
            let mut parent = Path::new(prefix).parent();
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
    pub(super) fn fingerprint(
        &self,
        ignored: &std::collections::BTreeSet<String>,
    ) -> Result<String, RestoreError> {
        let mut bindings: Vec<_> = self
            .bindings
            .iter()
            .filter(|binding| !binding.source || binding.hash.is_some())
            .cloned()
            .collect();
        bindings.sort_by(|left, right| {
            (left.source, &left.path, &left.hash).cmp(&(right.source, &right.path, &right.hash))
        });
        let bytes = serde_json::to_vec(&(
            "git-policy-capture-v1",
            &self.path,
            &self.inputs,
            bindings,
            ignored,
        ))
        .map_err(failed)?;
        Ok(blake3::hash(&bytes).to_hex().to_string())
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

/// Collect read-only policy and raw images. This is not native effect ownership.
pub(crate) fn capture_state(
    worktree: &Path,
    prefix: &str,
    inputs: &IgnoreInputs,
) -> Result<super::git_capture::Capture, RestoreError> {
    let mut capture = PolicyCapture {
        rules: Rules::open(worktree, prefix, inputs)?,
        ignored: std::collections::BTreeSet::new(),
        marks: vec![],
        nodes: 0,
        bytes: 0,
        files: std::collections::BTreeMap::new(),
        content_bytes: 0,
    };
    let mut directory = capture.rules.root.try_clone().map_err(failed)?;
    for part in Path::new(prefix).components() {
        directory = directory
            .open_dir_nofollow(part.as_os_str())
            .map_err(failed)?;
    }
    capture.walk(&directory, "", prefix, 0)?;
    for (path, before) in &capture.marks {
        let mut current = directory.try_clone().map_err(failed)?;
        for part in Path::new(path).components() {
            current = current
                .open_dir_nofollow(part.as_os_str())
                .map_err(|_| RestoreError::Stale)?;
        }
        if dir_generation(&current)? != *before {
            return Err(RestoreError::Stale);
        }
    }
    capture.rules.verify()?;
    Ok(super::git_capture::Capture {
        policy: capture.rules.fingerprint(&capture.ignored)?,
        files: capture.files,
    })
}
struct PolicyCapture {
    rules: Rules,
    ignored: std::collections::BTreeSet<String>,
    marks: Vec<(String, String)>,
    nodes: usize,
    bytes: usize,
    files: std::collections::BTreeMap<String, super::git_capture::RawImage>,
    content_bytes: usize,
}
impl PolicyCapture {
    fn walk(
        &mut self,
        dir: &Dir,
        path: &str,
        prefix: &str,
        depth: usize,
    ) -> Result<(), RestoreError> {
        if depth > 64 {
            return Err(failed("Git policy directory depth exceeds limit"));
        }
        let scope = if prefix.is_empty() {
            path.to_owned()
        } else if path.is_empty() {
            prefix.into()
        } else {
            format!("{prefix}/{path}")
        };
        self.rules.enter(&scope)?;
        let before = dir_generation(dir)?;
        self.charge(path.len() + before.len())?;
        for entry in dir.entries().map_err(failed)? {
            let leaf = entry.map_err(failed)?.file_name();
            let name = leaf
                .to_str()
                .ok_or_else(|| failed("Non-UTF-8 Git policy path is unsupported"))?;
            let relative = if path.is_empty() {
                name.to_owned()
            } else {
                format!("{path}/{name}")
            };
            super::tree::safe_path(&relative).map_err(RestoreError::Failed)?;
            self.charge(relative.len())?;
            let full = if prefix.is_empty() {
                relative.clone()
            } else {
                format!("{prefix}/{relative}")
            };
            if super::tree::protected(Path::new(&relative))
                || self
                    .rules
                    .inputs
                    .private_roots
                    .iter()
                    .any(|root| self.rules.path.join(&full).starts_with(root))
            {
                continue;
            }
            let metadata = dir.symlink_metadata(&leaf).map_err(failed)?;
            let directory = metadata.is_dir() && !metadata.file_type().is_symlink();
            if self.rules.ignored(&full, directory) {
                self.ignored
                    .insert(if directory { format!("{full}/") } else { full });
            } else if directory {
                // Embedded repositories need their own opaque Git-state proof.
                let child = dir
                    .open_dir_nofollow(&leaf)
                    .map_err(|_| RestoreError::Stale)?;
                if child.symlink_metadata(".git").is_ok() {
                    return Err(failed("Embedded Git policy proof is unavailable"));
                }
                self.walk(&child, &relative, prefix, depth + 1)?;
            } else {
                if self.files.len() >= 4096 {
                    return Err(failed("Git raw capture file count exceeds limit"));
                }
                let image = super::git_capture::read(dir, &leaf).map_err(failed)?;
                self.content_bytes = self
                    .content_bytes
                    .checked_add(image.bytes.len())
                    .ok_or_else(|| failed("Git raw capture size overflow"))?;
                if self.content_bytes > 200 * 1024 * 1024 {
                    return Err(failed("Git raw capture exceeds limit"));
                }
                self.files.insert(full, image);
            }
        }
        if dir_generation(dir)? != before {
            return Err(RestoreError::Stale);
        }
        self.rules.leave();
        self.marks.push((path.into(), before));
        Ok(())
    }
    fn charge(&mut self, bytes: usize) -> Result<(), RestoreError> {
        self.nodes += 1;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| failed("Git policy capture size overflow"))?;
        if self.nodes > 65536 || self.bytes > TOTAL_LIMIT {
            return Err(failed("Git policy capture exceeds limit"));
        }
        Ok(())
    }
}
fn dir_generation(dir: &Dir) -> Result<String, RestoreError> {
    super::ownership::generation(&dir.try_clone().map_err(failed)?.into_std_file())
        .map_err(RestoreError::Failed)
}
