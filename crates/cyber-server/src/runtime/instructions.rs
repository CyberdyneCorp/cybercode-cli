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

use super::context_sources::{Withheld, WithheldReason};

mod assembly;
pub(crate) use assembly::Reminder as InstructionReminder;

/// Files read from each directory, in load order. `CLAUDE.md` is read only where the
/// directory has no `AGENTS.md`. `CYBER.md`, Cyber Code's own instruction file, comes
/// last so Cyber-specific guidance wins ties within a directory.
pub const DIRECTORY_FILES: [&str; 4] = ["AGENTS.md", "CLAUDE.md", "CONTEXT.md", "CYBER.md"];
/// Files read from the global config directory, in load order.
pub const GLOBAL_FILES: [&str; 2] = ["AGENTS.md", "CYBER.md"];
/// Takes the `AGENTS.md` slot and replaces every file in [`OVERRIDDEN`] of its directory.
pub const OVERRIDE_FILE: &str = "AGENTS.override.md";
const OVERRIDDEN: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "CYBER.md"];
/// `instruction_limits.max_file_bytes`: each file is truncated here with a notice.
pub const DEFAULT_MAX_BYTES: usize = cyber_core::config::DEFAULT_MAX_FILE_BYTES;
/// All baseline instruction text together, imports included, stops here.
pub const DEFAULT_MAX_TOTAL_BYTES: usize = cyber_core::config::DEFAULT_MAX_TOTAL_BYTES;
/// The instruction text one nested read reminder carries; the rest is named, not attached.
pub const NESTED_MAX_TOTAL_BYTES: usize = DEFAULT_MAX_BYTES;
const MAX_IMPORT_DEPTH: usize = 5;
pub const INSTRUCTIONS_FROM: &str = "Instructions from: ";
/// The generation of a global rule binding: the owner's own files need no approval.
const OWNER_GLOBAL: &str = "owner-global";

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
    pub rule_gate: Option<RuleGate>,
    pub scoped_rules: bool,
    pub native_root: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuleAuthorization {
    pub root: PathBuf,
    pub digest: String,
    pub generation: String,
}
impl RuleAuthorization {
    pub fn from_report(report: &cyber_core::config::TrustReport, path: &Path) -> Option<Self> {
        if !report.trusted {
            return None;
        }
        Some(Self {
            root: report.checkout_root.clone(),
            digest: report.rule_digests.get(path)?.clone(),
            generation: report.digest.clone()?,
        })
    }
}

/// Definition identity retained separately from legacy canonical-path reminders.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuleBinding {
    pub root: String,
    pub path: String,
    pub digest: String,
    pub generation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_root: Option<String>,
    /// Physical operation target for this delivery. It is excluded from once-per-Epoch identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_path: Option<PathBuf>,
    /// Definition file size and modification time when its delivered bytes were read.
    /// Inspection compares it with current metadata; it is excluded from identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<super::context_sources::FileStamp>,
}
impl RuleBinding {
    /// Whether a checkout approval admitted this definition; owner-global ones need none.
    pub fn is_project(&self) -> bool {
        self.generation != OWNER_GLOBAL
    }

    pub fn identity(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(if self.native_root.is_some() {
            b"cyber-rule-reminder-v2\0"
        } else {
            b"cyber-rule-reminder-v1\0"
        });
        if let Some(root) = &self.native_root {
            hash.update((root.len() as u64).to_le_bytes());
            hash.update(root.as_bytes());
        }
        for field in [&self.root, &self.path, &self.digest, &self.generation] {
            hash.update((field.len() as u64).to_le_bytes());
            hash.update(field.as_bytes());
        }
        format!("rule:sha256:{hash:x}", hash = hash.finalize())
    }
}

type RuleCheck = dyn Fn(&Path, &Path) -> Result<Option<RuleAuthorization>, String> + Send + Sync;
#[derive(Clone)]
pub struct RuleGate(Arc<RuleCheck>);
impl RuleGate {
    pub fn new(
        check: impl Fn(&Path, &Path) -> Result<Option<RuleAuthorization>, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(check))
    }
    pub fn from_report(report: &cyber_core::config::TrustReport) -> Self {
        let report = report.clone();
        Self::new(move |path, _scope| Ok(RuleAuthorization::from_report(&report, path)))
    }
    fn authorize(&self, path: &Path, scope: &Path) -> Result<Option<RuleAuthorization>, String> {
        (self.0)(path, scope)
    }
    /// Whether the approval that admitted `binding` from `scope` is still the current one:
    /// the same checkout root, approval generation and definition digest. Global bindings
    /// need no approval and always pass.
    pub fn still_authorizes(&self, binding: &RuleBinding, scope: &Path) -> bool {
        if binding.generation == OWNER_GLOBAL {
            return true;
        }
        let Ok(Some(current)) = self.authorize(Path::new(&binding.path), scope) else {
            return false;
        };
        current.root.to_str() == Some(binding.root.as_str())
            && current.generation == binding.generation
            && current.digest == binding.digest
    }
}
impl std::fmt::Debug for RuleGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RuleGate")
    }
}
impl PartialEq for RuleGate {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for RuleGate {}

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
    pub fn set_limits(&mut self, limits: cyber_core::config::InstructionLimits) {
        self.max_bytes = limits.max_file_bytes;
        self.max_total_bytes = limits.max_total_bytes;
    }

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
            rule_gate: None,
            scoped_rules: true,
            native_root: None,
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
    pub(crate) rule_binding: Option<RuleBinding>,
    /// The directory a project rule's approval was checked from, rechecked at settlement.
    pub(crate) rule_scope: Option<PathBuf>,
    /// Size and modification time when the delivered bytes were read.
    pub stamp: Option<super::context_sources::FileStamp>,
}
impl InstructionFile {
    pub(crate) fn identity(&self) -> String {
        self.rule_binding.as_ref().map_or_else(
            || self.canonical.display().to_string(),
            RuleBinding::identity,
        )
    }
}

