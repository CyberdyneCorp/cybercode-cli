//! Read-only reviewed proposals; no destination mutation or executable source loading.
use super::{
    ConversionError, DiscoveryError, RequiredEnvironment, SourceFile, SourceKind, SourceLayer,
    SourceRoots, SourceSnapshot, SourceTool, claude_permissions, codex_provider_config,
    codex_rules, discover_sources, opencode_permissions,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportScope {
    Project,
    Global,
}
#[derive(Debug, Clone, Serialize)]
pub struct MappingRecord {
    pub source: PathBuf,
    pub field: String,
    pub status: &'static str,
    pub reason: &'static str,
}
#[derive(Debug, Serialize)]
pub struct PreviewOutput {
    pub target: PathBuf,
    pub diff: String,
    /// Raw file layers influencing the proposal; no profile or substitution evaluation.
    pub native_layers: Vec<PathBuf>,
    pub report: Vec<MappingRecord>,
    pub required_environment: Vec<RequiredEnvironment>,
    /// This increment does not implement every source adapter or destination transactions.
    pub complete: bool,
}
pub struct ImportPreview {
    output: PreviewOutput,
    sources: Vec<SourceSnapshot>,
    destinations: Vec<SourceSnapshot>,
    missing: Vec<PathBuf>,
}
impl ImportPreview {
    pub fn output(&self) -> &PreviewOutput {
        &self.output
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
    MappingRecord {
        source: source.into(),
        field: field.into(),
        status,
        reason,
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
    let mut proposed = existing.clone();
    let mut report = Vec::new();
    for issue in inventory.issues {
        report.push(record(&issue.path, "source", "not imported", issue.reason));
    }
    let mut context = SourceContext {
        roots,
        inventory: &inventory.files,
        snapshots: Vec::new(),
        required: Vec::new(),
        secrets: sensitive_values(&existing),
        remaining: native.remaining,
        report,
    };
    for family in [SourceTool::OpenCode, SourceTool::Codex, SourceTool::Claude] {
        if tool.is_some_and(|t| t != family) {
            continue;
        }
        let mut raw = json!({});
        let mut rules = json!({});
        let mut origins = BTreeMap::new();
        let mut representative = None;
        for source in inventory.files.iter().filter(|f| {
            f.tool == family && (scope == ImportScope::Project || f.layer != SourceLayer::Project)
        }) {
            let Some(delta) = source_config(&mut context, source)? else {
                continue;
            };
            if source.kind == SourceKind::Rule {
                overlay(&mut rules, &delta, "", true);
            } else {
                overlay(&mut raw, &delta, "", family == SourceTool::Claude);
                representative = Some(source);
            }
            record_origins(&delta, "", &source.path, &mut origins);
        }
        let mut config = if let Some(source) = representative {
            let result = document_config(source, &raw, &mut context.required, &mut context.report)?;
            record_origins(&result, "", &source.path, &mut origins);
            result
        } else {
            json!({})
        };
        overlay(&mut config, &rules, "", true);
        strongest_permissions(family, &mut config);
        fill_existing(&mut proposed, &config, "", &origins, &mut context.report);
    }
    reject_unsafe_strings(&proposed, &existing, &context.secrets, &target)?;
    let diff = render_diff(&target, &existing, &proposed, &context.secrets)?;
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
            complete: false,
        },
        sources: context.snapshots,
        destinations: native.snapshots,
        missing: native.missing,
    };
    preview.verify()?;
    Ok(preview)
}
struct NativeConfig {
    target: PathBuf,
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
            if path.parent() == Some(directory) {
                native.target = path.clone();
            }
            native.snapshots.push(snapshot);
        } else {
            native.missing.push(path);
        }
    }
    Ok(native)
}
struct SourceContext<'a> {
    roots: &'a SourceRoots,
    inventory: &'a [SourceFile],
    snapshots: Vec<SourceSnapshot>,
    secrets: Vec<String>,
    remaining: usize,
    required: Vec<RequiredEnvironment>,
    report: Vec<MappingRecord>,
}
fn source_config(
    context: &mut SourceContext<'_>,
    source: &SourceFile,
) -> Result<Option<Value>, DiscoveryError> {
    let SourceContext {
        roots,
        inventory,
        snapshots,
        secrets,
        remaining,
        required: _,
        report,
    } = context;
    if !matches!(
        source.kind,
        SourceKind::Config | SourceKind::Profile | SourceKind::Rule
    ) {
        let status = if matches!(source.kind, SourceKind::Plugin | SourceKind::CustomTool) {
            "requires manual port"
        } else {
            "not imported"
        };
        report.push(record(
            &source.path,
            "source",
            status,
            "source kind needs an additional adapter",
        ));
        return Ok(None);
    }
    if source.kind == SourceKind::Rule && source.tool != SourceTool::Codex {
        report.push(record(
            &source.path,
            "rules",
            "not imported",
            "instruction rule-file adapter remains required",
        ));
        return Ok(None);
    }
    if source.kind == SourceKind::Profile {
        report.push(record(
            &source.path,
            "profiles",
            "not imported",
            "named profile and inheritance adapter remains required",
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
        document
    };
    report.push(record(
        &source.path,
        "source",
        "read",
        "source parsed without execution; conversion/report coverage remains partial",
    ));
    snapshots.push(snapshot);
    Ok(Some(config))
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
            let converted = converted(source, codex_provider_config(document))?;
            config = converted.config;
            required.extend(converted.required_environment);
            for pending in converted.not_imported {
                report.push(record(
                    &source.path,
                    pending.field,
                    "not imported",
                    pending.reason,
                ));
            }
            return Ok(config);
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
            &["permissions", "model"]
        }
        SourceTool::OpenCode => {
            let rules = converted(source, opencode_permissions(document))?;
            if !rules.is_empty() {
                config["permissions"] = json!({"rules":rules});
            }
            if let Some(model) = document.get("model") {
                map_model(source, model, false, &mut config, report)?;
            }
            &["permission", "permissions", "tools", "model"]
        }
    };
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
    if anthropic && !model.starts_with("claude-") {
        report.push(record(
            &source.path,
            "model",
            "not imported",
            "model alias needs catalog-aware resolution",
        ));
    } else if !anthropic && !model.contains('/') {
        report.push(record(
            &source.path,
            "model",
            "not imported",
            "model needs provider qualification",
        ));
    } else {
        config["model"] = json!(if anthropic {
            format!("anthropic/{model}")
        } else {
            model.into()
        });
    }
    Ok(())
}
fn overlay(base: &mut Value, extra: &Value, pointer: &str, append_rules: bool) {
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            if let Some(previous) = base.get_mut(key) {
                overlay(previous, value, &format!("{pointer}/{key}"), append_rules);
            } else {
                base.insert(key.clone(), value.clone());
            }
        }
    } else if let (Some(base), Some(extra)) = (base.as_array_mut(), extra.as_array()) {
        if append_rules
            && (pointer == "/permissions/rules"
                || matches!(
                    pointer,
                    "/permissions/allow" | "/permissions/ask" | "/permissions/deny"
                ))
        {
            base.extend(extra.iter().cloned());
        } else {
            *base = extra.clone();
        }
    } else {
        *base = extra.clone();
    }
}
fn strongest_permissions(tool: SourceTool, config: &mut Value) {
    if tool == SourceTool::OpenCode {
        return;
    }
    if let Some(rules) = config
        .pointer_mut("/permissions/rules")
        .and_then(Value::as_array_mut)
    {
        rules.sort_by_key(|r| match r["effect"].as_str() {
            Some("allow") => 0,
            Some("ask") => 1,
            _ => 2,
        });
    }
}
fn fill_existing(
    base: &mut Value,
    extra: &Value,
    pointer: &str,
    origins: &BTreeMap<String, PathBuf>,
    report: &mut Vec<MappingRecord>,
) {
    for (key, value) in extra.as_object().into_iter().flatten() {
        let child = format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"));
        if let Some(previous) = base.get_mut(key) {
            if previous.is_object() && value.is_object() {
                fill_existing(previous, value, &child, origins, report);
            } else {
                report.push(record(
                    origins.get(&child).unwrap(),
                    child,
                    "merged",
                    "existing native or earlier auto-source key kept",
                ));
            }
        } else {
            base.as_object_mut()
                .unwrap()
                .insert(key.clone(), value.clone());
        }
    }
}
fn sensitive_values(value: &Value) -> Vec<String> {
    fn collect(value: &Value, force: bool, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    let normalized: String = key
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect();
                    let sensitive = force
                        || key == "headers"
                        || key == "http_headers"
                        || [
                            "apikey",
                            "token",
                            "secret",
                            "password",
                            "authorization",
                            "cookie",
                        ]
                        .iter()
                        .any(|s| normalized.contains(s));
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
fn reject_unsafe_strings(
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
                    let allowed = pointer.starts_with("/providers/")
                        && (pointer.ends_with("/api/settings/api_key")
                            || pointer.ends_with("/api/url")
                            || pointer.contains("/request/headers/"));
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
fn render_diff(
    target: &Path,
    existing: &Value,
    proposed: &Value,
    secrets: &[String],
) -> Result<String, DiscoveryError> {
    if existing == proposed {
        return Ok(String::new());
    }
    let render = |value: &Value| -> Result<String, DiscoveryError> {
        let mut text = serde_json::to_string_pretty(&crate::config::redact_secrets(value))
            .map_err(|_| error(target, "proposal cannot be rendered"))?;
        let mut secrets: Vec<_> = secrets.iter().collect();
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        for secret in secrets {
            let encoded = serde_json::to_string(secret)
                .map_err(|_| error(target, "secret cannot be redacted"))?;
            text = text.replace(&encoded[1..encoded.len() - 1], "***");
            text = text.replace(secret, "***");
        }
        text.push('\n');
        Ok(text)
    };
    let old = render(existing)?;
    let new = render(proposed)?;
    Ok(similar::TextDiff::from_lines(&old, &new)
        .unified_diff()
        .header(
            "native configuration (redacted)",
            "import proposal (redacted)",
        )
        .to_string())
}

fn record_origins(
    value: &Value,
    pointer: &str,
    source: &Path,
    origins: &mut BTreeMap<String, PathBuf>,
) {
    origins.insert(pointer.into(), source.into());
    for (key, value) in value.as_object().into_iter().flatten() {
        record_origins(
            value,
            &format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1")),
            source,
            origins,
        );
    }
}
