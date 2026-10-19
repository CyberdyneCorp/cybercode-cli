//! `cyber-plugin.json` (`plugins-marketplace` → Plugin manifest). Parsing is strict: a
//! missing, unknown or malformed field fails the whole plugin with an error naming it.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

/// The manifest file at a plugin root.
pub const FILE: &str = "cyber-plugin.json";

const MAX_BYTES: u64 = 64 * 1024;

/// Component path fields, in manifest order. Only the first five load in this release;
/// the others need the plugin host and are reported, never run.
pub const COMPONENTS: [&str; 11] = [
    "skills",
    "commands",
    "agents",
    "hooks",
    "mcp",
    "tools",
    "providers",
    "themes",
    "workflows",
    "channels",
    "mods",
];

const FIELDS: [&str; 12] = [
    "$schema",
    "name",
    "version",
    "description",
    "author",
    "license",
    "homepage",
    "engines",
    "dependencies",
    "capabilities",
    "main",
    "runtime",
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// `engines.cyber`: the Cyber Code versions the plugin supports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    pub dependencies: BTreeMap<String, String>,
    /// `main` and `runtime`: the plugin host entry, when the plugin runs code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<super::capabilities::HostEntry>,
    pub capabilities: super::capabilities::Capabilities,
    #[serde(rename = "userConfig", skip_serializing_if = "Option::is_none")]
    pub user_config: Option<Value>,
    /// Declared component paths, relative to the plugin root.
    pub components: BTreeMap<String, Vec<String>>,
}

/// Why a manifest cannot load, naming the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub field: String,
    pub message: String,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{FILE}: {}: {}", self.field, self.message)
    }
}

impl std::error::Error for ManifestError {}

fn error(field: &str, message: impl Into<String>) -> ManifestError {
    ManifestError {
        field: field.into(),
        message: message.into(),
    }
}

/// `^[a-z][a-z0-9-]{1,63}$`.
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (2..=64).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

impl Manifest {
    /// Read `<root>/cyber-plugin.json`; a linked or oversized file is refused.
    pub fn read(root: &Path) -> Result<Self, ManifestError> {
        let path = root.join(FILE);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| error("manifest", format!("{} is missing", path.display())))?;
        if !metadata.is_file() {
            return Err(error("manifest", "must be a regular file, not a link"));
        }
        if metadata.len() > MAX_BYTES {
            return Err(error("manifest", "exceeds 64 KiB"));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| error("manifest", format!("cannot be read: {e}")))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let value: Value = serde_json::from_str(text)
            .map_err(|e| error("manifest", format!("invalid JSON: {e}")))?;
        let map = value
            .as_object()
            .ok_or_else(|| error("manifest", "must be a JSON object"))?;
        if let Some(key) = map.keys().find(|key| {
            !FIELDS.contains(&key.as_str())
                && key.as_str() != "userConfig"
                && !COMPONENTS.contains(&key.as_str())
        }) {
            return Err(error(key, "unknown manifest field"));
        }
        let name = required_string(map, "name")?;
        if !valid_name(&name) {
            return Err(error(
                "name",
                format!("invalid plugin name {name:?}; expected ^[a-z][a-z0-9-]{{1,63}}$"),
            ));
        }
        let version = required_string(map, "version")?;
        semver::Version::parse(&version)
            .map_err(|_| error("version", format!("{version:?} is not a semver version")))?;
        Ok(Self {
            name,
            version,
            description: optional_string(map, "description")?,
            author: author(map)?,
            license: optional_string(map, "license")?,
            homepage: optional_string(map, "homepage")?,
            engine: engine(map)?,
            dependencies: dependencies(map)?,
            host: super::capabilities::host_entry(map)?,
            capabilities: super::capabilities::capabilities(map)?,
            user_config: optional_object(map, "userConfig")?,
            components: components(map)?,
        })
    }

    /// `requires cyber <range>, running <version>` when `running` is outside
    /// `engines.cyber`.
    pub fn engine_mismatch(&self, running: &str) -> Option<String> {
        let range = self.engine.as_deref()?;
        let accepted = semver::VersionReq::parse(range)
            .ok()
            .zip(semver::Version::parse(running).ok())
            .is_some_and(|(range, running)| range.matches(&running));
        (!accepted).then(|| format!("requires cyber {range}, running {running}"))
    }

    /// The declared paths of `component`, or `None` when the manifest omits it.
    pub fn component(&self, component: &str) -> Option<&[String]> {
        self.components.get(component).map(Vec::as_slice)
    }
}

fn required_string(map: &Map<String, Value>, field: &str) -> Result<String, ManifestError> {
    match map.get(field) {
        None => Err(error(field, "is required")),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(error(field, "must be a string")),
    }
}

fn optional_string(map: &Map<String, Value>, field: &str) -> Result<Option<String>, ManifestError> {
    match map.get(field) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(error(field, "must be a string")),
    }
}

