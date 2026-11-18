//! File selection (`code-intelligence` → Repository map file selection): the ignore-aware
//! walk of `grep`/`glob`, the default excludes, `.gitattributes` linguist attributes and
//! the configured `include`/`exclude` globs. Content checks (generated headers, binaries)
//! run later, only for files whose size or mtime changed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cyber_core::config::RepoMapSettings;
use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};

/// Directories excluded by default at any depth.
const DEFAULT_DIRS: &[&str] = &[
    ".cyber",
    "node_modules",
    "vendor",
    "third_party",
    "target",
    "dist",
    "build",
    ".venv",
    "__pycache__",
];
/// File name patterns excluded by default.
const DEFAULT_FILES: &[&str] = &[
    "*.min.js",
    "*.min.css",
    "*.map",
    "*.lock",
    "package-lock.json",
    "*.pb.go",
    "*_pb2.py",
    "*.generated.*",
];
/// Bytes read for the binary check.
pub const BINARY_PROBE: usize = 8 * 1024;
/// Lines read for the generated-file check.
const GENERATED_LINES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    DefaultExclude,
    Generated,
    Vendored,
    TooLarge,
    Binary,
    ConfigExclude,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DefaultExclude => "default_exclude",
            Self::Generated => "generated",
            Self::Vendored => "vendored",
            Self::TooLarge => "too_large",
            Self::Binary => "binary",
            Self::ConfigExclude => "config_exclude",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [
            Self::DefaultExclude,
            Self::Generated,
            Self::Vendored,
            Self::TooLarge,
            Self::Binary,
            Self::ConfigExclude,
        ]
        .into_iter()
        .find(|reason| reason.as_str() == text)
    }
}

/// One walked file: its Location-relative path (forward slashes), size, mtime and the
/// reason it is excluded, if any. A pruned directory is one entry ending in `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub excluded: Option<Reason>,
}

pub struct Walked {
    pub entries: Vec<Entry>,
    /// Candidate files beyond `max_files` were left out.
    pub truncated: bool,
    /// The walk stopped at its deadline; entries are incomplete.
    pub interrupted: bool,
}

#[derive(Clone)]
pub struct Selector {
    include: GlobSet,
    exclude: GlobSet,
    default_files: GlobSet,
    max_file_bytes: u64,
    has_include: bool,
    /// The literal directory prefix of each `include` glob (`vendor/acme/` for
    /// `vendor/acme/**`); empty when the glob can match anywhere.
    include_prefixes: Vec<String>,
}

impl Selector {
    pub fn new(settings: &RepoMapSettings) -> Self {
        Self {
            include: globs(&settings.include),
            exclude: globs(&settings.exclude),
            default_files: globs(DEFAULT_FILES.iter().map(|g| format!("**/{g}"))),
            max_file_bytes: settings.max_file_bytes,
            has_include: !settings.include.is_empty(),
            include_prefixes: settings.include.iter().map(|g| literal_dir(g)).collect(),
        }
    }

    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_bytes
    }

    fn included(&self, rel: &str) -> bool {
        self.has_include && self.include.is_match(rel)
    }

    /// A default-excluded directory is descended only when an `include` glob may match
    /// below it.
    fn prune_dir(&self, rel: &str, name: &str) -> bool {
        DEFAULT_DIRS.contains(&name) && !self.include_below(rel)
    }

    fn include_below(&self, rel: &str) -> bool {
        let dir = format!("{rel}/");
        self.include_prefixes
            .iter()
            .any(|prefix| prefix.starts_with(&dir) || dir.starts_with(prefix.as_str()))
    }

    /// The path-based exclusion reason of a candidate file.
    fn reason(&self, rel: &str, size: u64, attrs: &Attributes) -> Option<Reason> {
        if size > self.max_file_bytes {
            return Some(Reason::TooLarge);
        }
        if self.exclude.is_match(rel) {
            return Some(Reason::ConfigExclude);
        }
        if self.included(rel) {
            return None;
        }
        if in_default_dir(rel) || self.default_files.is_match(rel) {
            return Some(Reason::DefaultExclude);
        }
        attrs.reason(rel)
    }
}

