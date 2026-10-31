//! Layer discovery and merge order (`configuration` → Layering order).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::gate::{self, canonical_json};
use super::merge::{self, Sources};
use super::subst::{self, SubstContext};
use super::{
    ConfigError, Resolved, SCHEMA_URL, TrustReport, jsonc, managed, mcp_files, mcp_serve, peers,
    redaction, validate,
};
use crate::env::EnvSource;
use crate::paths::Paths;
use crate::trust::TrustStore;

/// Inputs for resolving the configuration of one Location.
pub struct LoadRequest<'a> {
    pub location: &'a Path,
    pub paths: &'a Paths,
    pub env: &'a dyn EnvSource,
    pub home: &'a Path,
    /// `--profile`; falls back to `CYBER_PROFILE`, then `default_profile`.
    pub profile: Option<&'a str>,
    /// Raw `--config key=value` overrides.
    pub overrides: &'a [String],
    /// CLI flag layer, for example `{"model": "..."}`.
    pub flags: Value,
}

pub(super) struct Layer {
    pub(super) label: String,
    path: Option<PathBuf>,
    base_dir: PathBuf,
    pub(super) value: Value,
    /// Notes from reading the layer, such as skipped `.mcp.json` servers.
    warnings: Vec<String>,
}

pub fn load(req: &LoadRequest<'_>) -> Result<Resolved, ConfigError> {
    let mut managed_warnings = Vec::new();
    let managed = managed::read(req.env, &mut managed_warnings);
    let pure = crate::plugins::pure(req.env);
    let project = read_project_layers(req)?;
    let trust = assess_trust(req, &project)?;
    // Checked before trust gating, which would hold `peers` back without saying why.
    let mut issues: Vec<String> = project
        .iter()
        .flat_map(|layer| {
            [
                peers::layer_issue(&layer.label, &layer.value),
                super::computer::layer_issue(&layer.label, &layer.value),
            ]
            .into_iter()
            .flatten()
        })
        .collect();
    let mut layers = vec![defaults_layer(req)];
    layers.extend(read_global_layers(req)?);
    layers.extend(gate_project(project, trust.trusted));
    layers.extend(read_env_layers(req)?);
    shadow_mcp_file(&mut layers);
    if pure {
        layers
            .iter_mut()
            .for_each(|layer| strip_external(&mut layer.value));
    }
    let plugins = add_plugin_layers(req, &managed, pure, &mut layers);

    let mut warnings: Vec<String> = layers
        .iter_mut()
        .flat_map(|layer| std::mem::take(&mut layer.warnings))
        .collect();
    let mut value = Value::Object(Map::new());
    let mut sources = Sources::new();
    let mut labels = Vec::new();
    let mut templates = Vec::new();
    for layer in &mut layers {
        restrict_layer(layer, &mut warnings);
        let scope = layer.label.split(':').next();
        if !matches!(scope, Some("default" | "global" | "project")) {
            issues.extend(peers::layer_issue(&layer.label, &layer.value));
            issues.extend(super::computer::layer_issue(&layer.label, &layer.value));
        }
        if layer.label.starts_with("project:") {
            crate::untrusted::restrict_project_layer(
                &mut layer.value,
                &value,
                &layer.label,
                &mut warnings,
            );
            subst::templates(&layer.value, "", &layer.label, &mut templates);
        }
        substitute_layer(layer, req, &mut issues);
        let project = layer.label.starts_with("project:");
        merge_keeping_untrusted(
            &mut value,
            &layer.value,
            project,
            &mut sources,
            |value, sources| {
                merge::merge_layer(value, &layer.value, &layer.label, sources);
            },
        );
        labels.push(layer.label.clone());
    }
    labels.extend(apply_profile(
        req,
        &mut value,
        &mut sources,
        &mut issues,
        &mut warnings,
    ));
    for (label, mut layer) in cli_layers(req, &mut issues) {
        issues.extend(super::computer::layer_issue(&label, &layer));
        mcp_serve::strip(&mut layer, &mut warnings);
        if pure {
            strip_external(&mut layer);
        }
        merge::merge_layer(&mut value, &layer, &label, &mut sources);
        labels.push(label);
    }

    issues.extend(validate::validate(&mut value, &mut sources, &mut warnings));
    if !issues.is_empty() {
        return Err(ConfigError::Invalid { issues });
    }
    set_plugin_components(&mut value, &mut sources, plugins.components);
    warnings.extend(managed_warnings);
    // Policy layers apply last and are read only from the OS-managed directory.
    for (label, settings) in managed.settings {
        merge::merge_layer(
            &mut value,
            &json!({ "policy": settings }),
            &label,
            &mut sources,
        );
        labels.push(label);
    }
    // Features resolve over the policy layers, which `policy.features.lock` lives in.
    let env_features = req.env.get(crate::features::ENV);
    let overrides = crate::features::Overrides {
        flag: crate::features::flag(),
        env: env_features.as_deref(),
    };
    crate::features::resolve(&mut value, &mut sources, overrides, &mut warnings)
        .map_err(|issues| ConfigError::Invalid { issues })?;
    for (label, hooks) in managed.hooks {
        merge::merge_layer(&mut value, &json!({ "hooks": hooks }), &label, &mut sources);
        labels.push(label);
    }
    // A managed server replaces any user or project definition of the same name whole,
    // so no other layer can change its command, URL or options.
    for (label, servers) in managed.mcp {
        for name in servers.as_object().into_iter().flat_map(|map| map.keys()) {
            replace_server(&mut value, &mut sources, name);
        }
        merge::merge_layer(&mut value, &json!({ "mcp": servers }), &label, &mut sources);
        labels.push(label);
    }
    // Keep a template only where its project layer still supplies the merged leaf.
    let templates = templates
        .into_iter()
        .filter(|(pointer, _, label)| sources.get(pointer) == Some(label))
        .map(|(pointer, template, _)| (pointer, template))
        .collect();
    Ok(Resolved {
        value,
        templates,
        sources,
        warnings,
        trust,
        layers: labels,
        managed: managed.policy,
        plugins: plugins.statuses,
        pure,
    })
}

