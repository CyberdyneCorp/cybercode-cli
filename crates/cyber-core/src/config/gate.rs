//! Classification of security-sensitive definitions in project-controlled config
//! (`workspace-trust` → Trust before interpretation).
//!
//! Sensitive definitions stay inactive until the checkout approves their digest:
//! executable integrations, provider and network endpoints, permission or Mode widening,
//! and any substitution that reads host files or environment variables.

use serde_json::{Map, Value};

/// Top-level keys that configure executable integrations, endpoints or boundaries.
const SENSITIVE_KEYS: &[&str] = &[
    "budgets",
    "providers",
    "mcp",
    "browser",
    "plugins",
    "hooks",
    "formatters",
    "lsp",
    "shell",
    "channels",
    "references",
    "sandbox",
    "network",
    "account",
    "remote",
    "runners",
    "services",
    "telemetry",
    "share",
    "peers",
    "fallback_models",
];

/// Modes more permissive than `default`.
const WIDE_MODES: &[&str] = &["accept-edits", "auto", "dont-ask", "bypass"];

#[derive(Debug, Clone, PartialEq)]
pub struct SensitiveSplit {
    /// The layer with sensitive definitions removed.
    pub safe: Value,
    /// Only the sensitive definitions, in the same shape.
    pub sensitive: Value,
    /// JSON pointers of the removed definitions.
    pub pointers: Vec<String>,
}

enum Class {
    Safe,
    Sensitive,
    Recurse,
}

pub fn split_sensitive(layer: &Value) -> SensitiveSplit {
    let mut found = Vec::new();
    collect(layer, &mut Vec::new(), &mut found);
    let mut safe = layer.clone();
    let mut sensitive = Value::Object(Map::new());
    for path in &found {
        if let Some(value) = remove_path(&mut safe, path) {
            insert_path(&mut sensitive, path, value);
        }
    }
    SensitiveSplit {
        safe,
        sensitive,
        pointers: found.iter().map(|p| pointer(p)).collect(),
    }
}

fn collect(value: &Value, path: &mut Vec<String>, found: &mut Vec<Vec<String>>) {
    let Some(map) = value.as_object() else { return };
    for (key, child) in map {
        path.push(key.clone());
        match classify(path, child) {
            Class::Sensitive => found.push(path.clone()),
            Class::Recurse => collect(child, path, found),
            Class::Safe => {}
        }
        path.pop();
    }
}

fn classify(path: &[String], value: &Value) -> Class {
    let parts: Vec<&str> = path.iter().map(String::as_str).collect();
    // A profile can set any key, so classify what it contains as if it were top level.
    if let ["profiles", _, rest @ ..] = parts.as_slice() {
        if rest.is_empty() {
            return Class::Recurse;
        }
        let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
        return classify(&rest, value);
    }
    match parts.as_slice() {
        [key] if SENSITIVE_KEYS.contains(key) => Class::Sensitive,
        ["mode"] | ["agents", _, "permission_mode"] => sensitive_if(widens_mode(value)),
        ["permissions"] | ["agents", _, "permissions"] => {
            sensitive_if(grants_permission(value) || value.get("auto_mode").is_some())
        }
        // An agent's sandbox profile is a boundary, as `sandbox` is.
        ["worktrees", "setup"] | ["agents", _, "request" | "sandbox"] => Class::Sensitive,
        ["messaging", rest @ ..] => messaging_class(rest, value),
        _ if value.is_object() => Class::Recurse,
        _ => sensitive_if(has_placeholder_deep(value)),
    }
}

/// `messaging` widens the inbound boundary of the checkout's Sessions unless it only
/// restricts: `enabled: false`, `inbound.default: "refuse"`, an empty `inbound.deliver`
/// and `inbound.refuse` (which concatenates, so it only adds refusals). `inbound.hold`
/// replaces a list (and can turn a refusal into a hold) and `max_hops` may raise the
/// limit, so both are sensitive, as is anything unknown.
fn messaging_class(rest: &[&str], value: &Value) -> Class {
    let restricts = match rest {
        [] | ["inbound"] if value.is_object() => return Class::Recurse,
        ["enabled"] => value == &Value::Bool(false),
        ["inbound", "default"] => value.as_str() == Some("refuse"),
        ["inbound", "deliver"] => value.as_array().is_some_and(Vec::is_empty),
        ["inbound", "refuse"] => value.is_array() && !has_placeholder_deep(value),
        _ => false,
    };
    sensitive_if(!restricts)
}

fn sensitive_if(condition: bool) -> Class {
    if condition {
        Class::Sensitive
    } else {
        Class::Safe
    }
}

