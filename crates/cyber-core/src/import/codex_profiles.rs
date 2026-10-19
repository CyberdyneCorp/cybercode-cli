//! Codex profiles, named permission profiles, `shell_environment_policy` and `notify`
//! (`compat-import` → Codex sources).
//!
//! - `[profiles.<name>]` and `<name>.config.toml` → `profiles.<name>` (`model`, `mode`,
//!   `sandbox.policy`); `profile` → `default_profile`.
//! - `[permissions.<name>]` → `sandbox.profiles.<name>`; `default_permissions` →
//!   `sandbox.profile`. A profile that cannot be converted is never dropped silently: a
//!   selection of it selects `read-only` instead.
//! - `[shell_environment_policy]` → `sandbox.env`.
//! - `notify` → a `Notification` command hook.
use super::adapted::Adapted;
use super::provenance::child;
use serde_json::{Map, Value, json};

const PROFILE_KEYS: [&str; 4] = ["model", "model_provider", "approval_policy", "sandbox_mode"];

/// The adapters over the merged Codex config document.
pub(super) fn convert(raw: &Value, adapted: &mut Adapted) {
    if let Some(profiles) = raw.get("profiles") {
        profile_tables(profiles, adapted);
    }
    if let Some(name) = raw.get("profile") {
        match name.as_str().filter(|n| valid_name(n)) {
            Some(name) => {
                adapted.config["default_profile"] = json!(name);
                adapted.raw("/default_profile".into(), &["/profile"]);
            }
            None => adapted.skip("/profile", "not imported", "profile must name a profile"),
        }
    }
    let converted = raw
        .get("permissions")
        .map(|p| permission_profiles(p, adapted))
        .unwrap_or_default();
    if let Some(selected) = raw.get("default_permissions") {
        default_permissions(selected, &converted, adapted);
    }
    if let Some(policy) = raw.get("shell_environment_policy") {
        environment(policy, adapted);
    }
    if let Some(argv) = raw.get("notify") {
        super::hooks::notify(argv, adapted);
    }
}

/// A `<name>.config.toml` profile file, whose whole document is the profile.
pub(super) fn profile_file(name: &str, document: &Value, adapted: &mut Adapted) {
    if let Some(profile) = profile(name, document, "", adapted) {
        place(adapted, "profiles", name, profile);
        adapted.raw(child("/profiles", name), &[""]);
    }
}