/// What a layer below user config may not set: `mcp_serve` outside the global layers,
/// and loosening `tool_output.redact` settings in a project or plugin layer.
fn restrict_layer(layer: &mut Layer, warnings: &mut Vec<String>) {
    let scope = layer.label.split(':').next();
    if !matches!(scope, Some("default" | "global")) {
        mcp_serve::strip(&mut layer.value, warnings);
    }
    if matches!(scope, Some("project" | "plugin")) && redaction::strip_loosening(&mut layer.value) {
        warnings.push(redaction::warning(&layer.label));
    }
}

/// `--pure`: hooks and MCP servers come only from organization policy.
fn strip_external(value: &mut Value) {
    let Some(map) = value.as_object_mut() else {
        return;
    };
    map.remove("hooks");
    map.remove("mcp");
    for profile in map
        .get_mut("profiles")
        .and_then(Value::as_object_mut)
        .into_iter()
        .flat_map(|profiles| profiles.values_mut())
    {
        if let Some(profile) = profile.as_object_mut() {
            profile.remove("hooks");
            profile.remove("mcp");
        }
    }
}

struct PluginLoad {
    statuses: Vec<crate::plugins::PluginStatus>,
    components: Vec<Value>,
}

/// One `plugin:` layer per loaded plugin, just above the defaults: plugin hooks still run
/// last (hooks are ordered by scope), and every other layer outranks plugin values.
fn add_plugin_layers(
    req: &LoadRequest<'_>,
    managed: &managed::ManagedRead,
    pure: bool,
    layers: &mut Vec<Layer>,
) -> PluginLoad {
    let locations = crate::plugins::Locations::new(req.paths, req.location);
    let project = !req.env.flag("CYBER_DISABLE_PROJECT_CONFIG");
    let resolution = crate::plugins::resolve(&locations, &managed.policy, pure, project);
    let mut agents = super::agent_profiles::builtin_names();
    let mut servers = std::collections::BTreeSet::new();
    for value in layers.iter().map(|layer| &layer.value) {
        let names = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|map| map.keys().cloned())
                .collect::<Vec<_>>()
        };
        agents.extend(names("agents"));
        servers.extend(names("mcp"));
    }
    for (_, managed_servers) in &managed.mcp {
        servers.extend(
            managed_servers
                .as_object()
                .into_iter()
                .flat_map(|m| m.keys().cloned()),
        );
    }
    let mut components = Vec::new();
    let mut plugin_layers = Vec::new();
    for plugin in &resolution.active {
        let built = crate::plugins::plugin_layer(plugin, &mut agents, &mut servers);
        components.push(built.components);
        plugin_layers.push(Layer {
            label: built.label,
            path: Some(built.root.join(crate::plugins::MANIFEST_FILE)),
            base_dir: built.root,
            value: built.value,
            warnings: built.warnings,
        });
    }
    if let Some(defaults) = layers.first_mut() {
        defaults.warnings.extend(resolution.warnings);
    }
    layers.splice(1..1, plugin_layers);
    PluginLoad {
        statuses: resolution.statuses,
        components,
    }
}

