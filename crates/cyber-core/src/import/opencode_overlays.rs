//! Static request overlays retain native merge order and exact source leaves.
use super::{ConversionError, OpenCodeProviderConfig, error, headers, mapped, pending};
use serde_json::{Map, Value, json};

fn body_leaves(
    value: &Value,
    raw: &str,
    native: &str,
    depth: usize,
    nodes: &mut usize,
    result: &mut OpenCodeProviderConfig,
) -> Result<(), ConversionError> {
    *nodes += 1;
    if depth > 16 || *nodes > 4096 {
        return Err(error("providers", "request body exceeds structural bounds"));
    }
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, value) in map {
                if key.len() > 256
                    || key.eq_ignore_ascii_case("apikey")
                    || key.eq_ignore_ascii_case("api_key")
                {
                    return Err(error(
                        "providers",
                        "request body contains unsupported credential metadata",
                    ));
                }
                body_leaves(
                    value,
                    &crate::import::provenance::child(raw, key),
                    &crate::import::provenance::child(native, key),
                    depth + 1,
                    nodes,
                    result,
                )?;
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (index, value) in items.iter().enumerate() {
                let index = index.to_string();
                body_leaves(
                    value,
                    &crate::import::provenance::child(raw, &index),
                    &crate::import::provenance::child(native, &index),
                    depth + 1,
                    nodes,
                    result,
                )?;
            }
        }
        Value::Null => {
            return Err(error(
                "providers",
                "null request fields require an additional merge adapter",
            ));
        }
        Value::String(text)
            if text.len() > 16384
                || text.contains('\0')
                || text.contains("{env:")
                || text.contains("{file:") =>
        {
            return Err(error(
                "providers",
                "request body contains unsupported substitutions or strings",
            ));
        }
        _ => mapped(result, native, raw),
    }
    Ok(())
}

pub(super) fn request(
    namespace: &str,
    source: &Map<String, Value>,
    raw: &str,
    native: &str,
    result: &mut OpenCodeProviderConfig,
) -> Result<Value, ConversionError> {
    let mut output = json!({});
    if let Some(body) = source.get("body") {
        if !body.is_object() {
            return Err(error("providers", "request body must be an object"));
        }
        body_leaves(
            body,
            &format!("{raw}/body"),
            &format!("{native}/body"),
            0,
            &mut 0,
            result,
        )?;
        output["body"] = body.clone();
    }
    if let Some(input) = source.get("headers") {
        output["headers"] = headers(
            namespace,
            input,
            &format!("{raw}/headers"),
            &format!("{native}/headers"),
            result,
        )?;
        if input.as_object().is_some_and(Map::is_empty) {
            mapped(
                result,
                &format!("{native}/headers"),
                &format!("{raw}/headers"),
            );
        }
    }
    Ok(output)
}

pub(super) fn variants(
    namespace: &str,
    source: &Value,
    raw: &str,
    native: &str,
    result: &mut OpenCodeProviderConfig,
) -> Result<Value, ConversionError> {
    let variants = source
        .as_array()
        .filter(|a| a.len() <= 64)
        .ok_or_else(|| error("providers", "expected at most 64 v2 variants"))?;
    let mut output = Map::new();
    for (index, value) in variants.iter().enumerate() {
        let variant = value
            .as_object()
            .filter(|m| m.len() <= 32)
            .ok_or_else(|| error("providers", "expected a bounded variant object"))?;
        let id = variant
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            })
            .ok_or_else(|| error("providers", "invalid variant identifier"))?;
        if output.contains_key(id) {
            return Err(error("providers", "duplicate variant identifiers"));
        }
        let raw = format!("{raw}/{index}");
        let target = crate::import::provenance::child(native, id);
        let request = request(
            &format!("{namespace}#{id}"),
            variant,
            &raw,
            &format!("{target}/request"),
            result,
        )?;
        let mut converted = json!({});
        if request.as_object().is_some_and(|m| !m.is_empty()) {
            converted["request"] = request;
        } else {
            mapped(result, &target, &format!("{raw}/id"));
        }
        for key in variant.keys() {
            if !["id", "body", "headers"].contains(&key.as_str()) {
                pending(result, crate::import::provenance::child(&raw, key));
            }
        }
        output.insert(id.into(), converted);
    }
    if output.is_empty() {
        mapped(result, native, raw);
    }
    Ok(Value::Object(output))
}
