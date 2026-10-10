use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PermissionRule {
    pub action: String,
    pub resource: String,
    pub effect: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaudePermissions {
    pub rules: Vec<PermissionRule>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {reason}")]
pub struct ConversionError {
    pub field: String,
    pub reason: &'static str,
}
fn refused(field: impl Into<String>, reason: &'static str) -> ConversionError {
    ConversionError {
        field: field.into(),
        reason,
    }
}

/// Convert only permission arrays and defaultMode; other source fields need separate adapters.
/// A rejected item refuses the entire batch rather than dropping a potentially protective deny.
pub fn claude_permissions(settings: &Value) -> Result<ClaudePermissions, ConversionError> {
    let settings = settings
        .as_object()
        .ok_or_else(|| refused("settings", "expected an object"))?;
    let Some(permissions) = settings.get("permissions") else {
        return Ok(ClaudePermissions {
            rules: Vec::new(),
            mode: None,
        });
    };
    let permissions = permissions
        .as_object()
        .ok_or_else(|| refused("permissions", "expected an object"))?;
    let mut rules = Vec::new();
    for effect in ["allow", "ask", "deny"] {
        let Some(group) = permissions.get(effect) else {
            continue;
        };
        let field = format!("permissions.{effect}");
        let group = group
            .as_array()
            .ok_or_else(|| refused(&field, "expected a string array"))?;
        for (index, item) in group.iter().enumerate() {
            let field = format!("{field}[{index}]");
            let text = item
                .as_str()
                .ok_or_else(|| refused(&field, "expected a string selector"))?;
            rules.push(selector(text, effect, &field)?);
        }
    }
    let mode = permissions.get("defaultMode").map(mode).transpose()?;
    Ok(ClaudePermissions { rules, mode })
}

fn mode(value: &Value) -> Result<String, ConversionError> {
    let value = value
        .as_str()
        .ok_or_else(|| refused("permissions.defaultMode", "expected a string mode"))?;
    let mode = match value {
        "default" => "default",
        "acceptEdits" => "accept-edits",
        "plan" => "plan",
        "auto" => "auto",
        "bypassPermissions" => "bypass",
        "dontAsk" => "dont-ask",
        _ => {
            return Err(refused(
                "permissions.defaultMode",
                "mode requires manual conversion",
            ));
        }
    };
    Ok(mode.into())
}

fn selector(text: &str, effect: &str, field: &str) -> Result<PermissionRule, ConversionError> {
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err(refused(field, "empty or control-containing selector"));
    }
    let (tool, resource) = match text.split_once('(') {
        Some((tool, rest)) => {
            let resource = rest
                .strip_suffix(')')
                .filter(|value| !value.is_empty())
                .ok_or_else(|| refused(field, "malformed selector"))?;
            (tool, resource)
        }
        None if !text.contains(')') => (text, "*"),
        None => return Err(refused(field, "malformed selector")),
    };
    let action = action(tool).ok_or_else(|| refused(field, "tool requires manual conversion"))?;
    let resource = if action == "bash" {
        match resource.strip_suffix(":*") {
            Some(prefix) if !prefix.is_empty() => format!("{prefix} *"),
            Some(_) => return Err(refused(field, "empty command prefix")),
            None => resource.to_owned(),
        }
    } else {
        resource.strip_prefix("./").unwrap_or(resource).to_owned()
    };
    if resource.is_empty() {
        return Err(refused(field, "empty resource"));
    }
    Ok(PermissionRule {
        action: action.into(),
        resource,
        effect: effect.into(),
    })
}

fn action(tool: &str) -> Option<&'static str> {
    match tool {
        "Bash" => Some("bash"),
        "Read" => Some("read"),
        "Glob" => Some("glob"),
        "Grep" => Some("grep"),
        "Skill" => Some("skill"),
        "Task" | "Agent" => Some("agent"),
        _ => None,
    }
}