fn literal_dir(glob: &str) -> String {
    let literal = &glob[..glob.find(['*', '?', '[', '{']).unwrap_or(glob.len())];
    literal[..literal.rfind('/').map_or(0, |i| i + 1)].to_owned()
}

fn in_default_dir(rel: &str) -> bool {
    let mut parts: Vec<&str> = rel.split('/').collect();
    parts.pop();
    parts.iter().any(|part| DEFAULT_DIRS.contains(part))
}

fn globs<I, S>(patterns: I) -> GlobSet
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        let pattern = pattern.as_ref();
        if let Ok(glob) = GlobBuilder::new(pattern).literal_separator(true).build() {
            set.add(glob);
        } else if let Ok(glob) = Glob::new(pattern) {
            set.add(glob);
        }
    }
    set.build().unwrap_or_else(|_| GlobSet::empty())
}

/// `linguist-generated` and `linguist-vendored` from `.gitattributes` files.
#[derive(Default)]
pub struct Attributes {
    /// (glob, generated, vendored) in file order; later lines win.
    rules: Vec<(globset::GlobMatcher, Option<bool>, Option<bool>)>,
}

impl Attributes {
    fn load(&mut self, dir_rel: &str, file: &Path) {
        let Ok(text) = std::fs::read_to_string(file) else {
            return;
        };
        for line in text.lines() {
            self.parse_line(dir_rel, line);
        }
    }

    fn parse_line(&mut self, dir_rel: &str, line: &str) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return;
        }
        let mut parts = line.split_whitespace();
        let Some(pattern) = parts.next() else {
            return;
        };
        let (mut generated, mut vendored) = (None, None);
        for attr in parts {
            match attr {
                "linguist-generated" | "linguist-generated=true" => generated = Some(true),
                "-linguist-generated" | "linguist-generated=false" => generated = Some(false),
                "linguist-vendored" | "linguist-vendored=true" => vendored = Some(true),
                "-linguist-vendored" | "linguist-vendored=false" => vendored = Some(false),
                _ => {}
            }
        }
        if generated.is_none() && vendored.is_none() {
            return;
        }
        let anchored = pattern.trim_start_matches('/');
        let glob = match (pattern.contains('/'), dir_rel.is_empty()) {
            (true, true) => anchored.to_owned(),
            (true, false) => format!("{dir_rel}/{anchored}"),
            (false, true) => format!("**/{pattern}"),
            (false, false) => format!("{dir_rel}/**/{pattern}"),
        };
        let glob = glob.trim_end_matches('/').to_owned();
        for candidate in [glob.clone(), format!("{glob}/**")] {
            if let Ok(built) = GlobBuilder::new(&candidate).literal_separator(true).build() {
                self.rules
                    .push((built.compile_matcher(), generated, vendored));
            }
        }
    }

    fn reason(&self, rel: &str) -> Option<Reason> {
        let (mut generated, mut vendored) = (false, false);
        for (glob, gen_attr, vendor_attr) in &self.rules {
            if glob.is_match(rel) {
                generated = gen_attr.unwrap_or(generated);
                vendored = vendor_attr.unwrap_or(vendored);
            }
        }
        if vendored {
            Some(Reason::Vendored)
        } else if generated {
            Some(Reason::Generated)
        } else {
            None
        }
    }
}

