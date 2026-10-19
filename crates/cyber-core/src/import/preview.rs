//! Reviewed proposals; nothing is written until [`ImportPreview::write`], and no source is
//! executed.
use super::adapted::Adapted;
use super::provenance::{self, Origins, SourceReference};
use super::{
    ConversionError, DiscoveryError, RequiredEnvironment, SourceFile, SourceKind, SourceLayer,
    SourceRoots, SourceSnapshot, SourceTool, claude_permissions, codex_provider_config,
    codex_rules, discover_sources, opencode_permissions,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportScope {
    Project,
    Global,
}
#[derive(Debug, Clone, Serialize)]
pub struct MappingRecord {
    pub source: PathBuf,
    pub sources: Vec<SourceReference>,
    pub field: String,
    pub status: &'static str,
    pub reason: &'static str,
    /// Normalized values; credential fields and known credential echoes are redacted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<super::report_values::ValueComparison>,
}
#[derive(Debug, Serialize)]
pub struct PreviewEnvironment {
    #[serde(flatten)]
    pub requirement: RequiredEnvironment,
    pub sources: Vec<SourceReference>,
}
#[derive(Debug, Serialize)]
pub struct PreviewOutput {
    pub target: PathBuf,
    pub diff: String,
    /// Raw file layers influencing the proposal; no profile or substitution evaluation.
    pub native_layers: Vec<PathBuf>,
    pub report: Vec<MappingRecord>,
    pub required_environment: Vec<PreviewEnvironment>,
    /// Source files and discovery issues of the selected tools and scope.
    pub sources: usize,
    /// This increment does not implement every source adapter or destination transactions.
    pub complete: bool,
}
/// The files an import wrote.
#[derive(Debug, Clone, Serialize)]
pub struct WrittenImport {
    pub target: PathBuf,
    pub report: PathBuf,
}
pub struct ImportPreview {
    output: PreviewOutput,
    sources: Vec<SourceSnapshot>,
    destinations: Vec<SourceSnapshot>,
    missing: Vec<PathBuf>,
    edited: Option<String>,
    report_dir: PathBuf,
}
impl ImportPreview {
    pub fn output(&self) -> &PreviewOutput {
        &self.output
    }
    /// The target document as [`Self::write`] would write it; `None` is "nothing to do".
    pub fn edited_text(&self) -> Option<&str> {
        self.edited.as_deref()
    }
    /// Whether writing would change the target document; `false` is "nothing to do".
    pub fn has_changes(&self) -> bool {
        self.edited.is_some()
    }
    /// Where [`Self::write`] puts the mapping report: `.cyber/` of the Location for a
    /// project import, the global config directory for a global one.
    pub fn report_dir(&self) -> &Path {
        &self.report_dir
    }
    /// Re-verify every reviewed source and destination, then write the mapping report
    /// `import-report-<timestamp>.md` headed by `title` and the target document. The
    /// report goes first, so a refused report directory leaves the target untouched; a
    /// target that cannot be replaced takes its report with it.
    pub fn write(&self, title: &str) -> Result<WrittenImport, DiscoveryError> {
        let Some(edited) = &self.edited else {
            return Err(error(&self.output.target, "nothing to do"));
        };
        self.verify()?;
        let report = super::write::write_report(&self.report_dir, &self.report_markdown(title))?;
        if let Err(failure) = super::write::replace_file(&self.output.target, edited) {
            let _ = std::fs::remove_file(&report);
            return Err(failure);
        }
        Ok(WrittenImport {
            target: self.output.target.clone(),
            report,
        })
    }
    /// The mapping report as Markdown: every item under its status, with its sources.
    pub fn report_markdown(&self, title: &str) -> String {
        super::report_values::markdown(title, &self.output)
    }
    pub fn verify(&self) -> Result<(), DiscoveryError> {
        for snapshot in self.sources.iter().chain(&self.destinations) {
            snapshot.verify()?;
        }
        for path in &self.missing {
            if SourceSnapshot::read_native_config(path)?.is_some() {
                return Err(error(path, "native configuration appeared since review"));
            }
        }
        Ok(())
    }
}
fn error(path: &Path, reason: &'static str) -> DiscoveryError {
    DiscoveryError {
        path: path.into(),
        reason,
    }
}
fn converted<T>(
    source: &SourceFile,
    result: Result<T, ConversionError>,
) -> Result<T, DiscoveryError> {
    result.map_err(|_| {
        error(
            &source.path,
            "source conversion refused; no partial permission/configuration proposal returned",
        )
    })
}
fn record(
    source: &Path,
    field: impl Into<String>,
    status: &'static str,
    reason: &'static str,
) -> MappingRecord {
    let field = field.into();
    MappingRecord {
        source: source.into(),
        sources: vec![SourceReference {
            source: source.into(),
            field: field.clone(),
        }],
        field,
        status,
        reason,
        comparison: None,
    }
}
/// `tool=None` means canonical auto order. Project adoption reads global defaults and project layers.
pub fn preview_import(
    roots: &SourceRoots,
    tool: Option<SourceTool>,
    scope: ImportScope,
    native_global: &Path,
) -> Result<ImportPreview, DiscoveryError> {
    let inventory = discover_sources(roots)?;
    let directory = match scope {
        ImportScope::Project => roots
            .directory
            .canonicalize()
            .map_err(|_| error(&roots.directory, "Location is unavailable"))?,
        ImportScope::Global => std::path::absolute(native_global)
            .map_err(|_| error(native_global, "global target is unavailable"))?,
    };
    let native = native_config(roots, scope, native_global, &directory)?;
    let existing = native.value;
    let target = native.target;
    let imported = &native.imported;
    let mut updated = std::collections::BTreeSet::new();
    let sources = selected_sources(&inventory, tool, scope);
    let mut proposed = existing.clone();
    let mut report = Vec::new();
    for issue in inventory.issues {
        report.push(record(&issue.path, "source", "not imported", issue.reason));
    }
    let personal = personal_sources(roots, scope);
    let mut context = SourceContext {
        roots,
        inventory: &inventory.files,
        snapshots: Vec::new(),
        required: Vec::new(),
        secrets: sensitive_values(&existing),
        remaining: native.remaining,
        report,
        accepted: Origins::new(),
    };
    for family in [SourceTool::OpenCode, SourceTool::Codex, SourceTool::Claude] {
        if tool.is_some_and(|t| t != family) {
            continue;
        }
        let mut raw = json!({});
        let mut rules = json!({});
        let mut raw_origins = Origins::new();
        let mut rule_origins = Origins::new();
        let mut representative = None;
        let markdown = markdown_sources(&mut context, &inventory.files, family, scope)?;
        let mut adapted = Adapted::new();
        for source in inventory.files.iter().filter(|f| {
            f.tool == family
                && f.kind != SourceKind::Command
                && (scope == ImportScope::Project || f.layer != SourceLayer::Project)
        }) {
            let Some(delta) = source_config(&mut context, source, &mut adapted)? else {
                continue;
            };
            if source.kind == SourceKind::Rule {
                provenance::overlay(
                    &mut rules,
                    &delta,
                    "",
                    &source.path,
                    provenance::AppendStyle::Claude,
                    &mut rule_origins,
                )?;
            } else {
                provenance::overlay(
                    &mut raw,
                    &delta,
                    "",
                    &source.path,
                    match family {
                        SourceTool::Claude => provenance::AppendStyle::Claude,
                        SourceTool::OpenCode => provenance::AppendStyle::OpenCode,
                        SourceTool::Codex => provenance::AppendStyle::None,
                    },
                    &mut raw_origins,
                )?;
                representative = Some(source);
            }
        }
        let mut required = Vec::new();
        let report_start = context.report.len();
        let mut config = if let Some(source) = representative {
            document_config(source, &raw, &mut required, &mut context.report)?
        } else {
            json!({})
        };
        for item in &mut context.report[report_start..] {
            if let Some(pointer) = provenance::indexed_field(&raw, &item.field) {
                let sources = provenance::references(&raw_origins, &pointer);
                if let Some(first) = sources.first() {
                    item.source = first.source.clone();
                    item.sources = sources;
                }
            }
        }
        for refs in rule_origins.values_mut() {
            for reference in refs {
                reference.field = format!("converted:{}", reference.field);
            }
        }
        let mut origins = if let Some(source) = representative {
            provenance::converted(family, &raw, &config, &raw_origins, &source.path)?
        } else {
            Origins::new()
        };
        provenance::append_rules(&mut config, &rules, &mut origins, &rule_origins);
        provenance::strongest_permissions(family, &mut config, &mut origins);
        merge_markdown_commands(
            &mut config,
            &mut origins,
            markdown,
            &inventory.files,
            &raw_origins,
        )?;
        let settings = Raw {
            value: &raw,
            origins: &raw_origins,
            directory: &directory,
            personal: &personal,
        };
        adapt(
            representative,
            &settings,
            &target,
            (&mut config, &mut origins),
            &mut context.report,
            adapted,
        )?;
        let mut accepted = Origins::new();
        fill_existing(
            &mut Fill {
                origins: &origins,
                accepted: &mut accepted,
                report: &mut context.report,
                imported,
                updated: &mut updated,
            },
            &mut proposed,
            &config,
            "",
        );
        context.accepted.append(&mut accepted.clone());
        let environment_origins = provenance::environment_origins(&config, &accepted)
            .map_err(|e| error(&target, e.reason))?;
        context.required.extend(
            super::environment_setup::accepted(required, environment_origins, &raw)
                .map_err(|e| error(&target, e.reason))?,
        );
    }
    let mut bindings: Vec<_> = context
        .required
        .iter()
        .map(|entry| entry.requirement.clone())
        .collect();
    bindings.extend(super::environment_setup::reserved(&existing));
    super::codex_providers::validate_environment_bindings(&bindings).map_err(|_| {
        error(
            &target,
            "generated credential variable conflicts across source adapters",
        )
    })?;
    context.required = super::environment_setup::coalesce(context.required);
    redact_report_fields(&mut context.report, &mut context.required, &context.secrets);
    reject_unsafe_strings(&proposed, &existing, &context.secrets, &target)?;
    if proposed
        .get("commands")
        .and_then(Value::as_object)
        .is_some_and(|m| m.len() > 128)
    {
        return Err(error(
            &target,
            "combined command definitions exceed the native registry bound",
        ));
    }
    let (edited, diff) = edited_document(
        &target,
        native.target_text.as_deref(),
        &super::write::changes(&target, &existing, &proposed, &updated)?,
        &super::write::Annotator {
            roots,
            origins: &context.accepted,
            secrets: &context.secrets,
        },
    )?;
    let preview = ImportPreview {
        output: PreviewOutput {
            target,
            diff,
            native_layers: native
                .snapshots
                .iter()
                .map(|s| s.path().to_owned())
                .collect(),
            report: context.report,
            required_environment: context.required,
            sources,
            complete: false,
        },
        sources: context.snapshots,
        destinations: native.snapshots,
        missing: native.missing,
        edited,
        report_dir: report_dir(scope, &directory),
    };
    preview.verify()?;
    Ok(preview)
}
/// The target document after `changes`, and its redacted unified diff; `None` and an
/// empty diff when nothing changes.
fn edited_document(
    target: &Path,
    original: Option<&str>,
    changes: &[super::write::Change],
    annotator: &super::write::Annotator<'_>,
) -> Result<(Option<String>, String), DiscoveryError> {
    let edited = super::write::edit_target(target, original, changes, annotator)?;
    let diff = edited.as_deref().map_or_else(String::new, |edited| {
        super::write::file_diff(target, original, edited, annotator.secrets)
    });
    Ok((edited, diff))
}
/// `.cyber/` of the Location for a project import, the global directory for a global one.
fn report_dir(scope: ImportScope, directory: &Path) -> PathBuf {
    match scope {
        ImportScope::Project => directory.join(".cyber"),
        ImportScope::Global => directory.to_path_buf(),
    }
}
/// Source files and discovery issues an import of `tool` at `scope` considers.
fn selected_sources(
    inventory: &super::SourceInventory,
    tool: Option<SourceTool>,
    scope: ImportScope,
) -> usize {
    let files = inventory.files.iter().filter(|f| {
        tool.is_none_or(|t| t == f.tool)
            && (scope == ImportScope::Project || f.layer != SourceLayer::Project)
    });
    inventory.issues.len() + files.count()
}
struct NativeConfig {
    /// `cyber.jsonc` of the Location (or of the global directory): imports write there.
    target: PathBuf,
    target_text: Option<String>,
    /// Pointer → digest of each entry an earlier import annotated in the target.
    imported: std::collections::BTreeMap<String, String>,
    value: Value,
    snapshots: Vec<SourceSnapshot>,
    missing: Vec<PathBuf>,
    remaining: usize,
}
fn native_config(
    roots: &SourceRoots,
    scope: ImportScope,
    native_global: &Path,
    directory: &Path,
) -> Result<NativeConfig, DiscoveryError> {
    let global = std::path::absolute(native_global)
        .map_err(|_| error(native_global, "native global path is unavailable"))?;
    let mut paths = crate::config::global_layer_paths(&global).to_vec();
    if scope == ImportScope::Project {
        let root = roots
            .project_root
            .canonicalize()
            .map_err(|_| error(&roots.project_root, "project root is unavailable"))?;
        if !directory.starts_with(&root) {
            return Err(error(directory, "Location is outside its project root"));
        }
        paths.extend(crate::config::project_layer_paths(directory, &root));
    }
    if paths.len() > 4096 {
        return Err(error(
            directory,
            "native file-layer candidate limit exceeded",
        ));
    }
    let mut native = NativeConfig {
        target: directory.join("cyber.jsonc"),
        target_text: None,
        imported: Default::default(),
        value: json!({}),
        snapshots: Vec::new(),
        missing: Vec::new(),
        remaining: 16 * 1024 * 1024,
    };
    let mut seen = std::collections::BTreeSet::new();
    for path in paths.into_iter().filter(|p| seen.insert(p.clone())) {
        if let Some(snapshot) = SourceSnapshot::read_native_config(&path)? {
            native.remaining = native
                .remaining
                .checked_sub(snapshot.bytes().len())
                .ok_or_else(|| error(&path, "preview exceeds aggregate sixteen MiB parse limit"))?;
            let document = crate::config::parse_jsonc("native import file layer", snapshot.text()?)
                .map_err(|_| error(&path, "invalid native configuration JSON/JSONC"))?;
            if !document.is_object() {
                return Err(error(&path, "native configuration must be an object"));
            }
            crate::config::merge_raw_layer(&mut native.value, &document);
            if path == native.target {
                native.target_text = Some(snapshot.text()?.to_owned());
            }
            native.snapshots.push(snapshot);
        } else {
            native.missing.push(path);
        }
    }
    native.imported =
        super::write::imported_digests(&native.target, native.target_text.as_deref())?;
    Ok(native)
}
struct SourceContext<'a> {
    roots: &'a SourceRoots,
    inventory: &'a [SourceFile],
    snapshots: Vec<SourceSnapshot>,
    secrets: Vec<String>,
    remaining: usize,
    required: Vec<PreviewEnvironment>,
    report: Vec<MappingRecord>,
    /// Accepted source fields of every adapter, by proposal pointer.
    accepted: Origins,
}

