//! Pure provider/model conversion; secrets become environment references, never copied values.
use super::ConversionError;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize)]
pub struct RequiredEnvironment {
    pub field: String,
    pub variable: String,
    pub from_literal: bool,
}
#[derive(Debug, Clone, Serialize)]
pub struct ProviderMappingIssue {
    pub field: String,
    pub reason: &'static str,
}
#[derive(Debug, Clone, Serialize)]
pub struct CodexProviderConfig {
    pub config: Value,
    pub required_environment: Vec<RequiredEnvironment>,
    pub not_imported: Vec<ProviderMappingIssue>,
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
        .filter(|s| !s.is_empty() && s.len() <= 16384)
        .ok_or_else(|| error(field, "expected a bounded nonempty string"))
}
fn identifier(value: &str, field: &str) -> Result<(), ConversionError> {
    if value.len() > 128
        || value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(error(field, "invalid provider identifier"));
    }
    Ok(())
}
fn env_name(value: &str, field: &str) -> Result<String, ConversionError> {
    let mut bytes = value.bytes();
    if value.len() > 256
        || !bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(error(field, "invalid environment variable name"));
    }
    Ok(value.into())
}
fn generated_variable(id: &str, suffix: &str) -> String {
    // Byte encoding distinguishes case and dash/underscore rather than colliding sanitized IDs.
    let encoded: String = id.bytes().map(|b| format!("{b:02X}")).collect();
    format!("CYBER_IMPORT_CODEX_{encoded}_{suffix}")
}
fn reference(
    variable: String,
    field: &str,
    from_literal: bool,
    required: &mut Vec<RequiredEnvironment>,
) -> Value {
    required.push(RequiredEnvironment {
        field: field.into(),
        variable: variable.clone(),
        from_literal,
    });
    Value::String(format!("{{env:{variable}}}"))
}
fn endpoint(value: &Value, field: &str) -> Result<Value, ConversionError> {
    let value = text(value, field)?;
    let url = url::Url::parse(value).map_err(|_| error(field, "invalid provider endpoint"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || value.contains(['{', '}'])
    {
        return Err(error(
            field,
            "endpoint credentials, queries, fragments or placeholders require manual conversion",
        ));
    }
    Ok(Value::String(value.into()))
}
/// Parse bounded TOML without executing source expressions or including parser values in errors.
pub fn codex_provider_source(source: &str) -> Result<CodexProviderConfig, ConversionError> {
    if source.len() > 1024 * 1024 {
        return Err(error("settings", "source exceeds one MiB"));
    }
    let table = source
        .parse::<toml::Table>()
        .map_err(|_| error("settings", "invalid source TOML"))?;
    let value = serde_json::to_value(table)
        .map_err(|_| error("settings", "source TOML cannot be represented"))?;
    codex_provider_config(&value)
}
/// Convert provider/model settings only. Other config fields are explicit pending mappings.
pub fn codex_provider_config(settings: &Value) -> Result<CodexProviderConfig, ConversionError> {
    let source = settings
        .as_object()
        .ok_or_else(|| error("settings", "expected an object"))?;
    if source.len() > 256 {
        return Err(error("settings", "too many source fields"));
    }
    let mut result = CodexProviderConfig {
        config: json!({}),
        required_environment: Vec::new(),
        not_imported: Vec::new(),
    };
    if let Some(providers) = source.get("model_providers") {
        let providers = providers
            .as_object()
            .filter(|p| p.len() <= 64)
            .ok_or_else(|| error("model_providers", "expected at most 64 providers"))?;
        let mut output = Map::new();
        for (index, (id, provider)) in providers.iter().enumerate() {
            let field = format!("model_providers[{index}]");
            identifier(id, &field)?;
            output.insert(
                id.clone(),
                provider_config(id, provider, &field, &mut result)?,
            );
        }
        result.config["providers"] = Value::Object(output);
    }
    if let Some(model) = source.get("model") {
        selected_model(source, model, &mut result)?;
    } else if source.contains_key("model_provider") {
        result.not_imported.push(ProviderMappingIssue {
            field: "model_provider".into(),
            reason: "provider selection without a model needs layer-aware resolution",
        });
    }
    validate_environment_bindings(&result.required_environment)?;
    validate_retained_fields(source, &result)?;
    pending_fields(
        source,
        &["model_providers", "model", "model_provider"],
        "settings",
        &mut result,
    );
    Ok(result)
}
fn selected_model(
    source: &Map<String, Value>,
    model: &Value,
    result: &mut CodexProviderConfig,
) -> Result<(), ConversionError> {
    let model = text(model, "model")?;
    if model.len() > 256
        || !model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:/-".contains(&b))
    {
        return Err(error(
            "model",
            "model identifier requires manual conversion",
        ));
    }
    let provider = source
        .get("model_provider")
        .map(|v| text(v, "model_provider"))
        .transpose()?
        .unwrap_or("openai");
    identifier(provider, "model_provider")?;
    result.config["model"] = Value::String(format!("{provider}/{model}"));
    // Explicit custom models must exist in the native catalog; native defaults retain unknown pricing.
    if let Some(config) = result
        .config
        .get_mut("providers")
        .and_then(|p| p.get_mut(provider))
    {
        config["models"] = json!({model: {}});
    }
    Ok(())
}
fn provider_config(
    id: &str,
    value: &Value,
    field: &str,
    result: &mut CodexProviderConfig,
) -> Result<Value, ConversionError> {
    let source = value
        .as_object()
        .filter(|p| p.len() <= 64)
        .ok_or_else(|| error(field, "expected a bounded provider object"))?;
    if let Some(auth) = source.get("requires_openai_auth")
        && auth.as_bool() != Some(false)
    {
        return Err(error(
            field,
            "OpenAI account authentication requires manual conversion",
        ));
    }
    let protocol = source
        .get("wire_api")
        .map(|v| text(v, field))
        .transpose()?
        .unwrap_or("responses");
    let kind = match protocol {
        "chat" => "openai-compatible",
        "responses" => "openai-responses",
        _ => return Err(error(field, "unsupported provider wire protocol")),
    };
    let mut output = json!({"api":{"type":kind}});
    if let Some(url) = source.get("base_url") {
        output["api"]["url"] =
            endpoint_config(id, source, url, field, &mut result.required_environment)?;
    }
    credentials(
        id,
        source,
        field,
        &mut output,
        &mut result.required_environment,
    )?;
    let headers = headers(id, source, field, &mut result.required_environment)?;
    if !headers.is_empty() {
        output["request"] = json!({"headers":headers});
    }
    pending_fields(
        source,
        &[
            "base_url",
            "wire_api",
            "env_key",
            "api_key",
            "experimental_bearer_token",
            "http_headers",
            "env_http_headers",
            "requires_openai_auth",
        ],
        field,
        result,
    );
    Ok(output)
}
fn credentials(
    id: &str,
    source: &Map<String, Value>,
    field: &str,
    output: &mut Value,
    required: &mut Vec<RequiredEnvironment>,
) -> Result<(), ConversionError> {
    let keys: Vec<_> = ["env_key", "api_key", "experimental_bearer_token"]
        .into_iter()
        .filter(|key| source.contains_key(*key))
        .collect();
    if keys.len() > 1 {
        return Err(error(field, "ambiguous provider credential sources"));
    }
    if let Some(key) = keys.first() {
        let text = text(&source[*key], field)?;
        let literal = *key != "env_key";
        let variable = if literal {
            generated_variable(id, "API_KEY")
        } else {
            env_name(text, field)?
        };
        output["api"]["settings"] =
            json!({"api_key":reference(variable, &format!("{field}.{key}"), literal, required)});
    }
    Ok(())
}
fn headers(
    id: &str,
    source: &Map<String, Value>,
    field: &str,
    required: &mut Vec<RequiredEnvironment>,
) -> Result<Map<String, Value>, ConversionError> {
    let mut output = Map::new();
    let mut names = BTreeSet::new();
    for (key, literal) in [("http_headers", true), ("env_http_headers", false)] {
        let Some(headers) = source.get(key) else {
            continue;
        };
        let headers = headers
            .as_object()
            .filter(|h| h.len() <= 32)
            .ok_or_else(|| error(field, "expected at most 32 headers per collection"))?;
        for (index, (name, value)) in headers.iter().enumerate() {
            let location = format!("{field}.{key}[{index}]");
            validate_header(name, &mut names, &location)?;
            let text = text(value, &location)?;
            if text.bytes().any(|b| (b < 32 && b != b'\t') || b == 127) {
                return Err(error(&location, "header value contains control bytes"));
            }
            let suffix: String = name.bytes().map(|b| format!("{b:02X}")).collect();
            let variable = if literal {
                generated_variable(id, &format!("HEADER_{suffix}"))
            } else {
                env_name(text, &location)?
            };
            output.insert(
                name.clone(),
                reference(variable, &location, literal, required),
            );
        }
    }
    Ok(output)
}
fn pending_fields(
    source: &Map<String, Value>,
    supported: &[&str],
    field: &str,
    result: &mut CodexProviderConfig,
) {
    for (index, key) in source.keys().enumerate() {
        if !supported.contains(&key.as_str()) {
            result.not_imported.push(ProviderMappingIssue {
                field: format!("{field}[{index}]"),
                reason: "source field requires an additional adapter; value withheld",
            });
        }
    }
}

fn validate_header(
    name: &str,
    names: &mut BTreeSet<String>,
    field: &str,
) -> Result<(), ConversionError> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        || !names.insert(name.to_ascii_lowercase())
    {
        return Err(error(field, "invalid or case-ambiguous HTTP header"));
    }
    Ok(())
}

fn environment_binding(name: &str) -> String {
    #[cfg(windows)]
    {
        name.to_ascii_uppercase()
    }
    #[cfg(not(windows))]
    {
        name.to_owned()
    }
}
pub(super) fn validate_environment_bindings(
    required: &[RequiredEnvironment],
) -> Result<(), ConversionError> {
    let generated: BTreeSet<_> = required
        .iter()
        .filter(|v| v.from_literal)
        .map(|v| environment_binding(&v.variable))
        .collect();
    for existing in required.iter().filter(|v| !v.from_literal) {
        if generated.contains(&environment_binding(&existing.variable)) {
            return Err(error(
                &existing.field,
                "generated credential variable conflicts with an existing source environment reference",
            ));
        }
    }
    Ok(())
}

fn endpoint_config(
    id: &str,
    source: &Map<String, Value>,
    value: &Value,
    field: &str,
    required: &mut Vec<RequiredEnvironment>,
) -> Result<Value, ConversionError> {
    let output = endpoint(value, field)?;
    let url = output.as_str().unwrap();
    let credentials = ["api_key", "experimental_bearer_token"]
        .into_iter()
        .filter_map(|key| source.get(key).and_then(Value::as_str));
    let headers = source
        .get("http_headers")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|h| h.values())
        .filter_map(Value::as_str);
    if credentials
        .chain(headers)
        .any(|secret| !secret.is_empty() && url.contains(secret))
    {
        return Ok(reference(
            generated_variable(id, "BASE_URL"),
            &format!("{field}.base_url"),
            true,
            required,
        ));
    }
    Ok(output)
}