/// Walk `root` in sorted order, stopping at `deadline` and after `max_files` candidates.
pub fn walk(
    root: &Path,
    selector: &Selector,
    max_files: usize,
    deadline: Option<Instant>,
    delay: Duration,
) -> Walked {
    let mut attrs = Attributes::default();
    if let Ok(meta) = std::fs::symlink_metadata(root.join(".gitattributes"))
        && meta.is_file()
    {
        attrs.load("", &root.join(".gitattributes"));
    }
    let mut walked = Walked {
        entries: Vec::new(),
        truncated: false,
        interrupted: false,
    };
    let root_owned = root.to_path_buf();
    let pruned = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let prune = (selector.clone(), pruned.clone());
    let mut candidates = 0usize;
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(move |e| keep_entry(&root_owned, e, &prune.0, &prune.1))
        .build();
    for entry in walker.filter_map(Result::ok) {
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        if deadline.is_some_and(|d| Instant::now() > d) {
            walked.interrupted = true;
            break;
        }
        let Some(rel) = relative(root, entry.path()) else {
            continue;
        };
        let Some(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            visit_dir(&rel, entry.path(), &mut attrs);
            continue;
        }
        if !kind.is_file() {
            continue; // Symlinks are never followed.
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let excluded = selector.reason(&rel, meta.len(), &attrs);
        if excluded.is_none() {
            if candidates == max_files {
                walked.truncated = true;
                break;
            }
            candidates += 1;
        }
        walked.entries.push(Entry {
            path: rel,
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
            excluded,
        });
    }
    let pruned = std::mem::take(&mut *pruned.lock().unwrap_or_else(|e| e.into_inner()));
    walked.entries.extend(pruned.into_iter().map(|path| Entry {
        path: format!("{path}/"),
        size: 0,
        mtime_ns: 0,
        excluded: Some(Reason::DefaultExclude),
    }));
    walked.entries.sort_by(|a, b| a.path.cmp(&b.path));
    walked
}

/// `.git/` is skipped, and so is every nested repository or submodule. A default-excluded
/// directory is pruned (and recorded once) unless an `include` glob may match below it.
fn keep_entry(
    root: &Path,
    entry: &ignore::DirEntry,
    selector: &Selector,
    pruned: &std::sync::Mutex<Vec<String>>,
) -> bool {
    if entry.file_name() == ".git" {
        return false;
    }
    let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
    if !is_dir || entry.path() == root {
        return true;
    }
    if entry.path().join(".git").exists() {
        return false;
    }
    let Some(rel) = relative(root, entry.path()) else {
        return true;
    };
    let name = rel.rsplit('/').next().unwrap_or(&rel);
    if selector.prune_dir(&rel, name) {
        pruned.lock().unwrap_or_else(|e| e.into_inner()).push(rel);
        return false;
    }
    true
}

/// Nested `.gitattributes` apply below their directory.
fn visit_dir(rel: &str, path: &Path, attrs: &mut Attributes) {
    let file = path.join(".gitattributes");
    if !rel.is_empty() && file.is_file() {
        attrs.load(rel, &file);
    }
}

pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

pub fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_nanos()).ok())
        .unwrap_or(0)
}

/// The content-based exclusion of a file: a NUL byte in its first 8 KiB, or
/// `@generated`/`DO NOT EDIT` in its first 5 lines. `include` globs override the
/// generated check but not the binary one.
pub fn content_reason(bytes: &[u8], included: bool) -> Option<Reason> {
    if bytes[..bytes.len().min(BINARY_PROBE)].contains(&0) {
        return Some(Reason::Binary);
    }
    if included {
        return None;
    }
    let head = bytes
        .split(|b| *b == b'\n')
        .take(GENERATED_LINES)
        .any(|line| contains(line, b"@generated") || contains(line, b"DO NOT EDIT"));
    head.then_some(Reason::Generated)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

impl Selector {
    pub fn is_included(&self, rel: &str) -> bool {
        self.included(rel)
    }
}

/// The absolute path of a Location-relative one.
pub fn absolute(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(root.to_path_buf(), |p, part| p.join(part))
}

#[cfg(test)]
#[path = "walk_tests.rs"]
mod tests;
