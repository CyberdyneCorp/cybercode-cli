//! Basic explicit approval/sandbox migration; advanced policy forms require their own adapter.
use super::ConversionError;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Serialize)]
pub struct CodexPolicyConfig {
    pub config: Value,
    pub deprecated_untrusted: bool,
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
        _ => Err(error(
            "approval_policy",
            "approval policy requires the granular/deprecated-policy adapter",
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
    };
    if !source.contains_key("approval_policy") && !source.contains_key("sandbox_mode") {
        return Ok(result);
    }
    if source.contains_key("default_permissions") {
        return Err(error(
            "default_permissions",
            "named permissions cannot be combined with legacy sandbox conversion",
        ));
    }
    let approval = source.get("approval_policy").map(approval).transpose()?;
    let policy = source
        .get("sandbox_mode")
        .map(sandbox)
        .transpose()?
        .unwrap_or("read-only");
    result.config["sandbox"] = json!({"policy": policy});
    let mode = if policy == "full-access" {
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
