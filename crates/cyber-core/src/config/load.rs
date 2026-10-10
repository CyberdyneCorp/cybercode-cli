//! Layer discovery and merge order (`configuration` → Layering order).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::gate::{self, canonical_json};
use super::merge::{self, Sources};
use super::subst::{self, SubstContext};
use super::{ConfigError, Resolved, SCHEMA_URL, TrustReport, jsonc, validate};
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

struct Layer {
    label: String,
    path: Option<PathBuf>,
    base_dir: PathBuf,
    value: Value,
}

pub fn load(req: &LoadRequest<'_>) -> Result<Resolved, ConfigError> {
    let project = read_project_layers(req)?;
    let trust = assess_trust(req, &project)?;
    let mut layers = vec![defaults_layer(req)];
    layers.extend(read_global_layers(req)?);
    layers.extend(gate_project(project, trust.trusted));
    layers.extend(read_env_layers(req)?);

    let mut issues = Vec::new();
    let mut value = Value::Object(Map::new());
    let mut sources = Sources::new();
    let mut labels = Vec::new();
    for layer in &mut layers {
        substitute_layer(layer, req, &mut issues);
        merge::merge_layer(&mut value, &layer.value, &layer.label, &mut sources);
        labels.push(layer.label.clone());
    }
    labels.extend(apply_profile(req, &mut value, &mut sources, &mut issues));
    for (label, layer) in cli_layers(req, &mut issues) {
        merge::merge_layer(&mut value, &layer, &label, &mut sources);
        labels.push(label);
    }

    let mut warnings = Vec::new();
    issues.extend(validate::validate(&mut value, &mut sources, &mut warnings));
    if !issues.is_empty() {
        return Err(ConfigError::Invalid { issues });
    }
    Ok(Resolved {
        value,
        sources,
        warnings,
        trust,
        layers: labels,
    })
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
    }
}

fn read_global_layers(req: &LoadRequest<'_>) -> Result<Vec<Layer>, ConfigError> {
    let dir = &req.paths.config;
    let files = global_layer_paths(dir);
    read_layers(&files, "global")
}

fn read_project_layers(req: &LoadRequest<'_>) -> Result<Vec<Layer>, ConfigError> {
    if req.env.flag("CYBER_DISABLE_PROJECT_CONFIG") {
        return Ok(Vec::new());
    }
    let root = project_root(req.location);
    read_layers(&project_layer_paths(req.location, &root), "project")
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

fn read_env_layers(req: &LoadRequest<'_>) -> Result<Vec<Layer>, ConfigError> {
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

fn substitute_layer(layer: &mut Layer, req: &LoadRequest<'_>, issues: &mut Vec<String>) {
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
    let label = format!("profile:{name}");
    merge::merge_profile(value, &overlay, &name, sources);
    Some(label)
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
    if matches!(path[0].as_str(), "policy" | "profiles") {
        return Err(format!("--config cannot set {}", path[0]));
    }
    let parsed = serde_json::from_str(&jsonc::to_json(value))
        .unwrap_or_else(|_| Value::String(value.to_string()));
    Ok((path, parsed))
}