fn widens_mode(value: &Value) -> bool {
    value.as_str().is_some_and(|m| WIDE_MODES.contains(&m))
}

/// Whether a `permissions` value grants an `allow`. `untrusted_content` holds settings,
/// not rules, and a project layer may only tighten them (its `bypass: "allow"` is ignored
/// unless it changes nothing), so it never makes the section sensitive.
fn grants_permission(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| key != "untrusted_content" && grants_allow(value)),
        other => grants_allow(other),
    }
}

fn grants_allow(value: &Value) -> bool {
    match value {
        Value::String(s) => s == "allow",
        Value::Array(items) => items.iter().any(grants_allow),
        Value::Object(map) => map.values().any(grants_allow),
        _ => false,
    }
}

fn has_placeholder_deep(value: &Value) -> bool {
    match value {
        Value::String(s) => super::subst::has_placeholder(s),
        Value::Array(items) => items.iter().any(has_placeholder_deep),
        Value::Object(map) => map.values().any(has_placeholder_deep),
        _ => false,
    }
}

fn remove_path(root: &mut Value, path: &[String]) -> Option<Value> {
    let (last, parents) = path.split_last()?;
    let mut node = root;
    for key in parents {
        node = node.get_mut(key)?;
    }
    node.as_object_mut()?.remove(last)
}

pub(super) fn insert_path(root: &mut Value, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut node = root;
    for key in parents {
        let Some(map) = node.as_object_mut() else {
            return;
        };
        node = map
            .entry(key.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if let Some(map) = node.as_object_mut() {
        map.insert(last.clone(), value);
    }
}

fn pointer(path: &[String]) -> String {
    path.iter().fold(String::new(), |acc, key| {
        super::merge::child_pointer(&acc, key)
    })
}

/// Serialize with object keys sorted recursively, for stable digests.
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        Value::String((*k).clone()),
                        canonical_json(&map[*k])
                    )
                })
                .collect();
            format!("{{{}}}", body.join(","))
        }
        Value::Array(items) => {
            let body: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", body.join(","))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_integrations_widening_and_placeholders() {
        let layer = json!({
            "model": "openai/gpt-6",
            "mode": "bypass",
            "mcp": {"db": {"type": "local", "command": "x"}},
            "permissions": {"bash": {"*": "ask", "rm *": "deny"}},
            "instructions": ["{file:~/.ssh/id_rsa}"],
            "agents": {"docs": {"description": "d", "permission_mode": "plan",
                                 "permissions": {"edit": "allow"}, "sandbox": "workspace"}},
            "worktrees": {"root": "../wt", "setup": ["npm ci"]}
        });
        let split = split_sensitive(&layer);
        assert_eq!(
            split.pointers,
            vec![
                "/mode",
                "/mcp",
                "/instructions",
                "/agents/docs/permissions",
                "/agents/docs/sandbox",
                "/worktrees/setup"
            ]
        );
        assert_eq!(split.safe["model"], "openai/gpt-6");
        assert_eq!(split.safe["permissions"]["bash"]["rm *"], "deny");
        assert_eq!(split.safe["agents"]["docs"]["permission_mode"], "plan");
        assert_eq!(split.safe["worktrees"]["root"], "../wt");
        assert!(split.safe.get("mcp").is_none());
    }

    #[test]
    fn messaging_is_sensitive_unless_it_only_restricts() {
        let restricting = json!({"messaging": {"enabled": false,
            "inbound": {"default": "refuse", "deliver": [], "refuse": ["mcp:*"]}}});
        let split = split_sensitive(&restricting);
        assert!(split.pointers.is_empty(), "{:?}", split.pointers);
        let widening = json!({"messaging": {"enabled": true, "max_hops": 64,
            "inbound": {"default": "deliver", "deliver": ["*"], "hold": [],
                        "refuse": "{env:REFUSE}"}}});
        let split = split_sensitive(&widening);
        let mut pointers = split.pointers.clone();
        pointers.sort();
        assert_eq!(
            pointers,
            [
                "/messaging/enabled",
                "/messaging/inbound/default",
                "/messaging/inbound/deliver",
                "/messaging/inbound/hold",
                "/messaging/inbound/refuse",
                "/messaging/max_hops",
            ]
        );
        assert_eq!(split.safe["messaging"]["inbound"], json!({}));
    }

    #[test]
    fn canonical_json_sorts_keys() {
        assert_eq!(
            canonical_json(&json!({"b": 1, "a": [true, {"d": 1, "c": 2}]})),
            r#"{"a":[true,{"c":2,"d":1}],"b":1}"#
        );
    }
}
