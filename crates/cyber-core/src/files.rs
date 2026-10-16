//! Project file browsing (`server-api` → File browsing routes): one directory level, a
//! fuzzy path search and file content, all confined to the project root. A path is
//! resolved lexically first and then canonically, so neither `..` nor a symlink can
//! reach outside the project. `.gitignore` and `.cyberignore` apply. A [`Target`]
//! keeps the identity of what a path named when it was resolved, so a read after a
//! permission wait refuses anything swapped in meanwhile.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;

use crate::vcs::slash_path;

/// Entries one listing returns at most.
pub const MAX_ENTRIES: usize = 2_000;
/// Bytes of content served at most.
pub const MAX_CONTENT: usize = 1024 * 1024;
/// Files a search looks at, at most.
const SEARCH_SCAN: usize = 50_000;
/// Bytes checked for a NUL to call a file binary.
const BINARY_PROBE: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    File,
    Directory,
    Symlink,
}

/// One directory entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Entry {
    pub name: String,
    /// `/`-separated, relative to the project root.
    pub path: String,
    pub kind: Kind,
    /// Bytes, for files.
    pub size: u64,
    /// `M`, `A`, `D` or `?`; a directory shows `M` when anything below it changed.
    pub git_status: Option<String>,
    /// Matched by `.gitignore` or `.cyberignore` (or the `.git` directory itself).
    pub ignored: bool,
    /// A symlink's target, project-relative when it stays inside the project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// More than [`MAX_ENTRIES`] entries were shown.
    pub truncated: bool,
    /// Hidden and ignored entries left out (`hidden=false`).
    pub hidden: usize,
    /// Changed files in the whole project, by git status.
    pub changed: usize,
}

/// A file's content, or that it is binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
    pub size: u64,
    pub language: Option<String>,
    pub lines: usize,
    /// Only the first [`MAX_CONTENT`] bytes are in `text`.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The path or a symlink on it leaves the project.
    OutsideProject,
    NotFound,
    /// A listing of a file, or content of a directory.
    WrongKind,
    /// The path names another file or directory than when it was resolved.
    Changed,
    Io(String),
}

/// A resolved path and the identity of the file or directory it named then.
#[derive(Debug)]
pub struct Target {
    path: PathBuf,
    identity: same_file::Handle,
}

impl Target {
    /// The canonical path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The path is still canonical and still names the same object.
    fn unchanged(&self) -> Result<(), Refusal> {
        let canonical = std::fs::canonicalize(&self.path).map_err(|_| Refusal::Changed)?;
        let now = open_plain(&self.path)
            .and_then(|file| same_file::Handle::from_file(file).map_err(io_refusal))
            .map_err(|_| Refusal::Changed)?;
        if canonical == self.path && now == self.identity {
            Ok(())
        } else {
            Err(Refusal::Changed)
        }
    }
}

/// `rel` resolved inside `root` (canonical), with the identity of what it names. Only a
/// regular file or a directory is a target: a FIFO or a device is refused before it is
/// opened, since opening one can block or act.
pub fn target(root: &Path, rel: &str) -> Result<Target, Refusal> {
    let path = resolve(root, rel)?;
    let kind = std::fs::metadata(&path)
        .map_err(|_| Refusal::NotFound)?
        .file_type();
    if !(kind.is_file() || kind.is_dir()) {
        return Err(Refusal::WrongKind);
    }
    let identity = same_file::Handle::from_file(open_plain(&path)?).map_err(io_refusal)?;
    Ok(Target { path, identity })
}

fn io_refusal(error: std::io::Error) -> Refusal {
    Refusal::Io(error.to_string())
}

/// `path` opened for reading without ever waiting: a FIFO swapped in after a check opens
/// at once (non-blocking on Unix) and is refused, as is anything but a regular file or a
/// directory.
fn open_plain(path: &Path) -> Result<std::fs::File, Refusal> {
    let file = open_nonblocking(path).map_err(|_| Refusal::NotFound)?;
    let kind = file.metadata().map_err(io_refusal)?.file_type();
    if kind.is_file() || kind.is_dir() {
        Ok(file)
    } else {
        Err(Refusal::WrongKind)
    }
}

