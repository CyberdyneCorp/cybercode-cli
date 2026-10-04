//! Input validation against the built-in tools' JSON Schemas
//! (`tool-registry` → Codec boundary on settlement).
//!
//! Covers what built-in schemas use: object properties, `required`, primitive types,
//! arrays with item schemas, `enum`, and `additionalProperties: false`.

use serde_json::Value;

/// `Err("Invalid tool input: <json-pointer>: <reason>")` on the first violation.
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    check(schema, value, "").map_err(|(pointer, reason)| {
        let pointer = if pointer.is_empty() {
            "/".to_string()
        } else {
            pointer
        };
        format!("Invalid tool input: {pointer}: {reason}")
    })
}

fn check(schema: &Value, value: &Value, pointer: &str) -> Result<(), (String, String)> {
    if let Some(kind) = schema.get("type").and_then(Value::as_str)
        && !type_matches(kind, value)
    {
        return Err((pointer.into(), format!("expected {kind}")));
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array)
        && !options.contains(value)
    {
        return Err((
            pointer.into(),
            format!("expected one of {}", Value::Array(options.clone())),
        ));
    }
    if let Some(object) = value.as_object() {
        check_object(schema, object, pointer)?;
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (i, item) in array.iter().enumerate() {
            check(items, item, &format!("{pointer}/{i}"))?;
        }
    }
    Ok(())
}

fn check_object(
    schema: &Value,
    object: &serde_json::Map<String, Value>,
    pointer: &str,
) -> Result<(), (String, String)> {
    for name in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !object.contains_key(name) {
            return Err((format!("{pointer}/{name}"), "required".into()));
        }
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    for (name, item) in object {
        match properties.and_then(|p| p.get(name)) {
            Some(sub) => check(sub, item, &format!("{pointer}/{name}"))?,
            None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                return Err((format!("{pointer}/{name}"), "unknown property".into()));
            }
            None => {}
        }
    }
    Ok(())
}

fn type_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "null" => value.is_null(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::validate;
    use serde_json::json;

    #[test]
    fn reports_the_first_violation_with_a_pointer() {
        let schema = json!({"type": "object", "required": ["path", "old_string"], "properties": {
            "path": {"type": "string"}, "old_string": {"type": "string"}, "limit": {"type": "integer"}}});
        assert_eq!(
            validate(&schema, &json!({"path": "a"})).unwrap_err(),
            "Invalid tool input: /old_string: required"
        );
        assert_eq!(
            validate(
                &schema,
                &json!({"path": "a", "old_string": "x", "limit": "5"})
            )
            .unwrap_err(),
            "Invalid tool input: /limit: expected integer"
        );
        assert!(
            validate(
                &schema,
                &json!({"path": "a", "old_string": "x", "limit": 5})
            )
            .is_ok()
        );
    }
}