/// Only the loader sets [`crate::plugins::COMPONENTS_KEY`]; a layer's value is dropped.
fn set_plugin_components(value: &mut Value, sources: &mut Sources, components: Vec<Value>) {
    let key = crate::plugins::COMPONENTS_KEY;
    let pointer = format!("/{key}");
    let prefix = format!("{pointer}/");
    sources.retain(|path, _| path != &pointer && !path.starts_with(&prefix));
    let Some(map) = value.as_object_mut() else {
        return;
    };
    map.remove(key);
    if !components.is_empty() {
        map.insert(key.into(), Value::Array(components));
    }
}

/// Drop every leaf of `mcp.<name>` and its origins before a managed definition merges.
fn replace_server(value: &mut Value, sources: &mut Sources, name: &str) {
    if let Some(servers) = value.get_mut("mcp").and_then(Value::as_object_mut) {
        servers.remove(name);
    }
    let escaped = name.replace('~', "~0").replace('/', "~1");
    let pointer = format!("/mcp/{escaped}");
    let prefix = format!("{pointer}/");
    sources.retain(|key, _| key != &pointer && !key.starts_with(&prefix));
}

/// The defaults and global layers only, substituted and validated: what global-only keys
/// (`mcp_serve`) and the user's global `mode` resolve from, whatever the Location.
pub fn load_global(req: &LoadRequest<'_>) -> Result<Value, ConfigError> {
    let mut layers = vec![defaults_layer(req)];
    layers.extend(read_global_layers(req)?);
    let mut issues = Vec::new();
    let mut value = Value::Object(Map::new());
    let mut sources = Sources::new();
    for layer in &mut layers {
        substitute_layer(layer, req, &mut issues);
        merge::merge_layer(&mut value, &layer.value, &layer.label, &mut sources);
    }
    issues.extend(validate::validate(
        &mut value,
        &mut sources,
        &mut Vec::new(),
    ));
    // Native control reads only global configuration. Still apply the OS-managed
    // feature lock here so that this separate security boundary cannot bypass an
    // organization disable through its user-only feature resolution.
    for (label, settings) in managed::read(req.env, &mut Vec::new()).settings {
        if let Some(lock) = settings.pointer("/features/lock/computer_use") {
            merge::merge_layer(
                &mut value,
                &json!({"policy":{"features":{"lock":{"computer_use":lock}}}}),
                &label,
                &mut sources,
            );
        }
    }
    if issues.is_empty() {
        Ok(value)
    } else {
        Err(ConfigError::Invalid { issues })
    }
}

