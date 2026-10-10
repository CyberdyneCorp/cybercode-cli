use super::{ConversionError, PermissionRule};
use serde_json::Value;

fn refused(field: &str, reason: &'static str) -> ConversionError {
    ConversionError {
        field: field.into(),
        reason,
    }
}

/// Convert explicit source permissions only; discovery, defaults and home expansion are separate.
/// Any unsupported item refuses the complete batch, including previously converted rules.
pub fn opencode_permissions(settings: &Value) -> Result<Vec<PermissionRule>, ConversionError> {
    let settings = settings
        .as_object()
        .ok_or_else(|| refused("settings", "expected an object"))?;
    if settings.contains_key("permission") && settings.contains_key("permissions") {
        return Err(refused(
            "permissions",
            "ambiguous singular and plural permission fields",
        ));
    }
    let mut rules = Vec::new();
    if let Some(tools) = settings.get("tools") {
        let tools = tools
            .as_object()
            .ok_or_else(|| refused("tools", "expected a boolean map"))?;
        for (index, (action, enabled)) in tools.iter().enumerate() {
            let field = format!("tools[{index}]");
            let enabled = enabled
                .as_bool()
                .ok_or_else(|| refused(&field, "expected a boolean"))?;
            rules.push(rule(
                action,
                "*",
                if enabled { "allow" } else { "deny" },
                &field,
            )?);
        }
    }
    let (name, value) = if let Some(value) = settings.get("permission") {
        ("permission", value)
    } else if let Some(value) = settings.get("permissions") {
        ("permissions", value)
    } else {
        return Ok(rules);
    };
    match value {
        Value::String(effect) => rules.push(rule("*", "*", effect, name)?),
        Value::Object(actions) => {
            for (index, (action, value)) in actions.iter().enumerate() {
                action_rules(&mut rules, action, value, &format!("{name}[{index}]"))?;
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                rules.push(ordered_rule(item, &format!("{name}[{index}]"))?);
            }
        }
        _ => {
            return Err(refused(
                name,
                "expected an effect, map or ordered rule array",
            ));
        }
    }
    Ok(rules)
}

fn action_rules(
    rules: &mut Vec<PermissionRule>,
    action: &str,
    value: &Value,
    field: &str,
) -> Result<(), ConversionError> {
    match value {
        Value::String(effect) => rules.push(rule(action, "*", effect, field)?),
        Value::Object(patterns) => {
            // Validate even an empty map so an unknown source action is not silently dropped.
            native_action(action).ok_or_else(|| refused(field, "unsupported permission action"))?;
            for (index, (pattern, effect)) in patterns.iter().enumerate() {
                let field = format!("{field}.patterns[{index}]");
                let effect = effect
                    .as_str()
                    .ok_or_else(|| refused(&field, "expected an effect"))?;
                rules.push(rule(action, pattern, effect, &field)?);
            }
        }
        _ => return Err(refused(field, "expected an effect or pattern map")),
    }
    Ok(())
}

fn ordered_rule(value: &Value, field: &str) -> Result<PermissionRule, ConversionError> {
    let item = value
        .as_object()
        .ok_or_else(|| refused(field, "expected a rule object"))?;
    if item
        .keys()
        .any(|key| !["action", "resource", "effect"].contains(&key.as_str()))
    {
        return Err(refused(field, "unsupported rule field"));
    }
    let string = |name| {
        item.get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| refused(field, "expected action, resource and effect strings"))
    };
    rule(
        string("action")?,
        string("resource")?,
        string("effect")?,
        field,
    )
}

fn rule(
    action: &str,
    resource: &str,
    effect: &str,
    field: &str,
) -> Result<PermissionRule, ConversionError> {
    let action =
        native_action(action).ok_or_else(|| refused(field, "unsupported permission action"))?;
    if !["allow", "ask", "deny"].contains(&effect) {
        return Err(refused(field, "unsupported permission effect"));
    }
    if resource.is_empty() || resource.chars().any(char::is_control) || resource.contains('\\') {
        return Err(refused(field, "unsupported resource pattern"));
    }
    if resource.starts_with('~') || resource.starts_with("$HOME") {
        return Err(refused(
            field,
            "resource requires explicit source home expansion",
        ));
    }
    let resource = if resource.ends_with(" *") {
        format!("{resource}*")
    } else {
        resource.into()
    };
    Ok(PermissionRule {
        tool: None,
        action: action.into(),
        resource,
        effect: effect.into(),
    })
}

fn native_action(action: &str) -> Option<&str> {
    match action {
        "shell" => Some("bash"),
        "write" | "patch" => Some("edit"),
        "task" | "subagent" => Some("agent"),
        "*" | "read" | "edit" | "glob" | "grep" | "bash" | "agent" | "skill" | "lsp"
        | "question" | "webfetch" | "websearch" | "external_directory" => Some(action),
        _ => None,
    }
}
