//! Instruction files (`system-context` → Instruction file discovery, Instruction imports,
//! Nested rule files on read, Path-scoped rules and overrides).
//!
//! One ordered list of file names drives the baseline `core/instructions`, nested on-read
//! discovery and `cyber doctor`, so every path loads the same files in the same order.
//!
//! Every file is privileged model context, so reading fails closed: a project file (or an
//! import of one) must resolve inside the canonical project root and pass the
//! [`ReadGate`] (the Location's `read` permission rules), only regular files are opened,
//! and no read goes past the per-file and total byte budgets.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use cyber_core::env::EnvSource;

/// Files read from each directory, in load order. `CLAUDE.md` is read only where the
/// directory has no `AGENTS.md`. `CYBER.md`, Cyber Code's own instruction file, comes
/// last so Cyber-specific guidance wins ties within a directory.
pub const DIRECTORY_FILES: [&str; 4] = ["AGENTS.md", "CLAUDE.md", "CONTEXT.md", "CYBER.md"];
/// Files read from the global config directory, in load order.
pub const GLOBAL_FILES: [&str; 2] = ["AGENTS.md", "CYBER.md"];
/// Takes the `AGENTS.md` slot and replaces every file in [`OVERRIDDEN`] of its directory.
pub const OVERRIDE_FILE: &str = "AGENTS.override.md";
const OVERRIDDEN: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "CYBER.md"];
/// `instructions.max_bytes`: each file is truncated here with a notice.
pub const DEFAULT_MAX_BYTES: usize = 32 * 1024;
/// All baseline instruction text together, imports included, stops here.
pub const DEFAULT_MAX_TOTAL_BYTES: usize = 4 * DEFAULT_MAX_BYTES;
/// The instruction text one nested read reminder carries; the rest is named, not attached.
pub const NESTED_MAX_TOTAL_BYTES: usize = DEFAULT_MAX_BYTES;
const MAX_IMPORT_DEPTH: usize = 5;
pub const INSTRUCTIONS_FROM: &str = "Instructions from: ";

/// What the environment and config allow to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionSettings {
    /// `CLAUDE.md` fallback (`compat.claude`, not `CYBER_DISABLE_CLAUDE_MD`).
    pub claude_md: bool,
    /// Project files (not `CYBER_DISABLE_PROJECT_CONFIG`). Global files always load.
    pub project: bool,
    pub max_bytes: usize,
    /// The budget for all baseline text; nested reminders use [`NESTED_MAX_TOTAL_BYTES`].
    pub max_total_bytes: usize,
    /// Where `@~/` imports resolve; `None` leaves them unresolved.
    pub home: Option<PathBuf>,
    /// Which project files (and imports of them) may be read; `None` allows every one,
    /// for hosts without a permission policy.
    pub read_gate: Option<ReadGate>,
}

/// Whether a project instruction file may enter the model context, given its path as
/// discovered (or as an import resolved it) and its canonical path. Only `true` loads it,
/// so a file the Location's `read` rules would ask about or deny (`.env` through an
/// `@.env` import or a symlink) never reaches the context without that prompt.
#[derive(Clone)]
pub struct ReadGate(Arc<ReadCheck>);

/// `(path as discovered, canonical path) -> may it load`.
type ReadCheck = dyn Fn(&Path, &Path) -> bool + Send + Sync;

impl ReadGate {
    pub fn new(allow: impl Fn(&Path, &Path) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(allow))
    }

    pub fn allows(&self, path: &Path, canonical: &Path) -> bool {
        (self.0)(path, canonical)
    }
}

impl std::fmt::Debug for ReadGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReadGate")
    }
}

impl PartialEq for ReadGate {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ReadGate {}

impl InstructionSettings {
    pub fn from_env(claude_compat: bool, env: &dyn EnvSource) -> Self {
        Self {
            claude_md: claude_compat
                && !env.flag("CYBER_DISABLE_CLAUDE_MD")
                && !env.flag("CYBER_DISABLE_CLAUDE_COMPAT"),
            project: !env.flag("CYBER_DISABLE_PROJECT_CONFIG"),
            max_bytes: DEFAULT_MAX_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            home: cyber_core::paths::home_dir(env),
            read_gate: None,
        }
    }

    fn read_allowed(&self, path: &Path, canonical: &Path) -> bool {
        self.read_gate
            .as_ref()
            .is_none_or(|gate| gate.allows(path, canonical))
    }
}

/// One file that was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFile {
    /// The path as discovered (or as an import resolved it).
    pub path: PathBuf,
    /// The deduplication key.
    pub canonical: PathBuf,
    pub truncated: bool,
}

