//! Static MCP configuration proposals; never connects or executes source declarations.
use super::{ConversionError, RequiredEnvironment, SourceTool};
use serde::Serialize;
use serde_json::{Map, Value, json};

#[derive(Debug, Serialize)]
pub struct McpImportConfig {
    pub config: Value,
    pub required_environment: Vec<RequiredEnvironment>,
    pub not_imported: Vec<McpMappingIssue>,
}
#[derive(Debug, Serialize)]
pub struct McpMappingIssue {
    pub field: String,
    pub reason: &'static str,
}
fn error(field: &str, reason: &'static str) -> ConversionError {
    ConversionError {
        field: field.into(),
        reason,
    }
}
fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str, ConversionError> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 16384 && !s.contains('\0'))
        .ok_or_else(|| error(field, "expected a bounded nonempty string"))
}
fn env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 256
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn encoded(value: &str) -> String {
    value.bytes().map(|b| format!("{b:02X}")).collect()
}
fn binding(
    tool: SourceTool,
    server: &str,
    kind: &str,
    key: &str,
    value: &Value,
    field: &str,
    required: &mut Vec<RequiredEnvironment>,
) -> Result<Value, ConversionError> {
    let value = value
        .as_str()
        .filter(|s| {
            s.len() <= 16384 && !s.contains('\0') && (kind == "ENV" || !s.contains(['\r', '\n']))
        })
        .ok_or_else(|| error(field, "expected a bounded static value"))?;
    let variable = if let Some(name) = value
        .strip_prefix("{env:")
        .and_then(|s| s.strip_suffix('}'))
    {
        if !env_name(name) {
            return Err(error(field, "invalid source environment reference"));
        }
        name.to_owned()
    } else {
        if value.contains(['{', '}']) {
            return Err(error(
                field,
                "source substitution needs an additional adapter",
            ));
        }
        let family = match tool {
            SourceTool::Claude => "CLAUDE",
            SourceTool::Codex => "CODEX",
            SourceTool::OpenCode => "OPENCODE",
        };
        format!(
            "CYBER_IMPORT_MCP_{family}_{}_{kind}_{}",
            encoded(server),
            encoded(key)
        )
    };
    required.push(RequiredEnvironment {
        field: field.into(),
        variable: variable.clone(),
        from_literal: !value.starts_with("{env:"),
    });
    Ok(json!(format!("{{env:{variable}}}")))
}
fn references(
    tool: SourceTool,
    name: &str,
    kind: &str,
    input: Option<&Value>,
    field: &str,
    required: &mut Vec<RequiredEnvironment>,
) -> Result<Value, ConversionError> {
    let Some(input) = input else {
        return Ok(json!({}));
    };
    let input = input
        .as_object()
        .filter(|m| m.len() <= 128)
        .ok_or_else(|| error(field, "expected at most 128 bindings"))?;
    let mut output = Map::new();
    let mut names = std::collections::BTreeSet::new();
    for (index, (key, value)) in input.iter().enumerate() {
        let location = format!("{field}[{index}]");
        let valid = if kind == "ENV" {
            env_name(key)
        } else {
            !key.is_empty()
                && key.len() <= 256
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        };
        if !valid {
            return Err(error(&location, "invalid environment or header name"));
        }
        let identity = if kind == "HEADER" || cfg!(windows) {
            key.to_ascii_lowercase()
        } else {
            key.clone()
        };
        if !names.insert(identity) {
            return Err(error(
                &location,
                "case-ambiguous header or environment bindings",
            ));
        }
        output.insert(
            key.clone(),
            binding(tool, name, kind, key, value, &location, required)?,
        );
    }
    Ok(Value::Object(output))
}
fn arguments(value: Option<&Value>, field: &str) -> Result<Vec<Value>, ConversionError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .filter(|a| a.len() <= 256)
        .ok_or_else(|| error(field, "expected at most 256 arguments"))?;
    values
        .iter()
        .map(|v| {
            v.as_str()
                .filter(|s| s.len() <= 16384 && !s.contains('\0'))
                .map(|s| json!(s))
                .ok_or_else(|| error(field, "invalid static argument"))
        })
        .collect()
}
fn local(
    tool: SourceTool,
    name: &str,
    source: &Map<String, Value>,
    field: &str,
    required: &mut Vec<RequiredEnvironment>,
) -> Result<Value, ConversionError> {
    let command = source
        .get("command")
        .ok_or_else(|| error(field, "local server requires a command"))?;
    let (command, args) = if command.is_array() {
        if tool != SourceTool::OpenCode || source.contains_key("args") {
            return Err(error(
                field,
                "ambiguous command array or separate arguments",
            ));
        }
        let mut args = arguments(Some(command), field)?;
        if args.is_empty() {
            return Err(error(field, "empty command array"));
        }
        let command = args.remove(0);
        text(&command, field)?;
        (command, args)
    } else {
        (
            json!(text(command, field)?),
            arguments(source.get("args"), field)?,
        )
    };
    let env = source.get("env").or_else(|| source.get("environment"));
    if source.contains_key("env") && source.contains_key("environment") {
        return Err(error(field, "ambiguous environment fields"));
    }
    let mut output = json!({"type":"local","command":command,"args":args});
    if env.is_some() {
        output["env"] = references(tool, name, "ENV", env, &format!("{field}.env"), required)?;
    }
    if let Some(cwd) = source.get("cwd") {
        output["cwd"] = json!(text(cwd, field)?);
    }
    Ok(output)
}
fn remote(
    tool: SourceTool,
    name: &str,
    source: &Map<String, Value>,
    field: &str,
    required: &mut Vec<RequiredEnvironment>,
) -> Result<Value, ConversionError> {
    let value = text(
        source
            .get("url")
            .ok_or_else(|| error(field, "remote server requires a URL"))?,
        field,
    )?;
    let url = url::Url::parse(value).map_err(|_| error(field, "invalid remote endpoint"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || value.contains(['{', '}'])
    {
        return Err(error(
            field,
            "endpoint credentials, query, fragment or substitutions require an adapter",
        ));
    }
    let mut output = json!({"type":"remote","url":value});
    if source.contains_key("headers") {
        output["headers"] = references(
            tool,
            name,
            "HEADER",
            source.get("headers"),
            &format!("{field}.headers"),
            required,
        )?;
    }
    if let Some(oauth) = source.get("oauth") {
        if oauth.as_bool() != Some(false) {
            return Err(error(
                field,
                "source OAuth settings require an additional adapter",
            ));
        }
        output["oauth"] = json!(false);
    }
    Ok(output)
}
fn server(
    tool: SourceTool,
    name: &str,
    value: &Value,
    field: &str,
    result: &mut McpImportConfig,
) -> Result<Value, ConversionError> {
    let source = value
        .as_object()
        .filter(|m| m.len() <= 64)
        .ok_or_else(|| error(field, "expected at most 64 server fields"))?;
    let kind = source.get("type").map(|v| text(v, field)).transpose()?;
    let has_command = source.contains_key("command");
    let has_url = source.contains_key("url");
    if has_command == has_url {
        return Err(error(field, "server must select exactly one transport"));
    }
    let allowed = if has_command {
        matches!(kind, None | Some("stdio" | "local"))
    } else {
        matches!(kind, None | Some("http" | "remote"))
    };
    if !allowed {
        return Err(error(field, "source transport needs an additional adapter"));
    }
    let mut output = if has_command {
        local(tool, name, source, field, &mut result.required_environment)?
    } else {
        remote(tool, name, source, field, &mut result.required_environment)?
    };
    if let Some(enabled) = source.get("enabled") {
        output["disabled"] = json!(
            !enabled
                .as_bool()
                .ok_or_else(|| error(field, "enabled must be boolean"))?
        );
    }
    if let Some(disabled) = source.get("disabled") {
        let disabled = disabled
            .as_bool()
            .ok_or_else(|| error(field, "disabled must be boolean"))?;
        if output
            .get("disabled")
            .is_some_and(|v| v.as_bool() != Some(disabled))
        {
            return Err(error(field, "conflicting enablement fields"));
        }
        output["disabled"] = json!(disabled);
    }
    for (index, key) in source.keys().enumerate() {
        let supported = if has_command {
            &[
                "type",
                "command",
                "args",
                "env",
                "environment",
                "cwd",
                "enabled",
                "disabled",
            ][..]
        } else {
            &["type", "url", "headers", "oauth", "enabled", "disabled"][..]
        };
        if !supported.contains(&key.as_str()) {
            result.not_imported.push(McpMappingIssue {
                field: format!("{field}[{index}]"),
                reason: "server option requires an additional adapter; value withheld",
            });
        }
    }
    Ok(output)
}
pub fn mcp_config(tool: SourceTool, document: &Value) -> Result<McpImportConfig, ConversionError> {
    document
        .as_object()
        .filter(|m| m.len() <= 256)
        .ok_or_else(|| error("mcp", "expected at most 256 root fields"))?;
    let key = match tool {
        SourceTool::Claude => "mcpServers",
        SourceTool::Codex => "mcp_servers",
        SourceTool::OpenCode => "mcp",
    };
    let mut result = McpImportConfig {
        config: json!({}),
        required_environment: Vec::new(),
        not_imported: Vec::new(),
    };
    let Some(mut value) = document.get(key) else {
        return Ok(result);
    };
    if tool == SourceTool::OpenCode && value.get("servers").is_some() {
        if value.as_object().is_none_or(|m| m.len() != 1) {
            return Err(error(key, "ambiguous MCP wrapper fields"));
        }
        value = &value["servers"];
    }
    let input = value
        .as_object()
        .filter(|m| m.len() <= 128)
        .ok_or_else(|| error(key, "expected at most 128 servers"))?;
    let mut output = Map::new();
    for (index, (name, value)) in input.iter().enumerate() {
        let field = format!("{key}[{index}]");
        if name.is_empty()
            || name.len() > 48
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err(error(&field, "invalid native server identifier"));
        }
        output.insert(
            name.clone(),
            server(tool, name, value, &field, &mut result)?,
        );
    }
    super::codex_providers::validate_environment_bindings(&result.required_environment)?;
    result.config = json!({"mcp":output});
    let secrets = super::preview::sensitive_values(document);
    super::preview::reject_unsafe_strings(
        &result.config,
        &json!({}),
        &secrets,
        std::path::Path::new("mcp"),
    )
    .map_err(|_| {
        error(
            key,
            "retained server fields contain declared credentials or unsafe substitutions",
        )
    })?;
    reject_secret_keys(&result.config, &secrets)?;
    crate::config::McpSettings::from_config(&result.config)
        .map_err(|_| error(key, "converted server definitions fail native validation"))?;
    Ok(result)
}

fn reject_secret_keys(value: &Value, secrets: &[String]) -> Result<(), ConversionError> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if secrets.iter().any(|secret| key.contains(secret)) {
                    return Err(error(
                        "mcp",
                        "retained metadata contains a declared credential",
                    ));
                }
                reject_secret_keys(value, secrets)?;
            }
        }
        Value::Array(items) => {
            for value in items {
                reject_secret_keys(value, secrets)?;
            }
        }
        _ => (),
    }
    Ok(())
}
