//! A project or plugin layer may only tighten `tool_output.redact` (`configuration` →
//! Tool output redaction settings): it may add `patterns`, but `enabled: false` and
//! `allow` entries, which would show secrets to the model, are dropped with a warning.

use serde_json::Value;

/// Drop the loosening redaction settings of a project or plugin layer, top level and in
/// its profiles. Returns whether anything was dropped.
pub(super) fn strip_loosening(layer: &mut Value) -> bool {
    let mut dropped = strip_section(layer);
    if let Some(profiles) = layer.get_mut("profiles").and_then(Value::as_object_mut) {
        for profile in profiles.values_mut() {
            dropped |= strip_section(profile);
        }
    }
    dropped
}

fn strip_section(value: &mut Value) -> bool {
    let Some(section) = value
        .pointer_mut("/tool_output/redact")
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    let mut dropped = section.remove("allow").is_some();
    if section
        .get("enabled")
        .is_some_and(|enabled| enabled != &Value::Bool(true))
    {
        section.remove("enabled");
        dropped = true;
    }
    dropped
}

/// The warning for a layer whose loosening settings were dropped.
pub(super) fn warning(label: &str) -> String {
    format!(
        "{label}: tool_output.redact.enabled: false and tool_output.redact.allow are ignored outside user config; a project may only add patterns"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn loosening_settings_are_dropped_and_patterns_kept() {
        let mut layer = json!({
            "tool_output": {"max_lines": 10, "redact": {"enabled": false, "allow": ["x"], "patterns": ["corp_[0-9]+"]}},
            "profiles": {"ci": {"tool_output": {"redact": {"enabled": "no"}}}},
        });
        assert!(strip_loosening(&mut layer));
        assert_eq!(
            layer,
            json!({
                "tool_output": {"max_lines": 10, "redact": {"patterns": ["corp_[0-9]+"]}},
                "profiles": {"ci": {"tool_output": {"redact": {}}}},
            })
        );
        let mut strict = json!({"tool_output": {"redact": {"enabled": true, "patterns": ["a+"]}}});
        assert!(!strip_loosening(&mut strict));
        assert_eq!(strict["tool_output"]["redact"]["enabled"], true);
    }
}