/// The global owner-server section only. Unrelated configuration is neither substituted
/// nor validated, so a disabled peer listener cannot prevent owner-server startup.
/// Callers must validate the returned server section before using it for transport policy.
pub fn load_owner_server(req: &LoadRequest<'_>) -> Result<Value, ConfigError> {
    let mut layers = vec![defaults_layer(req)];
    layers.extend(read_global_layers(req)?);
    let mut value = json!({});
    let mut sources = Sources::new();
    for layer in layers {
        if let Some(server) = layer.value.get("server") {
            merge::merge_layer(
                &mut value,
                &json!({"server": server}),
                &layer.label,
                &mut sources,
            );
        }
    }
    let mut layer = Layer {
        label: "global:server".into(),
        path: None,
        base_dir: req.paths.config.clone(),
        value,
        warnings: Vec::new(),
    };
    let mut issues = Vec::new();
    substitute_layer(&mut layer, req, &mut issues);
    if issues.is_empty() {
        Ok(layer.value)
    } else {
        Err(ConfigError::Invalid { issues })
    }
}

/// Trust state of the project layers without resolving the rest.
pub fn trust_report(req: &LoadRequest<'_>) -> Result<TrustReport, ConfigError> {
    let project = read_project_layers(req)?;
    assess_trust(req, &project)
}

/// Original, redacted project hook sections. Never substitute or validate handlers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct RawHookSection {
    pub source: String,
    pub pointer: String,
    pub scope: crate::hooks::HookScope,
    pub value: Value,
}

pub fn withheld_hook_sections(req: &LoadRequest<'_>) -> Result<Vec<RawHookSection>, ConfigError> {
    let project = read_project_layers(req)?;
    if assess_trust(req, &project)?.trusted {
        return Ok(Vec::new());
    }
    let mut sections = Vec::new();
    for layer in project {
        collect_hook_sections(&layer, &mut sections);
    }
    Ok(sections)
}

fn collect_hook_sections(layer: &Layer, sections: &mut Vec<RawHookSection>) {
    let mut append = |pointer: String, value: &Value| {
        sections.push(RawHookSection {
            source: layer
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            pointer,
            scope: if layer
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .is_some_and(|n| n == "cyber.local.jsonc")
            {
                crate::hooks::HookScope::Local
            } else {
                crate::hooks::HookScope::Project
            },
            value: super::redact_secrets(value),
        });
    };
    if let Some(hooks) = layer.value.get("hooks") {
        append("/hooks".into(), hooks);
    }
    if let Some(profiles) = layer.value.get("profiles").and_then(Value::as_object) {
        for (name, profile) in profiles {
            if let Some(hooks) = profile.get("hooks") {
                append(
                    format!("{}/hooks", merge::child_pointer("/profiles", name)),
                    hooks,
                );
            }
        }
    }
}

/// The git worktree root containing `location`, or `location` itself outside git.
pub fn project_root(location: &Path) -> PathBuf {
    location
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(location)
        .to_path_buf()
}

/// Create `cyber.jsonc` containing only `$schema` when no global document exists.
/// Returns whether a file was created.
pub fn ensure_global_config(paths: &Paths) -> std::io::Result<bool> {
    let json = paths.config.join("cyber.json");
    let jsonc = paths.config.join("cyber.jsonc");
    if json.exists() || jsonc.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(&paths.config)?;
    std::fs::write(&jsonc, format!("{{\n  \"$schema\": \"{SCHEMA_URL}\"\n}}\n"))?;
    Ok(true)
}

fn defaults_layer(req: &LoadRequest<'_>) -> Layer {
    Layer {
        label: "default".into(),
        path: None,
        base_dir: req.location.to_path_buf(),
        value: json!({"mode": "default", "share": "manual", "snapshots": true, "autoupdate": "notify"}),
        warnings: Vec::new(),
    }
}

pub(super) fn read_global_layers(req: &LoadRequest<'_>) -> Result<Vec<Layer>, ConfigError> {
    let dir = &req.paths.config;
    let files = global_layer_paths(dir);
    read_layers(&files, "global")
}

