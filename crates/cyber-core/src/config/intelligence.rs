//! Typed code-intelligence configuration; parsing grants no process authority.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

const BUILTIN_SERVERS: &[&str] = &[
    "rust-analyzer",
    "typescript",
    "pyright",
    "gopls",
    "clangd",
    "jdtls",
    "lua-language-server",
    "zls",
    "bash-language-server",
    "yaml-language-server",
    "svelte",
    "vue",
    "solidity",
    "verible",
];

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LspServerConfig {
    pub command: Option<Vec<String>>,
    pub extensions: Option<Vec<String>>,
    pub root_markers: Option<Vec<String>>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub initialization_options: Option<Value>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FormatterConfig {
    pub command: Option<Vec<String>>,
    pub extensions: Option<Vec<String>>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LspSettings {
    pub enabled: bool,
    pub auto_install: bool,
    pub diagnostics_wait_ms: u64,
    pub servers: BTreeMap<String, LspServerConfig>,
}
impl Default for LspSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_install: false,
            diagnostics_wait_ms: 5000,
            servers: BTreeMap::new(),
        }
    }
}
impl LspSettings {
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self::default();
        let Some(entries) = section(config, "lsp", &mut settings.enabled)? else {
            return Ok(settings);
        };
        for (id, value) in entries {
            match id.as_str() {
                "auto_install" => {
                    settings.auto_install =
                        value.as_bool().ok_or("lsp.auto_install must be boolean")?
                }
                "diagnostics_wait_ms" => {
                    settings.diagnostics_wait_ms = value
                        .as_u64()
                        .ok_or("lsp.diagnostics_wait_ms must be a nonnegative integer")?
                }
                _ => {
                    let server = parse_server(id, value)?;
                    settings.servers.insert(id.clone(), server);
                }
            }
        }
        Ok(settings)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FormatterSettings {
    pub enabled: bool,
    pub formatters: BTreeMap<String, FormatterConfig>,
}
impl Default for FormatterSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            formatters: BTreeMap::new(),
        }
    }
}
impl FormatterSettings {
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self::default();
        let Some(entries) = section(config, "formatters", &mut settings.enabled)? else {
            return Ok(settings);
        };
        for (id, value) in entries {
            let path = format!("formatters.{id}");
            validate_entry(value, &path)?;
            let formatter: FormatterConfig =
                serde_json::from_value(value.clone()).map_err(|e| format!("{path}: {e}"))?;
            validate_command(formatter.command.as_deref(), &path)?;
            validate_list(
                formatter.extensions.as_deref(),
                &format!("{path}.extensions"),
            )?;
            validate_env(&formatter.env, &path)?;
            settings.formatters.insert(id.clone(), formatter);
        }
        Ok(settings)
    }
}

fn parse_server(id: &str, value: &Value) -> Result<LspServerConfig, String> {
    let path = format!("lsp.{id}");
    validate_entry(value, &path)?;
    let server: LspServerConfig =
        serde_json::from_value(value.clone()).map_err(|e| format!("{path}: {e}"))?;
    validate_command(server.command.as_deref(), &path)?;
    validate_list(server.extensions.as_deref(), &format!("{path}.extensions"))?;
    validate_list(
        server.root_markers.as_deref(),
        &format!("{path}.root_markers"),
    )?;
    validate_env(&server.env, &path)?;
    if !BUILTIN_SERVERS.contains(&id) && server.extensions.as_ref().is_none_or(Vec::is_empty) {
        return Err(format!("{path}: custom servers require extensions"));
    }
    Ok(server)
}

fn section<'a>(
    config: &'a Value,
    key: &str,
    enabled: &mut bool,
) -> Result<Option<&'a Map<String, Value>>, String> {
    match config.get(key) {
        None => Ok(None),
        Some(Value::Bool(false)) => {
            *enabled = false;
            Ok(None)
        }
        Some(Value::Object(entries)) => Ok(Some(entries)),
        _ => Err(format!("{key} must be false or an object")),
    }
}
fn validate_entry(value: &Value, path: &str) -> Result<(), String> {
    let entries = value
        .as_object()
        .ok_or_else(|| format!("{path} must be an object"))?;
    for (field, value) in entries {
        if field != "initialization_options" && value.is_null() {
            return Err(format!("{path}.{field} must not be null"));
        }
    }
    Ok(())
}
fn validate_command(command: Option<&[String]>, path: &str) -> Result<(), String> {
    if let Some(command) = command
        && (command.first().is_none_or(|s| s.trim().is_empty())
            || command.iter().any(|s| s.contains('\0')))
    {
        return Err(format!(
            "{path}.command requires a nonempty executable and no NUL bytes"
        ));
    }
    Ok(())
}
fn validate_list(values: Option<&[String]>, path: &str) -> Result<(), String> {
    if values.is_some_and(|values| {
        values
            .iter()
            .any(|s| s.trim().is_empty() || s.contains('\0'))
    }) {
        return Err(format!(
            "{path} requires nonempty strings without NUL bytes"
        ));
    }
    Ok(())
}
fn validate_env(env: &BTreeMap<String, String>, path: &str) -> Result<(), String> {
    if env
        .iter()
        .any(|(key, value)| key.is_empty() || key.contains(['=', '\0']) || value.contains('\0'))
    {
        return Err(format!(
            "{path}.env contains an invalid environment variable"
        ));
    }
    Ok(())
}
