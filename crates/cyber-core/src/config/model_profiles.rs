//! `providers.<p>.models.<m>.profile` (`provider-catalog` → Model behavior profiles): the
//! keys and values a model profile override accepts. An invalid entry warns, naming its
//! key, and the catalog ignores it; the profile types in `cyber-llm` parse these values.

use serde_json::Value;

pub const REASONING_REPLAY: &[&str] = &["never", "tool_loop", "always"];
pub const REASONING_CARRIER: &[&str] = &[
    "none",
    "responses",
    "anthropic",
    "reasoning_content",
    "reasoning",
    "reasoning_details",
    "gemini_signature",
    "copilot_opaque",
];
pub const VERBOSITY: &[&str] = &["low", "medium", "high"];
pub const EDIT_TOOL: &[&str] = &["freeform_patch", "json_patch", "edit"];
pub const TRUNCATION: &[&str] = &["default", "head", "tail", "middle"];
pub const EFFORT_CLAMP: &[&str] = &["nearest_lower", "fail"];

/// What one key accepts.
enum Shape {
    OneOf(&'static [&'static str]),
    Bool,
    Tokens,
    Millis,
    Strings,
    Object(&'static [(&'static str, Shape)]),
}

const VERBOSITY_FIELDS: &[(&str, Shape)] = &[
    ("supported", Shape::Bool),
    ("default", Shape::OneOf(VERBOSITY)),
];
const TOOL_OUTPUT_FIELDS: &[(&str, Shape)] = &[
    ("tokens", Shape::Tokens),
    ("truncation", Shape::OneOf(TRUNCATION)),
];
const FIELDS: &[(&str, Shape)] = &[
    ("reasoning_replay", Shape::OneOf(REASONING_REPLAY)),
    ("reasoning_carrier", Shape::OneOf(REASONING_CARRIER)),
    ("reasoning_summary", Shape::Bool),
    ("verbosity", Shape::Object(VERBOSITY_FIELDS)),
    ("parallel_tool_calls", Shape::Bool),
    ("edit_tool", Shape::OneOf(EDIT_TOOL)),
    ("tool_output", Shape::Object(TOOL_OUTPUT_FIELDS)),
    ("efforts", Shape::Strings),
    ("effort_clamp", Shape::OneOf(EFFORT_CLAMP)),
];

/// `providers.<p>.timeout` (`provider-catalog` → Stream idle timeout).
const TIMEOUT_FIELDS: &[(&str, Shape)] = &[("idle_ms", Shape::Millis)];

/// Warn about every invalid key or value of the model profiles and stream timeouts
/// under `providers`.
pub fn check(providers: Option<&Value>, warnings: &mut Vec<String>) {
    let Some(providers) = providers.and_then(Value::as_object) else {
        return;
    };
    for (provider, value) in providers {
        if let Some(timeout) = value.get("timeout") {
            let path = format!("providers.{provider}.timeout");
            check_object(&path, timeout, TIMEOUT_FIELDS, warnings);
        }
        let models = value.get("models").and_then(Value::as_object);
        for (model, value) in models.into_iter().flatten() {
            let Some(profile) = value.get("profile") else {
                continue;
            };
            let path = format!("providers.{provider}.models.{model}.profile");
            check_object(&path, profile, FIELDS, warnings);
        }
    }
}

fn check_object(path: &str, value: &Value, fields: &[(&str, Shape)], warnings: &mut Vec<String>) {
    let Some(map) = value.as_object() else {
        warnings.push(format!("{path}: expected an object, got {value}; ignored"));
        return;
    };
    for (key, value) in map {
        let path = format!("{path}.{key}");
        match fields.iter().find(|(name, _)| name == key) {
            Some((_, shape)) => check_value(&path, value, shape, warnings),
            None => warnings.push(format!("unknown config key \"{path}\"; ignored")),
        }
    }
}

fn check_value(path: &str, value: &Value, shape: &Shape, warnings: &mut Vec<String>) {
    let expected = match shape {
        Shape::Object(fields) => return check_object(path, value, fields, warnings),
        Shape::OneOf(values) if value.as_str().is_some_and(|v| values.contains(&v)) => return,
        Shape::OneOf(values) => format!("one of {}", values.join(", ")),
        Shape::Bool if value.is_boolean() => return,
        Shape::Bool => "true or false".into(),
        Shape::Tokens if value.as_u64().is_some_and(|n| u32::try_from(n).is_ok()) => return,
        Shape::Tokens => "a token count".into(),
        Shape::Millis if value.is_u64() => return,
        Shape::Millis => "milliseconds (0 turns it off)".into(),
        Shape::Strings if is_strings(value) => return,
        Shape::Strings => "an array of strings".into(),
    };
    warnings.push(format!("{path}: expected {expected}, got {value}; ignored"));
}

fn is_strings(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_string))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn warnings(profile: Value) -> Vec<String> {
        let mut out = Vec::new();
        check(
            Some(&json!({"corp": {"models": {"coder": {"profile": profile}}}})),
            &mut out,
        );
        out
    }

    #[test]
    fn a_valid_profile_has_no_warnings() {
        let profile = json!({
            "reasoning_replay": "never", "reasoning_carrier": "reasoning_content",
            "reasoning_summary": false, "verbosity": {"supported": true, "default": "low"},
            "parallel_tool_calls": true, "edit_tool": "freeform_patch",
            "tool_output": {"tokens": 10000, "truncation": "middle"},
            "efforts": ["low", "high"], "effort_clamp": "fail",
        });
        assert_eq!(warnings(profile), Vec::<String>::new());
    }

    #[test]
    fn invalid_keys_and_values_warn_naming_the_key() {
        let found = warnings(json!({
            "reasoning_replay": "sometimes",
            "verbosity": {"default": "loud"},
            "tool_output": {"tokens": -1},
            "colour": "red",
        }));
        assert_eq!(
            found,
            [
                "providers.corp.models.coder.profile.reasoning_replay: expected one of never, tool_loop, always, got \"sometimes\"; ignored",
                "providers.corp.models.coder.profile.verbosity.default: expected one of low, medium, high, got \"loud\"; ignored",
                "providers.corp.models.coder.profile.tool_output.tokens: expected a token count, got -1; ignored",
                "unknown config key \"providers.corp.models.coder.profile.colour\"; ignored",
            ]
        );
        let mut timeouts = Vec::new();
        check(
            Some(
                &json!({"a": {"timeout": {"idle_ms": 0}}, "b": {"timeout": {"idle_ms": "5m", "connect_ms": 1}}}),
            ),
            &mut timeouts,
        );
        assert_eq!(
            timeouts,
            [
                "providers.b.timeout.idle_ms: expected milliseconds (0 turns it off), got \"5m\"; ignored",
                "unknown config key \"providers.b.timeout.connect_ms\"; ignored",
            ]
        );
        assert_eq!(
            warnings(json!("never")),
            ["providers.corp.models.coder.profile: expected an object, got \"never\"; ignored"]
        );
    }
}
