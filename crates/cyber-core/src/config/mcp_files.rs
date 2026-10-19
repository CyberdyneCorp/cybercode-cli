//! Read-time `.mcp.json` compatibility (`compat-import` → Read-time MCP compatibility).
//!
//! The project root's `.mcp.json` (Claude Code's project MCP file) becomes a project layer
//! holding only `mcp`. It is a `project:` layer, so it is withheld until the checkout is
//! trusted and every server still needs its own digest approval before it starts
//! (`cyber mcp trust`). It ranks below `cyber.jsonc`: a server named in any other layer
//! keeps that definition whole. Values are converted, never evaluated: `${VAR}` and
//! `${VAR:-default}` become `{env:…}` references, and a server holding any other
//! placeholder syntax is skipped, so a repository cannot smuggle in a `{file:…}` read.

use std::path::Path;

use serde_json::{Map, Value, json};

/// The file name, at the project root.
pub(super) const FILE: &str = ".mcp.json";

/// `compat.mcp_files` of raw layers, in merge order: the last explicit boolean wins.
pub(super) fn enabled<'a>(layers: impl Iterator<Item = &'a Value>) -> bool {
    layers
        .filter_map(|value| value.pointer("/compat/mcp_files").and_then(Value::as_bool))
        .last()
        .unwrap_or(true)
}

/// `{"mcp": {...}}` of a `.mcp.json` document, plus a warning for each skipped server or
/// ignored field. `path` only labels the warnings.
pub(crate) fn convert(document: &Value, path: &Path) -> (Value, Vec<String>) {
    let mut warnings = Vec::new();
    let mut servers = Map::new();
    let label = path.display();
    let Some(declared) = document.get("mcpServers") else {
        return (json!({}), warnings);
    };
    let Some(declared) = declared.as_object() else {
        warnings.push(format!("{label}: mcpServers must be an object; ignored"));
        return (json!({}), warnings);
    };
    for (name, server) in declared {
        match server_definition(name, server, &mut warnings) {
            Ok(definition) => {
                servers.insert(name.clone(), definition);
            }
            Err(reason) => {
                warnings.push(format!("{label}: MCP server \"{name}\" skipped: {reason}"))
            }
        }
    }
    let value = if servers.is_empty() {
        json!({})
    } else {
        json!({ "mcp": servers })
    };
    (value, warnings)
}

/// Remove every server another layer declares, so `cyber.jsonc` wins whole.
pub(super) fn shadow(value: &mut Value, declared: &[&Value]) {
    let Some(servers) = value.get_mut("mcp").and_then(Value::as_object_mut) else {
        return;
    };
    servers.retain(|name, _| {
        !declared
            .iter()
            .any(|layer| layer.pointer("/mcp").and_then(|m| m.get(name)).is_some())
    });
    if servers.is_empty() {
        value.as_object_mut().map(|map| map.remove("mcp"));
    }
}

fn server_definition(
    name: &str,
    server: &Value,
    warnings: &mut Vec<String>,
) -> Result<Value, String> {
    if matches!(name, "sampling" | "tool_timeout") {
        return Err("the name is reserved by Cyber Code".into());
    }
    let fields = server.as_object().ok_or("expected an object")?;
    let kind = match fields.get("type") {
        None => "stdio",
        Some(kind) => kind.as_str().ok_or("type must be a string")?,
    };
    let (native, known): (&str, &[&str]) = match kind {
        "stdio" => ("local", &["type", "command", "args", "env"]),
        "http" | "sse" => ("remote", &["type", "url", "headers"]),
        _ => return Err(format!("unsupported type \"{kind}\"")),
    };
    let mut output = json!({ "type": native });
    for (key, value) in fields {
        if !known.contains(&key.as_str()) {
            warnings.push(format!(
                "MCP server \"{name}\" field \"{key}\" of {FILE} is not supported and was ignored"
            ));
            continue;
        }
        if key != "type" {
            output[key] =
                converted(value).ok_or_else(|| format!("{key} has unsupported values"))?;
        }
    }
    let required = if native == "local" { "command" } else { "url" };
    if !output.get(required).is_some_and(Value::is_string) {
        return Err(format!("{required} must be a string"));
    }
    super::McpSettings::from_config(&json!({ "mcp": { name: output } }))?;
    Ok(output)
}