/// Bounded instruction bodies, the files behind them, and non-fatal problems.
#[derive(Debug, Default)]
pub struct Instructions {
    blocks: Vec<assembly::Block>,
    pub files: Vec<InstructionFile>,
    /// Files that apply but were left out because the total budget was spent.
    pub omitted: Vec<InstructionFile>,
    pub warnings: Vec<String>,
    /// Rule definitions that apply but were withheld, typed for inspection. Each also
    /// has a warning.
    pub withheld: Withheld,
    /// Valid path-scoped rule definitions this load discovered but did not attach
    /// because no operation target matched them; they attach on a matching file
    /// operation.
    pub scoped: usize,
    max_file_bytes: usize,
    max_total_bytes: usize,
}

impl Instructions {
    fn new(max_file_bytes: usize, max_total_bytes: usize) -> Self {
        Self {
            max_file_bytes,
            max_total_bytes,
            ..Default::default()
        }
    }

    pub fn text(&self) -> Option<String> {
        let (text, _) = assembly::blocks(
            self.blocks.iter(),
            self.max_file_bytes,
            self.max_total_bytes,
        );
        (!text.is_empty()).then_some(text)
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() && self.omitted.is_empty()
    }

    pub fn reminder(&self) -> Option<String> {
        self.reminder_delivery(&BTreeSet::new())
            .map(|delivery| delivery.text)
    }

    pub fn pointer(&self) -> Option<String> {
        self.pointer_delivery(&BTreeSet::new())
            .map(|delivery| delivery.text)
    }

    fn content_budget(&self) -> usize {
        self.max_total_bytes
            .saturating_sub(assembly::OPEN.len() + assembly::CLOSE.len())
    }

    pub(crate) fn reminder_delivery(&self, seen: &BTreeSet<String>) -> Option<InstructionReminder> {
        let eligible = |file: &&InstructionFile| !seen.contains(&file.identity());
        // Reserve complete labels first when some files could not enter the body.
        // A bounded prefix is truthful: unlisted files remain eligible next time.
        let pointers = (!self.omitted.is_empty())
            .then(|| {
                assembly::pointers(
                    "Instruction files for this path that may exceed the instruction budget",
                    self.files.iter().chain(&self.omitted).filter(eligible),
                    self.content_budget(),
                )
            })
            .flatten();
        let reserved = pointers
            .as_ref()
            .map_or(0, |pointer| pointer.text.len().saturating_add(2));
        let (mut text, mut files) = assembly::blocks(
            self.blocks
                .iter()
                .filter(|block| !seen.contains(&block.file.identity())),
            self.max_file_bytes,
            self.content_budget().saturating_sub(reserved),
        );
        if let Some(pointer) = pointers {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&pointer.text);
            files.extend(pointer.files);
        }
        assembly::finish(text, files)
    }

    pub(crate) fn pointer_delivery(&self, seen: &BTreeSet<String>) -> Option<InstructionReminder> {
        let pointer = assembly::pointers(
            "These instruction files apply here but exceed the tool output budget",
            self.files
                .iter()
                .chain(&self.omitted)
                .filter(|file| !seen.contains(&file.identity())),
            self.content_budget(),
        )?;
        assembly::finish(pointer.text, pointer.files)
    }

    /// Canonical identities actually labelled by the default bounded delivery.
    /// Whether every project rule this discovery may deliver is still approved with the
    /// generation and digest it was discovered under. `gate` is resolved only when a
    /// project rule is present; without one no project rule is authorized.
    pub(crate) fn rules_still_authorized(&self, gate: impl FnOnce() -> Option<RuleGate>) -> bool {
        let mut project = self
            .files
            .iter()
            .chain(&self.omitted)
            .filter_map(|file| Some((file.rule_binding.as_ref()?, file.rule_scope.as_deref()?)))
            .filter(|(binding, _)| binding.generation != OWNER_GLOBAL)
            .peekable();
        if project.peek().is_none() {
            return true;
        }
        let Some(gate) = gate() else {
            return false;
        };
        project.all(|(binding, scope)| gate.still_authorizes(binding, scope))
    }

    pub fn attached(&self) -> Vec<String> {
        self.reminder_delivery(&BTreeSet::new())
            .map_or_else(Vec::new, |delivery| {
                delivery.files.iter().map(|file| file.identity()).collect()
            })
    }
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
    load_nested_targets(
        global_dir,
        location,
        std::iter::once(target),
        settings,
        attached,
    )
}