fn command_root(source: &SourceFile) -> Option<&Path> {
    source.path.ancestors().skip(1).find(|dir| {
        dir.file_name()
            .is_some_and(|n| n == "commands" || n == "command")
            && dir
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == ".claude" || n == ".opencode" || n == "opencode")
    })
}

fn command_rank(source: &SourceFile) -> (u8, usize) {
    if source.layer != SourceLayer::Project {
        return (0, 0);
    }
    let directory = if source.kind == SourceKind::Command {
        command_root(source)
            .and_then(Path::parent)
            .and_then(Path::parent)
    } else {
        source.path.parent().map(|dir| {
            if dir
                .file_name()
                .is_some_and(|n| n == ".opencode" || n == ".claude")
            {
                dir.parent().unwrap_or(dir)
            } else {
                dir
            }
        })
    };
    (1, directory.map_or(0, |dir| dir.components().count()))
}

struct MarkdownSource {
    name: String,
    command: Option<crate::commands::MarkdownCommand>,
}

fn markdown_sources<'a>(
    context: &mut SourceContext<'_>,
    files: &'a [SourceFile],
    family: SourceTool,
    scope: ImportScope,
) -> Result<Vec<(&'a SourceFile, MarkdownSource)>, DiscoveryError> {
    let mut commands = Vec::new();
    for source in files.iter().filter(|f| {
        f.tool == family
            && f.kind == SourceKind::Command
            && (scope == ImportScope::Project || f.layer != SourceLayer::Project)
    }) {
        if let Some(command) = markdown_command(context, source)? {
            commands.push((source, command));
        }
    }
    Ok(commands)
}

