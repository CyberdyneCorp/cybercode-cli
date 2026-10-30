//! `permissions.untrusted_content` (`permissions-modes` → Untrusted content settings):
//! the typed settings, and the rule that a project layer may only make them stricter.

use serde::Deserialize;
use serde_json::Value;

use super::{BypassSetting, NoticePolicy, ScanScope, TaintSetting};

const POINTER: &str = "/permissions/untrusted_content";

/// One `allow_when_tainted` pattern: an action that keeps its `allow` in a tainted run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaintedAllow {
    pub action: String,
    pub resource: String,
}

/// The effective `permissions.untrusted_content` settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UntrustedSettings {
    pub taint: TaintSetting,
    pub bypass: BypassSetting,
    pub scan: ScanScope,
    pub allow_when_tainted: Vec<TaintedAllow>,
}

impl UntrustedSettings {
    /// The settings of a merged config, or why they are invalid.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let Some(value) = config.pointer(POINTER) else {
            return Ok(Self::default());
        };
        let settings: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!("permissions.untrusted_content: {error}"))?;
        if settings
            .allow_when_tainted
            .iter()
            .any(|rule| rule.action.trim().is_empty() || rule.resource.trim().is_empty())
        {
            return Err(
                "permissions.untrusted_content.allow_when_tainted requires nonempty action/resource patterns"
                    .into(),
            );
        }
        Ok(settings)
    }

    /// The settings to enforce: invalid settings (which config validation refuses to load)
    /// read as the strictest ones, so a bad value can never switch the defense off.
    pub fn effective(config: &Value) -> Self {
        Self::from_config(config).unwrap_or_else(|_| Self::strictest())
    }

    fn strictest() -> Self {
        Self {
            taint: TaintSetting::Any,
            bypass: BypassSetting::Deny,
            scan: ScanScope::All,
            allow_when_tainted: Vec::new(),
        }
    }

    pub fn notice_policy(&self) -> NoticePolicy {
        NoticePolicy {
            taint: self.taint,
            bypass: self.bypass,
        }
    }
}

/// How strict a value of `key` is, lowest first; `None` for an unknown key or value.
fn strictness(key: &str, value: &Value) -> Option<u8> {
    let ranks: &[&str] = match key {
        "taint" => &["off", "signal", "any"],
        "bypass" => &["allow", "deny"],
        "scan" => &["external", "all"],
        _ => return None,
    };
    let value = value.as_str()?;
    ranks
        .iter()
        .position(|rank| *rank == value)
        .and_then(|index| u8::try_from(index).ok())
}

/// The strictness of `key` before the layer applies: the lower layers' value, else the
/// default (`signal`, `allow`, `external`).
fn current_strictness(key: &str, below: &Value) -> u8 {
    below
        .pointer(&format!("{POINTER}/{key}"))
        .and_then(|value| strictness(key, value))
        .unwrap_or(u8::from(key == "taint"))
}

