//! Plugin settings (`plugins-marketplace` → User configuration): values live under
//! `plugins.<name>.config` and are validated against the manifest `userConfig` JSON
//! Schema. A property marked `"secret": true` never comes from a configuration file; its
//! value is read from the plugin secret store (the OS keyring) and reaches the host only
//! through `secrets/get`.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// The settings one plugin host starts with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginSettings {
    /// Non-secret values, passed in `initialize`.
    pub config: Value,
    /// Secret values by property name, served by `secrets/get`.
    pub secrets: BTreeMap<String, String>,
}

/// A `userConfig` property, for the install prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigField {
    pub name: String,
    pub description: Option<String>,
    pub secret: bool,
    pub required: bool,
}

/// The properties of a `userConfig` schema, in declaration order.
pub fn fields(schema: Option<&Value>) -> Vec<ConfigField> {
    let Some(schema) = schema else {
        return Vec::new();
    };
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, property)| ConfigField {
            name: name.clone(),
            description: property["description"].as_str().map(str::to_owned),
            secret: property["secret"] == true,
            required: required.contains(&name.as_str()),
        })
        .collect()
}

/// Resolve and validate `plugin`'s settings from the merged configuration. `secret`
/// reads one stored secret. Any problem names the setting and keeps the host from
/// starting.
pub fn resolve(
    plugin: &str,
    schema: Option<&Value>,
    config: &Value,
    secret: &dyn Fn(&str) -> Result<Option<String>, String>,
) -> Result<PluginSettings, String> {
    let prefix = format!("plugins.{plugin}.config");
    let values = match config.pointer(&format!("/plugins/{plugin}/config")) {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return Err(format!("{prefix} must be an object")),
    };
    let Some(schema) = schema else {
        if let Some(key) = values.keys().next() {
            return Err(format!(
                "{prefix}.{key}: plugin {plugin} declares no userConfig"
            ));
        }
        return Ok(PluginSettings {
            config: Value::Object(values),
            secrets: BTreeMap::new(),
        });
    };
    let mut secrets = BTreeMap::new();
    let mut instance = values.clone();
    for field in fields(Some(schema))
        .into_iter()
        .filter(|field| field.secret)
    {
        if values.contains_key(&field.name) {
            return Err(format!(
                "{prefix}.{} is secret and must not be set in a configuration file; \
                 run `cyber plugins configure {plugin}`",
                field.name
            ));
        }
        match secret(&field.name).map_err(|error| format!("{prefix}.{}: {error}", field.name))? {
            Some(value) => {
                instance.insert(field.name.clone(), Value::String(value.clone()));
                secrets.insert(field.name, value);
            }
            None if field.required => {
                return Err(format!(
                    "{prefix}.{} is required; run `cyber plugins configure {plugin}`",
                    field.name
                ));
            }
            None => {}
        }
    }
    validate(&prefix, schema, &Value::Object(instance))?;
    Ok(PluginSettings {
        config: Value::Object(values),
        secrets,
    })
}

fn validate(prefix: &str, schema: &Value, instance: &Value) -> Result<(), String> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| format!("{prefix}: invalid userConfig schema: {error}"))?;
    let Some(error) = validator.iter_errors(instance).next() else {
        return Ok(());
    };
    let path = error.instance_path().to_string();
    let field = path.trim_start_matches('/').replace('/', ".");
    let at = if field.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}.{field}")
    };
    // A secret's value never appears in the message.
    let message = if fields(Some(schema))
        .iter()
        .any(|f| f.secret && field.split('.').next() == Some(f.name.as_str()))
    {
        "is invalid".to_owned()
    } else {
        error.to_string()
    };
    Err(format!("{at}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "site": {"type": "string", "description": "Jira site"},
                "limit": {"type": "integer", "minimum": 1},
                "api_token": {"type": "string", "secret": true, "minLength": 8},
            },
            "required": ["site", "api_token"],
        })
    }

    #[test]
    fn settings_are_validated_and_secrets_come_only_from_the_store() {
        let config = json!({"plugins":{"jira":{"config":{"site":"acme","limit":5}}}});
        let stored = |_: &str| Ok(Some("token-1234".to_owned()));
        let settings = resolve("jira", Some(&schema()), &config, &stored).unwrap();
        assert_eq!(settings.config, json!({"site":"acme","limit":5}));
        assert_eq!(settings.secrets["api_token"], "token-1234");

        let missing = |_: &str| Ok(None);
        let error = resolve("jira", Some(&schema()), &config, &missing).unwrap_err();
        assert!(
            error.contains("plugins.jira.config.api_token is required"),
            "{error}"
        );
        let in_file = json!({"plugins":{"jira":{"config":{"site":"a","api_token":"x"}}}});
        let error = resolve("jira", Some(&schema()), &in_file, &stored).unwrap_err();
        assert!(error.contains("is secret"), "{error}");
        let wrong = json!({"plugins":{"jira":{"config":{"site":"a","limit":0}}}});
        let error = resolve("jira", Some(&schema()), &wrong, &stored).unwrap_err();
        assert!(error.starts_with("plugins.jira.config.limit:"), "{error}");
        let short = |_: &str| Ok(Some("short".to_owned()));
        let error = resolve("jira", Some(&schema()), &config, &short).unwrap_err();
        assert_eq!(error, "plugins.jira.config.api_token: is invalid");
        assert!(!error.contains("short"));
        let undeclared = resolve("jira", None, &config, &stored).unwrap_err();
        assert!(
            undeclared.contains("declares no userConfig"),
            "{undeclared}"
        );
    }

    #[test]
    fn fields_list_required_and_secret_properties() {
        let fields = fields(Some(&schema()));
        let names: Vec<_> = fields
            .iter()
            .map(|f| (f.name.as_str(), f.required, f.secret))
            .collect();
        assert_eq!(
            names,
            [
                ("site", true, false),
                ("limit", false, false),
                ("api_token", true, true)
            ]
        );
    }
}