fn markdown_command(
    context: &mut SourceContext<'_>,
    source: &SourceFile,
) -> Result<Option<MarkdownSource>, DiscoveryError> {
    let snapshot = SourceSnapshot::read_admitted(context.roots, source, context.inventory)?;
    context.remaining = context
        .remaining
        .checked_sub(snapshot.bytes().len())
        .ok_or_else(|| {
            error(
                &source.path,
                "preview exceeds aggregate sixteen MiB parse limit",
            )
        })?;
    let parsed = crate::commands::markdown(snapshot.text()?);
    context.snapshots.push(snapshot);
    let name = source
        .path
        .strip_prefix(
            command_root(source)
                .ok_or_else(|| error(&source.path, "command source root is unavailable"))?,
        )
        .map_err(|_| error(&source.path, "command source name is unavailable"))?
        .with_extension("")
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.join("/"))
        .ok_or_else(|| error(&source.path, "command name is not UTF-8"))?;
    if !crate::commands::valid_name(&name) {
        context.report.push(record(
            &source.path,
            "source",
            "not imported",
            "command name is unsupported",
        ));
        return Ok(None);
    }
    let command = match parsed {
        Ok(command) => {
            context.report.push(record(
                &source.path,
                "source",
                "read",
                "Markdown command parsed without execution",
            ));
            Some(command)
        }
        Err(_) => {
            context.report.push(record(
                &source.path,
                "source",
                "not imported",
                "Markdown command needs additional fields or execution support; value withheld",
            ));
            None
        }
    };
    Ok(Some(MarkdownSource { name, command }))
}