/// Strings, string arrays and string maps, with `${VAR}` rewritten.
fn converted(value: &Value) -> Option<Value> {
    match value {
        Value::String(text) => references(text).map(Value::String),
        Value::Array(items) => items
            .iter()
            .map(converted)
            .collect::<Option<_>>()
            .map(Value::Array),
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| Some((key.clone(), Value::String(references(value.as_str()?)?))))
            .collect::<Option<Map<_, _>>>()
            .map(Value::Object),
        _ => None,
    }
}

/// `${NAME}` → `{env:NAME}` and `${NAME:-default}` → `{env:NAME:-default}`; `None` when
/// any other brace remains.
fn references(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let (before, tail) = rest.split_at(start);
        if before.contains(['{', '}']) {
            return None;
        }
        out.push_str(before);
        let end = tail.find('}')?;
        let inner = &tail[2..end];
        let (variable, fallback) = inner.split_once(":-").unwrap_or((inner, ""));
        let valid = variable
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && variable
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !fallback.contains(['{', '$']);
        if !valid {
            return None;
        }
        out.push_str(&format!("{{env:{inner}}}"));
        rest = &tail[end + 1..];
    }
    if rest.contains(['{', '}']) {
        return None;
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_rewrite_shell_syntax_and_refuse_other_placeholders() {
        assert_eq!(references("${TOKEN}").as_deref(), Some("{env:TOKEN}"));
        assert_eq!(
            references("Bearer ${TOKEN:-none}").as_deref(),
            Some("Bearer {env:TOKEN:-none}")
        );
        assert_eq!(references("plain").as_deref(), Some("plain"));
        for refused in [
            "{file:~/.ssh/id_rsa}",
            "{env:HOME}",
            "${1BAD}",
            "${A:-${B}}",
            "x}",
        ] {
            assert_eq!(references(refused), None, "{refused}");
        }
    }

    #[test]
    fn stdio_and_http_servers_convert_and_unsafe_ones_are_skipped() {
        let (value, warnings) = convert(
            &json!({"mcpServers":{
                "db-tools":{"command":"db","args":["--url","${DB_URL}"],"env":{"MODE":"ro"},"alwaysAllow":["x"]},
                "docs":{"type":"http","url":"https://docs.test/mcp","headers":{"authorization":"Bearer ${DOCS_TOKEN}"}},
                "leak":{"command":"cat","args":["{file:~/.ssh/id_rsa}"]},
                "sampling":{"command":"x"},
                "odd":{"type":"websocket","url":"wss://x"}
            }}),
            Path::new("/repo/.mcp.json"),
        );
        assert_eq!(
            value,
            json!({"mcp":{
                "db-tools":{"type":"local","command":"db","args":["--url","{env:DB_URL}"],"env":{"MODE":"ro"}},
                "docs":{"type":"remote","url":"https://docs.test/mcp","headers":{"authorization":"Bearer {env:DOCS_TOKEN}"}}
            }})
        );
        let joined = warnings.join("\n");
        for needle in [
            "\"leak\" skipped",
            "\"sampling\" skipped",
            "\"odd\" skipped",
            "alwaysAllow",
        ] {
            assert!(joined.contains(needle), "{joined}");
        }
    }

    #[test]
    fn declared_servers_shadow_the_file_and_the_last_switch_wins() {
        let mut value =
            json!({"mcp":{"a":{"type":"local","command":"a"},"b":{"type":"local","command":"b"}}});
        shadow(&mut value, &[&json!({"mcp":{"a":{"command":"mine"}}})]);
        assert_eq!(value, json!({"mcp":{"b":{"type":"local","command":"b"}}}));
        shadow(&mut value, &[&json!({"mcp":{"b":{}}})]);
        assert_eq!(value, json!({}));
        let off = json!({"compat":{"mcp_files":false}});
        let on = json!({"compat":{"mcp_files":true}});
        assert!(enabled([].into_iter()));
        assert!(!enabled([&on, &off].into_iter()));
        assert!(enabled([&off, &on, &json!({})].into_iter()));
    }
}
