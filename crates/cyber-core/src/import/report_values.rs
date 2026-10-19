//! Credential-safe normalized comparisons for kept migration fields.
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize)]
pub struct ValueComparison {
    pub kept: Value,
    pub ignored: Value,
}

pub(super) fn redact(comparison: &mut ValueComparison, pointer: &str, secrets: &[String]) {
    comparison.kept = contextual(&comparison.kept, pointer);
    comparison.ignored = contextual(&comparison.ignored, pointer);
    // Replace longer tokens first so a short token cannot expose a longer suffix.
    let mut secrets: Vec<_> = secrets
        .iter()
        .filter(|s| !s.is_empty())
        .map(String::as_str)
        .collect();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    scrub(&mut comparison.kept, &secrets);
    scrub(&mut comparison.ignored, &secrets);
}
fn contextual(value: &Value, pointer: &str) -> Value {
    let mut wrapped = value.clone();
    for component in pointer
        .split('/')
        .skip(1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let mut map = Map::new();
        map.insert(component.replace("~1", "/").replace("~0", "~"), wrapped);
        wrapped = Value::Object(map);
    }
    crate::config::redact_secrets(&wrapped)
        .pointer(pointer)
        .cloned()
        .unwrap_or(Value::Null)
}
fn scrub(value: &mut Value, secrets: &[&str]) {
    match value {
        Value::Object(map) => {
            let mut redacted = Map::new();
            for (mut key, mut value) in std::mem::take(map) {
                scrub(&mut value, secrets);
                scrub_text(&mut key, secrets);
                redacted.insert(key, value);
            }
            *map = redacted;
        }
        Value::Array(items) => {
            for value in items {
                scrub(value, secrets);
            }
        }
        Value::String(text) => scrub_text(text, secrets),
        _ => (),
    }
}
fn scrub_text(text: &mut String, secrets: &[&str]) {
    for secret in secrets {
        *text = text.replace(secret, "***");
    }
}

/// The mapping report written beside an import (`compat-import` → Mapping report). Every
/// field and value in it was redacted with the preview, and paths are escaped onto one
/// line.
pub(super) fn markdown(title: &str, output: &super::PreviewOutput) -> String {
    let mut text = format!(
        "# {title}\n\nTarget: `{}`\n",
        escaped(&output.target.display().to_string())
    );
    let statuses = ["imported", "merged", "requires manual port", "not imported"];
    for status in statuses {
        section(
            &mut text,
            status,
            output.report.iter().filter(|r| r.status == status),
        );
    }
    section(
        &mut text,
        "read",
        output
            .report
            .iter()
            .filter(|r| !statuses.contains(&r.status)),
    );
    if !output.required_environment.is_empty() {
        text.push_str("\n## Environment to set\n\n");
        for required in &output.required_environment {
            let sources: Vec<_> = required
                .sources
                .iter()
                .map(|s| {
                    format!(
                        "`{}` `{}`",
                        escaped(&s.source.display().to_string()),
                        escaped(&s.field)
                    )
                })
                .collect();
            text.push_str(&format!(
                "- `{}` from the source field `{}` ({})\n",
                required.requirement.variable,
                escaped(&required.requirement.field),
                sources.join(", ")
            ));
        }
    }
    text
}

fn section<'a>(
    text: &mut String,
    status: &str,
    items: impl Iterator<Item = &'a super::MappingRecord>,
) {
    let mut items = items.peekable();
    if items.peek().is_none() {
        return;
    }
    text.push_str(&format!("\n## {status}\n\n"));
    for item in items {
        text.push_str(&format!(
            "- `{}` from `{}`: {}\n",
            escaped(&item.field),
            escaped(&item.source.display().to_string()),
            item.reason
        ));
        if let Some(comparison) = &item.comparison {
            text.push_str(&format!(
                "  - kept: `{}`\n  - ignored: `{}`\n",
                escaped(&comparison.kept.to_string()),
                escaped(&comparison.ignored.to_string())
            ));
        }
    }
}

/// One line, and no backtick that would end the code span early.
fn escaped(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '`' => out.push('\''),
            c if c.is_control() => out.extend(c.escape_debug()),
            c => out.push(c),
        }
    }
    out
}
