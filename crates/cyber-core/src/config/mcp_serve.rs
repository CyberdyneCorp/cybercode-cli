//! `mcp_serve`: MCP serve settings, read only from the global configuration
//! (`mcp-server` → MCP serve limits).

use serde::Serialize;
use serde_json::{Map, Value};

pub(super) const KEY: &str = "mcp_serve";
pub(super) const IGNORED: &str = "mcp_serve is ignored outside the global config";

/// The label used when `clientInfo.name` gives no valid one.
pub const FALLBACK_CLIENT_LABEL: &str = "mcp-client";
const MAX_LABEL: usize = 48;

/// Whether an MCP client label matches `^[a-z0-9][a-z0-9_-]{0,47}$`.
pub fn valid_client_label(label: &str) -> bool {
    let mut bytes = label.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && label.len() <= MAX_LABEL
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// The label of a client without an explicit one: `clientInfo.name` lowercased,
/// characters outside `[a-z0-9_-]` mapped to `-`, truncated to 48 characters, else
/// `mcp-client`. Labels are attribution only, never an identity.
pub fn derive_client_label(name: Option<&str>) -> String {
    let label: String = name
        .unwrap_or_default()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(MAX_LABEL)
        .collect();
    if valid_client_label(&label) {
        label
    } else {
        FALLBACK_CLIENT_LABEL.into()
    }
}

/// Bounds of MCP serve; every value has its own validated range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpServeLimits {
    /// Sessions one MCP connection may create.
    pub sessions_per_connection: u32,
    /// Concurrent waiting prompt calls per MCP connection.
    pub max_waits: u32,
    /// Running Sessions driven through MCP serve, across connections.
    pub max_running_sessions: u32,
    /// Request depth at which mutating calls are refused.
    pub max_depth: u32,
    /// Upper bound every `wait_seconds` is clamped to.
    pub max_wait_seconds: u32,
}

impl Default for McpServeLimits {
    fn default() -> Self {
        Self {
            sessions_per_connection: 8,
            max_waits: 8,
            max_running_sessions: 16,
            max_depth: 2,
            max_wait_seconds: 600,
        }
    }
}

/// The resolved `mcp_serve` key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpServeSettings {
    pub enabled: bool,
    /// Mode ceiling for Sessions created through MCP; never `bypass`.
    pub max_mode: String,
    pub limits: McpServeLimits,
}

impl McpServeSettings {
    /// Validate `mcp_serve` in a global configuration value and apply its defaults. The
    /// default ceiling is the document's `mode` (`default` when unset), with `bypass`
    /// lowered to `auto`.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self {
            enabled: true,
            max_mode: super::mode_ceiling(None, config.get("mode").and_then(Value::as_str)),
            limits: McpServeLimits::default(),
        };
        let Some(value) = config.get(KEY) else {
            return Ok(settings);
        };
        let map = value
            .as_object()
            .ok_or_else(|| format!("{KEY}: expected an object, got {value}"))?;
        for (key, value) in map {
            match key.as_str() {
                "enabled" => {
                    settings.enabled = value
                        .as_bool()
                        .ok_or_else(|| format!("{KEY}.enabled: expected a boolean, got {value}"))?;
                }
                "max_mode" => settings.max_mode = max_mode(value)?,
                "limits" => settings.limits = limits(value)?,
                other => return Err(format!("{KEY}.{other}: unknown key")),
            }
        }
        Ok(settings)
    }
}

fn max_mode(value: &Value) -> Result<String, String> {
    match value.as_str() {
        Some(mode) if mode != "bypass" && super::is_mode(mode) => Ok(mode.into()),
        _ => Err(format!(
            "{KEY}.max_mode: expected one of default, accept-edits, plan, auto, dont-ask (never bypass), got {value}"
        )),
    }
}

/// `(key, minimum, maximum)` of every limit.
const RANGES: [(&str, u32, u32); 5] = [
    ("sessions_per_connection", 1, 64),
    ("max_waits", 1, 32),
    ("max_running_sessions", 1, 64),
    ("max_depth", 1, 4),
    ("max_wait_seconds", 60, 3600),
];

fn limits(value: &Value) -> Result<McpServeLimits, String> {
    let map: &Map<String, Value> = value
        .as_object()
        .ok_or_else(|| format!("{KEY}.limits: expected an object, got {value}"))?;
    let mut limits = McpServeLimits::default();
    for (key, value) in map {
        let Some(&(_, min, max)) = RANGES.iter().find(|(name, _, _)| name == key) else {
            return Err(format!("{KEY}.limits.{key}: unknown key"));
        };
        let number = value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| (min..=max).contains(n))
            .ok_or_else(|| {
                format!("{KEY}.limits.{key}: expected an integer from {min} to {max}, got {value}")
            })?;
        let slot = match key.as_str() {
            "sessions_per_connection" => &mut limits.sessions_per_connection,
            "max_waits" => &mut limits.max_waits,
            "max_running_sessions" => &mut limits.max_running_sessions,
            "max_depth" => &mut limits.max_depth,
            _ => &mut limits.max_wait_seconds,
        };
        *slot = number;
    }
    Ok(limits)
}

/// Remove `mcp_serve` from a non-global layer, warning once.
pub(super) fn strip(layer: &mut Value, warnings: &mut Vec<String>) {
    let removed = layer
        .as_object_mut()
        .is_some_and(|map| map.remove(KEY).is_some());
    if removed && !warnings.iter().any(|w| w == IGNORED) {
        warnings.push(IGNORED.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_derive_from_the_client_name() {
        assert_eq!(derive_client_label(Some("Claude Code")), "claude-code");
        assert_eq!(derive_client_label(Some("codex_cli-2")), "codex_cli-2");
        let long = "a".repeat(60);
        assert_eq!(derive_client_label(Some(&long)), "a".repeat(48));
        assert_eq!(derive_client_label(Some("")), FALLBACK_CLIENT_LABEL);
        assert_eq!(derive_client_label(None), FALLBACK_CLIENT_LABEL);
        assert_eq!(
            derive_client_label(Some(" leading space")),
            FALLBACK_CLIENT_LABEL
        );
        assert_eq!(derive_client_label(Some("Café/ç")), "caf---");
    }

    #[test]
    fn client_labels_are_validated() {
        for good in ["claude-code", "a", "0_x", &"z".repeat(48)] {
            assert!(valid_client_label(good), "{good}");
        }
        for bad in ["", "Bad Label", "-x", "_x", "ü", &"z".repeat(49)] {
            assert!(!valid_client_label(bad), "{bad}");
        }
    }
}
