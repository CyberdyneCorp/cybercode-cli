use cyber_core::hooks::{HookDecision, HookEvent, ParsedDecision};
use serde_json::Value;

/// Exact placeholders preserve JSON types; embedded placeholders become text.
pub fn render_arguments(template: Option<&Value>, event: &HookEvent) -> Result<Value, String> {
    let event = event.as_json();
    let input = match template {
        Some(template) => render(template, event, 0)?,
        None => event.clone(),
    };
    if !input.is_object() {
        return Err("MCP tool arguments must be an object".into());
    }
    super::stdio::encode(&input).map_err(|_| "MCP tool arguments exceed 1 MiB".to_string())?;
    Ok(input)
}

fn field<'a>(event: &'a Value, key: &str) -> Result<&'a Value, String> {
    if key.is_empty() {
        return Err("empty MCP template field".into());
    }
    key.split('.').try_fold(event, |value, key| {
        value
            .get(key)
            .ok_or_else(|| "unknown MCP template field".into())
    })
}

fn render(value: &Value, event: &Value, depth: usize) -> Result<Value, String> {
    if depth > 64 {
        return Err("MCP arguments template nesting exceeds 64".into());
    }
    match value {
        Value::String(text) => render_text(text, event),
        Value::Array(values) => values
            .iter()
            .map(|value| render(value, event, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| Ok((key.clone(), render(value, event, depth + 1)?)))
            .collect::<Result<serde_json::Map<_, _>, String>>()
            .map(Value::Object),
        other => Ok(other.clone()),
    }
}

fn render_text(text: &str, event: &Value) -> Result<Value, String> {
    if let Some(key) = text
        .strip_prefix("${")
        .and_then(|text| text.strip_suffix('}'))
        && !key.contains(['{', '}'])
    {
        return Ok(field(event, key)?.clone());
    }
    let mut rest = text;
    let mut rendered = String::new();
    while let Some(start) = rest.find("${") {
        rendered.push_str(&rest[..start]);
        let placeholder = &rest[start + 2..];
        let end = placeholder
            .find('}')
            .ok_or("unterminated MCP template field")?;
        let value = field(event, &placeholder[..end])?;
        match value {
            Value::String(value) => rendered.push_str(value),
            other => rendered.push_str(&other.to_string()),
        }
        if rendered.len() > super::LIMIT {
            return Err("MCP tool arguments exceed 1 MiB".into());
        }
        rest = &placeholder[end + 1..];
    }
    rendered.push_str(rest);
    Ok(Value::String(rendered))
}

/// One JSON object in textual content supplies the policy; ambiguous objects fail.
pub fn decision(event: &HookEvent, result: &Value) -> Result<ParsedDecision, String> {
    if !result.is_object() || result.get("isError").is_some_and(|value| value != false) {
        return Err("MCP tool result failed or is malformed".into());
    }
    let content = result["content"]
        .as_array()
        .ok_or("MCP tool result has no content array")?;
    let mut object = None;
    let mut bytes = 0usize;
    for item in content {
        if item["type"] != "text" {
            continue;
        }
        let text = item["text"].as_str().ok_or("invalid MCP text content")?;
        bytes = bytes.saturating_add(text.len());
        if bytes > super::LIMIT {
            return Err("MCP text content exceeds 1 MiB".into());
        }
        if let Ok(value) = serde_json::from_str::<Value>(text)
            && value.is_object()
            && object.replace(value).is_some()
        {
            return Err("ambiguous MCP decision objects".into());
        }
    }
    HookDecision::parse(
        event.event(),
        &object.ok_or("MCP result has no JSON object decision")?,
    )
}