pub(super) fn load_nested_targets<'a>(
    global_dir: &Path,
    location: &Path,
    targets: impl IntoIterator<Item = &'a Path>,
    settings: &InstructionSettings,
    attached: &BTreeSet<String>,
) -> Instructions {
    let location = canonical(location);
    let root = canonical(&cyber_core::config::project_root(&location));
    let targets: Vec<_> = targets
        .into_iter()
        .filter_map(|target| {
            let target = canonical(target);
            let directory = if target.is_dir() {
                target.clone()
            } else {
                target.parent()?.to_path_buf()
            };
            let dirs = project_dirs(&location, &directory);
            (!dirs.is_empty()).then_some((target, directory, dirs))
        })
        .collect();
    if targets.is_empty() {
        return Instructions::default();
    }
    let mut baseline = Loader::new(settings, settings.max_total_bytes);
    let _ = fill_baseline(&mut baseline, global_dir, &location);
    let budget = NESTED_MAX_TOTAL_BYTES.min(settings.max_total_bytes);
    let mut loader = Loader::new(settings, budget);
    loader.rule_root = root.clone();
    loader.global_rules = canonical(global_dir).join("rules");
    loader.delivered = baseline
        .out
        .files
        .iter()
        .map(InstructionFile::identity)
        .collect();
    loader.delivered.extend(attached.iter().cloned());
    loader.remaining = budget.saturating_sub(assembly::OPEN.len() + assembly::CLOSE.len());

    let global_rules = loader.global_rules.clone();
    for (target, directory, dirs) in targets {
        // Applicability depends on the target. Keep the byte, file and output
        // budgets shared across all targets in the settlement.
        loader.rule_seen.clear();
        loader.rule_scope = directory;
        loader.rule_target = Some(target.clone());
        loader.rules(&global_rules, &root, Some(&target), false);
        if settings.project {
            for dir in dirs {
                for file in directory_files(&dir, settings) {
                    if let Err(error) = loader.load(&file, Some(&root)) {
                        loader.out.warnings.push(error);
                    }
                }
                loader.project_rules(&dir, &root, Some(&target));
            }
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
    loader.rule_root = canonical(&cyber_core::config::project_root(location));
    loader.global_rules = canonical(global_dir).join("rules");
    loader.rule_scope = location.into();
    for file in GLOBAL_FILES.iter().map(|name| global_dir.join(name)) {
        if let Err(error) = loader.load(&file, None) {
            first.get_or_insert(error);
        }
    }
    let project_root = canonical(&cyber_core::config::project_root(location));
    let global_rules = loader.global_rules.clone();
    loader.rules(&global_rules, &project_root, None, false);
    if !loader.settings.project {
        return first;
    }
    let root = canonical(&cyber_core::config::project_root(location));
    let dirs = project_dirs(location, location);
    for dir in dirs {
        for file in directory_files(&dir, loader.settings) {
            if let Err(error) = loader.load(&file, Some(&root)) {
                first.get_or_insert(error);
            }
        }
        loader.project_rules(&dir, &root, None);
    }
    first
}

/// Why a rule definition was not delivered, with the warning that reports it.
struct Refusal {
    reason: WithheldReason,
    message: String,
}

impl Refusal {
    fn new(reason: WithheldReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }

    fn unreadable(message: String) -> Self {
        Self::new(WithheldReason::ReadDenied, message)
    }
}

impl From<cyber_core::rules::DefinitionError> for Refusal {
    fn from(error: cyber_core::rules::DefinitionError) -> Self {
        match error {
            cyber_core::rules::DefinitionError::Unreadable(message) => Self::unreadable(message),
            cyber_core::rules::DefinitionError::Invalid(message) => {
                Self::new(WithheldReason::InvalidFrontmatter, message)
            }
        }
    }
}

/// Reads each file once by canonical path, expanding `@path` imports depth first.
struct Loader<'a> {
    settings: &'a InstructionSettings,
    seen: BTreeSet<String>,
    delivered: BTreeSet<String>,
    discovery_remaining: usize,
    rule_files: usize,
    rule_seen: BTreeSet<PathBuf>,
    rule_bytes: usize,
    rule_root: PathBuf,
    rule_scope: PathBuf,
    rule_target: Option<PathBuf>,
    global_rules: PathBuf,
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
            delivered: BTreeSet::new(),
            discovery_remaining: settings.max_total_bytes,
            rule_files: 0,
            rule_seen: BTreeSet::new(),
            rule_bytes: 0,
            rule_root: PathBuf::new(),
            rule_scope: PathBuf::new(),
            rule_target: None,
            global_rules: PathBuf::new(),
            stack: Vec::new(),
            remaining: budget,
            out: Instructions::new(settings.max_bytes, budget),
        }
    }

    fn project_rules(&mut self, directory: &Path, root: &Path, target: Option<&Path>) {
        if self.settings.claude_md {
            self.rules(&directory.join(".claude/rules"), root, target, true);
        }
        self.rules(&directory.join(".cyber/rules"), root, target, true);
    }

    fn rules(&mut self, directory: &Path, root: &Path, target: Option<&Path>, project: bool) {
        let files = match cyber_core::rules::candidates(directory) {
            Ok(files) => files,
            Err(error) => {
                self.out
                    .warnings
                    .push(format!("{}: {error}", directory.display()));
                // The directory's definitions are withheld as a whole.
                let reason = if cyber_core::rules::is_bound_refusal(&error) {
                    WithheldReason::BoundExceeded
                } else {
                    WithheldReason::ReadDenied
                };
                self.withhold(directory, reason);
                return;
            }
        };
        for path in files {
            if self.rule_files >= 2 * cyber_core::rules::MAX_FILES
                || self.rule_bytes >= cyber_core::rules::MAX_TOTAL_BYTES
            {
                self.out.warnings.push(format!(
                    "{}: rule discovery bound exceeded",
                    directory.display()
                ));
                self.withhold(&path, WithheldReason::BoundExceeded);
                break;
            }
            if let Err(refusal) = self.rule(&path, root, target, project) {
                self.out
                    .warnings
                    .push(format!("{}: {}", path.display(), refusal.message));
                self.withhold(&path, refusal.reason);
            }
        }
    }

    fn withhold(&mut self, path: &Path, reason: WithheldReason) {
        self.out.withheld.record(path.display().to_string(), reason);
    }

    /// Approval and read policy of a project definition, decided before its bytes are
    /// opened.
    fn project_authorization(
        &self,
        path: &Path,
        root: &Path,
    ) -> Result<RuleAuthorization, Refusal> {
        if !self.settings.claude_md
            && path
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(".claude"))
        {
            return Err(Refusal::new(
                WithheldReason::CompatDisabled,
                "Claude rule compatibility is disabled",
            ));
        }
        if !path.starts_with(root) || !self.settings.read_allowed(path, &canonical(path)) {
            return Err(Refusal::new(
                WithheldReason::ReadDenied,
                "rule definition is not allowed by read policy",
            ));
        }
        let held = |message: String| Refusal::new(WithheldReason::TrustHeld, message);
        let authorization = self
            .settings
            .rule_gate
            .as_ref()
            .map(|gate| gate.authorize(path, &self.rule_scope))
            .transpose()
            .map_err(held)?
            .flatten()
            .ok_or_else(|| {
                held(
                    "project rule is held until its definition is trusted (cyber trust inspect)"
                        .into(),
                )
            })?;
        if authorization.root != root {
            return Err(held("rule approval belongs to another project root".into()));
        }
        Ok(authorization)
    }

    fn rule(
        &mut self,
        path: &Path,
        root: &Path,
        target: Option<&Path>,
        project: bool,
    ) -> Result<(), Refusal> {
        if !self.rule_seen.insert(path.into()) {
            return Ok(());
        }
        self.rule_files += 1;
        if self.rule_files > 2 * cyber_core::rules::MAX_FILES {
            return Err(Refusal::new(
                WithheldReason::BoundExceeded,
                "rule file bound exceeded",
            ));
        }
        // Approval and read policy precede opening project definition bytes.
        let authorization = if project {
            Some(self.project_authorization(path, root)?)
        } else {
            None
        };
        let definition = cyber_core::rules::Definition::read(path)?;
        self.rule_bytes = self.rule_bytes.saturating_add(definition.byte_len());
        if self.rule_bytes > cyber_core::rules::MAX_TOTAL_BYTES {
            return Err(Refusal::new(
                WithheldReason::BoundExceeded,
                "rule definition byte bound exceeded",
            ));
        }
        if authorization
            .as_ref()
            .is_some_and(|auth| auth.digest != definition.digest)
        {
            return Err(Refusal::new(
                WithheldReason::TrustHeld,
                "rule definition differs from its approved digest",
            ));
        }
        if (definition.paths.is_some() && !self.settings.scoped_rules)
            || !definition.applies(root, target)
        {
            self.out.scoped += usize::from(definition.paths.is_some());
            return Ok(());
        }
        let generation = authorization
            .as_ref()
            .map_or(OWNER_GLOBAL, |auth| auth.generation.as_str());
        let invalid = |message: &str| Refusal::new(WithheldReason::InvalidFrontmatter, message);
        let binding = RuleBinding {
            root: root
                .to_str()
                .ok_or_else(|| invalid("rule root must be UTF-8"))?
                .into(),
            path: path
                .to_str()
                .ok_or_else(|| invalid("rule path must be UTF-8"))?
                .into(),
            digest: definition.digest.clone(),
            generation: generation.into(),
            native_root: self.settings.native_root.clone(),
            operation_path: target
                .filter(|_| self.settings.native_root.is_some())
                .map(Path::to_path_buf),
            stamp: None,
        };
        let identity = binding.identity();
        if !self.seen.insert(identity.clone()) {
            return Ok(());
        }
        if self.delivered.contains(&identity) {
            let limit = self.settings.max_bytes.min(self.discovery_remaining);
            let body = bounded_text(&definition.body, limit);
            self.discovery_remaining -= body.text.len();
            definition.verify().map_err(Refusal::unreadable)?;
            self.expand_rule_imports(path, root, project, &body);
            return Ok(());
        }
        let file = InstructionFile {
            path: path.into(),
            canonical: path.into(),
            truncated: false,
            rule_binding: Some(binding),
            rule_scope: authorization.is_some().then(|| self.rule_scope.clone()),
            stamp: None,
        };
        let label = format!("{INSTRUCTIONS_FROM}{}\n", path.display());
        let available = self
            .remaining
            .saturating_sub(if self.out.blocks.is_empty() { 0 } else { 2 });
        if available < label.len() {
            let _ = self.omit(file);
            return Ok(());
        }
        let limit = self.settings.max_bytes.min(available - label.len());
        let mut body = bounded_text(&definition.body, limit);
        definition.verify().map_err(Refusal::unreadable)?;
        // Taken after the definition's file identity was verified. An edit between the
        // definition's read and this call makes the stamp newer than the delivered
        // bytes; that edit still reaches the next request boundary, which re-reads rules.
        body.stamp = std::fs::metadata(path)
            .ok()
            .map(|metadata| super::context_sources::FileStamp::of(&metadata));
        if let Some(body) = self.record_body(file, body) {
            self.expand_rule_imports(path, root, project, &body);
        }
        Ok(())
    }

    fn expand_rule_imports(&mut self, path: &Path, root: &Path, project: bool, body: &Bounded) {
        self.stack.push(path.into());
        for reference in imports(body.complete_lines()) {
            self.import(path, &reference, project.then_some(root));
        }
        self.stack.pop();
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
        if !self.seen.insert(key.display().to_string()) {
            return Ok(());
        }
        if self.delivered.contains(&key.display().to_string()) {
            return self.discover_imports(path, key, confine);
        }
        let file = InstructionFile {
            path: path.to_path_buf(),
            canonical: key.clone(),
            rule_binding: None,
            rule_scope: None,
            truncated: false,
            stamp: None,
        };
        let separator = if self.out.blocks.is_empty() { 0 } else { 2 };
        let label = format!("{INSTRUCTIONS_FROM}{}\n", path.display());
        let available = self.remaining.saturating_sub(separator);
        if available < label.len() {
            return self.omit(file);
        }
        let limit = self
            .settings
            .max_bytes
            .min(available - label.len())
            .min(self.discovery_remaining);
        let read = read_bounded(path, limit)?;
        self.discovery_remaining -= read.text.len();
        let Some(body) = self.record_body(file, read) else {
            return Ok(());
        };
        let references = imports(body.complete_lines());
        self.stack.push(key);
        for reference in references {
            self.import(path, &reference, confine);
        }
        self.stack.pop();
        Ok(())
    }

    fn record_body(&mut self, file: InstructionFile, read: Bounded) -> Option<Bounded> {
        let path = file.path.clone();
        let path = path.as_path();
        let separator = if self.out.blocks.is_empty() { 0 } else { 2 };
        let available = self.remaining.saturating_sub(separator);
        let Some(rendered) = assembly::block(path, &read, self.settings.max_bytes, available)
        else {
            let _ = self.omit(file);
            return None;
        };
        self.remaining -= rendered.text.len() + separator;
        let file = InstructionFile {
            truncated: rendered.body.omitted > 0,
            stamp: rendered.body.stamp,
            ..file
        };
        if file.truncated {
            self.out
                .warnings
                .push(if rendered.body.limit == self.settings.max_bytes {
                    format!(
                        "{} exceeds instruction_limits.max_file_bytes ({} bytes) and was truncated",
                        path.display(),
                        self.settings.max_bytes
                    )
                } else {
                    format!(
                        "{} was truncated: instruction files exceed their total budget",
                        path.display()
                    )
                });
        }
        let body = rendered.body.clone();
        self.out.files.push(file.clone());
        self.out.blocks.push(assembly::Block {
            file,
            body: rendered.body,
        });
        Some(body)
    }

    /// Revisit authorized import declarations without redelivering an already labelled
    /// parent. Otherwise a bounded pointer prefix would strand its unlisted children.
    fn discover_imports(
        &mut self,
        path: &Path,
        key: PathBuf,
        confine: Option<&Path>,
    ) -> Result<(), String> {
        if self.discovery_remaining == 0 {
            self.out.warnings.push(format!(
                "{}: import discovery budget exhausted",
                path.display()
            ));
            return Ok(());
        }
        let read = read_bounded(path, self.settings.max_bytes.min(self.discovery_remaining))?;
        self.discovery_remaining -= read.text.len();
        self.stack.push(key);
        for reference in imports(read.complete_lines()) {
            self.import(path, &reference, confine);
        }
        self.stack.pop();
        Ok(())
    }

    fn omit(&mut self, file: InstructionFile) -> Result<(), String> {
        self.out.warnings.push(format!(
            "{} was not loaded: instruction files exceed their total budget",
            file.path.display()
        ));
        self.out.omitted.push(file);
        Ok(())
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
        if !self.is_rule(&key) {
            if let Err(e) = self.include(&target, key, confine) {
                self.warn(from, reference, &format!("could not be read: {e}"));
            }
            return;
        }
        let root = self.rule_root.clone();
        let target_file = self.rule_target.clone();
        let global = key.parent() == Some(self.global_rules.as_path());
        if let Err(refusal) = self.rule(&key, &root, target_file.as_deref(), !global) {
            let problem = format!("could not be read: {}", refusal.message);
            self.warn(from, reference, &problem);
            self.withhold(&key, refusal.reason);
        }
    }

    fn is_rule(&self, path: &Path) -> bool {
        let Some(parent) = path.parent() else {
            return false;
        };
        parent == self.global_rules
            || (parent
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("rules"))
                && parent
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| {
                        name.to_string_lossy().eq_ignore_ascii_case(".cyber")
                            || name.to_string_lossy().eq_ignore_ascii_case(".claude")
                    }))
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
#[derive(Debug, Clone)]
struct Bounded {
    text: String,
    omitted: usize,
    limit: usize,
    stamp: Option<super::context_sources::FileStamp>,
}