/// `path` opened for reading; never waits for a FIFO's writer.
fn open_nonblocking(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NONBLOCK);
    // Directories open only with backup semantics.
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::custom_flags(
        &mut options,
        windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS,
    );
    options.open(path)
}

/// The text of a small regular file, for settings read from a directory that may be
/// untrusted (a cloned project's `.cyber/`). Anything but a regular file, such as a
/// FIFO or a symlink to `/dev/zero`, is refused before it is opened, the open never
/// waits, and the read itself stops past `max` bytes, which is refused.
pub fn read_small(path: &Path, max: u64) -> std::io::Result<String> {
    use std::io::{Error, ErrorKind};
    let not_regular = || Error::new(ErrorKind::InvalidInput, "not a regular file");
    if !std::fs::metadata(path)?.is_file() {
        return Err(not_regular());
    }
    let file = open_nonblocking(path)?;
    if !file.metadata()?.is_file() {
        return Err(not_regular());
    }
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("larger than {max} bytes"),
        ));
    }
    String::from_utf8(bytes).map_err(|e| Error::new(ErrorKind::InvalidData, e))
}

/// `rel` inside `root` (canonical), canonicalized; refused when it leaves the project.
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, Refusal> {
    let mut lexical = PathBuf::new();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => lexical.push(part),
            Component::CurDir => {}
            Component::ParentDir if lexical.pop() => {}
            _ => return Err(Refusal::OutsideProject),
        }
    }
    let canonical = std::fs::canonicalize(root.join(&lexical)).map_err(|_| Refusal::NotFound)?;
    if canonical.starts_with(root) {
        Ok(canonical)
    } else {
        Err(Refusal::OutsideProject)
    }
}

/// Names the ignore rules keep in `dir`, walking from `root` so the project's own
/// ignore files (and nothing above it) apply.
fn visible_names(root: &Path, dir: &Path) -> HashSet<OsString> {
    let depth = dir
        .strip_prefix(root)
        .map_or(0, |rel| rel.components().count());
    let target = dir.to_path_buf();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .require_git(false)
        .add_custom_ignore_filename(".cyberignore")
        .max_depth(Some(depth + 1))
        .filter_entry(move |e| {
            e.file_name() != ".git"
                && (target.starts_with(e.path()) || e.path().parent() == Some(&target))
        })
        .build();
    walker
        .flatten()
        .filter(|e| e.path().parent() == Some(dir))
        .map(|e| e.file_name().to_owned())
        .collect()
}

/// The status letter of a file, or `M` for a directory with a change below it.
fn status_of(statuses: &BTreeMap<String, char>, path: &str, directory: bool) -> Option<String> {
    if !directory {
        return statuses.get(path).map(char::to_string);
    }
    let prefix = format!("{path}/");
    statuses
        .range(prefix.clone()..)
        .next()
        .filter(|(key, _)| key.starts_with(&prefix))
        .map(|_| "M".to_owned())
}

/// A symlink's target as the listing shows it.
fn link_target(root: &Path, path: &Path) -> Option<String> {
    let raw = std::fs::read_link(path).ok()?;
    match std::fs::canonicalize(path) {
        Ok(resolved) if resolved.starts_with(root) => {
            resolved.strip_prefix(root).ok().map(slash_path)
        }
        _ => Some(raw.to_string_lossy().into_owned()),
    }
}

fn entry(
    root: &Path,
    dirent: &std::fs::DirEntry,
    ignored: bool,
    statuses: &BTreeMap<String, char>,
) -> Entry {
    let path = dirent.path();
    let name = dirent.file_name().to_string_lossy().into_owned();
    let rel = path
        .strip_prefix(root)
        .map(slash_path)
        .unwrap_or_else(|_| name.clone());
    let meta = std::fs::symlink_metadata(&path).ok();
    let kind = match meta.as_ref().map(std::fs::Metadata::file_type) {
        Some(t) if t.is_symlink() => Kind::Symlink,
        Some(t) if t.is_dir() => Kind::Directory,
        _ => Kind::File,
    };
    Entry {
        git_status: status_of(statuses, &rel, kind == Kind::Directory),
        size: meta.filter(|_| kind == Kind::File).map_or(0, |m| m.len()),
        target: (kind == Kind::Symlink)
            .then(|| link_target(root, &path))
            .flatten(),
        name,
        path: rel,
        kind,
        ignored,
    }
}