/// `requirements.toml` is admin-owned policy and is never written; each key is reported
/// with the org-policy setting that expresses it.
pub(super) fn requirements(document: &Value, adapted: &mut Adapted) {
    for key in document.as_object().into_iter().flat_map(|m| m.keys()) {
        let reason = match key.as_str() {
            "allowed_approval_policies" | "allowed_sandbox_modes" => {
                "policy file; the org-policy equivalent is modes.disable (and modes.max)"
            }
            "allowed_permission_profiles" | "default_permissions" => {
                "policy file; the org-policy equivalent is sandbox.min_profile"
            }
            "experimental_network" => {
                "policy file; the org-policy equivalent is sandbox.network.allow_domains"
            }
            "mcp_servers" => "policy file; the org-policy equivalent is mcp.allow / mcp.deny",
            "allow_managed_hooks_only" => {
                "policy file; the org-policy equivalent is hooks.allow_user_hooks = false"
            }
            "features" => "policy file; the org-policy equivalent is the features switches",
            "permissions" | "rules" => {
                "policy file; the org-policy equivalent is permissions.deny / permissions.ask"
            }
            _ => "policy file; no org-policy equivalent",
        };
        adapted.skip(child("", key), "not imported", reason);
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}

fn place(adapted: &mut Adapted, key: &str, name: &str, value: Value) {
    if !adapted.config.get(key).is_some_and(Value::is_object) {
        adapted.config[key] = json!({});
    }
    adapted.config[key][name] = value;
}

fn profile_tables(profiles: &Value, adapted: &mut Adapted) {
    let Some(profiles) = profiles.as_object().filter(|p| p.len() <= 64) else {
        adapted.skip("/profiles", "not imported", "expected at most 64 profiles");
        return;
    };
    for (name, table) in profiles {
        let raw = child("/profiles", name);
        if let Some(profile) = profile(name, table, &raw, adapted) {
            place(adapted, "profiles", name, profile);
            adapted.raw(raw.clone(), &[raw.as_str()]);
        }
    }
}

/// One Codex profile as a native profile overlay; `raw` is its pointer in the source.
fn profile(name: &str, table: &Value, raw: &str, adapted: &mut Adapted) -> Option<Value> {
    let Some(fields) = table.as_object().filter(|_| valid_name(name)) else {
        adapted.skip(
            raw,
            "not imported",
            "profile must be a table with a simple name",
        );
        return None;
    };
    for key in fields
        .keys()
        .filter(|k| !PROFILE_KEYS.contains(&k.as_str()))
    {
        adapted.skip(
            child(raw, key),
            "not imported",
            "profile field has no Cyber Code equivalent",
        );
    }
    let mut native = json!({});
    if let Some(model) = fields.get("model") {
        let provider = fields
            .get("model_provider")
            .map_or(Some("openai"), Value::as_str);
        match (model.as_str(), provider) {
            (Some(model), Some(provider)) if model_part(model) && valid_name(provider) => {
                native["model"] = json!(format!("{provider}/{model}"));
            }
            _ => adapted.skip(
                child(raw, "model"),
                "not imported",
                "profile model needs manual conversion",
            ),
        }
    }
    let policy = Value::Object(
        fields
            .iter()
            .filter(|(k, _)| ["approval_policy", "sandbox_mode"].contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    match super::codex_policy_config(&policy) {
        Ok(policy) if policy.rules.is_empty() && !policy.reject_rule_prompts => {
            for (key, value) in policy.config.as_object().into_iter().flatten() {
                native[key] = value.clone();
            }
        }
        _ => {
            adapted.skip(
                raw,
                "not imported",
                "profile approval/sandbox needs manual conversion",
            );
            return None;
        }
    }
    Some(native)
}

fn model_part(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 256
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}

/// Native name of a Codex built-in permission profile.
fn builtin(name: &str) -> Option<&'static str> {
    match name {
        ":read-only" => Some("read-only"),
        ":workspace" => Some("workspace"),
        ":danger-full-access" => Some("full"),
        _ => None,
    }
}

/// The names of the profiles converted.
fn permission_profiles(profiles: &Value, adapted: &mut Adapted) -> Vec<String> {
    let mut converted = Vec::new();
    let Some(profiles) = profiles.as_object().filter(|p| p.len() <= 64) else {
        adapted.skip(
            "/permissions",
            "not imported",
            "expected at most 64 permission profiles",
        );
        return converted;
    };
    for (name, definition) in profiles {
        let raw = child("/permissions", name);
        match permission_profile(definition, &raw, adapted) {
            Some(profile) if valid_name(name) => {
                place_sandbox_profile(adapted, name, profile);
                adapted.raw(child("/sandbox/profiles", name), &[raw.as_str()]);
                converted.push(name.clone());
            }
            _ => adapted.skip(
                raw,
                "not imported",
                "permission profile needs manual conversion",
            ),
        }
    }
    converted
}

fn place_sandbox_profile(adapted: &mut Adapted, name: &str, profile: Value) {
    if !adapted.config.get("sandbox").is_some_and(Value::is_object) {
        adapted.config["sandbox"] = json!({});
    }
    if !adapted.config["sandbox"]
        .get("profiles")
        .is_some_and(Value::is_object)
    {
        adapted.config["sandbox"]["profiles"] = json!({});
    }
    adapted.config["sandbox"]["profiles"][name] = profile;
}

/// `{ description?, extends?, filesystem?, network? }`; `None` for any other shape.
fn permission_profile(definition: &Value, raw: &str, adapted: &mut Adapted) -> Option<Value> {
    let fields = definition.as_object()?;
    let mut native = json!({});
    for (key, value) in fields {
        match key.as_str() {
            "description" => adapted.skip(
                child(raw, key),
                "not imported",
                "profile descriptions are not kept",
            ),
            "extends" => {
                let parent = value.as_str()?;
                native["extends"] = json!(builtin(parent).unwrap_or(parent));
            }
            "filesystem" => filesystem(value, &mut native)?,
            "network" => native["network"] = network(value)?,
            _ => return None,
        }
    }
    if native.get("extends").is_none() {
        native["policy"] = json!("read-only");
    }
    Some(native)
}

fn filesystem(value: &Value, native: &mut Value) -> Option<()> {
    for (path, access) in value.as_object()? {
        if path.starts_with(':') || path.is_empty() {
            return None;
        }
        let list = match access.as_str()? {
            "write" => "writable_roots",
            "read" => "readable_paths",
            "none" | "deny" => "deny_read",
            _ => return None,
        };
        if !native.get(list).is_some_and(Value::is_array) {
            native[list] = json!([]);
        }
        native[list].as_array_mut()?.push(json!(path));
    }
    Some(())
}

/// `enabled = false` → `off`; `enabled = true` → `proxy` (the allowlisted network, which
/// asks for unlisted hosts); `domains` → allowed and denied domains.
fn network(value: &Value) -> Option<Value> {
    let mut native = json!({});
    for (key, value) in value.as_object()? {
        match key.as_str() {
            "enabled" => native["mode"] = json!(if value.as_bool()? { "proxy" } else { "off" }),
            "domains" => {
                for (host, decision) in value.as_object()? {
                    let list = match decision.as_str()? {
                        "allow" => "allowed_domains",
                        "deny" => "denied_domains",
                        _ => return None,
                    };
                    if !native.get(list).is_some_and(Value::is_array) {
                        native[list] = json!([]);
                    }
                    native[list].as_array_mut()?.push(json!(host));
                }
            }
            _ => return None,
        }
    }
    Some(native)
}

fn default_permissions(selected: &Value, converted: &[String], adapted: &mut Adapted) {
    let name = selected.as_str().unwrap_or_default();
    let native = builtin(name)
        .map(str::to_owned)
        .or_else(|| converted.iter().find(|c| *c == name).cloned());
    let native = native.unwrap_or_else(|| {
        adapted.skip(
            "/default_permissions",
            "not imported",
            "the selected permission profile was not converted; read-only is selected instead",
        );
        "read-only".into()
    });
    if !adapted.config.get("sandbox").is_some_and(Value::is_object) {
        adapted.config["sandbox"] = json!({});
    }
    adapted.config["sandbox"]["profile"] = json!(native);
    adapted.raw("/sandbox/profile".into(), &["/default_permissions"]);
}

/// `[shell_environment_policy]` → `sandbox.env`. `core` keeps only base variables, like
/// `none`. Credential masking cannot be turned off, so `ignore_default_excludes` is
/// reported; `include_only` and `filters` narrow the environment further than
/// `sandbox.env` can, so a policy using them is reported whole.
fn environment(policy: &Value, adapted: &mut Adapted) {
    let raw = "/shell_environment_policy";
    let Some(fields) = policy.as_object() else {
        adapted.skip(
            raw,
            "not imported",
            "shell_environment_policy must be a table",
        );
        return;
    };
    if fields.contains_key("include_only") || fields.contains_key("filters") {
        adapted.skip(
            raw,
            "not imported",
            "include_only/filters narrow the environment further than sandbox.env; port by hand",
        );
        return;
    }
    let mut env = Map::new();
    for (key, value) in fields {
        let field = child(raw, key);
        let mapped = match key.as_str() {
            "inherit" => inherit(value),
            "exclude" => value
                .as_array()
                .filter(|a| a.iter().all(Value::is_string))
                .map(|a| ("deny", json!(a))),
            "set" => set(value, &field, adapted).map(|set| ("set", set)),
            "experimental_use_profile" => value.as_bool().map(|b| ("login_shell", json!(b))),
            "ignore_default_excludes" if value.as_bool() == Some(false) => continue,
            "ignore_default_excludes" => {
                adapted.skip(
                    field,
                    "not imported",
                    "credential variables are always masked",
                );
                continue;
            }
            _ => None,
        };
        match mapped {
            Some((native, value)) => {
                adapted.raw(format!("/sandbox/env/{native}"), &[field.as_str()]);
                env.insert(native.into(), value);
            }
            None => adapted.skip(
                field,
                "not imported",
                "environment policy field needs manual conversion",
            ),
        }
    }
    if !env.is_empty() {
        if !adapted.config.get("sandbox").is_some_and(Value::is_object) {
            adapted.config["sandbox"] = json!({});
        }
        adapted.config["sandbox"]["env"] = Value::Object(env);
    }
}

fn inherit(value: &Value) -> Option<(&'static str, Value)> {
    match value.as_str()? {
        "all" => Some(("mode", json!("inherit"))),
        "core" | "none" => Some(("mode", json!("none"))),
        _ => None,
    }
}

/// Plain variables; a credential-named one is withheld and reported.
pub(super) fn set(value: &Value, raw: &str, adapted: &mut Adapted) -> Option<Value> {
    let mut out = Map::new();
    for (name, value) in value.as_object()? {
        let value = value.as_str()?;
        if credential_name(name) {
            adapted.skip(
                child(raw, name),
                "not imported",
                "credential variable withheld; set it in the environment instead",
            );
        } else if !value.contains(['{', '}']) {
            out.insert(name.clone(), json!(value));
        } else {
            adapted.skip(
                child(raw, name),
                "not imported",
                "variable value holds a placeholder",
            );
        }
    }
    Some(Value::Object(out))
}

fn credential_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "key",
        "token",
        "secret",
        "password",
        "auth",
        "cookie",
        "credential",
    ]
    .iter()
    .any(|part| lower.contains(part))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_permissions_environment_and_notify_map_to_native_settings() {
        let mut adapted = Adapted::new();
        convert(
            &json!({
                "profile":"fast",
                "profiles":{"fast":{"model":"o4-mini","approval_policy":"never","sandbox_mode":"workspace-write","model_reasoning_effort":"low"}},
                "default_permissions":"build",
                "permissions":{
                    "build":{"extends":":workspace","filesystem":{"/opt/cache":"write","secrets/**":"none"},"network":{"enabled":false}},
                    "odd":{"filesystem":{":project_roots":"write"}}
                },
                "shell_environment_policy":{"inherit":"core","exclude":["AWS_*"],"set":{"CI":"1","GH_TOKEN":"x"},"ignore_default_excludes":true},
                "notify":["notify-send","done"]
            }),
            &mut adapted,
        );
        assert_eq!(adapted.config["default_profile"], "fast");
        assert_eq!(
            adapted.config["profiles"]["fast"],
            json!({"model":"openai/o4-mini","mode":"dont-ask","sandbox":{"policy":"workspace-write"}})
        );
        assert_eq!(
            adapted.config["sandbox"]["profiles"],
            json!({"build":{"extends":"workspace","writable_roots":["/opt/cache"],"deny_read":["secrets/**"],"network":{"mode":"off"}}})
        );
        assert_eq!(adapted.config["sandbox"]["profile"], "build");
        assert_eq!(
            adapted.config["sandbox"]["env"],
            json!({"mode":"none","deny":["AWS_*"],"set":{"CI":"1"}})
        );
        assert!(adapted.config["hooks"]["Notification"].is_array());
        let skipped: Vec<_> = adapted.pending.iter().map(|p| p.field.as_str()).collect();
        assert_eq!(
            skipped,
            [
                "/profiles/fast/model_reasoning_effort",
                "/permissions/odd",
                "/shell_environment_policy/set/GH_TOKEN",
                "/shell_environment_policy/ignore_default_excludes",
            ]
        );
    }

    #[test]
    fn an_unconverted_selected_profile_falls_back_to_read_only() {
        let mut adapted = Adapted::new();
        convert(
            &json!({"default_permissions":"odd","permissions":{"odd":{"unknown":true}}}),
            &mut adapted,
        );
        assert_eq!(adapted.config["sandbox"]["profile"], "read-only");
        assert!(adapted.config["sandbox"].get("profiles").is_none());
        let mut adapted = Adapted::new();
        convert(
            &json!({"shell_environment_policy":{"inherit":"all","include_only":["PATH"]}}),
            &mut adapted,
        );
        assert!(adapted.config.get("sandbox").is_none());
        assert_eq!(adapted.pending[0].field, "/shell_environment_policy");
    }
}
