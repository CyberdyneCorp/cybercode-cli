//! Deep merge of config layers with per-leaf source attribution.

use std::collections::BTreeMap;

use serde_json::Value;

/// Top-level keys whose arrays concatenate (de-duplicated) instead of being replaced.
const CONCAT_KEYS: [&str; 4] = ["/instructions", "/plugins", "/skills", "/skills/paths"];

pub type Sources = BTreeMap<String, String>;

pub fn merge_layer(base: &mut Value, overlay: &Value, source: &str, sources: &mut Sources) {
    merge_value(base, overlay, "", source, sources);
}

pub fn merge_profile(base: &mut Value, overlay: &Value, name: &str, sources: &mut Sources) {
    let origins = profile_hook_origins(base, overlay, name, sources);
    merge_layer(base, overlay, &format!("profile:{name}"), sources);
    sources.extend(origins);
}

fn profile_hook_origins(base: &Value, overlay: &Value, name: &str, sources: &Sources) -> Sources {
    let mut origins = Sources::new();
    let Some(events) = overlay.get("hooks").and_then(Value::as_object) else {
        return origins;
    };
    let profile = child_pointer("/profiles", name);
    for (event, groups) in events {
        let Some(groups) = groups
            .as_array()
            .filter(|_| super::hooks::is_event_name(event))
        else {
            continue;
        };
        let target = child_pointer("/hooks", event);
        let offset = base
            .pointer(&target)
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        for index in 0..groups.len() {
            let original = format!("{profile}{target}/{index}");
            let destination = format!("{target}/{}", offset + index);
            let prefix = format!("{original}/");
            for (pointer, source) in sources
                .range(original.clone()..)
                .take_while(|(pointer, _)| *pointer == &original || pointer.starts_with(&prefix))
            {
                origins.insert(
                    format!("{destination}{}", &pointer[original.len()..]),
                    source.clone(),
                );
            }
        }
    }
    origins
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
    if append_arrays(slot, overlay, pointer, source, sources) {
        return;
    }
    *slot = overlay.clone();
    clear_under(pointer, sources);
    record(overlay, pointer, source, sources);
}

fn append_arrays(
    slot: &mut Value,
    overlay: &Value,
    pointer: &str,
    source: &str,
    sources: &mut Sources,
) -> bool {
    let (Some(items), Some(over)) = (slot.as_array_mut(), overlay.as_array()) else {
        return false;
    };
    if hook_event(pointer) {
        let offset = items.len();
        for (index, group) in over.iter().enumerate() {
            record_hook_group(
                group,
                &child_pointer(pointer, &(offset + index).to_string()),
                source,
                sources,
            );
            items.push(group.clone());
        }
        if !over.is_empty() {
            sources.insert(pointer.to_string(), source.to_string());
        }
        return true;
    }
    if !CONCAT_KEYS.contains(&pointer) {
        return false;
    }
    for item in over {
        if !items.contains(item) {
            items.push(item.clone());
        }
    }
    sources.insert(pointer.to_string(), source.to_string());
    true
}

fn record(value: &Value, pointer: &str, source: &str, sources: &mut Sources) {
    if hook_event(pointer)
        && let Some(groups) = value.as_array()
    {
        sources.insert(pointer.to_string(), source.to_string());
        for (index, group) in groups.iter().enumerate() {
            record_hook_group(
                group,
                &child_pointer(pointer, &index.to_string()),
                source,
                sources,
            );
        }
        return;
    }
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

fn hook_event(pointer: &str) -> bool {
    if let Some(event) = pointer.strip_prefix("/hooks/") {
        return super::hooks::is_event_name(event);
    }
    pointer
        .strip_prefix("/profiles/")
        .and_then(|rest| rest.split_once('/'))
        .and_then(|(_, rest)| rest.strip_prefix("hooks/"))
        .is_some_and(super::hooks::is_event_name)
}

fn record_hook_group(group: &Value, pointer: &str, source: &str, sources: &mut Sources) {
    sources.insert(pointer.to_string(), source.to_string());
    record(group, pointer, source, sources);
    if let Some(handlers) = group.get("hooks").and_then(Value::as_array) {
        let handlers_pointer = child_pointer(pointer, "hooks");
        for (index, handler) in handlers.iter().enumerate() {
            let handler_pointer = child_pointer(&handlers_pointer, &index.to_string());
            sources.insert(handler_pointer.clone(), source.to_string());
            record(handler, &handler_pointer, source, sources);
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

    #[test]
    fn hook_sources_cover_every_handler_and_clear_on_replacement() {
        let mut base = json!({});
        let mut sources = Sources::new();
        let group = json!({"hooks":[
            {"type":"command","command":"first"},
            {"type":"command","command":"second"}
        ]});
        for source in ["global", "project"] {
            merge_layer(
                &mut base,
                &json!({"hooks":{"PostToolUse":[group.clone()]}}),
                source,
                &mut sources,
            );
        }
        for (index, source) in ["global", "project"].into_iter().enumerate() {
            let pointer = format!("/hooks/PostToolUse/{index}");
            assert_eq!(sources[&pointer], source);
            for handler in 0..2 {
                let pointer = format!("{pointer}/hooks/{handler}");
                assert_eq!(sources[&pointer], source);
                assert_eq!(sources[&format!("{pointer}/command")], source);
            }
        }
        merge_layer(&mut base, &json!({"hooks":false}), "invalid", &mut sources);
        assert_eq!(sources["/hooks"], "invalid");
        assert!(!sources.keys().any(|key| key.starts_with("/hooks/")));
    }
}
