//! Inline source settings with real native consumers; no source code or paths are evaluated.
use super::{ConversionError, ProviderMappingIssue, SourceTool};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct OpenCodeSettingsConfig {
    pub config: Value,
    pub not_imported: Vec<ProviderMappingIssue>,
    pub field_sources: BTreeMap<String, Vec<String>>,
}
fn error(reason: &'static str) -> ConversionError {
    ConversionError {
        field: "settings".into(),
        reason,
    }
}
fn map(result: &mut OpenCodeSettingsConfig, native: &str, raw: &str) {
    result.field_sources.insert(native.into(), vec![raw.into()]);
}
fn pending(result: &mut OpenCodeSettingsConfig, raw: String, reason: &'static str) {
    result
        .not_imported
        .push(ProviderMappingIssue { field: raw, reason });
}
fn text(value: &Value) -> Result<&str, ConversionError> {
    value
        .as_str()
        .filter(|s| s.len() <= 65536 && !s.contains('\0'))
        .ok_or_else(|| error("expected a bounded static string"))
}
fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-/".contains(&b))
}
fn bounded(value: &Value) -> Result<(), ConversionError> {
    let mut nodes = 0;
    let mut stack = vec![(value, 0)];
    while let Some((value, depth)) = stack.pop() {
        nodes += 1;
        if nodes > 4096 || depth > 16 {
            return Err(error("agent definition exceeds structural bounds"));
        }
        match value {
            Value::Object(map) => {
                if map.len() > 4096usize.saturating_sub(nodes + stack.len()) {
                    return Err(error("agent definition exceeds structural bounds"));
                }
                stack.extend(map.values().map(|v| (v, depth + 1)));
            }
            Value::Array(items) => {
                if items.len() > 4096usize.saturating_sub(nodes + stack.len()) {
                    return Err(error("agent definition exceeds structural bounds"));
                }
                stack.extend(items.iter().map(|v| (v, depth + 1)));
            }
            _ => (),
        }
    }
    Ok(())
}
fn model(value: &Value) -> Result<String, ConversionError> {
    let value = if let Some(raw) = value.as_str() {
        raw.into()
    } else {
        let object = value
            .as_object()
            .filter(|o| {
                o.len() <= 3
                    && o.keys()
                        .all(|k| ["providerID", "model", "variant"].contains(&k.as_str()))
            })
            .ok_or_else(|| error("invalid expanded model selection"))?;
        let provider = object
            .get("providerID")
            .and_then(Value::as_str)
            .filter(|s| name(s) && !s.contains('/'))
            .ok_or_else(|| error("invalid model provider identifier"))?;
        let id = object
            .get("model")
            .and_then(Value::as_str)
            .ok_or_else(|| error("model identifier is missing"))?;
        let mut output = format!("{provider}/{id}");
        if let Some(variant) = object.get("variant") {
            let variant = variant
                .as_str()
                .filter(|s| name(s) && !s.contains('/'))
                .ok_or_else(|| error("invalid model variant identifier"))?;
            output.push('#');
            output.push_str(variant);
        }
        output
    };
    let (base, variant) = value
        .split_once('#')
        .map_or((value.as_str(), None), |(base, variant)| {
            (base, Some(variant))
        });
    if value.len() > 384
        || base
            .split_once('/')
            .is_none_or(|(p, m)| p.is_empty() || m.is_empty())
        || !base
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:/-".contains(&b))
        || variant.is_some_and(|v| !name(v) || v.contains('/'))
    {
        return Err(error(
            "model selection must be a static qualified reference",
        ));
    }
    Ok(value)
}
fn alias<'a>(
    source: &'a Map<String, Value>,
    names: &[&'a str],
) -> Result<Option<(&'a str, &'a Value)>, ConversionError> {
    let mut found = names
        .iter()
        .filter_map(|key| source.get(*key).map(|value| (*key, value)));
    let first = found.next();
    if found.next().is_some() {
        return Err(error("ambiguous source aliases"));
    }
    Ok(first)
}
fn profile_value(key: &str, value: &Value) -> Result<Value, ConversionError> {
    match key {
        "model" => Ok(json!(model(value)?)),
        "disabled" | "hidden" => value
            .as_bool()
            .map(|v| json!(v))
            .ok_or_else(|| error("agent flags must be boolean")),
        "steps" => value
            .as_u64()
            .filter(|n| *n > 0)
            .map(|n| json!(n))
            .ok_or_else(|| error("agent steps must be positive integers")),
        "mode" => value
            .as_str()
            .filter(|s| ["primary", "subagent", "all"].contains(s))
            .map(|s| json!(s))
            .ok_or_else(|| error("invalid agent mode")),
        _ => Ok(json!(text(value)?)),
    }
}
fn agent(
    id: &str,
    value: &Value,
    raw: &str,
    v2: bool,
    result: &mut OpenCodeSettingsConfig,
) -> Result<Option<Value>, ConversionError> {
    let fields = value
        .as_object()
        .filter(|m| m.len() <= 64)
        .ok_or_else(|| error("expected a bounded agent object"))?;
    bounded(value)?;
    if [
        "build",
        "plan",
        "general",
        "explore",
        "compaction",
        "title",
        "summary",
        "evaluator",
    ]
    .contains(&id)
    {
        pending(
            result,
            raw.into(),
            "built-in source defaults require a catalog-equivalence adapter; value withheld",
        );
        return Ok(None);
    }
    let target = super::provenance::child("/agents", id);
    let mut output = json!({});
    for (names, destination) in [
        (&["prompt", "system"][..], "system"),
        (&["disable", "disabled"][..], "disabled"),
        (&["maxSteps", "steps"][..], "steps"),
    ] {
        if let Some((key, value)) = alias(fields, names)? {
            let raw = super::provenance::child(raw, key);
            if destination == "system" && text(value)?.contains("{file:") {
                pending(
                    result,
                    raw,
                    "source prompt file needs admitted path resolution; value withheld",
                );
                continue;
            }
            output[destination] = profile_value(destination, value)?;
            map(result, &format!("{target}/{destination}"), &raw);
        }
    }
    for key in ["description", "mode", "model", "hidden", "color"] {
        if let Some(value) = fields.get(key) {
            output[key] = profile_value(key, value)?;
            map(
                result,
                &format!("{target}/{key}"),
                &super::provenance::child(raw, key),
            );
        }
    }
    if !fields.contains_key("mode") {
        output["mode"] = json!(if v2 { "primary" } else { "all" });
        map(result, &format!("{target}/mode"), raw);
    }
    let rules = super::opencode_permissions(value)
        .map_err(|_| error("agent permission conversion requires supported static rules"))?;
    if !rules.is_empty() {
        output["permissions"] = json!({"rules":rules});
        let inputs = super::provenance::permission_inputs(SourceTool::OpenCode, value);
        for (index, input) in inputs.iter().enumerate() {
            for key in ["action", "resource", "effect"] {
                map(
                    result,
                    &format!("{target}/permissions/rules/{index}/{key}"),
                    &format!("{raw}{input}"),
                );
            }
        }
    }
    for key in fields.keys() {
        if ![
            "description",
            "mode",
            "model",
            "hidden",
            "color",
            "prompt",
            "system",
            "disable",
            "disabled",
            "maxSteps",
            "steps",
            "permission",
            "permissions",
            "tools",
        ]
        .contains(&key.as_str())
        {
            pending(
                result,
                super::provenance::child(raw, key),
                "agent field needs an additional adapter; value withheld",
            );
        }
    }
    Ok(Some(output))
}
fn agents(
    source: &Map<String, Value>,
    result: &mut OpenCodeSettingsConfig,
) -> Result<(), ConversionError> {
    let Some((key, value)) = alias(source, &["agent", "agents"])? else {
        return Ok(());
    };
    let agents = value
        .as_object()
        .filter(|m| m.len() <= 128)
        .ok_or_else(|| error("expected at most 128 agents"))?;
    let mut output = Map::new();
    for (id, value) in agents {
        if !name(id) || ["max_concurrent", "max_depth", "result_max_bytes"].contains(&id.as_str()) {
            return Err(error(
                "agent identifier conflicts with native settings or is invalid",
            ));
        }
        let raw = super::provenance::child(&format!("/{key}"), id);
        if let Some(agent) = agent(id, value, &raw, key == "agents", result)? {
            output.insert(id.clone(), agent);
        }
    }
    if !output.is_empty() {
        result.config["agents"] = Value::Object(output);
    }
    Ok(())
}
fn compaction(
    source: &Map<String, Value>,
    result: &mut OpenCodeSettingsConfig,
) -> Result<(), ConversionError> {
    let Some(value) = source.get("compaction") else {
        return Ok(());
    };
    let fields = value
        .as_object()
        .filter(|m| m.len() <= 32)
        .ok_or_else(|| error("expected bounded compaction settings"))?;
    let mut output = json!({});
    for key in ["auto", "buffer"] {
        if let Some(value) = fields.get(key) {
            if (key == "auto" && !value.is_boolean())
                || (key == "buffer" && value.as_u64().is_none())
            {
                return Err(error("invalid explicit compaction setting"));
            }
            output[key] = value.clone();
            map(
                result,
                &format!("/compaction/{key}"),
                &format!("/compaction/{key}"),
            );
        }
    }
    let keep = fields
        .get("keep")
        .map(|v| {
            v.as_object()
                .filter(|m| m.len() <= 16)
                .ok_or_else(|| error("invalid compaction keep settings"))
        })
        .transpose()?;
    let current = keep.and_then(|m| m.get("tokens"));
    let legacy = fields.get("preserve_recent_tokens");
    if current.is_some() && legacy.is_some() {
        return Err(error("ambiguous compaction token aliases"));
    }
    if let Some(tokens) = current.or(legacy) {
        if tokens.as_u64().is_none() {
            return Err(error("compaction tokens must be nonnegative integers"));
        }
        output["keep"] = json!({"tokens":tokens});
        map(
            result,
            "/compaction/keep/tokens",
            if legacy.is_some() {
                "/compaction/preserve_recent_tokens"
            } else {
                "/compaction/keep/tokens"
            },
        );
    }
    for key in fields
        .keys()
        .filter(|k| !["auto", "buffer", "keep", "preserve_recent_tokens"].contains(&k.as_str()))
    {
        pending(
            result,
            super::provenance::child("/compaction", key),
            "compaction field requires an additional adapter; value withheld",
        );
    }
    for key in keep
        .into_iter()
        .flat_map(|m| m.keys())
        .filter(|k| k.as_str() != "tokens")
    {
        pending(
            result,
            super::provenance::child("/compaction/keep", key),
            "compaction tail behavior requires an additional adapter; value withheld",
        );
    }
    if output.as_object().is_some_and(|m| !m.is_empty()) {
        result.config["compaction"] = output;
    }
    Ok(())
}

pub fn opencode_settings_config(
    document: &Value,
) -> Result<OpenCodeSettingsConfig, ConversionError> {
    let source = document
        .as_object()
        .filter(|m| m.len() <= 256)
        .ok_or_else(|| error("expected a bounded source object"))?;
    let mut result = OpenCodeSettingsConfig {
        config: json!({}),
        not_imported: vec![],
        field_sources: BTreeMap::new(),
    };
    agents(source, &mut result)?;
    compaction(source, &mut result)?;
    let secrets = super::preview::sensitive_values(document);
    super::preview::reject_unsafe_strings(
        &result.config,
        &json!({}),
        &secrets,
        std::path::Path::new("settings"),
    )
    .map_err(|_| error("retained settings contain credentials or unsafe substitutions"))?;
    for pointer in result
        .field_sources
        .keys()
        .chain(result.field_sources.values().flatten())
        .chain(result.not_imported.iter().map(|r| &r.field))
    {
        if secrets.iter().any(|s| pointer.contains(s)) {
            return Err(error("source attribution contains a declared credential"));
        }
    }
    crate::config::resolve_agents(&result.config)
        .map_err(|_| error("converted agent configuration is invalid"))?;
    Ok(result)
}
