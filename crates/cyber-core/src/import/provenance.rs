//! Value-free source attribution for supported migration adapters.
use super::{DiscoveryError, SourceTool};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct SourceReference {
    pub source: PathBuf,
    pub field: String,
}
pub(super) type Origins = BTreeMap<String, Vec<SourceReference>>;
const LIMIT: usize = 16_384;
pub(super) fn child(pointer: &str, key: &str) -> String {
    format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"))
}
fn under(key: &str, prefix: &str) -> bool {
    key == prefix || key.strip_prefix(prefix).is_some_and(|s| s.starts_with('/'))
}
pub(super) fn fields<'a>(
    origins: &'a Origins,
    pointer: &'a str,
) -> impl Iterator<Item = &'a String> {
    origins
        .range(pointer.to_owned()..)
        .take_while(move |(key, _)| under(key, pointer))
        .map(|(key, _)| key)
}
pub(super) fn references(origins: &Origins, pointer: &str) -> Vec<SourceReference> {
    let mut refs = BTreeSet::new();
    for (_, values) in origins
        .range(pointer.to_owned()..)
        .take_while(|(key, _)| under(key, pointer))
    {
        refs.extend(values.iter().cloned());
    }
    let mut ancestor = pointer;
    while let Some((parent, _)) = ancestor.rsplit_once('/') {
        if let Some(values) = origins.get(parent) {
            refs.extend(values.iter().cloned());
        }
        ancestor = parent;
    }
    refs.into_iter().collect()
}
fn leaves(value: &Value, pointer: &str, output: &mut Vec<String>) {
    if output.len() > LIMIT * 8 {
        return;
    }
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, value) in map {
                leaves(value, &child(pointer, key), output);
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (index, value) in items.iter().enumerate() {
                leaves(value, &child(pointer, &index.to_string()), output);
            }
        }
        _ => output.push(pointer.into()),
    }
}
pub(super) fn overlay(
    base: &mut Value,
    extra: &Value,
    pointer: &str,
    source: &Path,
    append: bool,
    origins: &mut Origins,
) -> Result<(), DiscoveryError> {
    if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
        if !extra.is_empty() {
            origins.remove(pointer);
        }
        for (key, value) in extra {
            let slot = base.entry(key).or_insert(Value::Null);
            overlay(slot, value, &child(pointer, key), source, append, origins)?;
        }
        return Ok(());
    }
    let mut inputs = Vec::new();
    let mut array_offset = None;
    if append
        && matches!(
            pointer,
            "/permissions/rules" | "/permissions/allow" | "/permissions/ask" | "/permissions/deny"
        )
        && let (Some(base), Some(extra)) = (base.as_array_mut(), extra.as_array())
    {
        if !extra.is_empty() {
            origins.remove(pointer);
        }
        let offset = base.len();
        array_offset = Some(offset);
        for (index, value) in extra.iter().enumerate() {
            leaves(
                value,
                &child(pointer, &(offset + index).to_string()),
                &mut inputs,
            );
        }
        base.extend(extra.iter().cloned());
    } else {
        origins.retain(|key, _| !under(key, pointer));
        leaves(extra, pointer, &mut inputs);
        *base = extra.clone();
    }
    if origins.len().saturating_add(inputs.len()) > LIMIT {
        return Err(DiscoveryError {
            path: source.into(),
            reason: "source provenance exceeds 16384 tracked leaves",
        });
    }
    for field in inputs {
        origins.insert(
            field.clone(),
            vec![SourceReference {
                source: source.into(),
                field: array_offset.map_or_else(
                    || field.clone(),
                    |offset| input_pointer(&field, pointer, offset),
                ),
            }],
        );
    }
    Ok(())
}
fn input_pointer(field: &str, pointer: &str, offset: usize) -> String {
    let suffix = field
        .strip_prefix(pointer)
        .and_then(|s| s.strip_prefix('/'))
        .unwrap_or(field);
    let (index, rest) = suffix
        .split_once('/')
        .map_or((suffix, ""), |(index, rest)| (index, rest));
    let Some(index) = index
        .parse::<usize>()
        .ok()
        .and_then(|index| index.checked_sub(offset))
    else {
        return field.into();
    };
    if rest.is_empty() {
        format!("{pointer}/{index}")
    } else {
        format!("{pointer}/{index}/{rest}")
    }
}
fn inputs(origins: &Origins, pointers: &[String]) -> Vec<SourceReference> {
    pointers
        .iter()
        .flat_map(|p| references(origins, p))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
fn permission_inputs(tool: SourceTool, raw: &Value) -> Vec<String> {
    let mut result = Vec::new();
    if tool == SourceTool::Claude {
        for effect in ["allow", "ask", "deny"] {
            let pointer = format!("/permissions/{effect}");
            if let Some(items) = raw.pointer(&pointer).and_then(Value::as_array) {
                result.extend((0..items.len()).map(|i| child(&pointer, &i.to_string())));
            }
        }
        return result;
    }
    if let Some(tools) = raw.get("tools").and_then(Value::as_object) {
        result.extend(tools.keys().map(|key| child("/tools", key)));
    }
    let name = if raw.get("permission").is_some() {
        "permission"
    } else {
        "permissions"
    };
    let pointer = child("", name);
    match raw.get(name) {
        Some(Value::Array(items)) => {
            result.extend((0..items.len()).map(|i| child(&pointer, &i.to_string())))
        }
        Some(Value::Object(actions)) => {
            for (action, value) in actions {
                let pointer = child(&pointer, action);
                if let Some(patterns) = value.as_object() {
                    result.extend(patterns.keys().map(|key| child(&pointer, key)));
                } else {
                    result.push(pointer);
                }
            }
        }
        Some(_) => result.push(pointer),
        None => (),
    }
    result
}
fn provider_inputs(pointer: &str, raw: &Value) -> Vec<String> {
    let parts: Vec<_> = pointer.split('/').collect();
    let id = parts[2];
    let base = format!("/model_providers/{id}");
    let suffix = parts[3..].join("/");
    match suffix.as_str() {
        "api/type" => {
            let field = child(&base, "wire_api");
            vec![if raw.pointer(&field).is_some() {
                field
            } else {
                base
            }]
        }
        "api/url" => {
            let mut fields = vec![child(&base, "base_url")];
            for key in ["api_key", "experimental_bearer_token", "http_headers"] {
                if raw.pointer(&child(&base, key)).is_some() {
                    fields.push(child(&base, key));
                }
            }
            fields
        }
        "api/settings/api_key" => ["env_key", "api_key", "experimental_bearer_token"]
            .into_iter()
            .map(|key| child(&base, key))
            .filter(|p| raw.pointer(p).is_some())
            .collect(),
        _ if suffix.starts_with("models/") => vec!["/model".into(), "/model_provider".into()],
        _ if suffix.starts_with("request/headers/") => {
            let name = parts[5..].join("/");
            ["http_headers", "env_http_headers"]
                .into_iter()
                .map(|key| format!("{base}/{key}/{name}"))
                .filter(|p| raw.pointer(p).is_some())
                .collect()
        }
        _ => vec![base],
    }
}
pub(super) fn converted(
    tool: SourceTool,
    raw: &Value,
    config: &Value,
    origins: &Origins,
    source: &Path,
) -> Result<Origins, DiscoveryError> {
    if config.as_object().is_some_and(|map| map.is_empty()) {
        return Ok(Origins::new());
    }
    let permissions = permission_inputs(tool, raw);
    let mut pointers = Vec::new();
    leaves(config, "", &mut pointers);
    if pointers.len() > LIMIT * 8 {
        return Err(DiscoveryError {
            path: source.into(),
            reason: "converted provenance exceeds tracked leaf limit",
        });
    }
    Ok(pointers
        .into_iter()
        .map(|pointer| {
            let fields = match pointer.as_str() {
                "/model" if tool == SourceTool::Codex => {
                    vec!["/model".into(), "/model_provider".into()]
                }
                "/model" => vec!["/model".into()],
                "/mode" => vec!["/permissions/defaultMode".into()],
                _ if tool == SourceTool::Codex && pointer.starts_with("/providers/") => {
                    provider_inputs(&pointer, raw)
                }
                _ if pointer.starts_with("/permissions/rules/") => {
                    let index = pointer
                        .split('/')
                        .nth(3)
                        .and_then(|s| s.parse::<usize>().ok());
                    index
                        .and_then(|i| permissions.get(i))
                        .cloned()
                        .into_iter()
                        .collect()
                }
                _ => vec![pointer.clone()],
            };
            (pointer, inputs(origins, &fields))
        })
        .collect())
}
pub(super) fn append_rules(
    config: &mut Value,
    rules: &Value,
    origins: &mut Origins,
    rule_origins: &Origins,
) {
    if rules.pointer("/permissions/rules").is_some() {
        config["permissions"] = rules["permissions"].clone();
        origins.extend(rule_origins.clone());
    }
}
pub(super) fn strongest_permissions(tool: SourceTool, config: &mut Value, origins: &mut Origins) {
    if tool == SourceTool::OpenCode {
        return;
    }
    let Some(rules) = config
        .pointer_mut("/permissions/rules")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let mut indexed: Vec<_> = rules.drain(..).enumerate().collect();
    indexed.sort_by_key(|(_, rule)| match rule["effect"].as_str() {
        Some("allow") => 0,
        Some("ask") => 1,
        _ => 2,
    });
    let original = origins.clone();
    origins.retain(|key, _| !under(key, "/permissions/rules"));
    for (index, (old, rule)) in indexed.into_iter().enumerate() {
        let old = format!("/permissions/rules/{old}");
        let new = format!("/permissions/rules/{index}");
        for (key, refs) in original
            .range(old.clone()..)
            .take_while(|(key, _)| under(key, &old))
        {
            origins.insert(format!("{new}{}", &key[old.len()..]), refs.clone());
        }
        rules.push(rule);
    }
}
pub(super) fn environment_origins(
    config: &Value,
    origins: &Origins,
) -> Result<BTreeMap<String, Vec<SourceReference>>, DiscoveryError> {
    let mut pointers = Vec::new();
    leaves(config, "", &mut pointers);
    if pointers.len() > LIMIT * 8 {
        return Err(DiscoveryError {
            path: PathBuf::new(),
            reason: "combined converted provenance exceeds tracked leaf limit",
        });
    }
    let mut output: BTreeMap<String, BTreeSet<SourceReference>> = BTreeMap::new();
    for pointer in pointers {
        let Some(variable) = config
            .pointer(&pointer)
            .and_then(Value::as_str)
            .and_then(|s| s.strip_prefix("{env:"))
            .and_then(|s| s.strip_suffix('}'))
        else {
            continue;
        };
        output
            .entry(variable.into())
            .or_default()
            .extend(references(origins, &pointer));
    }
    Ok(output
        .into_iter()
        .map(|(key, values)| (key, values.into_iter().collect()))
        .collect())
}

pub(super) fn indexed_field(raw: &Value, descriptor: &str) -> Option<String> {
    if let Some(index) = descriptor
        .strip_prefix("settings[")
        .and_then(|s| s.strip_suffix(']'))
        .and_then(|s| s.parse::<usize>().ok())
    {
        return raw.as_object()?.keys().nth(index).map(|key| child("", key));
    }
    if let Some(rest) = descriptor.strip_prefix("model_providers[") {
        let (index, rest) = rest.split_once(']')?;
        let providers = raw.get("model_providers")?.as_object()?;
        let id = providers.keys().nth(index.parse::<usize>().ok()?)?;
        let base = child("/model_providers", id);
        if rest.is_empty() {
            return Some(base);
        }
        let index = rest
            .strip_prefix('[')?
            .strip_suffix(']')?
            .parse::<usize>()
            .ok()?;
        return providers[id]
            .as_object()?
            .keys()
            .nth(index)
            .map(|key| child(&base, key));
    }
    let pointer = child("", descriptor);
    raw.pointer(&pointer).map(|_| pointer)
}
