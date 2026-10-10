//! Static configured commands consumed by the public command API.
mod discovery;
pub use discovery::{CommandIssue, CommandScope, discover};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StaticCommand {
    pub template: String,
    #[serde(default)]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandSourceScope {
    Global,
    Project,
    Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandSourceKind {
    NativeMarkdown,
    CompatibilityMarkdown,
    Configuration,
}

/// Only source locations are retained; definition values and loader labels are omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct CommandOrigin {
    pub scope: CommandSourceScope,
    pub kind: CommandSourceKind,
    pub paths: Vec<std::path::PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct CommandProvenance {
    pub winner: CommandOrigin,
    pub shadowed: Vec<CommandOrigin>,
}

#[derive(Debug, Default)]
pub struct Commands {
    pub entries: BTreeMap<String, StaticCommand>,
    /// Definition names only; never include template or parser error values.
    pub unavailable: Vec<String>,
    pub issues: Vec<CommandIssue>,
    /// Includes unavailable winning definitions, which must not fall back.
    pub provenance: BTreeMap<String, CommandProvenance>,
}

pub struct MarkdownCommand {
    pub definition: Value,
    /// Markdown descriptors, distinct from JSON pointers.
    pub sources: BTreeMap<String, &'static str>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frontmatter {
    description: Option<String>,
    argument_hint: Option<String>,
    #[serde(rename = "argument-hint")]
    dashed_hint: Option<String>,
}

pub fn markdown(text: &str) -> Result<MarkdownCommand, &'static str> {
    if text.len() > 1024 * 1024 {
        return Err("command document exceeds byte limit");
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut header = Frontmatter::default();
    let mut body = text;
    if let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    {
        let mut offset = 0;
        let mut closing = None;
        for line in rest.split_inclusive('\n') {
            if line.trim_end_matches(['\r', '\n']) == "---" {
                closing = Some((offset, offset + line.len()));
                break;
            }
            offset += line.len();
            if offset > 16384 {
                return Err("command frontmatter exceeds byte limit");
            }
        }
        let (end, after) = closing.ok_or("command frontmatter has no closing delimiter")?;
        if !rest[..end].trim().is_empty() {
            header = serde_yaml_ng::from_str(&rest[..end])
                .map_err(|_| "command frontmatter requires supported static fields")?;
        }
        body = &rest[after..];
    }
    if header.argument_hint.is_some() && header.dashed_hint.is_some() {
        return Err("command argument hint aliases conflict");
    }
    let mut definition = serde_json::json!({"template":body.trim_start_matches(['\r', '\n'])});
    let mut sources = BTreeMap::from([("template".into(), "body")]);
    if let Some(description) = header.description {
        definition["description"] = Value::String(description);
        sources.insert("description".into(), "frontmatter:description");
    }
    if let Some(hint) = header.argument_hint.or(header.dashed_hint.clone()) {
        definition["argument_hint"] = Value::String(hint);
        sources.insert(
            "argument_hint".into(),
            if header.dashed_hint.is_some() {
                "frontmatter:argument-hint"
            } else {
                "frontmatter:argument_hint"
            },
        );
    }
    StaticCommand::parse(&definition)?;
    Ok(MarkdownCommand {
        definition,
        sources,
    })
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.split('/').all(|part| {
            !part.is_empty()
                && ![".", ".."].contains(&part)
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
}

pub fn invocation_name(name: &str) -> String {
    if ["help", "exit", "goal", "loop", "workflows", "mode"].contains(&name) {
        format!("project:{name}")
    } else {
        name.into()
    }
}

impl StaticCommand {
    pub fn parse(value: &Value) -> Result<Self, &'static str> {
        let fields = value
            .as_object()
            .filter(|m| m.len() <= 3)
            .ok_or("command requires a supported static definition")?;
        if fields
            .values()
            .any(|v| v.as_str().is_none_or(|s| s.len() > 65536))
        {
            return Err("command fields must be bounded strings");
        }
        let command: Self = serde_json::from_value(value.clone())
            .map_err(|_| "command requires a supported static definition")?;
        let bounded = |s: &str| s.len() <= 65536 && !s.contains('\0');
        if command.template.trim().is_empty()
            || !bounded(&command.template)
            || !bounded(&command.description)
            || command
                .argument_hint
                .as_deref()
                .is_some_and(|s| !bounded(s))
            || command.template.contains("!`")
            || command
                .template
                .split_whitespace()
                .any(|token| token.starts_with('@'))
            || crate::config::has_placeholder(&command.template)
        {
            return Err(
                "command requires a bounded literal template without execution or substitution",
            );
        }
        Ok(command)
    }

    pub fn expand(&self, arguments: &str) -> String {
        crate::skills::expand(&self.template, arguments)
    }
}

pub fn configured(config: &Value) -> Commands {
    let mut result = Commands::default();
    let Some(entries) = config.get("commands") else {
        return result;
    };
    let Some(entries) = entries.as_object().filter(|m| m.len() <= 128) else {
        result.unavailable.push("commands".into());
        return result;
    };
    for (name, value) in entries {
        if !valid_name(name) {
            result.unavailable.push("invalid command name".into());
            continue;
        }
        match StaticCommand::parse(value) {
            Ok(command) => {
                result.entries.insert(invocation_name(name), command);
            }
            Err(_) => result.unavailable.push(invocation_name(name)),
        }
    }
    result
}