/// Rendered blocks, the files behind them in load order, and non-fatal problems.
#[derive(Debug, Default)]
pub struct Instructions {
    pub blocks: Vec<String>,
    pub files: Vec<InstructionFile>,
    /// Files that apply but were left out because the total budget was spent.
    pub omitted: Vec<InstructionFile>,
    pub warnings: Vec<String>,
}

impl Instructions {
    pub fn text(&self) -> Option<String> {
        (!self.blocks.is_empty()).then(|| self.blocks.join("\n\n"))
    }

    /// Whether a nested read has anything to attach.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() && self.omitted.is_empty()
    }

    /// The text appended to a tool result for nested instructions, if any: the files'
    /// text, then the paths of the files the budget left out.
    pub fn reminder(&self) -> Option<String> {
        let mut parts: Vec<String> = self.text().into_iter().collect();
        if !self.omitted.is_empty() {
            parts.push(pointer_text(
                "These instruction files also apply here but exceed the instruction budget",
                &self.omitted,
            ));
        }
        (!parts.is_empty()).then(|| reminder_block(&parts.join("\n\n")))
    }

    /// A short reminder naming every file, for when [`Self::reminder`] does not fit the
    /// tool output budget.
    pub fn pointer(&self) -> Option<String> {
        let all: Vec<InstructionFile> = self.files.iter().chain(&self.omitted).cloned().collect();
        (!all.is_empty()).then(|| {
            reminder_block(&pointer_text(
                "These instruction files apply here but exceed the tool output budget",
                &all,
            ))
        })
    }

    /// Canonical paths of every file a reminder covers (attached or named).
    pub fn attached(&self) -> Vec<String> {
        self.files
            .iter()
            .chain(&self.omitted)
            .map(|file| file.canonical.display().to_string())
            .collect()
    }

    /// The paths of the files a reminder covers, as discovered.
    pub fn paths(&self) -> Vec<String> {
        self.files
            .iter()
            .chain(&self.omitted)
            .map(|file| file.path.display().to_string())
            .collect()
    }
}

fn reminder_block(text: &str) -> String {
    format!("\n\n<system-reminder>\n{text}\n</system-reminder>")
}

fn pointer_text(reason: &str, files: &[InstructionFile]) -> String {
    let list: Vec<String> = files
        .iter()
        .map(|file| format!("- {}", file.path.display()))
        .collect();
    format!(
        "{reason}; read them with the read tool:\n{}",
        list.join("\n")
    )
}

/// The candidate files of one directory, in load order.
pub fn directory_files(dir: &Path, settings: &InstructionSettings) -> Vec<PathBuf> {
    let overridden = dir.join(OVERRIDE_FILE).is_file();
    let has_agents = dir.join("AGENTS.md").exists();
    DIRECTORY_FILES
        .iter()
        .filter_map(|name| match *name {
            "AGENTS.md" if overridden => Some(dir.join(OVERRIDE_FILE)),
            name if overridden && OVERRIDDEN.contains(&name) => None,
            "CLAUDE.md" if !settings.claude_md || has_agents => None,
            name => Some(dir.join(name)),
        })
        .collect()
}

/// Directories from the project root of `location` down to `dir`, root first; empty
/// when `dir` is outside that project.
pub fn project_dirs(location: &Path, dir: &Path) -> Vec<PathBuf> {
    let root = cyber_core::config::project_root(location);
    let mut dirs: Vec<PathBuf> = dir
        .ancestors()
        .take_while(|d| d.starts_with(&root))
        .map(Path::to_path_buf)
        .collect();
    dirs.reverse();
    dirs
}

/// The baseline: global files, then each directory from the project root down to the
/// Location. A file that exists but cannot be read makes the source unavailable.
pub fn load_baseline(
    global_dir: &Path,
    location: &Path,
    settings: &InstructionSettings,
) -> Result<Instructions, String> {
    let mut loader = Loader::new(settings, settings.max_total_bytes);
    match fill_baseline(&mut loader, global_dir, location) {
        Some(error) => Err(error),
        None => Ok(loader.out),
    }
}

