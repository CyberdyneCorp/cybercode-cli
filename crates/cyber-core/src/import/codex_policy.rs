//! Explicit approval/sandbox migration, including granular approvals; named permission
//! profiles are converted by `codex_profiles`.
use super::ConversionError;
use serde::Serialize;
use serde_json::{Map, Value, json};

#[derive(Debug, Serialize)]
pub struct CodexPolicyConfig {
    pub config: Value,
    pub deprecated_untrusted: bool,
    /// `deny` rules equivalent to the granular approvals turned off.
    pub rules: Vec<Value>,
    /// `granular.rules = false`: execpolicy `prompt` decisions are rejected, not asked.
    pub reject_rule_prompts: bool,
    /// `granular.mcp_elicitations = false`, which has no native equivalent.
    pub decline_elicitations: bool,
}

/// The categories of `approval_policy = { granular = { ... } }`. `false` rejects that
/// category's prompts; `true` (the default) asks.
#[derive(Debug)]
struct Granular {
    sandbox: bool,
    rules: bool,
    skills: bool,
    elicitations: bool,
}

fn granular(value: &Value) -> Result<Granular, ConversionError> {
    let fields = value
        .get("granular")
        .and_then(Value::as_object)
        .filter(|_| value.as_object().is_some_and(|m| m.len() == 1))
        .ok_or_else(|| {
            error(
                "approval_policy",
                "approval policy requires the deprecated-policy adapter",
            )
        })?;
    let flag = |fields: &Map<String, Value>, key: &str| -> Result<bool, ConversionError> {
        fields.get(key).map_or(Ok(true), |v| {
            v.as_bool()
                .ok_or_else(|| error("approval_policy", "granular approvals must be booleans"))
        })
    };
    if fields.keys().any(|k| {
        ![
            "sandbox_approval",
            "rules",
            "skill_approval",
            "request_permissions",
            "mcp_elicitations",
        ]
        .contains(&k.as_str())
    }) {
        return Err(error(
            "approval_policy",
            "unknown granular approval category",
        ));
    }
    Ok(Granular {
        sandbox: flag(fields, "sandbox_approval")? && flag(fields, "request_permissions")?,
        rules: flag(fields, "rules")?,
        skills: flag(fields, "skill_approval")?,
        elicitations: flag(fields, "mcp_elicitations")?,
    })
}
fn error(field: &str, reason: &'static str) -> ConversionError {
    ConversionError {
        field: field.into(),
        reason,
    }
}
fn approval(value: &Value) -> Result<&str, ConversionError> {
    match value.as_str() {
        Some(value @ ("on-request" | "never" | "untrusted")) => Ok(value),
        None if value.is_object() => Ok("granular"),
        _ => Err(error(
            "approval_policy",
            "approval policy requires the deprecated-policy adapter",
        )),
    }
}
fn sandbox(value: &Value) -> Result<&str, ConversionError> {
    match value.as_str() {
        Some("read-only") => Ok("read-only"),
        Some("workspace-write") => Ok("workspace-write"),
        Some("danger-full-access") => Ok("full-access"),
        _ => Err(error(
            "sandbox_mode",
            "unsupported or malformed sandbox mode",
        )),
    }
}
pub fn codex_policy_config(source: &Value) -> Result<CodexPolicyConfig, ConversionError> {
    let source = source
        .as_object()
        .filter(|s| s.len() <= 256)
        .ok_or_else(|| error("settings", "expected a bounded configuration object"))?;
    let mut result = CodexPolicyConfig {
        config: json!({}),
        deprecated_untrusted: false,
        rules: Vec::new(),
        reject_rule_prompts: false,
        decline_elicitations: false,
    };
    if !source.contains_key("approval_policy") && !source.contains_key("sandbox_mode") {
        return Ok(result);
    }
    let named = source.contains_key("default_permissions");
    if named && source.contains_key("sandbox_mode") {
        return Err(error(
            "default_permissions",
            "named permissions cannot be combined with legacy sandbox conversion",
        ));
    }
    let approval = source.get("approval_policy").map(approval).transpose()?;
    if approval == Some("granular") {
        apply_granular(&granular(&source["approval_policy"])?, &mut result);
    }
    let policy = source
        .get("sandbox_mode")
        .map(sandbox)
        .transpose()?
        .unwrap_or("read-only");
    if !named {
        result.config["sandbox"] = json!({"policy": policy});
    }
    let mode = if policy == "full-access" && !named {
        Some("bypass")
    } else {
        approval.map(|value| {
            if value == "never" {
                "dont-ask"
            } else {
                "default"
            }
        })
    };
    if let Some(mode) = mode {
        result.config["mode"] = json!(mode);
    }
    result.deprecated_untrusted = approval == Some("untrusted");
    Ok(result)
}

fn apply_granular(granular: &Granular, result: &mut CodexPolicyConfig) {
    let deny = |action: &str| json!({"action":action,"resource":"*","effect":"deny"});
    if !granular.sandbox {
        result.rules.push(deny("sandbox.escalate"));
    }
    if !granular.skills {
        result.rules.push(deny("skill"));
    }
    result.reject_rule_prompts = !granular.rules;
    result.decline_elicitations = !granular.elicitations;
}
