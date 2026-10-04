//! JSON helpers shared by adapters and the catalog.

use serde_json::Value;

/// Deep-merge `overlay` into `base`: objects merge, everything else is replaced.
pub fn deep_merge(base: &mut Value, overlay: &Value) {
    match (base.as_object_mut(), overlay.as_object()) {
        (Some(target), Some(source)) => {
            for (key, value) in source {
                match target.get_mut(key) {
                    Some(existing) => deep_merge(existing, value),
                    None => {
                        target.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        _ if !overlay.is_null() => *base = overlay.clone(),
        _ => {}
    }
}

/// Remove credential-shaped keys from a request body; keys are never sent in bodies.
pub fn strip_credentials(body: &mut Value) {
    if let Some(map) = body.as_object_mut() {
        map.retain(|k, _| !matches!(k.as_str(), "apiKey" | "api_key" | "apikey"));
        for value in map.values_mut() {
            strip_credentials(value);
        }
    }
}

pub fn u64_at(value: &Value, pointer: &str) -> u64 {
    value.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

pub fn str_at<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
}