/// Instruction files of the directories between the project root and `target` that are
/// neither in the baseline of `location` nor in `attached` (canonical paths already
/// attached this Context Epoch), within [`NESTED_MAX_TOTAL_BYTES`].
pub fn load_nested(
    global_dir: &Path,
    location: &Path,
    target: &Path,
    settings: &InstructionSettings,
    attached: &BTreeSet<String>,
) -> Instructions {
    // Canonical on both sides, so a `..` or symlinked path cannot escape the project.
    let (location, target) = (canonical(location), canonical(target));
    let dir = if target.is_dir() {
        Some(target.as_path())
    } else {
        target.parent()
    };
    let dirs = dir
        .map(|dir| project_dirs(&location, dir))
        .unwrap_or_default();
    if !settings.project || dirs.is_empty() {
        return Instructions::default();
    }
    let mut loader = Loader::new(settings, settings.max_total_bytes);
    let _ = fill_baseline(&mut loader, global_dir, &location);
    loader.out = Instructions::default();
    loader.remaining = NESTED_MAX_TOTAL_BYTES;
    loader.seen.extend(attached.iter().map(PathBuf::from));
    let root = canonical(&cyber_core::config::project_root(&location));
    for file in dirs.iter().flat_map(|dir| directory_files(dir, settings)) {
        if let Err(error) = loader.load(&file, Some(&root)) {
            loader.out.warnings.push(error);
        }
    }
    loader.out
}

/// The file a `read` of `path` targets, normalized lexically like the read tool does:
/// `~/` expands to `home`, and a relative path resolves against the Location.
pub fn read_target(location: &Path, home: Option<&Path>, path: &str) -> PathBuf {
    let joined = match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => location.join(path),
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Load every baseline file; returns the first read error, if any.
fn fill_baseline(loader: &mut Loader<'_>, global_dir: &Path, location: &Path) -> Option<String> {
    let mut first = None;
    for file in GLOBAL_FILES.iter().map(|name| global_dir.join(name)) {
        if let Err(error) = loader.load(&file, None) {
            first.get_or_insert(error);
        }
    }
    if !loader.settings.project {
        return first;
    }
    let root = canonical(&cyber_core::config::project_root(location));
    let dirs = project_dirs(location, location);
    for file in dirs
        .iter()
        .flat_map(|dir| directory_files(dir, loader.settings))
    {
        if let Err(error) = loader.load(&file, Some(&root)) {
            first.get_or_insert(error);
        }
    }
    first
}

/// Reads each file once by canonical path, expanding `@path` imports depth first.
struct Loader<'a> {
    settings: &'a InstructionSettings,
    seen: BTreeSet<PathBuf>,
    /// Canonical paths of the files whose imports are being expanded.
    stack: Vec<PathBuf>,
    /// Bytes of instruction text still allowed.
    remaining: usize,
    out: Instructions,
}

impl<'a> Loader<'a> {
    fn new(settings: &'a InstructionSettings, budget: usize) -> Self {
        Self {
            settings,
            seen: BTreeSet::new(),
            stack: Vec::new(),
            remaining: budget,
            out: Instructions::default(),
        }
    }

