//! Static configured commands consumed by the public command API.
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

#[derive(Debug, Default)]
pub struct Commands {
    pub entries: BTreeMap<String, StaticCommand>,
    /// Definition names only; never include template or parser error values.
    pub unavailable: Vec<String>,
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.split('/').all(|part| {
            !part.is_empty()
                && ![".", ".."].contains(&part)
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
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
        if command.template.is_empty()
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
