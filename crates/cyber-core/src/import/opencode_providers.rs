//! Static OpenCode provider proposals; source packages are never loaded.
use super::{ConversionError, RequiredEnvironment};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct OpenCodeProviderConfig {
    pub config: Value,
    pub required_environment: Vec<RequiredEnvironment>,
    pub not_imported: Vec<super::ProviderMappingIssue>,
    /// Native leaf pointer to raw source pointers (values are never included).
    pub field_sources: BTreeMap<String, Vec<String>>,
}
fn error(field: &str, reason: &'static str) -> ConversionError {
    ConversionError {
        field: field.into(),
        reason,
    }
}
fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str, ConversionError> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 16384 && !s.contains('\0'))
        .ok_or_else(|| error(field, "expected a bounded static string"))
}
fn env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 256
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn encoded(value: &str) -> String {
    value.bytes().map(|b| format!("{b:02X}")).collect()
}
fn binding(
    id: &str,
    suffix: &str,
    value: &Value,
    pointer: &str,
    result: &mut OpenCodeProviderConfig,
) -> Result<Value, ConversionError> {
    let value = value
        .as_str()
        .filter(|s| s.len() <= 16384 && !s.contains(['\0', '\r', '\n']))
        .ok_or_else(|| error("providers", "invalid credential or header value"))?;
    let (variable, literal) = if let Some(name) = value
        .strip_prefix("{env:")
        .and_then(|s| s.strip_suffix('}'))
    {
        if !env_name(name) {
            return Err(error("providers", "invalid environment reference"));
        }
        (name.into(), false)
    } else {
        if value.contains(['{', '}']) {
            return Err(error(
                "providers",
                "source substitutions need an additional adapter",
            ));
        }
        (
            format!("CYBER_IMPORT_OPENCODE_{}_{suffix}", encoded(id)),
            true,
        )
    };
    if !env_name(&variable) {
        return Err(error(
            "providers",
            "generated environment name exceeds bounds",
        ));
    }
    result.required_environment.push(RequiredEnvironment {
        field: pointer.into(),
        variable: variable.clone(),
        from_literal: literal,
    });
    Ok(json!(format!("{{env:{variable}}}")))
}
fn mapped(result: &mut OpenCodeProviderConfig, native: &str, source: &str) {
    result
        .field_sources
        .insert(native.into(), vec![source.into()]);
}
fn pending(result: &mut OpenCodeProviderConfig, pointer: String) {
    result.not_imported.push(super::ProviderMappingIssue {
        field: pointer,
        reason: "provider field requires an additional adapter; value withheld",
    });
}
fn protocol(package: &str) -> Option<&'static str> {
    match package {
        "@ai-sdk/openai-compatible"
        | "@opencode/ai/providers/openai-compatible"
        | "@opencode/ai/providers/openai/chat" => Some("openai-compatible"),
        "@ai-sdk/openai"
        | "@opencode/ai/providers/openai/responses"
        | "@opencode/ai/providers/openai-compatible/responses" => Some("openai-responses"),
        "@ai-sdk/anthropic"
        | "@opencode/ai/providers/anthropic"
        | "@opencode/ai/providers/anthropic-compatible" => Some("anthropic"),
        _ => None,
    }
}
fn headers(
    id: &str,
    source: &Value,
    pointer: &str,
    native: &str,
    result: &mut OpenCodeProviderConfig,
) -> Result<Value, ConversionError> {
    let source = source
        .as_object()
        .filter(|m| m.len() <= 128)
        .ok_or_else(|| error("providers", "expected at most 128 headers"))?;
    let mut output = Map::new();
    let mut names = std::collections::BTreeSet::new();
    for (key, value) in source {
        if key.is_empty()
            || key.len() > 256
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            || !names.insert(key.to_ascii_lowercase())
        {
            return Err(error("providers", "invalid or case-ambiguous headers"));
        }
        let source = super::provenance::child(pointer, key);
        output.insert(
            key.clone(),
            binding(
                id,
                &format!("HEADER_{}", encoded(key)),
                value,
                &source,
                result,
            )?,
        );
        mapped(result, &super::provenance::child(native, key), &source);
    }
    Ok(Value::Object(output))
}
fn models(
    source: &Value,
    pointer: &str,
    native: &str,
    v2: bool,
    result: &mut OpenCodeProviderConfig,
) -> Result<Value, ConversionError> {
    let source = source
        .as_object()
        .filter(|m| m.len() <= 256)
        .ok_or_else(|| error("providers", "expected at most 256 models"))?;
    let mut output = Map::new();
    for (id, value) in source {
        if id.is_empty()
            || id.len() > 256
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:/-".contains(&b))
        {
            return Err(error("providers", "invalid model identifier"));
        }
        let model = value
            .as_object()
            .filter(|m| m.len() <= 64)
            .ok_or_else(|| error("providers", "expected a bounded model object"))?;
        let source = super::provenance::child(pointer, id);
        let target = super::provenance::child(native, id);
        let mut converted = json!({});
        for (key, value) in model {
            let raw = super::provenance::child(&source, key);
            let destination = match key.as_str() {
                "name" => "name",
                "modelID" if v2 => "id",
                "id" if !v2 => "id",
                "disabled" => {
                    if !value.is_boolean() {
                        return Err(error("providers", "model disabled flag must be boolean"));
                    }
                    "disabled"
                }
                "limit" => "limits",
                _ => {
                    pending(result, raw);
                    continue;
                }
            };
            if destination == "limits" {
                let limits = value
                    .as_object()
                    .filter(|m| m.len() <= 16)
                    .ok_or_else(|| error("providers", "invalid model limits"))?;
                converted[destination] = json!({});
                for (limit, value) in limits {
                    let raw = super::provenance::child(&raw, limit);
                    if !matches!(limit.as_str(), "context" | "input" | "output") {
                        pending(result, raw);
                        continue;
                    }
                    if value.as_u64().is_none_or(|n| n == 0) {
                        return Err(error("providers", "model limits must be positive integers"));
                    }
                    converted[destination][limit] = value.clone();
                    mapped(
                        result,
                        &super::provenance::child(
                            &super::provenance::child(&target, destination),
                            limit,
                        ),
                        &raw,
                    );
                }
            } else {
                converted[destination] = if destination == "disabled" {
                    value.clone()
                } else {
                    json!(text(value, "providers")?)
                };
                mapped(
                    result,
                    &super::provenance::child(&target, destination),
                    &raw,
                );
            }
        }
        if model.is_empty() {
            mapped(result, &target, &source);
        }
        output.insert(id.clone(), converted);
    }
    Ok(Value::Object(output))
}