/// One level of `rel`: directories first, then by name. Without `hidden`, dot entries
/// and ignored ones are left out and counted.
pub fn list(
    root: &Path,
    rel: &str,
    hidden: bool,
    statuses: &BTreeMap<String, char>,
) -> Result<Listing, Refusal> {
    list_target(root, &target(root, rel)?, hidden, statuses)
}

/// [`list`] of a directory resolved earlier; refused when the path no longer names it,
/// checked before and after reading.
pub fn list_target(
    root: &Path,
    target: &Target,
    hidden: bool,
    statuses: &BTreeMap<String, char>,
) -> Result<Listing, Refusal> {
    let dir = target.path();
    if !dir.is_dir() {
        return Err(Refusal::WrongKind);
    }
    target.unchanged()?;
    let visible = visible_names(root, dir);
    let mut entries = Vec::new();
    let mut left_out = 0;
    let dirents: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Refusal::Io(e.to_string()))?
        .collect();
    target.unchanged()?;
    for dirent in dirents {
        let Ok(dirent) = dirent else { continue };
        let name = dirent.file_name();
        let ignored = name == ".git" || !visible.contains(&name);
        if !hidden && (ignored || name.to_string_lossy().starts_with('.')) {
            left_out += 1;
            continue;
        }
        entries.push(entry(root, &dirent, ignored, statuses));
    }
    entries.sort_by(|a, b| {
        (a.kind != Kind::Directory, &a.name).cmp(&(b.kind != Kind::Directory, &b.name))
    });
    let truncated = entries.len() > MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);
    Ok(Listing {
        entries,
        truncated,
        hidden: left_out,
        changed: statuses.len(),
    })
}

/// How well `query`'s characters appear in order in `path` (lower is better), or
/// `None` when they do not.
pub fn fuzzy_score(query: &str, path: &str) -> Option<usize> {
    let path: Vec<char> = path.to_lowercase().chars().collect();
    let mut at = 0;
    let mut gaps = 0;
    for wanted in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = path[at..].iter().position(|c| *c == wanted)?;
        gaps += usize::from(found > 0 && at > 0);
        at += found + 1;
    }
    Some(gaps * 1000 + path.len())
}

/// Non-ignored files whose path fuzzily matches `query`, best first.
pub fn search(root: &Path, query: &str, limit: usize) -> Vec<String> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .require_git(false)
        .add_custom_ignore_filename(".cyberignore")
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    let mut hits: Vec<(usize, String)> = walker
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .take(SEARCH_SCAN)
        .filter_map(|e| {
            let rel = slash_path(e.path().strip_prefix(root).ok()?);
            fuzzy_score(query, &rel).map(|score| (score, rel))
        })
        .collect();
    hits.sort();
    hits.into_iter().take(limit).map(|(_, rel)| rel).collect()
}

/// The language a file's extension or name names, for display.
pub fn language(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let by_name = match name.as_str() {
        "dockerfile" => Some("dockerfile"),
        "makefile" => Some("makefile"),
        _ => None,
    };
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    let by_ext = match ext.as_deref()? {
        "rs" => "rust",
        "py" => "python",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "java" => "java",
        "sol" => "solidity",
        "sv" | "svh" | "v" => "systemverilog",
        "sh" | "bash" | "zsh" => "bash",
        "ps1" => "powershell",
        "json" | "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "css" => "css",
        "sql" => "sql",
        _ => return by_name.map(str::to_owned),
    };
    Some(by_name.unwrap_or(by_ext).to_owned())
}

/// The first [`MAX_CONTENT`] bytes of a file as text, or that it is binary.
pub fn content(root: &Path, rel: &str) -> Result<Content, Refusal> {
    content_target(&target(root, rel)?)
}

