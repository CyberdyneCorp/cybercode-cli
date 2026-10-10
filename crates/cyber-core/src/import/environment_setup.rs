//! Setup requirements for accepted proposal leaves and reserved native bindings.
use super::{PreviewEnvironment, RequiredEnvironment, SourceReference};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn accepted(
    requirements: Vec<RequiredEnvironment>,
    origins: BTreeMap<String, Vec<SourceReference>>,
    raw: &Value,
) -> Result<Vec<PreviewEnvironment>, super::DiscoveryError> {
    let mut output = Vec::new();
    for mut requirement in requirements {
        let pointer = descriptor_pointer(raw, &requirement.field).ok_or(super::DiscoveryError {
            path: std::path::PathBuf::new(),
            reason: "credential source attribution is unavailable",
        })?;
        let sources: Vec<_> = origins
            .get(&requirement.variable)
            .into_iter()
            .flatten()
            .filter(|source| source.field == pointer)
            .cloned()
            .collect();
        if let Some(first) = sources.first() {
            requirement.field = first.field.clone();
            output.push(PreviewEnvironment {
                requirement,
                sources,
            });
        }
    }
    Ok(output)
}
fn descriptor_pointer(raw: &Value, descriptor: &str) -> Option<String> {
    if descriptor.starts_with('/') {
        return raw.pointer(descriptor).map(|_| descriptor.into());
    }
    let root_end = descriptor.find('[').unwrap_or(descriptor.len());
    let root = &descriptor[..root_end];
    let mut pointer = super::provenance::child("", root);
    if root == "mcp" && raw.pointer("/mcp/servers").is_some() {
        pointer.push_str("/servers");
    }
    let mut rest = &descriptor[root_end..];
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix('[') {
            let (index, tail) = tail.split_once(']')?;
            let map = raw.pointer(&pointer)?.as_object()?;
            pointer =
                super::provenance::child(&pointer, map.keys().nth(index.parse::<usize>().ok()?)?);
            rest = tail;
        } else {
            let tail = rest.strip_prefix('.')?;
            let end = tail.find(['.', '[']).unwrap_or(tail.len());
            let key = &tail[..end];
            let key = if key == "env"
                && raw.pointer(&pointer)?.get("env").is_none()
                && raw.pointer(&pointer)?.get("environment").is_some()
            {
                "environment"
            } else {
                key
            };
            pointer = super::provenance::child(&pointer, key);
            rest = &tail[end..];
        }
    }
    raw.pointer(&pointer).map(|_| pointer)
}

pub(super) fn coalesce(requirements: Vec<PreviewEnvironment>) -> Vec<PreviewEnvironment> {
    let mut output: BTreeMap<String, PreviewEnvironment> = BTreeMap::new();
    for mut incoming in requirements {
        if let Some(existing) = output.get_mut(&incoming.requirement.variable) {
            existing.sources.append(&mut incoming.sources);
            existing.sources.sort();
            existing.sources.dedup();
        } else {
            output.insert(incoming.requirement.variable.clone(), incoming);
        }
    }
    output.into_values().collect()
}

pub(super) fn reserved(config: &Value) -> Vec<RequiredEnvironment> {
    let mut variables = BTreeSet::new();
    collect(config, &mut variables);
    variables
        .into_iter()
        .map(|variable| RequiredEnvironment {
            field: "native environment reference".into(),
            variable,
            from_literal: false,
        })
        .collect()
}
fn collect(value: &Value, variables: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for value in map.values() {
                collect(value, variables);
            }
        }
        Value::Array(items) => {
            for value in items {
                collect(value, variables);
            }
        }
        Value::String(text) => collect_text(text, variables),
        _ => (),
    }
}
fn collect_text(mut text: &str, variables: &mut BTreeSet<String>) {
    while let Some(start) = text.find("{env:") {
        let rest = &text[start + "{env:".len()..];
        let Some(end) = rest.find('}') else {
            break;
        };
        let name = rest[..end]
            .split_once(":-")
            .map_or(&rest[..end], |(name, _)| name);
        if !name.is_empty() {
            variables.insert(name.into());
        }
        text = &rest[end + 1..];
    }
}