/// Drop what a project layer (`label`, such as `project:/repo/.cyber/cyber.jsonc`) may
/// not set in `permissions.untrusted_content`, with one warning per file and key: a value
/// less strict than the lower layers' (`below`), an unknown value, and any
/// `allow_when_tainted`. A hostile checkout must not switch the defense off.
pub fn restrict_project_layer(
    layer: &mut Value,
    below: &Value,
    label: &str,
    warnings: &mut Vec<String>,
) {
    let Some(settings) = layer.pointer_mut(POINTER).and_then(Value::as_object_mut) else {
        return;
    };
    let file = label.strip_prefix("project:").unwrap_or(label);
    let mut ignored = Vec::new();
    for key in ["taint", "bypass", "scan"] {
        let Some(value) = settings.get(key) else {
            continue;
        };
        let loosens =
            strictness(key, value).is_none_or(|rank| rank < current_strictness(key, below));
        if loosens {
            ignored.push(key);
        }
    }
    if settings.contains_key("allow_when_tainted") {
        ignored.push("allow_when_tainted");
    }
    for key in ignored {
        settings.remove(key);
        warnings.push(format!(
            "{file}: permissions.untrusted_content.{key} ignored: a project layer can only make it stricter"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn restricted(layer: Value, below: Value) -> (Value, Vec<String>) {
        let mut layer = json!({"permissions": {"untrusted_content": layer}});
        let below = json!({"permissions": {"untrusted_content": below}});
        let mut warnings = Vec::new();
        restrict_project_layer(
            &mut layer,
            &below,
            "project:/r/.cyber/cyber.jsonc",
            &mut warnings,
        );
        (layer["permissions"]["untrusted_content"].clone(), warnings)
    }

    /// Every tightening a project layer may make is kept, without warnings.
    #[test]
    fn project_tightening_is_kept() {
        for (layer, below) in [
            (json!({"taint": "signal"}), json!({"taint": "off"})),
            (json!({"taint": "any"}), json!({})),
            (json!({"taint": "any"}), json!({"taint": "signal"})),
            (json!({"bypass": "deny"}), json!({})),
            (json!({"scan": "all"}), json!({})),
            (
                json!({"taint": "signal", "bypass": "allow", "scan": "external"}),
                json!({}),
            ),
        ] {
            let (kept, warnings) = restricted(layer.clone(), below);
            assert_eq!(kept, layer);
            assert!(warnings.is_empty(), "{warnings:?}");
        }
    }

    /// Loosening values, unknown values and any `allow_when_tainted` are dropped with one
    /// warning each that names the file and the key.
    #[test]
    fn project_loosening_is_ignored_with_a_warning_per_key() {
        let cases = [
            (json!({"taint": "off"}), json!({}), "taint"),
            (json!({"taint": "signal"}), json!({"taint": "any"}), "taint"),
            (
                json!({"bypass": "allow"}),
                json!({"bypass": "deny"}),
                "bypass",
            ),
            (json!({"scan": "external"}), json!({"scan": "all"}), "scan"),
            (json!({"taint": "maybe"}), json!({}), "taint"),
            (json!({"bypass": true}), json!({}), "bypass"),
            (
                json!({"allow_when_tainted": [{"action": "*", "resource": "*"}]}),
                json!({}),
                "allow_when_tainted",
            ),
        ];
        for (layer, below, key) in cases {
            let (kept, warnings) = restricted(layer, below);
            assert_eq!(kept, json!({}), "{key}");
            assert_eq!(
                warnings,
                [format!(
                    "/r/.cyber/cyber.jsonc: permissions.untrusted_content.{key} ignored: a project layer can only make it stricter"
                )]
            );
        }
        let (kept, warnings) = restricted(json!({"taint": "off", "scan": "all"}), json!({}));
        assert_eq!(kept, json!({"scan": "all"}));
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn settings_parse_strictly_and_enforce_strictest_when_invalid() {
        let config = |value: Value| json!({"permissions": {"untrusted_content": value}});
        assert_eq!(
            UntrustedSettings::from_config(&json!({})).unwrap(),
            UntrustedSettings::default()
        );
        let parsed = UntrustedSettings::from_config(&config(json!({
            "taint": "any", "bypass": "deny", "scan": "all",
            "allow_when_tainted": [{"action": "webfetch", "resource": "https://docs.rs/*"}]
        })))
        .unwrap();
        assert_eq!(parsed.taint, TaintSetting::Any);
        assert_eq!(parsed.bypass, BypassSetting::Deny);
        assert_eq!(parsed.scan, ScanScope::All);
        assert_eq!(parsed.allow_when_tainted[0].resource, "https://docs.rs/*");
        for invalid in [
            json!({"taint": "never"}),
            json!({"extra": 1}),
            json!({"allow_when_tainted": [{"action": "", "resource": "*"}]}),
            json!("off"),
        ] {
            assert!(UntrustedSettings::from_config(&config(invalid.clone())).is_err());
            assert_eq!(
                UntrustedSettings::effective(&config(invalid)),
                UntrustedSettings::strictest()
            );
        }
    }
}
