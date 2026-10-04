//! Deep merge of config layers with per-leaf source attribution.

use std::collections::BTreeMap;

use serde_json::Value;

/// Top-level keys whose arrays concatenate (de-duplicated) instead of being replaced.
const CONCAT_KEYS: [&str; 4] = ["/instructions", "/plugins", "/skills", "/skills/paths"];

pub type Sources = BTreeMap<String, String>;

pub fn merge_layer(base: &mut Value, overlay: &Value, source: &str, sources: &mut Sources) {
    merge_value(base, overlay, "", source, sources);
}

fn merge_value(
    slot: &mut Value,
    overlay: &Value,
    pointer: &str,
    source: &str,
    sources: &mut Sources,
) {
    if let (Some(map), Some(over)) = (slot.as_object_mut(), overlay.as_object()) {
        for (key, value) in over {
            let child = child_pointer(pointer, key);
            match map.get_mut(key) {
                Some(existing) => merge_value(existing, value, &child, source, sources),
                None => {
                    map.insert(key.clone(), value.clone());
                    record(value, &child, source, sources);
                }
            }
        }
        return;
    }
    if CONCAT_KEYS.contains(&pointer)
        && let (Some(items), Some(over)) = (slot.as_array_mut(), overlay.as_array())
    {
        for item in over {
            if !items.contains(item) {
                items.push(item.clone());
            }
        }
        sources.insert(pointer.to_string(), source.to_string());
        return;
    }
    *slot = overlay.clone();
    clear_under(pointer, sources);
    record(overlay, pointer, source, sources);
}

fn record(value: &Value, pointer: &str, source: &str, sources: &mut Sources) {
    match value.as_object() {
        Some(map) if !map.is_empty() => {
            for (key, child) in map {
                record(child, &child_pointer(pointer, key), source, sources);
            }
        }
        _ => {
            sources.insert(pointer.to_string(), source.to_string());
        }
    }
}

fn clear_under(pointer: &str, sources: &mut Sources) {
    let prefix = format!("{pointer}/");
    sources.retain(|k, _| k != pointer && !k.starts_with(&prefix));
}

pub fn child_pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nearest_scalar_wins_and_objects_merge() {
        let mut base = json!({"mode": "default", "tools": {"a": 1}});
        let mut sources = Sources::new();
        merge_layer(
            &mut base,
            &json!({"mode": "plan", "tools": {"b": 2}}),
            "p",
            &mut sources,
        );
        assert_eq!(base, json!({"mode": "plan", "tools": {"a": 1, "b": 2}}));
        assert_eq!(sources["/mode"], "p");
    }

    #[test]
    fn instructions_concatenate_other_arrays_replace() {
        let mut base = json!({"instructions": ["a"], "x": [1]});
        let mut s = Sources::new();
        merge_layer(
            &mut base,
            &json!({"instructions": ["b", "a"], "x": [2]}),
            "p",
            &mut s,
        );
        assert_eq!(base, json!({"instructions": ["a", "b"], "x": [2]}));
    }
}
