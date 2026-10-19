//! Claude Code settings beyond permissions (`compat-import` → Claude Code sources): model
//! aliases, `fallbackModel`, `env`, `apiKeyHelper` and `hooks`.
use super::adapted::Adapted;
use serde_json::{Value, json};

/// The model Claude Code's aliases select in this release. Review after an upgrade.
const ALIASES: [(&str, &str); 3] = [
    ("opus", "claude-opus-5-5"),
    ("sonnet", "claude-sonnet-5-5"),
    ("haiku", "claude-haiku-4-5"),
];

/// `anthropic/<id>` for a full Claude model id or one of the [`ALIASES`].
pub(super) fn model(name: &str) -> Option<String> {
    let id = ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map(|(_, id)| *id)
        .or_else(|| {
            name.starts_with("claude-").then_some(name).filter(|n| {
                n.len() <= 256
                    && n.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
            })
        })?;
    Some(format!("anthropic/{id}"))
}

/// `personal` says whether a raw field comes from personal settings; such `env` values
/// are withheld from the shared document.
pub(super) fn convert(raw: &Value, personal: &dyn Fn(&str) -> bool, adapted: &mut Adapted) {
    if let Some(hooks) = raw.get("hooks") {
        super::hooks::convert(hooks, "/hooks", adapted);
    }
    if let Some(fallback) = raw.get("fallbackModel") {
        match fallback.as_str().and_then(model) {
            Some(model) => {
                adapted.config["fallback_models"] = json!([model]);
                adapted.raw("/fallback_models".into(), &["/fallbackModel"]);
            }
            None => adapted.skip(
                "/fallbackModel",
                "not imported",
                "fallback model is not a Claude model or alias",
            ),
        }
    }
    if let Some(env) = raw.get("env") {
        let env = shared_env(env, personal, adapted);
        match super::codex_profiles::set(&env, "/env", adapted) {
            Some(set) if set.as_object().is_some_and(|m| !m.is_empty()) => {
                adapted.config["sandbox"] = json!({"env":{"set":set}});
                adapted.raw("/sandbox/env/set".into(), &["/env"]);
            }
            Some(_) => (),
            None => adapted.skip("/env", "not imported", "env must map names to strings"),
        }
    }
    if raw.get("apiKeyHelper").is_some() {
        adapted.skip(
            "/apiKeyHelper",
            "not imported",
            "command credentials are not supported yet; set ANTHROPIC_API_KEY instead; value withheld",
        );
    }
}

/// `env` without the variables that come from personal settings, each reported.
fn shared_env(env: &Value, personal: &dyn Fn(&str) -> bool, adapted: &mut Adapted) -> Value {
    let Some(variables) = env.as_object() else {
        return env.clone();
    };
    let mut shared = serde_json::Map::new();
    for (name, value) in variables {
        let field = super::provenance::child("/env", name);
        if personal(&field) {
            adapted.skip(
                field,
                "not imported",
                "personal settings (settings.local.json or ~/.claude) stay out of a shared document",
            );
        } else {
            shared.insert(name.clone(), value.clone());
        }
    }
    Value::Object(shared)
}

/// Native tool names of a Claude `tools`/`disallowedTools` list: a comma-separated string
/// or an array. `Err` carries the names without an equivalent.
pub(super) fn tools(value: &Value) -> Option<(Vec<String>, Vec<String>)> {
    let names: Vec<String> = match value {
        Value::String(text) => text
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
        Value::Array(items) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let mut native = Vec::new();
    let mut unknown = Vec::new();
    for name in names {
        let mapped = super::hooks::native_tool(&name)
            .map(str::to_owned)
            .or_else(|| name.starts_with("mcp__").then(|| name.clone()));
        match mapped {
            Some(tool) if !native.contains(&tool) => native.push(tool),
            Some(_) => (),
            None => unknown.push(name),
        }
    }
    Some((native, unknown))
}

/// The Claude `permissionMode` names.
pub(super) fn mode(value: &str) -> Option<&'static str> {
    Some(match value {
        "default" => "default",
        "acceptEdits" => "accept-edits",
        "plan" => "plan",
        "auto" => "auto",
        "bypassPermissions" => "bypass",
        "dontAsk" => "dont-ask",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_fallback_env_and_api_key_helper_map_or_are_reported() {
        assert_eq!(
            model("sonnet").as_deref(),
            Some("anthropic/claude-sonnet-5-5")
        );
        assert_eq!(
            model("claude-opus-5-5").as_deref(),
            Some("anthropic/claude-opus-5-5")
        );
        assert_eq!(model("opusplan"), None);
        let mut adapted = Adapted::new();
        convert(
            &json!({"fallbackModel":"haiku","env":{"CI":"1","ANTHROPIC_API_KEY":"sk-x","LOCAL":"mine"},"apiKeyHelper":"~/bin/key.sh"}),
            &|field| field == "/env/LOCAL",
            &mut adapted,
        );
        assert_eq!(
            adapted.config,
            json!({"fallback_models":["anthropic/claude-haiku-4-5"],"sandbox":{"env":{"set":{"CI":"1"}}}})
        );
        let skipped: Vec<_> = adapted.pending.iter().map(|p| p.field.as_str()).collect();
        assert_eq!(
            skipped,
            ["/env/LOCAL", "/env/ANTHROPIC_API_KEY", "/apiKeyHelper"]
        );
        assert!(!format!("{:?}", adapted.pending).contains("sk-x"));
    }
}