fn validate_retained_fields(
    source: &Map<String, Value>,
    result: &CodexProviderConfig,
) -> Result<(), ConversionError> {
    let literals: Vec<_> = source
        .get("model_providers")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|p| p.values())
        .flat_map(|p| {
            ["api_key", "experimental_bearer_token"]
                .into_iter()
                .filter_map(move |key| p.get(key).and_then(Value::as_str))
        })
        .filter(|s| !s.is_empty())
        .collect();
    let check = |text: &str| -> Result<(), ConversionError> {
        if literals.iter().any(|secret| text.contains(secret)) {
            return Err(error(
                "settings",
                "declared credential reused in retained metadata requires manual conversion",
            ));
        }
        Ok(())
    };
    if let Some(model) = result.config.get("model").and_then(Value::as_str) {
        check(model)?;
    }
    for (id, provider) in result
        .config
        .get("providers")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        check(id)?;
        if let Some(url) = provider.pointer("/api/url").and_then(Value::as_str)
            && !url.starts_with("{env:")
        {
            check(url)?;
        }
        for name in provider
            .pointer("/request/headers")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|headers| headers.keys())
        {
            check(name)?;
        }
    }
    for existing in result
        .required_environment
        .iter()
        .filter(|v| !v.from_literal)
    {
        check(&existing.variable)?;
    }
    Ok(())
}