fn optional_object(map: &Map<String, Value>, field: &str) -> Result<Option<Value>, ManifestError> {
    match map.get(field) {
        None => Ok(None),
        Some(value) if value.is_object() => Ok(Some(value.clone())),
        Some(_) => Err(error(field, "must be an object")),
    }
}

fn author(map: &Map<String, Value>) -> Result<Option<Value>, ManifestError> {
    match map.get("author") {
        None => Ok(None),
        Some(value @ (Value::String(_) | Value::Object(_))) => Ok(Some(value.clone())),
        Some(_) => Err(error("author", "must be a string or an object")),
    }
}

fn engine(map: &Map<String, Value>) -> Result<Option<String>, ManifestError> {
    let Some(engines) = optional_object(map, "engines")? else {
        return Ok(None);
    };
    if let Some(key) = engines
        .as_object()
        .into_iter()
        .flatten()
        .find_map(|(key, _)| (key != "cyber").then_some(key))
    {
        return Err(error(&format!("engines.{key}"), "unknown engine"));
    }
    let Some(range) = engines.get("cyber") else {
        return Ok(None);
    };
    let range = range
        .as_str()
        .ok_or_else(|| error("engines.cyber", "must be a string"))?;
    semver::VersionReq::parse(range)
        .map_err(|_| error("engines.cyber", format!("{range:?} is not a semver range")))?;
    Ok(Some(range.to_owned()))
}

fn dependencies(map: &Map<String, Value>) -> Result<BTreeMap<String, String>, ManifestError> {
    let Some(dependencies) = optional_object(map, "dependencies")? else {
        return Ok(BTreeMap::new());
    };
    let mut out = BTreeMap::new();
    for (name, range) in dependencies.as_object().into_iter().flatten() {
        let field = format!("dependencies.{name}");
        if !valid_name(name) {
            return Err(error(&field, "invalid plugin name"));
        }
        let range = range
            .as_str()
            .filter(|range| semver::VersionReq::parse(range).is_ok())
            .ok_or_else(|| error(&field, "must be a semver range"))?;
        out.insert(name.clone(), range.to_owned());
    }
    Ok(out)
}

fn components(map: &Map<String, Value>) -> Result<BTreeMap<String, Vec<String>>, ManifestError> {
    let mut out = BTreeMap::new();
    for component in COMPONENTS {
        let Some(value) = map.get(component) else {
            continue;
        };
        let paths = match value {
            Value::String(path) => vec![path.clone()],
            Value::Array(items) => items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| error(component, "must be a path or a list of paths"))?,
            _ => return Err(error(component, "must be a path or a list of paths")),
        };
        if let Some(path) = paths.iter().find(|path| inside(path).is_none()) {
            return Err(error(
                component,
                format!("{path:?} must be a relative path inside the plugin"),
            ));
        }
        out.insert(component.to_owned(), paths);
    }
    Ok(out)
}

/// A manifest path as a relative path that cannot leave the plugin root.
pub fn inside(path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.contains('\\') {
        return None;
    }
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_manifest_loads_and_names_the_failing_field() {
        let manifest =
            Manifest::parse(r#"{"name":"lint-guard","version":"1.0.0","hooks":"hooks.json"}"#)
                .unwrap();
        assert_eq!(
            manifest.component("hooks"),
            Some(&["hooks.json".to_owned()][..])
        );
        for (text, field, message) in [
            (
                r#"{"name":"Lint Guard","version":"1.0.0"}"#,
                "name",
                "invalid plugin name",
            ),
            (r#"{"version":"1.0.0"}"#, "name", "is required"),
            (r#"{"name":"ok-name","version":"one"}"#, "version", "semver"),
            (
                r#"{"name":"ok-name","version":"1.0.0","engines":{"cyber":"nope"}}"#,
                "engines.cyber",
                "semver range",
            ),
            (
                r#"{"name":"ok-name","version":"1.0.0","skills":"../x"}"#,
                "skills",
                "inside the plugin",
            ),
            (
                r#"{"name":"ok-name","version":"1.0.0","colour":1}"#,
                "colour",
                "unknown",
            ),
        ] {
            let error = Manifest::parse(text).unwrap_err();
            assert_eq!(error.field, field, "{text}");
            assert!(error.to_string().contains(message), "{error}");
        }
    }

    #[test]
    fn engine_range_is_checked_against_the_running_version() {
        let manifest = Manifest::parse(
            r#"{"name":"pr-tools","version":"1.0.0","engines":{"cyber":">=2.0.0"}}"#,
        )
        .unwrap();
        assert_eq!(
            manifest.engine_mismatch("1.4.0").as_deref(),
            Some("requires cyber >=2.0.0, running 1.4.0")
        );
        assert_eq!(manifest.engine_mismatch("2.1.0"), None);
    }

    #[test]
    fn names_follow_the_lowercase_pattern() {
        for name in ["ab", "review-kit", "a1"] {
            assert!(valid_name(name), "{name}");
        }
        for name in ["a", "Review", "1abc", "a_b", &"a".repeat(65)] {
            assert!(!valid_name(name), "{name}");
        }
    }
}