pub(super) fn read_project_layers(req: &LoadRequest<'_>) -> Result<Vec<Layer>, ConfigError> {
    if req.env.flag("CYBER_DISABLE_PROJECT_CONFIG") {
        return Ok(Vec::new());
    }
    let root = project_root(req.location);
    let mut layers = read_layers(&project_layer_paths(req.location, &root), "project")?;
    let file = root.join(mcp_files::FILE);
    if std::fs::symlink_metadata(&file).is_err() {
        return Ok(layers);
    }
    let global = read_global_layers(req)?;
    let values = global.iter().chain(&layers).map(|layer| &layer.value);
    if mcp_files::enabled(values)
        && let Some(layer) = read_mcp_file(&file)
    {
        layers.insert(0, layer);
    }
    Ok(layers)
}

/// The project `.mcp.json` as the lowest project layer; a file that cannot be read or
/// parsed loads nothing and says why.
fn read_mcp_file(path: &Path) -> Option<Layer> {
    let label = format!("project:{}", path.display());
    let linked = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    let document = if linked {
        Err(format!(
            "{} ignored: a linked file is not read",
            path.display()
        ))
    } else {
        read_document(path).map_err(|error| format!("{} ignored: {error}", path.display()))
    };
    let (value, warnings) = match document {
        Ok(None) => return None,
        Ok(Some(document)) => mcp_files::convert(&document, path),
        Err(warning) => (json!({}), vec![warning]),
    };
    let mut layer = layer(label, Some(path.to_path_buf()), value);
    layer.warnings = warnings;
    Some(layer)
}

/// Drop `.mcp.json` servers that any configuration document also names.
fn shadow_mcp_file(layers: &mut [Layer]) {
    let is_file = |layer: &Layer| {
        layer
            .path
            .as_deref()
            .and_then(Path::file_name)
            .is_some_and(|name| name == mcp_files::FILE)
    };
    let Some(index) = layers.iter().position(is_file) else {
        return;
    };
    let declared: Vec<Value> = layers
        .iter()
        .filter(|layer| !is_file(layer))
        .map(|layer| layer.value.clone())
        .collect();
    let declared: Vec<&Value> = declared.iter().collect();
    mcp_files::shadow(&mut layers[index].value, &declared);
}

pub(crate) fn global_layer_paths(directory: &Path) -> [PathBuf; 2] {
    [directory.join("cyber.json"), directory.join("cyber.jsonc")]
}

pub(crate) fn project_layer_paths(location: &Path, root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<&Path> = location
        .ancestors()
        .take_while(|d| d.starts_with(root))
        .collect();
    dirs.reverse();
    let mut files: Vec<PathBuf> = dirs.iter().flat_map(|d| global_layer_paths(d)).collect();
    files.extend(dirs.iter().flat_map(|d| {
        let cyber = d.join(".cyber");
        [
            cyber.join("cyber.json"),
            cyber.join("cyber.jsonc"),
            cyber.join("cyber.local.jsonc"),
        ]
    }));
    files
}

pub(super) fn read_env_layers(req: &LoadRequest<'_>) -> Result<Vec<Layer>, ConfigError> {
    let mut layers = Vec::new();
    if let Some(file) = req.env.get("CYBER_CONFIG") {
        let path = PathBuf::from(&file);
        let value = read_document(&path)?.ok_or_else(|| ConfigError::Io {
            path: file.clone(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        })?;
        layers.push(layer(format!("env:CYBER_CONFIG:{file}"), Some(path), value));
    }
    if let Some(content) = req.env.get("CYBER_CONFIG_CONTENT") {
        let value = object(
            jsonc::parse("CYBER_CONFIG_CONTENT", &content)?,
            "CYBER_CONFIG_CONTENT",
        )?;
        layers.push(Layer {
            label: "env:CYBER_CONFIG_CONTENT".into(),
            path: None,
            base_dir: req.location.to_path_buf(),
            value,
            warnings: Vec::new(),
        });
    }
    Ok(layers)
}

fn read_layers(files: &[PathBuf], scope: &str) -> Result<Vec<Layer>, ConfigError> {
    let mut layers = Vec::new();
    for path in files {
        if let Some(value) = read_document(path)? {
            layers.push(layer(
                format!("{scope}:{}", path.display()),
                Some(path.clone()),
                value,
            ));
        }
    }
    Ok(layers)
}

fn layer(label: String, path: Option<PathBuf>, value: Value) -> Layer {
    let base_dir = path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    Layer {
        label,
        path,
        base_dir,
        value,
        warnings: Vec::new(),
    }
}

fn read_document(path: &Path) -> Result<Option<Value>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Io {
                path: path.display().to_string(),
                source,
            });
        }
    };
    let label = path.display().to_string();
    object(jsonc::parse(&label, &text)?, &label).map(Some)
}