fn merge_markdown_commands(
    config: &mut Value,
    origins: &mut Origins,
    commands: Vec<(&SourceFile, MarkdownSource)>,
    inventory: &[SourceFile],
    raw_origins: &Origins,
) -> Result<(), DiscoveryError> {
    let mut claimed = std::collections::BTreeMap::new();
    let mut declared_roots = std::collections::BTreeMap::new();
    for (source, definition) in commands {
        let MarkdownSource { name, command } = definition;
        let root = command_root(source).unwrap();
        let rank = command_rank(source);
        let declaration = (name.clone(), rank);
        if let Some(previous) = declared_roots.insert(declaration, root)
            && previous != root
        {
            return Err(error(
                &source.path,
                "ambiguous legacy and plural command definitions at one scope",
            ));
        }
        let pointer = provenance::child("/commands", &name);
        let mut references = provenance::references(origins, &pointer);
        for key in ["command", "commands"] {
            references.extend(provenance::references(
                raw_origins,
                &provenance::child(&format!("/{key}"), &name),
            ));
        }
        let earlier = references
            .iter()
            .filter_map(|r| inventory.iter().find(|s| s.path == r.source))
            .map(command_rank)
            .chain(claimed.get(&name).copied())
            .max();
        if earlier.is_some_and(|rank| rank > command_rank(source)) {
            continue;
        }
        if !config.get("commands").is_some_and(Value::is_object) {
            config["commands"] = json!({})
        }
        claimed.insert(name.clone(), command_rank(source));
        origins.retain(|key, _| key != &pointer && !key.starts_with(&format!("{pointer}/")));
        let Some(command) = command else {
            config["commands"].as_object_mut().unwrap().remove(&name);
            continue;
        };
        config["commands"][&name] = command.definition;
        for (field, descriptor) in command.sources {
            origins.insert(
                provenance::child(&pointer, &field),
                vec![SourceReference {
                    source: source.path.clone(),
                    field: descriptor.into(),
                }],
            );
        }
        if config["commands"]
            .as_object()
            .is_some_and(|m| m.len() > 128)
        {
            return Err(error(
                &source.path,
                "combined command definitions exceed the native registry bound",
            ));
        }
    }
    if config
        .get("commands")
        .and_then(Value::as_object)
        .is_some_and(|m| m.is_empty())
    {
        config.as_object_mut().unwrap().remove("commands");
    }
    Ok(())
}
/// Run the settings adapters of a family and fold every adapter's output into its
/// proposal; items left behind are attributed to the family's settings file, or to the
/// target when the family has none.
fn adapt(
    representative: Option<&SourceFile>,
    raw: &Raw<'_>,
    target: &Path,
    (config, origins): (&mut Value, &mut Origins),
    report: &mut Vec<MappingRecord>,
    mut adapted: Adapted,
) -> Result<(), DiscoveryError> {
    if let Some(source) = representative {
        raw_adapters(source, raw, config, &mut adapted)?;
    }
    let fallback = representative.map_or(target, |s| s.path.as_path());
    super::adapted::absorb(config, origins, report, adapted, raw.origins, fallback);
    Ok(())
}

/// The admitted text of one source, kept under review and charged to the parse budget.
fn admitted_text(
    context: &mut SourceContext<'_>,
    source: &SourceFile,
) -> Result<String, DiscoveryError> {
    let snapshot = SourceSnapshot::read_admitted(context.roots, source, context.inventory)?;
    context.remaining = context
        .remaining
        .checked_sub(snapshot.bytes().len())
        .ok_or_else(|| {
            error(
                &source.path,
                "preview exceeds aggregate sixteen MiB parse limit",
            )
        })?;
    let text = snapshot.text()?.to_owned();
    context.snapshots.push(snapshot);
    Ok(text)
}

