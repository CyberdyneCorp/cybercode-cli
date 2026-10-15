//! Argument validation for the served tools: the subset of JSON Schema their strict
//! `inputSchema`s use (`object` with `required` and `additionalProperties: false`,
//! `string` with length and `enum`, `integer` with bounds, `boolean`). A violation names
//! the offending argument as a JSON pointer, so the calling model can correct it.

use serde_json::{Map, Value};

use super::super::ApiError;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Violation {
    pub path: String,
    pub message: String,
}

impl From<Violation> for ApiError {
    fn from(v: Violation) -> Self {
        ApiError::invalid(format!("invalid arguments at {}: {}", v.path, v.message))
            .with_reason("schema")
            .with_field("path", v.path)
    }
}

pub(super) fn validate(schema: &Value, value: &Value) -> Result<(), Violation> {
    check(schema, value, "")
}

fn violation(path: &str, message: impl Into<String>) -> Violation {
    Violation {
        path: path.into(),
        message: message.into(),
    }
}

fn check(schema: &Value, value: &Value, path: &str) -> Result<(), Violation> {
    match schema["type"].as_str() {
        Some("object") => object(schema, value, path),
        Some("string") => string(schema, value, path),
        Some("integer") => integer(schema, value, path),
        Some("boolean") if !value.is_boolean() => Err(violation(path, "expected a boolean")),
        _ => Ok(()),
    }
}

fn object(schema: &Value, value: &Value, path: &str) -> Result<(), Violation> {
    let Some(map) = value.as_object() else {
        return Err(violation(path, "expected an object"));
    };
    let empty = Map::new();
    let properties = schema["properties"].as_object().unwrap_or(&empty);
    let closed = schema["additionalProperties"] == Value::Bool(false);
    for (key, item) in map {
        let child = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
        match properties.get(key) {
            Some(property) => check(property, item, &child)?,
            None if closed => return Err(violation(&child, "is not a known argument")),
            None => {}
        }
    }
    let required = schema["required"].as_array().map_or(&[][..], Vec::as_slice);
    match required
        .iter()
        .filter_map(Value::as_str)
        .find(|key| !map.contains_key(*key))
    {
        Some(key) => Err(violation(&format!("{path}/{key}"), "is required")),
        None => Ok(()),
    }
}

fn string(schema: &Value, value: &Value, path: &str) -> Result<(), Violation> {
    let Some(text) = value.as_str() else {
        return Err(violation(path, "expected a string"));
    };
    let chars = text.chars().count() as u64;
    if schema["minLength"].as_u64().is_some_and(|min| chars < min) {
        return Err(violation(
            path,
            format!("must have at least {} characters", schema["minLength"]),
        ));
    }
    if schema["maxLength"].as_u64().is_some_and(|max| chars > max) {
        return Err(violation(
            path,
            format!("must have at most {} characters", schema["maxLength"]),
        ));
    }
    match schema["enum"].as_array() {
        Some(allowed) if !allowed.iter().any(|a| a.as_str() == Some(text)) => Err(violation(
            path,
            format!("must be one of {}", Value::Array(allowed.clone())),
        )),
        _ => Ok(()),
    }
}

fn integer(schema: &Value, value: &Value, path: &str) -> Result<(), Violation> {
    let Some(number) = value.as_i64() else {
        return Err(violation(path, "expected an integer"));
    };
    let min = schema["minimum"].as_i64().unwrap_or(i64::MIN);
    let max = schema["maximum"].as_i64().unwrap_or(i64::MAX);
    if (min..=max).contains(&number) {
        Ok(())
    } else {
        Err(violation(path, format!("must be from {min} to {max}")))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> Value {
        json!({"type":"object","additionalProperties":false,"required":["id"],"properties":{
            "id":{"type":"string","minLength":1,"maxLength":4},
            "from":{"type":"string","enum":["start","end"]},
            "limit":{"type":"integer","minimum":1,"maximum":100},
            "wait":{"type":"boolean"}}})
    }

    #[test]
    fn violations_name_the_argument() {
        let path = |value: Value| validate(&schema(), &value).map_err(|v| v.path);
        assert_eq!(
            path(json!({"id":"a","from":"end","limit":5,"wait":true})),
            Ok(())
        );
        assert_eq!(path(json!({})), Err("/id".into()));
        assert_eq!(path(json!({"id":""})), Err("/id".into()));
        assert_eq!(path(json!({"id":"abcde"})), Err("/id".into()));
        assert_eq!(path(json!({"id":"a","from":"middle"})), Err("/from".into()));
        assert_eq!(path(json!({"id":"a","limit":500})), Err("/limit".into()));
        assert_eq!(path(json!({"id":"a","limit":1.5})), Err("/limit".into()));
        assert_eq!(path(json!({"id":"a","wait":"yes"})), Err("/wait".into()));
        assert_eq!(path(json!({"id":"a","a/b":1})), Err("/a~1b".into()));
        assert_eq!(path(json!([1])), Err(String::new()));
    }

    #[test]
    fn a_violation_is_an_invalid_request_with_reason_schema() {
        let error = ApiError::from(Violation {
            path: "/limit".into(),
            message: "must be from 1 to 200".into(),
        });
        let body = serde_json::to_value(&error.body).unwrap();
        assert_eq!(body["_tag"], "InvalidRequestError");
        assert_eq!(body["reason"], "schema");
        assert_eq!(body["path"], "/limit");
    }
}