fn object(value: Value, label: &str) -> Result<Value, ConfigError> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(ConfigError::Invalid {
            issues: vec![format!("{label}: the document must be a JSON object")],
        })
    }
}

fn assess_trust(req: &LoadRequest<'_>, project: &[Layer]) -> Result<TrustReport, ConfigError> {
    let root = project_root(req.location);
    let checkout_root = std::fs::canonicalize(&root).unwrap_or(root.clone());
    let mut subset = Map::new();
    let mut definitions = Vec::new();
    for layer in project {
        let split = gate::split_sensitive(&layer.value);
        if split.pointers.is_empty() {
            continue;
        }
        let path = layer.path.clone().unwrap_or_default();
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        definitions.extend(
            split
                .pointers
                .iter()
                .map(|p| format!("{}#{p}", path.display())),
        );
        subset.insert(relative, split.sensitive);
    }
    // Project team definitions choose agents, models and budgets for Sessions the lead
    // starts, and project output styles are prompt text, so both take effect only once
    // the checkout approves them.
    if !req.env.flag("CYBER_DISABLE_PROJECT_CONFIG") {
        let files = crate::teams::trust_subset(req.location)
            .into_iter()
            .chain(crate::output_styles::trust_subset(req.location));
        for (relative, path, text) in files {
            definitions.push(path.display().to_string());
            subset.insert(relative, Value::String(text));
        }
    }
    if subset.is_empty() {
        return Ok(TrustReport {
            checkout_root,
            digest: None,
            trusted: true,
            definitions,
        });
    }
    let digest = format!(
        "sha256:{:x}",
        Sha256::digest(canonical_json(&Value::Object(subset)))
    );
    let trusted = TrustStore::new(req.paths.trust_file())
        .is_approved(&checkout_root, &digest)
        .map_err(|source| ConfigError::Io {
            path: req.paths.trust_file().display().to_string(),
            source,
        })?;
    Ok(TrustReport {
        checkout_root,
        digest: Some(digest),
        trusted,
        definitions,
    })
}

fn gate_project(layers: Vec<Layer>, trusted: bool) -> Vec<Layer> {
    if trusted {
        return layers;
    }
    layers
        .into_iter()
        .map(|mut l| {
            l.value = gate::split_sensitive(&l.value).safe;
            l
        })
        .collect()
}

pub(super) fn substitute_layer(layer: &mut Layer, req: &LoadRequest<'_>, issues: &mut Vec<String>) {
    let ctx = SubstContext {
        env: req.env,
        base_dir: &layer.base_dir,
        home: req.home,
    };
    let prefix = format!("{}#", layer.label);
    subst::substitute(&mut layer.value, &ctx, &prefix, issues);
}