/// Sources the per-file adapters handle: agent and mode files, Codex profile files and
/// `hooks.json`, and files read in place at runtime. `false` leaves `source` to
/// [`source_config`].
fn extra_source(
    context: &mut SourceContext<'_>,
    source: &SourceFile,
    adapted: &mut Adapted,
) -> Result<bool, DiscoveryError> {
    match (source.tool, source.kind) {
        (SourceTool::Claude | SourceTool::OpenCode, SourceKind::Agent | SourceKind::Mode) => {
            let text = admitted_text(context, source)?;
            super::agents::convert(source, &text, adapted);
        }
        (SourceTool::Codex, SourceKind::Profile | SourceKind::Hook | SourceKind::Policy) => {
            let text = admitted_text(context, source)?;
            let document = super::detection::parse_document(source, &text)?;
            context.secrets.extend(sensitive_values(&document));
            let mark = adapted.mark();
            codex_file(source, &document, adapted);
            adapted.attribute(mark, &source.path);
        }
        (_, SourceKind::Skill | SourceKind::Instruction) => read_in_place(context, source),
        (_, SourceKind::SkillAsset) => (),
        _ => return Ok(false),
    }
    Ok(true)
}

fn codex_file(source: &SourceFile, document: &Value, adapted: &mut Adapted) {
    if source.kind == SourceKind::Policy {
        return super::codex_profiles::requirements(document, adapted);
    }
    if source.kind == SourceKind::Hook {
        match document.get("hooks") {
            Some(hooks) => super::hooks::convert(hooks, "/hooks", adapted),
            None => adapted.skip("", "not imported", "hooks.json has no hooks object"),
        }
        return;
    }
    let name = source
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".config.toml"))
        .unwrap_or_default();
    super::codex_profiles::profile_file(name, document, adapted);
}

/// Skills and instruction files are not copied: report whether the runtime reads them
/// where they are.
fn read_in_place(context: &mut SourceContext<'_>, source: &SourceFile) {
    let (covered, kind) = if source.kind == SourceKind::Skill {
        let roots = super::detection_sessions::skill_roots(context.roots);
        (
            super::detection_sessions::under_any(&source.path, &roots),
            "skill",
        )
    } else {
        (
            super::detection_sessions::instruction_covered(context.roots, &source.path),
            "instruction",
        )
    };
    let (status, reason) = match (covered, kind) {
        (true, "skill") => (
            "imported",
            "read in place by read-time skill compatibility; nothing to copy",
        ),
        (true, _) => (
            "imported",
            "read in place by read-time instruction compatibility; nothing to copy",
        ),
        (false, "skill") => (
            "not imported",
            "outside the runtime skill folders; move it under .cyber/skills",
        ),
        (false, _) => (
            "not imported",
            "not read in place; copy its text into AGENTS.md",
        ),
    };
    context
        .report
        .push(record(&source.path, "source", status, reason));
}

/// The adapters over a family's merged settings document: Claude model/env/hooks
/// settings, Codex profiles, permission profiles, environment policy, notify and
/// granular approvals.
fn raw_adapters(
    source: &SourceFile,
    raw: &Raw<'_>,
    config: &mut Value,
    adapted: &mut Adapted,
) -> Result<(), DiscoveryError> {
    match source.tool {
        SourceTool::Claude => {
            let personal = |field: &str| {
                provenance::references(raw.origins, field)
                    .iter()
                    .any(|r| (raw.personal)(&r.source))
            };
            super::claude_settings::convert(raw.value, &personal, adapted)
        }
        SourceTool::Codex => {
            super::codex_profiles::convert(raw.value, adapted);
            granular_approvals(source, raw.value, config, adapted)?;
        }
        SourceTool::OpenCode => instructions(raw, adapted),
    }
    Ok(())
}

/// A family's merged settings document and the native directory it imports into.
struct Raw<'a> {
    value: &'a Value,
    origins: &'a Origins,
    /// Where the target `cyber.jsonc` lives; its relative paths resolve there.
    directory: &'a Path,
    /// Whether a source file holds personal settings that a shared document must not
    /// receive (see [`personal_sources`]).
    personal: &'a dyn Fn(&Path) -> bool,
}

/// Claude's `settings.local.json` files, and for a project import the user's
/// `~/.claude` settings: values from them stay out of a project's `cyber.jsonc`.
fn personal_sources(roots: &SourceRoots, scope: ImportScope) -> impl Fn(&Path) -> bool {
    let home = roots
        .home
        .canonicalize()
        .unwrap_or_else(|_| roots.home.clone())
        .join(".claude");
    move |source: &Path| {
        source
            .file_name()
            .is_some_and(|name| name == "settings.local.json")
            || (scope == ImportScope::Project && source.starts_with(&home))
    }
}