/// [`content`] of a file resolved earlier; refused unless the file opened is the one
/// the path named then.
pub fn content_target(target: &Target) -> Result<Content, Refusal> {
    let path = target.path();
    if !path.is_file() {
        return Err(Refusal::WrongKind);
    }
    let mut file = same_file::Handle::from_file(open_plain(path)?).map_err(io_refusal)?;
    if file != target.identity {
        return Err(Refusal::Changed);
    }
    let file = file.as_file_mut();
    let size = file.metadata().map_or(0, |m| m.len());
    let mut bytes = Vec::new();
    file.take(MAX_CONTENT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Refusal::Io(e.to_string()))?;
    let truncated = bytes.len() > MAX_CONTENT;
    bytes.truncate(MAX_CONTENT);
    let language = language(path);
    let binary = bytes[..bytes.len().min(BINARY_PROBE)].contains(&0);
    let text = (!binary).then(|| text_prefix(&bytes, truncated)).flatten();
    let Some(text) = text else {
        return Ok(Content {
            text: None,
            binary: true,
            size,
            language,
            lines: 0,
            truncated: false,
        });
    };
    Ok(Content {
        lines: text.lines().count(),
        text: Some(text),
        binary: false,
        size,
        language,
        truncated,
    })
}

/// The bytes as UTF-8; a cut may split the last character, which is dropped.
fn text_prefix(bytes: &[u8], truncated: bool) -> Option<String> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text.to_owned()),
        Err(e) if truncated && e.error_len().is_none() => {
            Some(String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned())
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("crates/a/src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join(".cyberignore"), "secret.txt\n").unwrap();
        std::fs::write(root.join("secret.txt"), "s").unwrap();
        std::fs::write(root.join("crates/a/src/lib.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "# hi\n").unwrap();
        (dir, root)
    }

    #[test]
    fn a_level_hides_ignored_and_dot_entries_unless_asked_and_sorts_directories_first() {
        let (_dir, root) = project();
        let statuses = BTreeMap::from([("crates/a/src/lib.rs".to_owned(), 'M')]);
        let shown = list(&root, "", false, &statuses).unwrap();
        let names: Vec<_> = shown.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["crates", "README.md"]);
        assert_eq!(shown.hidden, 4);
        assert_eq!(shown.entries[0].git_status.as_deref(), Some("M"));
        let all = list(&root, "", true, &statuses).unwrap();
        let target = all.entries.iter().find(|e| e.name == "target").unwrap();
        assert!(target.ignored);
        assert!(
            all.entries
                .iter()
                .find(|e| e.name == "secret.txt")
                .unwrap()
                .ignored
        );
        assert!(
            !all.entries
                .iter()
                .find(|e| e.name == ".gitignore")
                .unwrap()
                .ignored
        );
        let lib = list(&root, "crates/a/src", false, &statuses).unwrap();
        assert_eq!(lib.entries[0].path, "crates/a/src/lib.rs");
        assert_eq!(lib.entries[0].git_status.as_deref(), Some("M"));
        assert_eq!(
            list(&root, "README.md", false, &statuses),
            Err(Refusal::WrongKind)
        );
    }

    #[test]
    fn paths_and_symlinks_cannot_leave_the_project() {
        let (_dir, root) = project();
        assert_eq!(
            resolve(&root, "../../etc/passwd"),
            Err(Refusal::OutsideProject)
        );
        assert_eq!(resolve(&root, "/etc/passwd"), Err(Refusal::OutsideProject));
        assert_eq!(
            resolve(&root, "crates/../../x"),
            Err(Refusal::OutsideProject)
        );
        assert!(resolve(&root, "crates/a/../a/src/lib.rs").is_ok());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("key"), "k").unwrap();
            std::os::unix::fs::symlink(outside.path().join("key"), root.join("link")).unwrap();
            assert_eq!(content(&root, "link"), Err(Refusal::OutsideProject));
            let listed = list(&root, "", false, &BTreeMap::new()).unwrap();
            let link = listed.entries.iter().find(|e| e.name == "link").unwrap();
            assert_eq!(link.kind, Kind::Symlink);
            assert!(link.target.as_deref().unwrap().ends_with("key"));
        }
    }

    /// Regression: content and listings re-resolved the name after a permission wait,
    /// so a file or directory swapped in meanwhile was served under the approved name.
    #[test]
    fn a_target_swapped_after_resolving_is_refused() {
        let (_dir, root) = project();
        std::fs::write(root.join("approved.txt"), "fine").unwrap();
        let file = target(&root, "approved.txt").unwrap();
        let dir = target(&root, "crates/a").unwrap();
        // A hard link keeps the canonical path but names another file.
        std::fs::remove_file(root.join("approved.txt")).unwrap();
        std::fs::hard_link(root.join("secret.txt"), root.join("approved.txt")).unwrap();
        assert_eq!(content_target(&file), Err(Refusal::Changed));
        std::fs::rename(root.join("crates/a"), root.join("crates/old")).unwrap();
        std::fs::create_dir(root.join("crates/a")).unwrap();
        assert_eq!(
            list_target(&root, &dir, true, &BTreeMap::new()),
            Err(Refusal::Changed)
        );
        #[cfg(unix)]
        {
            std::fs::write(root.join("plain.txt"), "fine").unwrap();
            let plain = target(&root, "plain.txt").unwrap();
            std::fs::remove_file(root.join("plain.txt")).unwrap();
            std::os::unix::fs::symlink(root.join("secret.txt"), root.join("plain.txt")).unwrap();
            assert_eq!(content_target(&plain), Err(Refusal::Changed));
        }
        let fresh = target(&root, "approved.txt").unwrap();
        assert_eq!(content_target(&fresh).unwrap().text.as_deref(), Some("s"));
    }

    /// Regression: resolving opened the path blocking, so a FIFO with no writer in the
    /// project pinned the caller (an HTTP worker) for good, even when `read` was denied.
    #[cfg(unix)]
    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let (_dir, root) = project();
        let fifo = root.join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        std::fs::write(root.join("plain"), "fine").unwrap();
        let plain = target(&root, "plain").unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        let at = root.clone();
        std::thread::spawn(move || {
            let refused = (target(&at, "pipe").err(), content(&at, "pipe").err());
            // A FIFO swapped in after resolving is refused, not waited on.
            std::fs::remove_file(at.join("plain")).unwrap();
            std::fs::rename(at.join("pipe"), at.join("plain")).unwrap();
            let swapped = (
                content_target(&plain).is_err(),
                list_target(&at, &plain, true, &BTreeMap::new()).is_err(),
                plain.unchanged().is_err(),
            );
            done.send((refused, swapped)).unwrap();
        });
        let (refused, swapped) = finished
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("refused promptly");
        assert_eq!(
            refused,
            (Some(Refusal::WrongKind), Some(Refusal::WrongKind))
        );
        assert_eq!(swapped, (true, true, true));
    }

    #[test]
    fn small_reads_refuse_what_is_not_a_small_regular_file() {
        let (_dir, root) = project();
        std::fs::write(root.join("big"), "x".repeat(65)).unwrap();
        std::fs::write(root.join("fits"), "x".repeat(64)).unwrap();
        assert_eq!(read_small(&root.join("fits"), 64).unwrap().len(), 64);
        let big = read_small(&root.join("big"), 64).unwrap_err();
        assert_eq!(big.kind(), std::io::ErrorKind::InvalidData);
        assert!(big.to_string().contains("larger than 64 bytes"), "{big}");
        let dir = read_small(&root.join("crates"), 64).unwrap_err();
        assert_eq!(dir.kind(), std::io::ErrorKind::InvalidInput);
        let missing = read_small(&root.join("missing"), 64).unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn content_is_capped_binary_is_detected_and_search_is_fuzzy() {
        let (_dir, root) = project();
        let text = content(&root, "crates/a/src/lib.rs").unwrap();
        assert_eq!(text.text.as_deref(), Some("fn a() {}\n"));
        assert_eq!((text.lines, text.language.as_deref()), (1, Some("rust")));
        std::fs::write(root.join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        let blob = content(&root, "blob.bin").unwrap();
        assert!(blob.binary && blob.text.is_none());
        assert_eq!(blob.size, 4);
        std::fs::write(root.join("big.txt"), "é".repeat(MAX_CONTENT)).unwrap();
        let big = content(&root, "big.txt").unwrap();
        assert!(big.truncated);
        assert_eq!(big.text.unwrap().len(), MAX_CONTENT);
        assert_eq!(search(&root, "alib", 10), ["crates/a/src/lib.rs"]);
        assert!(search(&root, "debug", 10).is_empty());
    }
}
