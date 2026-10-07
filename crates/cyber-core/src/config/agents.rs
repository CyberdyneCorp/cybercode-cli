//! Validate resolved agent profiles before runtime materialization.

use serde_json::{Map, Value};

use super::validate::{check_mode, check_model_ref};

pub(super) fn check(value: Option<&Value>, issues: &mut Vec<String>) {
    let Some(value) = value else { return };
    let Some(agents) = value.as_object() else {
        issues.push("agents: expected an object".into());
        return;
    };
    for (name, value) in agents {
        let path = format!("agents.{name}");
        if name == "max_concurrent" {
            positive(value, &path, issues);
            continue;
        }
        if matches!(name.as_str(), "max_depth" | "result_max_bytes") {
            require(
                value.as_u64().is_some(),
                &path,
                "a nonnegative integer",
                issues,
            );
            continue;
        }
        let Some(fields) = value.as_object() else {
            issues.push(format!("{path}: expected an agent definition object"));
            continue;
        };
        check_fields(fields, &path, issues);
    }
}

fn check_fields(fields: &Map<String, Value>, path: &str, issues: &mut Vec<String>) {
    for (name, value) in fields {
        let path = format!("{path}.{name}");
        match name.as_str() {
            "description" | "system" | "variant" | "color" => {
                require(value.is_string(), &path, "a string", issues);
            }
            "model" => check_model_ref(Some(value), &path, issues),
            "permission_mode" => check_mode(Some(value), &path, issues),
            "mode" => choice(value, &path, &["primary", "subagent", "all"], issues),
            "isolation" => choice(value, &path, &["none", "worktree"], issues),
            "memory" => choice(value, &path, &["none", "project", "user"], issues),
            "hidden" | "disabled" | "background" => {
                require(value.is_boolean(), &path, "a boolean", issues);
            }
            "steps" => positive(value, &path, issues),
            "skills" | "mcp" => strings(value, &path, issues),
            "tools" => tools(value, &path, issues),
            "request" => request(value, &path, issues),
            "permissions" if value.is_string() => {
                choice(value, &path, &["allow", "ask", "deny"], issues);
            }
            "permissions" => require(
                value.is_object() || value.is_array(),
                &path,
                "a permission effect, rules object or ordered array",
                issues,
            ),
            _ => issues.push(format!("{path}: unknown agent field")),
        }
    }
}

fn positive(value: &Value, path: &str, issues: &mut Vec<String>) {
    require(
        value.as_u64().is_some_and(|value| value > 0),
        path,
        "a positive integer",
        issues,
    );
}

fn strings(value: &Value, path: &str, issues: &mut Vec<String>) {
    require(
        value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string)),
        path,
        "an array of strings",
        issues,
    );
}

fn choice(value: &Value, path: &str, choices: &[&str], issues: &mut Vec<String>) {
    require(
        value.as_str().is_some_and(|value| choices.contains(&value)),
        path,
        &format!("one of {}", choices.join(", ")),
        issues,
    );
}

fn tools(value: &Value, path: &str, issues: &mut Vec<String>) {
    let Some(fields) = value.as_object() else {
        issues.push(format!("{path}: expected an object"));
        return;
    };
    for (name, value) in fields {
        let path = format!("{path}.{name}");
        match name.as_str() {
            "allow" | "deny" => strings(value, &path, issues),
            _ => issues.push(format!("{path}: unknown agent tools field")),
        }
    }
}

fn request(value: &Value, path: &str, issues: &mut Vec<String>) {
    let Some(fields) = value.as_object() else {
        issues.push(format!("{path}: expected an object"));
        return;
    };
    for (name, value) in fields {
        let path = format!("{path}.{name}");
        match name.as_str() {
            "body" => require(value.is_object(), &path, "an object", issues),
            "headers" => require(
                value
                    .as_object()
                    .is_some_and(|headers| headers.values().all(Value::is_string)),
                &path,
                "an object of string header values",
                issues,
            ),
            _ => issues.push(format!("{path}: unknown agent request field")),
        }
    }
}

fn require(valid: bool, path: &str, expected: &str, issues: &mut Vec<String>) {
    if !valid {
        issues.push(format!("{path}: expected {expected}"));
    }
}
