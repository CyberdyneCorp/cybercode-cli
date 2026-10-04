//! Validation of the merged document and secret redaction for display.

use serde_json::{Map, Value};

use super::merge::Sources;

const KNOWN_KEYS: &[&str] = &[
    "$schema",
    "model",
    "small_model",
    "model_roles",
    "fallback_models",
    "providers",
    "agents",
    "default_agent",
    "mode",
    "permissions",
    "sandbox",
    "hooks",
    "plugins",
    "mcp",
    "skills",
    "commands",
    "instructions",
    "references",
    "memory",
    "compaction",
    "snapshots",
    "worktrees",
    "lsp",
    "formatters",
    "tool_output",
    "workflows",
    "goals",
    "loops",
    "messaging",
    "peers",
    "remote",
    "runners",
    "channels",
    "account",
    "share",
    "telemetry",
    "budgets",
    "policy",
    "profiles",
    "default_profile",
    "shell",
    "attachments",
    "background",
    "tools",
    "git",
    "ide",
    "autofix",
    "network",
    "storage",
    "compat",
    "services",
    "features",
    "experimental",
    "autoupdate",
];

/// Keys that belong in `tui.jsonc`.
const TUI_KEYS: &[&str] = &[
    "theme",
    "keybinds",
    "statusline",
    "scroll",
    "vim_mode",
    "notifications",
    "diff_style",
];

const MODES: &[&str] = &[
    "default",
    "accept-edits",
    "plan",
    "auto",
    "dont-ask",
    "bypass",
];

/// Validate the merged document. Returns hard issues; pushes warnings; drops TUI keys.
pub fn validate(
    value: &mut Value,
    sources: &mut Sources,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut issues = Vec::new();
    let Some(map) = value.as_object_mut() else {
        return vec!["the configuration document must be a JSON object".into()];
    };
    drop_tui_keys(map, sources, warnings);
    for key in map.keys() {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            warnings.push(format!("unknown config key \"{key}\""));
        }
    }
    check_mode(map.get("mode"), "mode", &mut issues);
    for key in ["model", "small_model"] {
        check_model_ref(map.get(key), key, &mut issues);
    }
    if let Some(Value::Object(profiles)) = map.get("profiles") {
        check_profiles(profiles, &mut issues);
    }
    issues
}

fn drop_tui_keys(map: &mut Map<String, Value>, sources: &mut Sources, warnings: &mut Vec<String>) {
    for key in TUI_KEYS {
        if map.remove(*key).is_some() {
            warnings.push(format!("{key} belongs in tui.jsonc"));
            let prefix = format!("/{key}");
            sources.retain(|k, _| k != &prefix && !k.starts_with(&format!("{prefix}/")));
        }
    }
}

fn check_mode(value: Option<&Value>, path: &str, issues: &mut Vec<String>) {
    match value {
        None => {}
        Some(Value::String(m)) if MODES.contains(&m.as_str()) => {}
        Some(other) => issues.push(format!(
            "{path}: expected one of {}, got {other}",
            MODES.join(", ")
        )),
    }
}

fn check_model_ref(value: Option<&Value>, path: &str, issues: &mut Vec<String>) {
    match value {
        None => {}
        Some(Value::String(r)) if is_model_ref(r) => {}
        Some(other) => issues.push(format!(
            "{path}: expected provider/model[#variant], got {other}"
        )),
    }
}

fn is_model_ref(r: &str) -> bool {
    r.split_once('/')
        .is_some_and(|(provider, model)| !provider.is_empty() && !model.is_empty())
}

fn check_profiles(profiles: &Map<String, Value>, issues: &mut Vec<String>) {
    for (name, profile) in profiles {
        for forbidden in ["profiles", "policy"] {
            if profile.get(forbidden).is_some() {
                issues.push(format!("profiles.{name}.{forbidden} is not allowed"));
            }
        }
        check_mode(
            profile.get("mode"),
            &format!("profiles.{name}.mode"),
            issues,
        );
    }
}

const SECRET_KEYS: &[&str] = &[
    "api_key",
    "token",
    "secret",
    "password",
    "authorization",
    "cookie",
];

/// Replace secret values with `***` (`configuration` → Secrets handling).
pub fn redact_secrets(value: &Value) -> Value {
    redact(value, false)
}

fn redact(value: &Value, force: bool) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let secret = force || is_secret_key(k);
                    let child = if secret && !v.is_object() {
                        Value::String("***".into())
                    } else {
                        redact(v, secret || k == "headers")
                    };
                    (k.clone(), child)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| redact(v, force)).collect()),
        Value::String(_) if force => Value::String("***".into()),
        other => other.clone(),
    }
}

fn is_secret_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SECRET_KEYS.iter().any(|s| lower.contains(s))
}