fn bounded_text(text: &str, limit: usize) -> Bounded {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Bounded {
        text: text[..end].into(),
        omitted: text.len() - end,
        limit,
        stamp: None,
    }
}

impl Bounded {
    /// A cut final line is not an import declaration: its truncated path could name
    /// a different existing file. Full final lines without a newline remain valid.
    fn complete_lines(&self) -> &str {
        if self.omitted > 0 && !self.text.ends_with('\n') {
            self.text
                .rsplit_once('\n')
                .map_or("", |(complete, _)| complete)
        } else {
            &self.text
        }
    }
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
    file.take((limit as u64).saturating_add(1))
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
    Ok(Bounded {
        text,
        omitted,
        limit,
        stamp: Some(super::context_sources::FileStamp::of(&metadata)),
    })
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
        assert!(
            text.contains(
                "exceeds instruction_limits.max_file_bytes (11 bytes); 70 bytes omitted]"
            )
        );
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
        let attached = nested.files.iter().map(|file| file.identity()).collect();
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
        let read = read_bounded(&path, usize::MAX).unwrap();
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
        assert!(text.len() <= 350, "{} bytes", text.len());
        assert!(!loaded.omitted.is_empty());
        assert!(text.contains("CYBER.md"));
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
        assert!(reminder.len() <= NESTED_MAX_TOTAL_BYTES);
        assert!(reminder.contains(&format!("- {}", cyber.display())));
        assert_eq!(nested.attached().len(), 2);
        let pointer = nested.pointer().unwrap();
        assert!(
            pointer.len() < 1024 && pointer.contains("CYBER.md"),
            "{pointer}"
        );
    }

    #[test]
    fn a_cut_import_line_never_loads_the_file_named_by_its_prefix() {
        let repo = Repo::new();
        repo.write("CYBER.md", "@secrets.md");
        repo.write("secret", "unintended file");
        let mut settings = settings();
        settings.max_bytes = 7;
        let loaded = repo.baseline("", &settings);
        assert_eq!(order(&repo, &loaded), ["CYBER.md"]);
    }

    #[test]
    fn unlisted_imports_remain_discoverable_after_their_parent_was_labelled() {
        let repo = Repo::new();
        let refs: Vec<_> = (0..20).map(|i| format!("@r/{i:02}.md")).collect();
        repo.write("web/CYBER.md", &refs.join("\n"));
        for i in 0..20 {
            repo.write(&format!("web/r/{i:02}.md"), "x");
        }
        let target = repo.write("web/app.ts", "");
        let mut settings = settings();
        settings.max_bytes = 512;
        settings.max_total_bytes = 512;
        let mut attached = BTreeSet::new();
        for _ in 0..22 {
            let nested = load_nested(&repo.global, &repo.root, &target, &settings, &attached);
            let Some(delivery) = nested.reminder_delivery(&attached) else {
                break;
            };
            assert!(delivery.text.len() <= 512);
            let before = attached.len();
            for file in delivery.files {
                assert!(delivery.text.contains(&file.path.display().to_string()));
                attached.insert(file.identity());
            }
            assert!(attached.len() > before);
        }
        assert_eq!(attached.len(), 21, "{attached:?}");
    }

    #[test]
    fn global_scoped_rules_survive_project_opt_out_without_loading_project_rules() {
        let repo = Repo::new();
        std::fs::create_dir(repo.global.join("rules")).unwrap();
        std::fs::write(
            repo.global.join("rules/web.md"),
            "---\npaths: ['web/**']\n---\nGLOBAL_SCOPED_RULE",
        )
        .unwrap();
        repo.write(".cyber/rules/web.md", "PROJECT_RULE");
        let target = repo.write("web/app.ts", "");
        let mut settings = settings();
        settings.project = false;
        assert!(repo.baseline("", &settings).text().is_none());
        let nested = load_nested(
            &repo.global,
            &repo.root,
            &target,
            &settings,
            &BTreeSet::new(),
        );
        let text = nested.reminder().unwrap();
        assert!(text.contains("GLOBAL_SCOPED_RULE"));
        assert!(!text.contains("PROJECT_RULE"));
    }

    /// Withheld definitions are typed for inspection, once per path, and keep their
    /// warnings; their bytes are never part of the instruction text.
    #[test]
    fn withheld_rule_definitions_are_recorded_with_their_reason() {
        let repo = Repo::new();
        std::fs::create_dir(repo.global.join("rules")).unwrap();
        std::fs::write(
            repo.global.join("rules/broken.md"),
            "---\npaths: 7\n---\nBROKEN",
        )
        .unwrap();
        repo.write(".cyber/rules/held.md", "HELD_RULE");
        repo.write(".claude/rules/compat.md", "COMPAT_RULE");
        repo.write("CYBER.md", "@.claude/rules/compat.md");
        let mut settings = settings();
        settings.claude_md = false;
        let loaded = repo.baseline("", &settings);
        assert!(loaded.text().is_none_or(|text| !text.contains("_RULE")));
        let reasons: Vec<_> = loaded
            .withheld
            .entries
            .iter()
            .map(|entry| {
                let name = Path::new(&entry.path).file_name().unwrap();
                (name.to_string_lossy().into_owned(), entry.reason)
            })
            .collect();
        assert_eq!(
            reasons,
            [
                ("broken.md".to_owned(), WithheldReason::InvalidFrontmatter),
                ("compat.md".to_owned(), WithheldReason::CompatDisabled),
                ("held.md".to_owned(), WithheldReason::TrustHeld),
            ],
            "{:?}",
            loaded.warnings
        );
        assert_eq!(loaded.withheld.omitted, 0);
        assert!(loaded.warnings.iter().any(|w| w.contains("held until")));
    }

    /// A rule directory refused as a whole (here reached through a symlink) is reported
    /// as one withheld entry instead of only a warning.
    #[cfg(unix)]
    #[test]
    fn a_refused_rule_directory_is_withheld_as_a_whole() {
        let repo = Repo::new();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::write(elsewhere.path().join("linked.md"), "LINKED_RULE").unwrap();
        std::fs::create_dir(repo.root.join(".cyber")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), repo.root.join(".cyber/rules")).unwrap();
        let loaded = repo.baseline("", &settings());
        assert!(
            loaded
                .text()
                .is_none_or(|text| !text.contains("LINKED_RULE"))
        );
        assert_eq!(loaded.withheld.entries.len(), 1, "{:?}", loaded.warnings);
        let entry = &loaded.withheld.entries[0];
        assert!(entry.path.ends_with(".cyber/rules"), "{entry:?}");
        assert_eq!(entry.reason, WithheldReason::ReadDenied);
    }

    #[test]
    fn withheld_listing_is_bounded_and_counts_the_rest() {
        let mut withheld = Withheld::default();
        for index in 0..70 {
            withheld.record(format!("/r/{index}.md"), WithheldReason::TrustHeld);
        }
        withheld.record("/r/0.md".into(), WithheldReason::ReadDenied);
        assert_eq!(withheld.entries.len(), 64);
        assert_eq!(withheld.omitted, 6);
    }

    /// A binding records the approval generation it was discovered under. Revoking or
    /// re-reviewing the checkout changes that generation, so settlement must not deliver
    /// or consume the binding; owner-global bindings need no approval.
    #[test]
    fn a_binding_from_a_revoked_generation_is_not_authorized() {
        let repo = Repo::new();
        let path = repo.write(".cyber/rules/a.md", "RULE");
        let digest = cyber_core::rules::trust_subset(&repo.root).digests[&path].clone();
        let generation = std::sync::Arc::new(std::sync::Mutex::new(Some("first".to_owned())));
        let (root, current) = (repo.root.clone(), generation.clone());
        let gate = RuleGate::new(move |_, _| {
            Ok(current
                .lock()
                .unwrap()
                .clone()
                .map(|generation| RuleAuthorization {
                    root: root.clone(),
                    digest: digest.clone(),
                    generation,
                }))
        });
        let binding = RuleBinding {
            root: repo.root.to_str().unwrap().into(),
            path: path.to_str().unwrap().into(),
            digest: cyber_core::rules::trust_subset(&repo.root).digests[&path].clone(),
            generation: "first".into(),
            native_root: None,
            operation_path: None,
            stamp: None,
        };
        assert!(gate.still_authorizes(&binding, &repo.root));
        let changed_digest = RuleBinding {
            digest: "sha256:other".into(),
            ..binding.clone()
        };
        assert!(!gate.still_authorizes(&changed_digest, &repo.root));
        *generation.lock().unwrap() = Some("second".into());
        assert!(!gate.still_authorizes(&binding, &repo.root));
        *generation.lock().unwrap() = None;
        assert!(!gate.still_authorizes(&binding, &repo.root));
        let global = RuleBinding {
            generation: OWNER_GLOBAL.into(),
            ..binding
        };
        assert!(gate.still_authorizes(&global, &repo.root));
    }

    #[test]
    fn approval_snapshot_cannot_authorize_changed_rule_bytes() {
        let repo = Repo::new();
        let path = repo.write(".cyber/rules/a.md", "APPROVED_RULE");
        let digests = cyber_core::rules::trust_subset(&repo.root).digests;
        let root = repo.root.clone();
        let mut settings = settings();
        settings.rule_gate = Some(RuleGate::new(move |path, _| {
            Ok(digests.get(path).map(|digest| RuleAuthorization {
                root: root.clone(),
                digest: digest.clone(),
                generation: "reviewed".into(),
            }))
        }));
        assert!(
            repo.baseline("", &settings)
                .text()
                .unwrap()
                .contains("APPROVED_RULE")
        );
        std::fs::write(path, "CHANGED_RULE").unwrap();
        let changed = repo.baseline("", &settings);
        assert!(changed.text().is_none());
        assert!(
            changed
                .warnings
                .iter()
                .any(|warning| warning.contains("differs from its approved digest"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_explicit_global_root_alias_cannot_turn_a_scoped_import_into_baseline_text() {
        let repo = Repo::new();
        std::fs::create_dir(repo.global.join("rules")).unwrap();
        std::fs::write(
            repo.global.join("rules/web.md"),
            "---\npaths: ['web/**']\n---\nSCOPED_GLOBAL_GUIDANCE",
        )
        .unwrap();
        std::fs::write(repo.global.join("CYBER.md"), "@rules/web.md").unwrap();
        let alias = repo.global.parent().unwrap().join("global-alias");
        std::os::unix::fs::symlink(&repo.global, &alias).unwrap();
        let baseline = load_baseline(&alias, &repo.root, &settings()).unwrap();
        assert!(!baseline.text().unwrap().contains("SCOPED_GLOBAL_GUIDANCE"));
        let target = repo.write("web/app.ts", "");
        let nested = load_nested(&alias, &repo.root, &target, &settings(), &BTreeSet::new());
        assert!(
            nested
                .reminder()
                .unwrap()
                .contains("SCOPED_GLOBAL_GUIDANCE")
        );
    }

    #[test]
    fn imported_definitions_are_not_charged_again_when_the_directory_lists_them() {
        let repo = Repo::new();
        repo.write(".cyber/rules/a.md", "@b.md\n@c.md\n@d.md");
        for name in ["b", "c", "d"] {
            repo.write(&format!(".cyber/rules/{name}.md"), &"x".repeat(1024 * 1024));
        }
        let digests = cyber_core::rules::trust_subset(&repo.root).digests;
        let root = repo.root.clone();
        let mut settings = settings();
        settings.rule_gate = Some(RuleGate::new(move |path, _| {
            Ok(digests.get(path).map(|digest| RuleAuthorization {
                root: root.clone(),
                digest: digest.clone(),
                generation: "reviewed".into(),
            }))
        }));
        let loaded = repo.baseline("", &settings);
        assert_eq!(loaded.files.len(), 4);
        assert!(loaded.text().unwrap().len() <= DEFAULT_MAX_TOTAL_BYTES);
        assert!(
            !loaded
                .warnings
                .iter()
                .any(|warning| warning.contains("rule discovery bound")
                    || warning.contains("rule definition byte bound")),
            "{:?}",
            loaded.warnings
        );
    }
}