/// OpenCode `instructions` → `instructions`. URLs and absolute or `~/` paths keep their
/// meaning anywhere; a relative path or glob is kept only when its source file sits in
/// the target's directory, since both tools resolve it against the declaring file.
fn instructions(raw: &Raw<'_>, adapted: &mut Adapted) {
    let Some(entries) = raw.value.get("instructions") else {
        return;
    };
    let Some(entries) = entries.as_array().filter(|e| e.len() <= 64) else {
        adapted.skip(
            "/instructions",
            "not imported",
            "expected at most 64 instruction entries",
        );
        return;
    };
    let beside = provenance::references(raw.origins, "/instructions")
        .first()
        .and_then(|r| r.source.parent().map(Path::to_path_buf))
        .is_some_and(|dir| dir.canonicalize().ok().as_deref() == Some(raw.directory));
    let mut kept = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let field = format!("/instructions/{index}");
        let Some(text) = entry
            .as_str()
            .filter(|s| !s.is_empty() && !s.contains('\0'))
        else {
            adapted.skip(field, "not imported", "instruction entries must be strings");
            continue;
        };
        let portable =
            text.starts_with("https://") || text.starts_with("~/") || Path::new(text).is_absolute();
        let relative = !text.contains("://")
            && !Path::new(text)
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir));
        if portable || (beside && relative) {
            adapted.raw(format!("/instructions/{}", kept.len()), &[field.as_str()]);
            kept.push(json!(text));
        } else {
            adapted.skip(
                field,
                "not imported",
                "relative instruction path resolves elsewhere from the target; port by hand",
            );
        }
    }
    if !kept.is_empty() {
        adapted.config["instructions"] = Value::Array(kept);
    }
}

/// Granular approvals turned off become `deny` rules; `rules = false` turns execpolicy
/// `prompt` rules (the only `ask` rules of a Codex proposal) into `deny`.
fn granular_approvals(
    source: &SourceFile,
    raw: &Value,
    config: &mut Value,
    adapted: &mut Adapted,
) -> Result<(), DiscoveryError> {
    let policy = super::codex_policy_config(raw).map_err(|_| {
        error(
            &source.path,
            "approval/sandbox conversion refused; advanced source policy adapter required",
        )
    })?;
    if !policy.rules.is_empty() {
        for index in 0..policy.rules.len() {
            adapted.raw(format!("/permissions/rules/{index}"), &["/approval_policy"]);
        }
        adapted.config["permissions"] = json!({"rules": policy.rules});
    }
    if policy.reject_rule_prompts {
        for rule in config
            .pointer_mut("/permissions/rules")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
            .filter(|rule| rule["effect"] == "ask")
        {
            rule["effect"] = json!("deny");
        }
    }
    if policy.decline_elicitations {
        adapted.skip(
            "/approval_policy",
            "not imported",
            "MCP elicitations are always asked; Cyber Code has no automatic decline",
        );
    }
    Ok(())
}

/// The settings delta of one source; `None` for a source handled (or reported) by
/// another adapter.
fn source_config(
    context: &mut SourceContext<'_>,
    source: &SourceFile,
    adapted: &mut Adapted,
) -> Result<Option<Value>, DiscoveryError> {
    if extra_source(context, source, adapted)? {
        return Ok(None);
    }
    let SourceContext {
        roots,
        inventory,
        snapshots,
        secrets,
        remaining,
        required: _,
        report,
        accepted: _,
    } = context;
    if !matches!(
        source.kind,
        SourceKind::Config | SourceKind::Rule | SourceKind::Mcp
    ) {
        let (status, reason) = unconverted(source.kind);
        report.push(record(&source.path, "source", status, reason));
        return Ok(None);
    }
    if source.kind == SourceKind::Rule && source.tool != SourceTool::Codex {
        report.push(record(
            &source.path,
            "rules",
            "not imported",
            "Cyber Code does not load rule files yet (system-context Path-scoped rules); keep it in .claude/rules",
        ));
        return Ok(None);
    }
    let snapshot = SourceSnapshot::read_admitted(roots, source, inventory)?;
    *remaining = remaining
        .checked_sub(snapshot.bytes().len())
        .ok_or_else(|| {
            error(
                &source.path,
                "preview exceeds aggregate sixteen MiB parse limit",
            )
        })?;
    let config = if source.kind == SourceKind::Rule {
        json!({"permissions":{"rules":converted(source,codex_rules(snapshot.text()?))?.rules}})
    } else {
        let document = super::detection::parse_document(source, snapshot.text()?)?;
        secrets.extend(sensitive_values(&document));
        if source.kind == SourceKind::Mcp {
            for (index, key) in document
                .as_object()
                .into_iter()
                .flat_map(|m| m.keys())
                .enumerate()
            {
                if key != "mcpServers" {
                    report.push(record(&source.path, format!("settings[{index}]"), "not imported",
                        "MCP state/project association requires an additional adapter; value withheld"));
                }
            }
            document
                .get("mcpServers")
                .map_or_else(|| json!({}), |servers| json!({"mcpServers": servers}))
        } else {
            document
        }
    };
    report.push(record(
        &source.path,
        "source",
        "read",
        "source parsed without execution",
    ));
    snapshots.push(snapshot);
    Ok(Some(config))
}
/// The report entry of a source kind no adapter converts.
fn unconverted(kind: SourceKind) -> (&'static str, &'static str) {
    match kind {
        SourceKind::Plugin | SourceKind::CustomTool => (
            "requires manual port",
            "JS/TS plugins and tools use a different protocol; see the plugin kit migration guide: \
             https://github.com/CyberdyneCorp/cybercode-cli/blob/main/docs/local-core-guide.md#porting-opencode-plugins-and-tools",
        ),
        SourceKind::Agent => (
            "requires manual port",
            "Codex agent files have no native equivalent; recreate the agent under agents",
        ),
        _ => ("not imported", "source kind needs an additional adapter"),
    }
}

