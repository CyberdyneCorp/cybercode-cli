//! `repo_map` (`code-intelligence` → Repository map configuration). Invalid values are
//! ignored with a warning naming the key, and the default applies. None of these keys
//! is security-sensitive, so they are not part of the checkout's trust digest.

use serde_json::Value;

pub const DEFAULT_MAX_FILES: usize = 50_000;
pub const DEFAULT_MAX_FILE_BYTES: u64 = 1_048_576;
pub const DEFAULT_BUILD_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoMapSettings {
    pub enabled: bool,
    /// Whether `core/repo-map` is rendered into baselines.
    pub context: bool,
    /// Override of the model profiles' `repo_map.context_tokens`.
    pub context_tokens: Option<u32>,
    /// Override of the model profiles' `repo_map.tool_tokens`.
    pub tool_tokens: Option<u32>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub build_timeout_ms: u64,
}

impl Default for RepoMapSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            context: true,
            context_tokens: None,
            tool_tokens: None,
            include: Vec::new(),
            exclude: Vec::new(),
            max_files: DEFAULT_MAX_FILES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            build_timeout_ms: DEFAULT_BUILD_TIMEOUT_MS,
        }
    }
}

impl RepoMapSettings {
    /// The settings and a warning per ignored key or glob.
    pub fn from_config(config: &Value) -> (Self, Vec<String>) {
        Self::from_section(config.get("repo_map"))
    }

    /// The settings of the `repo_map` value itself.
    pub fn from_section(section: Option<&Value>) -> (Self, Vec<String>) {
        let mut settings = Self::default();
        let mut warnings = Vec::new();
        let Some(section) = section else {
            return (settings, warnings);
        };
        let Some(map) = section.as_object() else {
            warnings.push(format!(
                "repo_map: expected an object, got {section}; ignored"
            ));
            return (settings, warnings);
        };
        for (key, value) in map {
            settings.apply(key, value, &mut warnings);
        }
        (settings, warnings)
    }

    fn apply(&mut self, key: &str, value: &Value, warnings: &mut Vec<String>) {
        let ok = match key {
            "enabled" => set(&mut self.enabled, value.as_bool()),
            "context" => set(&mut self.context, value.as_bool()),
            "context_tokens" => set_some(&mut self.context_tokens, tokens(value)),
            "tool_tokens" => set_some(&mut self.tool_tokens, tokens(value)),
            "include" => set(&mut self.include, globs(key, value, warnings)),
            "exclude" => set(&mut self.exclude, globs(key, value, warnings)),
            "max_files" => set(&mut self.max_files, positive(value).map(|n| n as usize)),
            "max_file_bytes" => set(&mut self.max_file_bytes, positive(value)),
            "build_timeout_ms" => set(&mut self.build_timeout_ms, positive(value)),
            _ => {
                warnings.push(format!("unknown config key \"repo_map.{key}\"; ignored"));
                return;
            }
        };
        if !ok {
            warnings.push(format!(
                "repo_map.{key}: invalid value {value}; the default is used"
            ));
        }
    }
}

fn set<T>(slot: &mut T, value: Option<T>) -> bool {
    value.map(|value| *slot = value).is_some()
}

fn set_some<T>(slot: &mut Option<T>, value: Option<T>) -> bool {
    value.map(|value| *slot = Some(value)).is_some()
}

fn positive(value: &Value) -> Option<u64> {
    value.as_u64().filter(|n| *n > 0)
}

fn tokens(value: &Value) -> Option<u32> {
    positive(value).and_then(|n| u32::try_from(n).ok())
}

/// The glob strings; a glob that could resolve outside the Location is dropped with a
/// warning. `None` when the value is not an array of strings.
fn globs(key: &str, value: &Value, warnings: &mut Vec<String>) -> Option<Vec<String>> {
    let items = value.as_array()?;
    let mut out = Vec::new();
    for item in items {
        let glob = item.as_str()?;
        if escapes_location(glob) {
            warnings.push(format!(
                "repo_map.{key}: glob {glob:?} resolves outside the Location; ignored"
            ));
            continue;
        }
        out.push(glob.to_owned());
    }
    Some(out)
}

fn escapes_location(glob: &str) -> bool {
    let normalized = glob.replace('\\', "/");
    normalized.starts_with('/')
        || normalized.starts_with('~')
        || normalized.get(1..2) == Some(":")
        || normalized.split('/').any(|part| part == "..")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn defaults_without_a_section() {
        let (settings, warnings) = RepoMapSettings::from_config(&json!({}));
        assert_eq!(settings, RepoMapSettings::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn map_turned_off_for_a_project() {
        let (settings, warnings) =
            RepoMapSettings::from_config(&json!({"repo_map": {"enabled": false}}));
        assert!(!settings.enabled);
        assert!(warnings.is_empty());
    }

    #[test]
    fn invalid_budget_ignored() {
        let (settings, warnings) =
            RepoMapSettings::from_config(&json!({"repo_map": {"tool_tokens": "big"}}));
        assert_eq!(settings.tool_tokens, None);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("repo_map.tool_tokens"), "{warnings:?}");
    }

    #[test]
    fn globs_outside_the_location_are_ignored() {
        let (settings, warnings) = RepoMapSettings::from_config(&json!({"repo_map": {
            "include": ["vendor/acme/**", "../other/**", "/etc/*"],
            "exclude": ["gen/**"],
            "max_files": 10,
        }}));
        assert_eq!(settings.include, vec!["vendor/acme/**"]);
        assert_eq!(settings.exclude, vec!["gen/**"]);
        assert_eq!(settings.max_files, 10);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn zero_and_negative_limits_keep_defaults() {
        let (settings, warnings) = RepoMapSettings::from_config(&json!({"repo_map": {
            "max_files": 0, "build_timeout_ms": -1, "context": "yes",
        }}));
        assert_eq!(settings.max_files, DEFAULT_MAX_FILES);
        assert_eq!(settings.build_timeout_ms, DEFAULT_BUILD_TIMEOUT_MS);
        assert!(settings.context);
        assert_eq!(warnings.len(), 3);
    }
}
