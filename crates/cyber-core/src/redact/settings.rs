//! `tool_output.redact` (`configuration` → Tool output redaction settings).

use regex::{Regex, RegexBuilder};
use serde_json::Value;

/// At most this many extra patterns and allow entries, each at most [`MAX_SOURCE`] bytes.
pub const MAX_PATTERNS: usize = 32;
pub const MAX_ALLOW: usize = 64;
pub const MAX_SOURCE: usize = 1024;
/// Compiled size bounds of one configured regex; the engine is linear-time, so size is
/// the only complexity to bound.
const REGEX_SIZE_LIMIT: usize = 256 * 1024;
const DFA_SIZE_LIMIT: usize = 1024 * 1024;

/// The validated `tool_output.redact` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactSettings {
    /// `enabled` (default `true`): `false` turns tool output redaction off.
    pub enabled: bool,
    /// `patterns`: extra regexes; each whole match is redacted as `custom`.
    pub patterns: Vec<String>,
    /// `allow`: values never redacted; `/re/` is a regex the whole value must match,
    /// anything else a literal.
    pub allow: Vec<String>,
}

impl Default for RedactSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            patterns: Vec::new(),
            allow: Vec::new(),
        }
    }
}

impl RedactSettings {
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let Some(section) = config.pointer("/tool_output/redact") else {
            return Ok(Self::default());
        };
        let map = section
            .as_object()
            .ok_or("tool_output.redact must be an object")?;
        if let Some(key) = map
            .keys()
            .find(|key| !matches!(key.as_str(), "enabled" | "patterns" | "allow"))
        {
            return Err(format!("tool_output.redact.{key} is not a known setting"));
        }
        let enabled = match map.get("enabled") {
            None => true,
            Some(Value::Bool(enabled)) => *enabled,
            Some(_) => return Err("tool_output.redact.enabled must be a boolean".into()),
        };
        let patterns = strings(map.get("patterns"), "patterns", MAX_PATTERNS)?;
        for (index, pattern) in patterns.iter().enumerate() {
            compile(pattern)
                .map_err(|error| format!("tool_output.redact.patterns[{index}]: {error}"))?;
        }
        let allow = strings(map.get("allow"), "allow", MAX_ALLOW)?;
        for (index, entry) in allow.iter().enumerate() {
            if let Some(pattern) = allow_regex(entry) {
                compile(pattern)
                    .map_err(|error| format!("tool_output.redact.allow[{index}]: {error}"))?;
            }
        }
        Ok(Self {
            enabled,
            patterns,
            allow,
        })
    }
}

fn strings(value: Option<&Value>, key: &str, max: usize) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| format!("tool_output.redact.{key} must be an array of strings"))?;
    if items.len() > max {
        return Err(format!(
            "tool_output.redact.{key} has {} entries; at most {max} are allowed",
            items.len()
        ));
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| match item.as_str() {
            Some(text) if !text.is_empty() && text.len() <= MAX_SOURCE => Ok(text.to_string()),
            _ => Err(format!(
                "tool_output.redact.{key}[{index}] must be a non-empty string of at most {MAX_SOURCE} bytes"
            )),
        })
        .collect()
}

/// The regex of an `allow` entry written `/re/`.
pub(super) fn allow_regex(entry: &str) -> Option<&str> {
    entry
        .strip_prefix('/')
        .and_then(|rest| rest.strip_suffix('/'))
        .filter(|pattern| !pattern.is_empty())
}

/// A configured regex within the size bounds that cannot match empty text.
pub(super) fn compile(pattern: &str) -> Result<Regex, String> {
    let regex = RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(DFA_SIZE_LIMIT)
        .build()
        .map_err(|error| match error {
            regex::Error::CompiledTooBig(_) => "the pattern is too complex".to_string(),
            other => other.to_string(),
        })?;
    if regex.is_match("") {
        return Err("the pattern matches empty text".into());
    }
    Ok(regex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_and_valid_sections() {
        assert_eq!(
            RedactSettings::from_config(&json!({})).unwrap(),
            RedactSettings::default()
        );
        let settings = RedactSettings::from_config(&json!({"tool_output": {"redact": {
            "enabled": false, "patterns": ["corp_[a-z0-9]{12}"], "allow": ["AKIAIOSFODNN7EXAMPLE", "/sk-test-[a-z]+/"]}}}))
        .unwrap();
        assert!(!settings.enabled);
        assert_eq!(settings.patterns, ["corp_[a-z0-9]{12}"]);
        assert_eq!(settings.allow.len(), 2);
    }

    #[test]
    fn invalid_sections_are_rejected() {
        let cases = [
            (json!({"redact": []}), "must be an object"),
            (json!({"redact": {"enabled": "no"}}), "must be a boolean"),
            (json!({"redact": {"patterns": ["("]}}), "patterns[0]"),
            (
                json!({"redact": {"patterns": ["a*"]}}),
                "matches empty text",
            ),
            (
                json!({"redact": {"patterns": ["\\w{1000}{1000}"]}}),
                "too complex",
            ),
            (
                json!({"redact": {"patterns": ["x".repeat(MAX_SOURCE + 1)]}}),
                "at most 1024 bytes",
            ),
            (json!({"redact": {"allow": ["/(/"]}}), "allow[0]"),
            (json!({"redact": {"allow": [1]}}), "allow[0]"),
            (
                json!({"redact": {"patterns": vec!["a"; MAX_PATTERNS + 1]}}),
                "at most 32",
            ),
            (json!({"redact": {"mode": "x"}}), "not a known setting"),
        ];
        for (section, expected) in cases {
            let error = RedactSettings::from_config(&json!({"tool_output": section})).unwrap_err();
            assert!(error.contains(expected), "{error} / {expected}");
        }
    }
}