fn codex_config(
    source: &SourceFile,
    document: &Value,
    required: &mut Vec<RequiredEnvironment>,
    report: &mut Vec<MappingRecord>,
) -> Result<Value, DiscoveryError> {
    let converted = converted(source, codex_provider_config(document))?;
    let mut config = converted.config;
    let policy = super::codex_policy_config(document).map_err(|_| {
        error(
            &source.path,
            "approval/sandbox conversion refused; advanced source policy adapter required",
        )
    })?;
    for (key, value) in policy.config.as_object().into_iter().flatten() {
        config[key] = value.clone();
    }
    if policy.deprecated_untrusted {
        report.push(record(
            &source.path,
            "approval_policy",
            "imported",
            "retired untrusted approval policy; deprecation note",
        ));
    }
    required.extend(converted.required_environment);
    for pending in converted.not_imported {
        let field = provenance::indexed_field(document, &pending.field);
        if matches!(
            field.as_deref(),
            Some(
                "/approval_policy"
                    | "/sandbox_mode"
                    | "/mcp_servers"
                    | "/profiles"
                    | "/profile"
                    | "/permissions"
                    | "/default_permissions"
                    | "/shell_environment_policy"
                    | "/notify"
            )
        ) {
            continue;
        }
        report.push(record(
            &source.path,
            pending.field,
            "not imported",
            pending.reason,
        ));
    }
    Ok(config)
}
fn document_config(
    source: &SourceFile,
    document: &Value,
    required: &mut Vec<RequiredEnvironment>,
    report: &mut Vec<MappingRecord>,
) -> Result<Value, DiscoveryError> {
    let mut config = json!({});
    let supported: &[&str] = match source.tool {
        SourceTool::Codex => {
            config = codex_config(source, document, required, report)?;
            &["mcp_servers"]
        }
        SourceTool::Claude => {
            let permissions = converted(source, claude_permissions(document))?;
            if !permissions.rules.is_empty() {
                config["permissions"] = json!({"rules":permissions.rules});
            }
            if let Some(mode) = permissions.mode {
                config["mode"] = json!(mode);
            }
            if let Some(model) = document.get("model") {
                map_model(source, model, true, &mut config, report)?;
            }
            &[
                "permissions",
                "model",
                "mcpServers",
                "hooks",
                "fallbackModel",
                "env",
                "apiKeyHelper",
            ]
        }
        SourceTool::OpenCode => {
            let rules = converted(source, opencode_permissions(document))?;
            if !rules.is_empty() {
                config["permissions"] = json!({"rules":rules});
            }
            if let Some(model) = document.get("model") {
                map_model(source, model, false, &mut config, report)?;
            }
            let settings = converted(source, super::opencode_settings_config(document))?;
            for (key, value) in settings.config.as_object().into_iter().flatten() {
                config[key] = value.clone();
            }
            for pending in settings.not_imported {
                report.push(record(
                    &source.path,
                    pending.field,
                    "not imported",
                    pending.reason,
                ));
            }
            let providers = converted(source, super::opencode_provider_config(document))?;
            for (key, value) in providers.config.as_object().into_iter().flatten() {
                config[key] = value.clone();
            }
            required.extend(providers.required_environment);
            for pending in providers.not_imported {
                report.push(record(
                    &source.path,
                    pending.field,
                    "not imported",
                    pending.reason,
                ));
            }
            &[
                "permission",
                "permissions",
                "tools",
                "model",
                "mcp",
                "provider",
                "providers",
                "agent",
                "agents",
                "compaction",
                "command",
                "commands",
                "instructions",
            ]
        }
    };
    let mcp = converted(source, super::mcp_config(source.tool, document))?;
    for (key, value) in mcp.config.as_object().into_iter().flatten() {
        config[key] = value.clone();
    }
    required.extend(mcp.required_environment);
    for pending in mcp.not_imported {
        report.push(record(
            &source.path,
            pending.field,
            "not imported",
            pending.reason,
        ));
    }
    if source.tool == SourceTool::Codex {
        return Ok(config);
    }
    for (index, key) in document.as_object().unwrap().keys().enumerate() {
        if !supported.contains(&key.as_str()) {
            report.push(record(
                &source.path,
                format!("settings[{index}]"),
                "not imported",
                "source field requires an additional adapter; value withheld",
            ));
        }
    }
    Ok(config)
}
fn map_model(
    source: &SourceFile,
    model: &Value,
    anthropic: bool,
    config: &mut Value,
    report: &mut Vec<MappingRecord>,
) -> Result<(), DiscoveryError> {
    let model = model
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 256
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:/-".contains(&b))
        })
        .ok_or_else(|| error(&source.path, "model requires manual conversion"))?;
    if anthropic {
        match super::claude_settings::model(model) {
            Some(model) => config["model"] = json!(model),
            None => report.push(record(
                &source.path,
                "model",
                "not imported",
                "model is not a Claude model or alias",
            )),
        }
    } else if !model.contains('/') {
        report.push(record(
            &source.path,
            "model",
            "not imported",
            "model needs provider qualification",
        ));
    } else {
        config["model"] = json!(model);
    }
    Ok(())
}
struct Fill<'a> {
    origins: &'a Origins,
    accepted: &'a mut Origins,
    report: &'a mut Vec<MappingRecord>,
    /// Pointer → digest recorded by an earlier import's annotation in the target.
    imported: &'a std::collections::BTreeMap<String, String>,
    /// Annotated entries this import replaces.
    updated: &'a mut std::collections::BTreeSet<String>,
}
impl Fill<'_> {
    fn accept(&mut self, pointer: &str, reason: &'static str) {
        for field in provenance::fields(self.origins, pointer) {
            self.accepted
                .insert(field.clone(), self.origins[field].clone());
            report_mapping(self.report, self.origins, field, "imported", reason, None);
        }
    }
    /// An entry an earlier import wrote and nobody has edited since.
    fn updatable(&self, pointer: &str, current: &Value) -> bool {
        self.imported
            .get(pointer)
            .is_some_and(|digest| *digest == super::write::digest(current))
    }
}
fn fill_existing(fill: &mut Fill<'_>, base: &mut Value, extra: &Value, pointer: &str) {
    for (key, value) in extra.as_object().into_iter().flatten() {
        let child = provenance::child(pointer, key);
        let Some(previous) = base.get_mut(key) else {
            base.as_object_mut()
                .unwrap()
                .insert(key.clone(), value.clone());
            fill.accept(&child, "supported source field");
            continue;
        };
        if previous == value {
            continue;
        }
        if fill.updatable(&child, previous) {
            *previous = value.clone();
            fill.updated.insert(child.clone());
            fill.accept(&child, "entry of an earlier import updated from its source");
        } else if previous.is_object() && value.is_object() {
            fill_existing(fill, previous, value, &child);
        } else {
            report_mapping(
                fill.report,
                fill.origins,
                &child,
                "merged",
                "existing native or earlier auto-source key kept",
                Some(super::report_values::ValueComparison {
                    kept: previous.clone(),
                    ignored: value.clone(),
                }),
            );
        }
    }
}
fn report_mapping(
    report: &mut Vec<MappingRecord>,
    origins: &Origins,
    pointer: &str,
    status: &'static str,
    reason: &'static str,
    comparison: Option<super::report_values::ValueComparison>,
) {
    let sources = provenance::references(origins, pointer);
    if let Some(first) = sources.first() {
        report.push(MappingRecord {
            source: first.source.clone(),
            sources,
            field: pointer.into(),
            status,
            reason,
            comparison,
        });
    }
}
fn redact_report_fields(
    report: &mut [MappingRecord],
    required: &mut [PreviewEnvironment],
    secrets: &[String],
) {
    for item in report.iter_mut() {
        if let Some(comparison) = &mut item.comparison {
            super::report_values::redact(comparison, &item.field, secrets);
        }
    }
    let mut secrets: Vec<_> = secrets.iter().collect();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let redact = |field: &mut String| {
        for secret in &secrets {
            *field = field.replace(secret.as_str(), "***");
        }
    };
    for item in report {
        redact(&mut item.field);
        for source in &mut item.sources {
            redact(&mut source.field);
        }
    }
    for required in required {
        for source in &mut required.sources {
            redact(&mut source.field);
        }
    }
}
pub(super) fn sensitive_values(value: &Value) -> Vec<String> {
    fn collect(value: &Value, force: bool, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    let normalized: String = key
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect();
                    // A path or glob key (`"secrets/**" = "none"`) names files, not a
                    // credential field.
                    let field = !key.contains(['/', '*', '\\']);
                    let sensitive = force
                        || key == "headers"
                        || key == "http_headers"
                        || (field
                            && [
                                "apikey",
                                "token",
                                "secret",
                                "password",
                                "authorization",
                                "cookie",
                            ]
                            .iter()
                            .any(|s| normalized.contains(s)));
                    collect(value, sensitive, out);
                }
            }
            Value::Array(values) => {
                for value in values {
                    collect(value, force, out);
                }
            }
            Value::String(s) if force && !s.is_empty() && !s.starts_with("{env:") => {
                out.push(s.clone())
            }
            _ => (),
        }
    }
    let mut out = Vec::new();
    collect(value, false, &mut out);
    out.sort();
    out.dedup();
    out
}
pub(super) fn reject_unsafe_strings(
    proposed: &Value,
    existing: &Value,
    secrets: &[String],
    path: &Path,
) -> Result<(), DiscoveryError> {
    if proposed == existing {
        return Ok(());
    }
    fn safe(value: &Value, secrets: &[String], pointer: &str) -> bool {
        match value {
            Value::Object(map) => map
                .iter()
                .all(|(key, value)| safe(value, secrets, &format!("{pointer}/{key}"))),
            Value::Array(values) => values.iter().all(|value| safe(value, secrets, pointer)),
            Value::String(s) => {
                if s.contains("{file:") {
                    return false;
                }
                if s.contains("{env:") {
                    let allowed = (pointer.starts_with("/providers/")
                        && (pointer.ends_with("/api/settings/api_key")
                            || pointer.ends_with("/api/url")
                            || pointer.contains("/request/headers/")))
                        || (pointer.starts_with("/mcp/")
                            && (pointer.contains("/env/") || pointer.contains("/headers/")));
                    let valid = s
                        .strip_prefix("{env:")
                        .and_then(|s| s.strip_suffix('}'))
                        .is_some_and(|n| {
                            !n.is_empty()
                                && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                        });
                    return allowed && valid;
                }
                !secrets.iter().any(|secret| s.contains(secret))
            }
            _ => true,
        }
    }
    // Existing values are preserved; validate only newly introduced leaves.
    fn added(base: &Value, proposed: &Value, secrets: &[String], pointer: &str) -> bool {
        if base == proposed {
            return true;
        }
        match (base.as_object(), proposed.as_object()) {
            (Some(base), Some(proposed)) => proposed.iter().all(|(k, v)| {
                base.get(k).map_or_else(
                    || safe(v, secrets, &format!("{pointer}/{k}")),
                    |old| added(old, v, secrets, &format!("{pointer}/{k}")),
                )
            }),
            _ => safe(proposed, secrets, pointer),
        }
    }
    if !added(existing, proposed, secrets, "") {
        return Err(error(
            path,
            "new configuration contains a declared secret or unsafe file placeholder; manual conversion required",
        ));
    }
    Ok(())
}