    /// Load a candidate file. A missing file is skipped; a file that resolves outside
    /// `confine` (the canonical project root of a project file) or that the read gate
    /// refuses is skipped with a warning; any other problem, a path that is not a regular
    /// file included, is returned, so the baseline fails closed. Imports of a project
    /// file stay inside the same root and pass the same gate, so a checkout cannot pull in
    /// files from elsewhere or secrets the `read` rules protect.
    fn load(&mut self, path: &Path, confine: Option<&Path>) -> Result<(), String> {
        match std::fs::metadata(path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
        let key = canonical(path);
        if confine.is_some_and(|root| !key.starts_with(root)) {
            self.out.warnings.push(format!(
                "{} resolves outside the project and was not loaded",
                path.display()
            ));
            return Ok(());
        }
        if confine.is_some() && !self.settings.read_allowed(path, &key) {
            self.out.warnings.push(format!(
                "{} is not allowed by the read permission rules and was not loaded",
                path.display()
            ));
            return Ok(());
        }
        self.include(path, key, confine)
    }

    /// Read and render one file whose canonical path is `key`, then its imports.
    fn include(&mut self, path: &Path, key: PathBuf, confine: Option<&Path>) -> Result<(), String> {
        if !self.seen.insert(key.clone()) {
            return Ok(());
        }
        let file = InstructionFile {
            path: path.to_path_buf(),
            canonical: key.clone(),
            truncated: false,
        };
        if self.remaining == 0 {
            self.out.warnings.push(format!(
                "{} was not loaded: instruction files exceed their total budget",
                path.display()
            ));
            self.out.omitted.push(file);
            return Ok(());
        }
        let limit = self.remaining.min(self.settings.max_bytes);
        let read = read_bounded(path, limit)?;
        let body = self.render(path, read, limit);
        self.remaining -= body.kept;
        self.out.files.push(InstructionFile {
            truncated: body.omitted > 0,
            ..file
        });
        self.stack.push(key);
        for reference in imports(&body.text) {
            self.import(path, &reference, confine);
        }
        self.stack.pop();
        Ok(())
    }

    /// The block for `read`, with a truncation notice and warning when it was cut.
    fn render(&mut self, path: &Path, read: Bounded, limit: usize) -> Rendered {
        let (kept, omitted) = (read.text.len(), read.omitted);
        let mut block = read.text.trim_end().to_owned();
        if omitted > 0 {
            let max = self.settings.max_bytes;
            let (notice, warning) = if limit == max {
                (
                    format!(
                        "[Truncated: {} exceeds instructions.max_bytes ({max} bytes); {omitted} bytes omitted]",
                        path.display()
                    ),
                    format!(
                        "{} exceeds instructions.max_bytes ({max} bytes) and was truncated",
                        path.display()
                    ),
                )
            } else {
                (
                    format!(
                        "[Truncated: instruction files exceed their total budget; {omitted} bytes of {} omitted]",
                        path.display()
                    ),
                    format!(
                        "{} was truncated: instruction files exceed their total budget",
                        path.display()
                    ),
                )
            };
            block = format!("{}\n{notice}", read.text);
            self.out.warnings.push(warning);
        }
        self.out
            .blocks
            .push(format!("{INSTRUCTIONS_FROM}{}\n{block}", path.display()));
        Rendered {
            text: read.text,
            kept,
            omitted,
        }
    }

    fn import(&mut self, from: &Path, reference: &str, confine: Option<&Path>) {
        let target = self.resolve(from, reference);
        let Some(key) = target.as_deref().and_then(|t| t.canonicalize().ok()) else {
            return self.warn(from, reference, "does not resolve to a file");
        };
        if self.stack.contains(&key) {
            return self.warn(from, reference, "is an import cycle and was skipped");
        }
        if self.stack.len() > MAX_IMPORT_DEPTH {
            return self.warn(from, reference, "exceeds the import depth of 5");
        }
        if confine.is_some_and(|root| !key.starts_with(root)) {
            return self.warn(
                from,
                reference,
                "is outside the project and was not imported",
            );
        }
        let target = target.unwrap_or_default();
        if confine.is_some() && !self.settings.read_allowed(&target, &key) {
            return self.warn(
                from,
                reference,
                "is not allowed by the read permission rules and was not imported",
            );
        }
        if let Err(e) = self.include(&target, key, confine) {
            self.warn(from, reference, &format!("could not be read: {e}"));
        }
    }

    fn resolve(&self, from: &Path, reference: &str) -> Option<PathBuf> {
        if let Some(rest) = reference.strip_prefix("~/") {
            return self.settings.home.as_ref().map(|home| home.join(rest));
        }
        let path = Path::new(reference);
        if path.is_absolute() {
            return Some(path.to_path_buf());
        }
        from.parent().map(|dir| dir.join(path))
    }

    fn warn(&mut self, from: &Path, reference: &str, problem: &str) {
        self.out
            .warnings
            .push(format!("{}: import @{reference} {problem}", from.display()));
    }
}

/// A file's text read up to a byte limit, and how many bytes past it were left unread.
struct Bounded {
    text: String,
    omitted: usize,
}

/// The kept text of a rendered file (imports are parsed from it) and its size.
struct Rendered {
    text: String,
    kept: usize,
    omitted: usize,
}

/// Read at most `limit` bytes of `path`, cut back to a character boundary. A path that
/// is not a regular file is an error and is never read, so a FIFO, device or directory
/// cannot block or exhaust the loader; invalid UTF-8 is an error too.
fn read_bounded(path: &Path, limit: usize) -> Result<Bounded, String> {
    let error = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let irregular = || error(&"not a regular file");
    // Checked before opening: opening a FIFO for reading blocks.
    if !std::fs::metadata(path).map_err(|e| error(&e))?.is_file() {
        return Err(irregular());
    }
    let file = std::fs::File::open(path).map_err(|e| error(&e))?;
    let metadata = file.metadata().map_err(|e| error(&e))?;
    if !metadata.is_file() {
        return Err(irregular());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| error(&e))?;
    let cut = bytes.len() > limit;
    bytes.truncate(limit);
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        // A cut can split the last character; anything else is not UTF-8 text.
        Err(e) if cut && e.utf8_error().error_len().is_none() => {
            let valid = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).map_err(|e| error(&e))?
        }
        Err(e) => return Err(error(&e.utf8_error())),
    };
    let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    let omitted = if cut {
        size.saturating_sub(text.len()).max(1)
    } else {
        0
    };
    Ok(Bounded { text, omitted })
}