fn endpoint_value(
    id: &str,
    endpoint: &Value,
    raw: &str,
    provider: &Value,
    result: &mut OpenCodeProviderConfig,
) -> Result<Value, ConversionError> {
    let text = text(endpoint, "providers")?;
    let url = if text.starts_with("{env:") {
        binding(id, "ENDPOINT", endpoint, raw, result)?
    } else {
        let parsed = url::Url::parse(text).map_err(|_| error("providers", "invalid endpoint"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || text.contains(['{', '}'])
        {
            return Err(error(
                "providers",
                "unsafe endpoint requires an additional adapter",
            ));
        }
        if super::preview::sensitive_values(provider)
            .iter()
            .any(|secret| text.contains(secret))
        {
            binding(id, "ENDPOINT", endpoint, raw, result)?
        } else {
            json!(text)
        }
    };
    Ok(url)
}

fn provider(
    id: &str,
    value: &Value,
    pointer: &str,
    v2: bool,
    result: &mut OpenCodeProviderConfig,
) -> Result<Option<Value>, ConversionError> {
    let source = value
        .as_object()
        .filter(|m| m.len() <= 64)
        .ok_or_else(|| error("providers", "expected a bounded provider object"))?;
    let package_key = if v2 { "package" } else { "npm" };
    let settings_key = if v2 { "settings" } else { "options" };
    let Some(package) = source.get(package_key) else {
        pending(result, pointer.into());
        return Ok(None);
    };
    let Some(kind) = protocol(text(package, "providers")?) else {
        pending(result, pointer.into());
        return Ok(None);
    };
    let native = super::provenance::child("/providers", id);
    let mut output = json!({"api":{"type":kind}});
    mapped(
        result,
        &format!("{native}/api/type"),
        &super::provenance::child(pointer, package_key),
    );
    if let Some(name) = source.get("name") {
        output["name"] = json!(text(name, "providers")?);
        mapped(
            result,
            &format!("{native}/name"),
            &super::provenance::child(pointer, "name"),
        );
    }
    let settings = source
        .get(settings_key)
        .cloned()
        .unwrap_or_else(|| json!({}));
    let settings = settings
        .as_object()
        .filter(|m| m.len() <= 64)
        .ok_or_else(|| error("providers", "expected bounded provider settings"))?;
    let settings_pointer = super::provenance::child(pointer, settings_key);
    if let Some(endpoint) = settings.get("baseURL") {
        let raw = super::provenance::child(&settings_pointer, "baseURL");
        let url = endpoint_value(id, endpoint, &raw, value, result)?;
        output["api"]["url"] = url;
        mapped(result, &format!("{native}/api/url"), &raw);
    }
    if let Some(key) = settings.get("apiKey") {
        let raw = super::provenance::child(&settings_pointer, "apiKey");
        output["api"]["settings"] = json!({"api_key":binding(id,"API_KEY",key,&raw,result)?});
        mapped(result, &format!("{native}/api/settings/api_key"), &raw);
    }
    if let Some(env) = source.get("env") {
        let env = env.as_array().filter(|a| a.len() <= 32).ok_or_else(|| {
            error(
                "providers",
                "expected at most 32 credential environment names",
            )
        })?;
        output["env"] = json!([]);
        for (index, name) in env.iter().enumerate() {
            let name = name
                .as_str()
                .filter(|s| env_name(s))
                .ok_or_else(|| error("providers", "invalid credential environment name"))?;
            output["env"].as_array_mut().unwrap().push(json!(name));
            mapped(
                result,
                &format!("{native}/env/{index}"),
                &format!("{pointer}/env/{index}"),
            );
        }
    }
    let top_headers = source.get("headers");
    let option_headers = settings.get("headers");
    if top_headers.is_some() && option_headers.is_some() {
        return Err(error("providers", "ambiguous provider header locations"));
    }
    if let Some(input) = top_headers.or(option_headers) {
        let raw = if top_headers.is_some() {
            super::provenance::child(pointer, "headers")
        } else {
            super::provenance::child(&settings_pointer, "headers")
        };
        output["request"] =
            json!({"headers":headers(id,input,&raw,&format!("{native}/request/headers"),result)?});
    }
    if let Some(input) = source.get("models") {
        output["models"] = models(
            input,
            &format!("{pointer}/models"),
            &format!("{native}/models"),
            v2,
            result,
        )?;
    }
    for key in source.keys() {
        if ![
            package_key,
            settings_key,
            "name",
            "env",
            "headers",
            "models",
        ]
        .contains(&key.as_str())
        {
            pending(result, super::provenance::child(pointer, key));
        }
    }
    for key in settings.keys() {
        if !["baseURL", "apiKey", "headers"].contains(&key.as_str()) {
            pending(result, super::provenance::child(&settings_pointer, key));
        }
    }
    Ok(Some(output))
}

pub fn opencode_provider_config(
    document: &Value,
) -> Result<OpenCodeProviderConfig, ConversionError> {
    let source = document
        .as_object()
        .filter(|m| m.len() <= 256)
        .ok_or_else(|| error("providers", "expected a bounded source object"))?;
    let mut result = OpenCodeProviderConfig {
        config: json!({}),
        required_environment: Vec::new(),
        not_imported: Vec::new(),
        field_sources: BTreeMap::new(),
    };
    if source.contains_key("provider") && source.contains_key("providers") {
        return Err(error("providers", "ambiguous provider aliases"));
    }
    let key = if source.contains_key("provider") {
        "provider"
    } else {
        "providers"
    };
    let Some(input) = source.get(key) else {
        return Ok(result);
    };
    let input = input
        .as_object()
        .filter(|m| m.len() <= 64)
        .ok_or_else(|| error("providers", "expected at most 64 providers"))?;
    let mut output = Map::new();
    for (id, value) in input {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(error("providers", "invalid provider identifier"));
        }
        let pointer = super::provenance::child(&format!("/{key}"), id);
        if let Some(provider) = provider(id, value, &pointer, key == "providers", &mut result)? {
            output.insert(id.clone(), provider);
        }
    }
    if !output.is_empty() {
        result.config = json!({"providers":output});
    }
    let mut bindings = result.required_environment.clone();
    for provider in output_environment_names(&result.config) {
        bindings.push(RequiredEnvironment {
            field: "providers.env".into(),
            variable: provider.into(),
            from_literal: false,
        });
    }
    super::codex_providers::validate_environment_bindings(&bindings)?;
    let secrets = super::preview::sensitive_values(document);
    super::preview::reject_unsafe_strings(
        &result.config,
        &json!({}),
        &secrets,
        std::path::Path::new("providers"),
    )
    .map_err(|_| {
        error(
            "providers",
            "retained provider fields contain credentials or unsafe substitutions",
        )
    })?;
    reject_metadata(&result.config, &secrets)?;
    for pointer in result
        .not_imported
        .iter()
        .map(|item| &item.field)
        .chain(result.field_sources.keys())
        .chain(result.field_sources.values().flatten())
    {
        if secrets.iter().any(|secret| pointer.contains(secret)) {
            return Err(error(
                "providers",
                "source attribution contains a declared credential",
            ));
        }
    }
    Ok(result)
}
fn reject_metadata(value: &Value, secrets: &[String]) -> Result<(), ConversionError> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if secrets.iter().any(|secret| key.contains(secret)) {
                    return Err(error(
                        "providers",
                        "metadata contains a declared credential",
                    ));
                }
                reject_metadata(value, secrets)?;
            }
        }
        Value::Array(items) => {
            for value in items {
                reject_metadata(value, secrets)?;
            }
        }
        Value::String(text)
            if text.starts_with("{env:") && secrets.iter().any(|s| text.contains(s)) =>
        {
            return Err(error(
                "providers",
                "environment metadata contains a declared credential",
            ));
        }
        _ => (),
    }
    Ok(())
}

fn output_environment_names(config: &Value) -> impl Iterator<Item = &str> {
    config
        .get("providers")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|providers| providers.values())
        .flat_map(|provider| {
            provider
                .get("env")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(Value::as_str)
}
