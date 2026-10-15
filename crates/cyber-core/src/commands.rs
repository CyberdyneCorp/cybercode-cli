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
    /// The agent the command runs with; discovery checks that it is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// `provider/model[#variant]`, ahead of the agent's and the Session's model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The reasoning variant applied to whichever model is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Run in a child Session and hand back only its summary.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subtask: bool,
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
    agent: Option<String>,
    model: Option<String>,
    variant: Option<String>,
    subtask: Option<bool>,
}

/// A Markdown command for import review: shell injection and `@` references are refused.
pub fn markdown(text: &str) -> Result<MarkdownCommand, &'static str> {
    markdown_with(text, StaticCommand::parse)
}

/// A discovered Markdown command, whose template the command service expands.
pub fn markdown_template(text: &str) -> Result<MarkdownCommand, &'static str> {
    markdown_with(text, StaticCommand::parse_template)
}

fn markdown_with(
    text: &str,
    parse: fn(&Value) -> Result<StaticCommand, &'static str>,
) -> Result<MarkdownCommand, &'static str> {
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
    for (field, value) in [
        ("agent", header.agent),
        ("model", header.model),
        ("variant", header.variant),
    ] {
        if let Some(value) = value {
            definition[field] = Value::String(value);
        }
    }
    if let Some(subtask) = header.subtask {
        definition["subtask"] = Value::Bool(subtask);
    }
    for (field, source) in [
        ("agent", "frontmatter:agent"),
        ("model", "frontmatter:model"),
        ("variant", "frontmatter:variant"),
        ("subtask", "frontmatter:subtask"),
    ] {
        if definition.get(field).is_some() {
            sources.insert(field.into(), source);
        }
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
    parse(&definition)?;
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

/// Built-ins that no command, MCP prompt or skill can replace; a definition with one of
/// these names is invocable as `/project:<name>` instead.
pub const RESERVED: &[&str] = &["help", "exit", "goal", "loop", "workflows", "mode"];

pub fn invocation_name(name: &str) -> String {
    if RESERVED.contains(&name) {
        format!("project:{name}")
    } else {
        name.into()
    }
}

impl StaticCommand {
    /// A literal definition, as import review accepts: no shell injection, `@`
    /// references or substitution.
    pub fn parse(value: &Value) -> Result<Self, &'static str> {
        let command = Self::parse_template(value)?;
        if command.has_overrides() {
            return Err("command agent, model, variant and subtask overrides are not imported");
        }
        if command.template.contains("!`")
            || command
                .template
                .split_whitespace()
                .any(|token| token.starts_with('@'))
        {
            return Err(
                "command requires a bounded literal template without execution or substitution",
            );
        }
        Ok(command)
    }

    /// A definition the command service plans: `` !`cmd` `` and `@` references are
    /// expanded at admission under permissions, trust and the sandbox.
    pub fn parse_template(value: &Value) -> Result<Self, &'static str> {
        let fields = value
            .as_object()
            .filter(|m| m.len() <= 7)
            .ok_or("command requires a supported static definition")?;
        if fields.iter().any(|(key, v)| match key.as_str() {
            "subtask" => !v.is_boolean(),
            _ => v.as_str().is_none_or(|s| s.len() > 65536),
        }) {
            return Err("command definition fields must be bounded strings and a boolean subtask");
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
            || crate::config::has_placeholder(&command.template)
        {
            return Err(
                "command requires a bounded literal template without execution or substitution",
            );
        }
        command.check_overrides()?;
        Ok(command)
    }

    /// Whether the command selects an agent, model, variant or child Session.
    pub fn has_overrides(&self) -> bool {
        self.agent.is_some() || self.model.is_some() || self.variant.is_some() || self.subtask
    }

    /// Bounded override values; whether the agent exists is checked at discovery.
    fn check_overrides(&self) -> Result<(), &'static str> {
        let name =
            |s: &str| !s.trim().is_empty() && s.len() <= 128 && !s.chars().any(char::is_control);
        if self.agent.as_deref().is_some_and(|agent| !name(agent)) {
            return Err("command agent must be a bounded name");
        }
        if self
            .model
            .as_deref()
            .is_some_and(|model| !crate::skills::valid_model_ref(model))
        {
            return Err("command model must be provider/model[#variant]");
        }
        if let Some(variant) = &self.variant
            && (!name(variant)
                || variant.contains('#')
                || self.model.as_deref().is_some_and(|m| m.contains('#')))
        {
            return Err("command variant must be a bounded name not already set by its model");
        }
        Ok(())
    }

    /// Arguments without placeholders are appended raw after a blank line.
    pub fn expand(&self, arguments: &str) -> String {
        self.expand_template(arguments).text
    }

    pub fn expand_template(&self, arguments: &str) -> crate::skills::Expanded {
        crate::skills::expand_template(&self.template, arguments, crate::skills::Append::Raw)
    }
}

/// The trust digest of a template that runs shell commands: approval binds the kind,
/// the name and the exact template text.
pub fn template_digest(kind: &str, name: &str, template: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in ["cyber-template-v1", kind, name, template] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("sha256:{:x}", hasher.finalize())
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
        match StaticCommand::parse_template(value) {
            Ok(command) => {
                result.entries.insert(invocation_name(name), command);
            }
            Err(_) => result.unavailable.push(invocation_name(name)),
        }
    }
    result
}