fn apply_profile(
    req: &LoadRequest<'_>,
    value: &mut Value,
    sources: &mut Sources,
    issues: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let name = req
        .profile
        .map(str::to_string)
        .or_else(|| req.env.get("CYBER_PROFILE"))
        .or_else(|| {
            value
                .get("default_profile")
                .and_then(Value::as_str)
                .map(str::to_string)
        })?;
    let profiles = value
        .get("profiles")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let Some(profile) = profiles.get(&name) else {
        let mut names: Vec<&str> = profiles.keys().map(String::as_str).collect();
        names.sort_unstable();
        let available = if names.is_empty() {
            "(none)".to_string()
        } else {
            names.join(", ")
        };
        issues.push(format!(
            "unknown profile \"{name}\"; available: {available}"
        ));
        return None;
    };
    let mut overlay = profile.clone();
    if let Some(map) = overlay.as_object_mut() {
        map.remove("profiles");
        map.remove("policy");
    }
    mcp_serve::strip(&mut overlay, warnings);
    // A profile a project layer defined may only tighten the untrusted-content settings,
    // as that layer itself may.
    if let Some(project) = project_source(sources, &name, "/permissions/untrusted_content") {
        crate::untrusted::restrict_project_layer(&mut overlay, value, &project, warnings);
    }
    let label = format!("profile:{name}");
    issues.extend(super::computer::layer_issue(&label, &overlay));
    let project = project_source(sources, &name, "/permissions").is_some();
    merge_keeping_untrusted(value, &overlay, project, sources, |value, sources| {
        merge::merge_profile(value, &overlay, &name, sources);
    });
    Some(label)
}

/// Merge a layer with `merge`. When a `project` layer replaces the whole `permissions`
/// section (a string, an array or `null`), the lower layers' `permissions.untrusted_content`
/// is put back with its sources: a project layer may only make those settings stricter.
fn merge_keeping_untrusted(
    value: &mut Value,
    overlay: &Value,
    project: bool,
    sources: &mut Sources,
    merge: impl FnOnce(&mut Value, &mut Sources),
) {
    let saved = project
        .then(|| crate::untrusted::replaced_settings(overlay, value))
        .flatten();
    let Some(saved) = saved else {
        merge(value, sources);
        return;
    };
    const POINTER: &str = "/permissions/untrusted_content";
    let saved_sources: Sources = sources
        .iter()
        .filter(|(key, _)| {
            key.strip_prefix(POINTER)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
        .map(|(key, source)| (key.clone(), source.clone()))
        .collect();
    merge(value, sources);
    let moved = crate::untrusted::restore_settings(value, saved);
    if let Some((moved, source)) = moved.zip(sources.get("/permissions").cloned()) {
        sources.insert(moved.to_string(), source);
    }
    sources.extend(saved_sources);
}

/// The project layer label that set anything under `pointer` in profile `name`, if any.
fn project_source(sources: &Sources, name: &str, pointer: &str) -> Option<String> {
    let prefix = format!(
        "/profiles/{}{pointer}",
        name.replace('~', "~0").replace('/', "~1")
    );
    sources
        .iter()
        .find(|(key, label)| {
            label.starts_with("project:")
                && key
                    .strip_prefix(&prefix)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
        .map(|(_, label)| label.clone())
}

fn cli_layers(req: &LoadRequest<'_>, issues: &mut Vec<String>) -> Vec<(String, Value)> {
    let mut layers = Vec::new();
    if !req.overrides.is_empty() {
        let mut root = Value::Object(Map::new());
        for raw in req.overrides {
            match parse_override(raw) {
                Ok((path, value)) => gate::insert_path(&mut root, &path, value),
                Err(e) => issues.push(e),
            }
        }
        layers.push(("cli:--config".to_string(), root));
    }
    if req.flags.as_object().is_some_and(|m| !m.is_empty()) {
        layers.push(("cli:flags".to_string(), req.flags.clone()));
    }
    layers
}

/// `key.path=value`, where value is parsed as JSON(C) when possible and as a string otherwise.
fn parse_override(raw: &str) -> Result<(Vec<String>, Value), String> {
    let (key, value) = raw
        .split_once('=')
        .ok_or_else(|| format!("--config {raw}: expected key=value"))?;
    let path: Vec<String> = key.trim().split('.').map(str::to_string).collect();
    if path.iter().any(String::is_empty) {
        return Err(format!("--config {raw}: invalid key"));
    }
    if matches!(path[0].as_str(), "policy" | "profiles" | "peers") {
        return Err(format!("--config cannot set {}", path[0]));
    }
    let parsed = serde_json::from_str(&jsonc::to_json(value))
        .unwrap_or_else(|_| Value::String(value.to_string()));
    Ok((path, parsed))
}