/// `@path` references on their own line, outside code fences.
fn imports(text: &str) -> Vec<String> {
    let mut fenced = false;
    let mut found = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        let Some(reference) = line.strip_prefix('@').filter(|_| !fenced) else {
            continue;
        };
        if !reference.is_empty() && !reference.contains(char::is_whitespace) {
            found.push(reference.to_owned());
        }
    }
    found
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Repo {
        _temp: tempfile::TempDir,
        root: PathBuf,
        global: PathBuf,
    }

    impl Repo {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let base = temp.path().canonicalize().unwrap();
            let root = base.join("repo");
            std::fs::create_dir_all(root.join(".git")).unwrap();
            let global = base.join("global");
            std::fs::create_dir_all(&global).unwrap();
            Self {
                _temp: temp,
                root,
                global,
            }
        }

        fn write(&self, relative: &str, text: &str) -> PathBuf {
            let path = self.root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        }

        fn baseline(&self, location: &str, settings: &InstructionSettings) -> Instructions {
            load_baseline(&self.global, &self.root.join(location), settings).unwrap()
        }
    }

    fn settings() -> InstructionSettings {
        InstructionSettings::from_env(true, &HashMap::<String, String>::new())
    }

    fn env(key: &str) -> InstructionSettings {
        InstructionSettings::from_env(true, &HashMap::from([(key.to_owned(), "1".to_owned())]))
    }

    /// File names in load order (the global ones prefixed with `~`).
    fn order(repo: &Repo, loaded: &Instructions) -> Vec<String> {
        loaded
            .files
            .iter()
            .map(|file| match file.path.strip_prefix(&repo.root) {
                Ok(relative) => relative.display().to_string().replace('\\', "/"),
                Err(_) => format!("~{}", file.path.file_name().unwrap().to_string_lossy()),
            })
            .collect()
    }

    #[test]
    fn one_directory_loads_agents_context_then_cyber() {
        let repo = Repo::new();
        for name in ["CYBER.md", "CONTEXT.md", "CLAUDE.md", "AGENTS.md"] {
            repo.write(name, name);
        }
        let loaded = repo.baseline("", &settings());
        assert_eq!(
            order(&repo, &loaded),
            ["AGENTS.md", "CONTEXT.md", "CYBER.md"]
        );
        std::fs::remove_file(repo.root.join("AGENTS.md")).unwrap();
        let loaded = repo.baseline("", &settings());
        assert_eq!(
            order(&repo, &loaded),
            ["CLAUDE.md", "CONTEXT.md", "CYBER.md"]
        );
        let text = loaded.text().unwrap();
        assert!(text.find("CONTEXT.md\nCONTEXT.md") < text.find("CYBER.md\nCYBER.md"));
    }

    #[test]
    fn more_specific_directories_load_later_and_global_cyber_follows_global_agents() {
        let repo = Repo::new();
        std::fs::write(repo.global.join("CYBER.md"), "global cyber").unwrap();
        std::fs::write(repo.global.join("AGENTS.md"), "global agents").unwrap();
        repo.write("CYBER.md", "root cyber");
        repo.write("packages/api/AGENTS.md", "api agents");
        repo.write("packages/api/CYBER.md", "api cyber");
        let loaded = repo.baseline("packages/api", &settings());
        assert_eq!(
            order(&repo, &loaded),
            [
                "~AGENTS.md",
                "~CYBER.md",
                "CYBER.md",
                "packages/api/AGENTS.md",
                "packages/api/CYBER.md"
            ]
        );
    }

    #[test]
    fn a_project_with_only_cyber_md_loads_it() {
        let repo = Repo::new();
        let cyber = repo.write("CYBER.md", "Use cargo nextest.");
        let loaded = repo.baseline("", &settings());
        assert_eq!(
            loaded.text().unwrap(),
            format!("{INSTRUCTIONS_FROM}{}\nUse cargo nextest.", cyber.display())
        );
    }

    #[test]
    fn disabling_claude_md_keeps_cyber_md() {
        let repo = Repo::new();
        repo.write("CLAUDE.md", "claude");
        repo.write("CYBER.md", "cyber");
        let loaded = repo.baseline("", &env("CYBER_DISABLE_CLAUDE_MD"));
        assert_eq!(order(&repo, &loaded), ["CYBER.md"]);
        let compat_off = InstructionSettings::from_env(false, &HashMap::<String, String>::new());
        assert_eq!(order(&repo, &repo.baseline("", &compat_off)), ["CYBER.md"]);
    }

    #[test]
    fn disabling_project_config_skips_project_cyber_md_but_keeps_global() {
        let repo = Repo::new();
        std::fs::write(repo.global.join("CYBER.md"), "global").unwrap();
        repo.write("CYBER.md", "project");
        let settings = env("CYBER_DISABLE_PROJECT_CONFIG");
        assert_eq!(order(&repo, &repo.baseline("", &settings)), ["~CYBER.md"]);
        let target = repo.write("web/CYBER.md", "nested");
        let nested = load_nested(
            &repo.global,
            &repo.root,
            &target,
            &settings,
            &BTreeSet::new(),
        );
        assert!(nested.files.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn cyber_md_symlinked_to_agents_md_loads_once() {
        let repo = Repo::new();
        repo.write("AGENTS.md", "shared");
        std::os::unix::fs::symlink("AGENTS.md", repo.root.join("CYBER.md")).unwrap();
        let loaded = repo.baseline("", &settings());
        assert_eq!(order(&repo, &loaded), ["AGENTS.md"]);
    }

    #[test]
    fn import_cycle_through_cyber_md_is_broken_with_a_warning() {
        let repo = Repo::new();
        repo.write("CYBER.md", "cyber\n@docs/a.md");
        repo.write("docs/a.md", "a\n@../CYBER.md\n```\n@ignored.md\n```");
        let loaded = repo.baseline("", &settings());
        assert_eq!(order(&repo, &loaded), ["CYBER.md", "docs/a.md"]);
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
        let warning = &loaded.warnings[0];
        assert!(
            warning.contains("a.md") && warning.contains("CYBER.md"),
            "{warning}"
        );
        assert!(warning.contains("cycle"), "{warning}");
    }

    #[test]
    fn imports_stop_at_a_depth_of_five_with_a_warning() {
        let repo = Repo::new();
        repo.write("CYBER.md", "cyber\n@d1.md");
        for depth in 1..=6 {
            repo.write(
                &format!("d{depth}.md"),
                &format!("d{depth}\n@d{}.md", depth + 1),
            );
        }
        let loaded = repo.baseline("", &settings());
        assert_eq!(
            order(&repo, &loaded),
            ["CYBER.md", "d1.md", "d2.md", "d3.md", "d4.md", "d5.md"]
        );
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
        assert!(loaded.warnings[0].contains("@d6.md exceeds the import depth of 5"));
    }

    #[test]
    fn project_imports_stay_inside_the_project() {
        let repo = Repo::new();
        std::fs::write(repo.global.join("secret.md"), "secret").unwrap();
        repo.write("CYBER.md", "@../global/secret.md\n@missing.md");
        let loaded = repo.baseline("", &settings());
        assert_eq!(order(&repo, &loaded), ["CYBER.md"]);
        assert!(!loaded.text().unwrap().contains("secret\n"));
        assert_eq!(loaded.warnings.len(), 2, "{:?}", loaded.warnings);
        assert!(loaded.warnings[0].contains("outside the project"));
    }

    /// Regression: project files and their imports skipped the read rules, so `@.env`
    /// pulled a local secret into the privileged baseline without a prompt.
    #[test]
    fn the_read_gate_refuses_project_files_and_imports_but_not_global_files() {
        let repo = Repo::new();
        std::fs::write(repo.global.join("CYBER.md"), "global rule").unwrap();
        repo.write(".env", "TOKEN=hunter2");
        repo.write("CYBER.md", "root rule\n@.env");
        repo.write("web/AGENTS.md", "web rule");
        let mut settings = settings();
        settings.read_gate = Some(ReadGate::new(|path, canonical| {
            !canonical.ends_with(".env") && !path.ends_with("web/AGENTS.md")
        }));
        let loaded = repo.baseline("web", &settings);
        assert_eq!(order(&repo, &loaded), ["~CYBER.md", "CYBER.md"]);
        assert!(!loaded.text().unwrap().contains("hunter2"));
        assert_eq!(loaded.warnings.len(), 2, "{:?}", loaded.warnings);
        assert!(loaded.warnings[0].contains("@.env is not allowed by the read permission rules"));
        assert!(loaded.warnings[1].contains("not allowed by the read permission rules"));
        let nested = load_nested(
            &repo.global,
            &repo.root,
            &repo.root.join("web/app.ts"),
            &settings,
            &BTreeSet::new(),
        );
        assert!(nested.is_empty(), "{:?}", nested.blocks);
    }

    #[test]
    fn oversized_cyber_md_is_truncated_with_a_notice() {
        let repo = Repo::new();
        repo.write("CYBER.md", &"é".repeat(40));
        let mut settings = settings();
        settings.max_bytes = 11;
        let loaded = repo.baseline("", &settings);
        assert!(loaded.files[0].truncated);
        let text = loaded.text().unwrap();
        assert!(
            text.contains(&format!("\n{}\n[Truncated: ", "é".repeat(5))),
            "{text}"
        );
        assert!(text.contains("exceeds instructions.max_bytes (11 bytes); 70 bytes omitted]"));
        assert_eq!(loaded.warnings.len(), 1);
    }

    #[test]
    fn agents_override_replaces_agents_claude_and_cyber_md() {
        let repo = Repo::new();
        repo.write("CYBER.md", "root cyber");
        for name in [
            "AGENTS.md",
            "CLAUDE.md",
            "CONTEXT.md",
            "CYBER.md",
            OVERRIDE_FILE,
        ] {
            repo.write(&format!("packages/api/{name}"), name);
        }
        let loaded = repo.baseline("packages/api", &settings());
        assert_eq!(
            order(&repo, &loaded),
            [
                "CYBER.md",
                "packages/api/AGENTS.override.md",
                "packages/api/CONTEXT.md"
            ]
        );
    }

    #[test]
    fn nested_read_attaches_unloaded_directory_files_once() {
        let repo = Repo::new();
        repo.write("CYBER.md", "root");
        repo.write("web/AGENTS.md", "web agents");
        let web_cyber = repo.write("web/CYBER.md", "web cyber");
        let file = repo.write("web/src/app.ts", "");
        let nested = load_nested(
            &repo.global,
            &repo.root,
            &file,
            &settings(),
            &BTreeSet::new(),
        );
        assert_eq!(order(&repo, &nested), ["web/AGENTS.md", "web/CYBER.md"]);
        let reminder = nested.reminder().unwrap();
        assert!(reminder.starts_with("\n\n<system-reminder>\n"));
        assert!(reminder.ends_with(&format!(
            "{}\nweb cyber\n</system-reminder>",
            web_cyber.display()
        )));
        let attached = nested
            .files
            .iter()
            .map(|file| file.canonical.display().to_string())
            .collect();
        let again = load_nested(&repo.global, &repo.root, &file, &settings(), &attached);
        assert!(again.files.is_empty());
        let outside = load_nested(
            &repo.global,
            &repo.root.join("web"),
            &repo.global.join("x.md"),
            &settings(),
            &BTreeSet::new(),
        );
        assert!(outside.files.is_empty());
    }

    fn nested(repo: &Repo, target: &Path) -> Instructions {
        load_nested(
            &repo.global,
            &repo.root,
            target,
            &settings(),
            &BTreeSet::new(),
        )
    }

    /// Regression: a project `CYBER.md` symlinked outside the checkout (to `~/.ssh/...`,
    /// say) was read into the privileged context; only imports were confined.
    #[cfg(unix)]
    #[test]
    fn project_files_symlinked_outside_the_project_are_not_loaded() {
        let repo = Repo::new();
        let secret = repo.global.join("id_rsa");
        std::fs::write(&secret, "PRIVATE KEY").unwrap();
        std::os::unix::fs::symlink("../global/id_rsa", repo.root.join("CYBER.md")).unwrap();
        repo.write("AGENTS.md", "agents");
        let loaded = repo.baseline("", &settings());
        assert_eq!(order(&repo, &loaded), ["AGENTS.md"]);
        assert!(!loaded.text().unwrap().contains("PRIVATE KEY"));
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
        assert!(loaded.warnings[0].contains("outside the project"));
        std::fs::create_dir_all(repo.root.join("web")).unwrap();
        std::os::unix::fs::symlink("../../global/id_rsa", repo.root.join("web/CYBER.md")).unwrap();
        let file = repo.write("web/app.ts", "");
        let nested = nested(&repo, &file);
        assert!(nested.is_empty(), "{:?}", nested.files);
        assert!(
            nested
                .warnings
                .iter()
                .any(|w| w.contains("outside the project"))
        );
    }

    /// A read through a symlinked directory that leaves the project attaches nothing.
    #[cfg(unix)]
    #[test]
    fn a_read_through_a_symlinked_directory_attaches_nothing_from_outside() {
        let repo = Repo::new();
        let elsewhere = repo.global.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("CYBER.md"), "outside rule").unwrap();
        std::fs::write(elsewhere.join("x.ts"), "").unwrap();
        std::fs::create_dir_all(repo.root.join("web")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, repo.root.join("web/link")).unwrap();
        let target = read_target(&repo.root, None, "web/link/x.ts");
        assert!(nested(&repo, &target).is_empty());
        // `..` folds lexically, as the read tool resolves it, before any symlink.
        let folded = read_target(&repo.root, None, "web/link/../app.ts");
        assert_eq!(folded, repo.root.join("web/app.ts"));
    }

    /// Regression: instruction files were read with an unbounded `read_to_string`, so a
    /// `CYBER.md` linked to `/dev/zero` (or a FIFO) exhausted memory or blocked forever.
    /// Such a file now fails the baseline closed without being read, and a nested one is
    /// a warning.
    #[cfg(unix)]
    #[test]
    fn special_files_are_never_read() {
        let repo = Repo::new();
        std::os::unix::fs::symlink("/dev/zero", repo.global.join("CYBER.md")).unwrap();
        let error = load_baseline(&repo.global, &repo.root, &settings()).unwrap_err();
        assert!(error.contains("CYBER.md: not a regular file"), "{error}");
        std::fs::remove_file(repo.global.join("CYBER.md")).unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(repo.root.join("CONTEXT.md"))
            .status()
            .unwrap();
        assert!(made.success());
        let error = load_baseline(&repo.global, &repo.root, &settings()).unwrap_err();
        assert!(error.contains("CONTEXT.md: not a regular file"), "{error}");
        std::fs::create_dir_all(repo.root.join("web/api")).unwrap();
        std::os::unix::fs::symlink("/dev/zero", repo.root.join("web/CYBER.md")).unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(repo.root.join("web/api/CYBER.md"))
            .status()
            .unwrap();
        assert!(made.success());
        let file = repo.write("web/api/app.ts", "");
        let loaded = nested(&repo, &file);
        assert!(loaded.files.is_empty(), "{:?}", loaded.files);
        let warnings = loaded.warnings.join("\n");
        assert!(warnings.contains("outside the project"), "{warnings}");
        assert!(
            warnings.contains("api/CYBER.md: not a regular file"),
            "{warnings}"
        );
    }

    /// Reads stop at the byte limit even when the file is larger than its metadata says
    /// (a file that grows), and the notice counts what was left out.
    #[test]
    fn reads_are_bounded_by_the_limit() {
        let repo = Repo::new();
        let path = repo.write("big.md", &"x".repeat(1000));
        let read = read_bounded(&path, 10).unwrap();
        assert_eq!(read.text, "x".repeat(10));
        assert_eq!(read.omitted, 990);
        let read = read_bounded(&path, 1000).unwrap();
        assert_eq!((read.text.len(), read.omitted), (1000, 0));
    }

    /// Regression: per-file truncation did not bound the whole, so a `CYBER.md` importing
    /// many files grew the baseline without limit.
    #[test]
    fn the_baseline_stops_at_its_total_budget() {
        let repo = Repo::new();
        let refs: Vec<String> = (0..10).map(|i| format!("@docs/{i}.md")).collect();
        repo.write("CYBER.md", &refs.join("\n"));
        for i in 0..10 {
            repo.write(&format!("docs/{i}.md"), &"y".repeat(100));
        }
        let mut settings = settings();
        settings.max_total_bytes = 350;
        let loaded = repo.baseline("", &settings);
        let text = loaded.text().unwrap();
        assert_eq!(loaded.files.len(), 4, "{:?}", loaded.files);
        assert!(loaded.files[3].truncated);
        assert!(text.contains("instruction files exceed their total budget"));
        assert_eq!(loaded.omitted.len(), 7);
        assert!(text.matches('y').count() <= 350);
        assert!(
            loaded
                .warnings
                .iter()
                .any(|w| w.contains("docs/9.md was not loaded"))
        );
    }

    /// A nested reminder carries at most [`NESTED_MAX_TOTAL_BYTES`] of text and names the
    /// files left out, which still count as attached.
    #[test]
    fn nested_reminders_name_the_files_over_their_budget() {
        let repo = Repo::new();
        repo.write("web/AGENTS.md", &"a".repeat(DEFAULT_MAX_BYTES));
        let cyber = repo.write("web/CYBER.md", &"c".repeat(DEFAULT_MAX_BYTES));
        let file = repo.write("web/app.ts", "");
        let nested = nested(&repo, &file);
        assert_eq!(order(&repo, &nested), ["web/AGENTS.md"]);
        assert_eq!(nested.omitted.len(), 1);
        let reminder = nested.reminder().unwrap();
        assert!(reminder.len() < DEFAULT_MAX_BYTES + 1024);
        assert!(reminder.contains(&format!("- {}", cyber.display())));
        assert_eq!(nested.attached().len(), 2);
        let pointer = nested.pointer().unwrap();
        assert!(
            pointer.len() < 1024 && pointer.contains("CYBER.md"),
            "{pointer}"
        );
    }
}
