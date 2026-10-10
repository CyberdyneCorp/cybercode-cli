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
