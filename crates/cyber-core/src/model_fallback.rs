//! The model fallback chain (`provider-catalog` → Model fallback chain): `fallback_models`,
//! the ordered models a failed Turn is re-sent on, and `fallback.sticky_minutes`, how
//! long a Session stays on the fallback before returning to its configured model.

use serde_json::Value;

const DEFAULT_STICKY_MINUTES: u64 = 10;
/// A day: the longest a Session stays on a fallback.
const MAX_STICKY_MINUTES: u64 = 1440;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackSettings {
    /// Model refs, in the order they are tried.
    pub models: Vec<String>,
    /// At least one minute, so the Turn re-sent on the fallback always uses it.
    pub sticky_minutes: u64,
}

impl Default for FallbackSettings {
    fn default() -> Self {
        Self {
            models: Vec::new(),
            sticky_minutes: DEFAULT_STICKY_MINUTES,
        }
    }
}

impl FallbackSettings {
    /// `fallback_models` and `fallback` of `config`; an invalid field is an error naming it.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self::default();
        if let Some(models) = config.get("fallback_models").filter(|v| !v.is_null()) {
            settings.models = models
                .as_array()
                .and_then(|models| {
                    models
                        .iter()
                        .map(|m| m.as_str().filter(|r| r.contains('/')).map(str::to_owned))
                        .collect()
                })
                .ok_or("fallback_models: expected a list of provider/model refs")?;
        }
        let Some(value) = config.get("fallback").filter(|v| !v.is_null()) else {
            return Ok(settings);
        };
        let map = value.as_object().ok_or("fallback: expected an object")?;
        for (field, value) in map {
            match field.as_str() {
                "sticky_minutes" => {
                    settings.sticky_minutes = value
                        .as_u64()
                        .filter(|m| (1..=MAX_STICKY_MINUTES).contains(m))
                        .ok_or("fallback.sticky_minutes: expected minutes from 1 to 1440")?;
                }
                other => return Err(format!("fallback.{other}: unknown setting")),
            }
        }
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_and_configured_values() {
        assert_eq!(
            FallbackSettings::from_config(&json!({})).unwrap(),
            FallbackSettings::default()
        );
        let settings = FallbackSettings::from_config(&json!({
            "fallback_models": ["anthropic/claude-sonnet", "openai/gpt-6"],
            "fallback": {"sticky_minutes": 30}
        }))
        .unwrap();
        assert_eq!(settings.models, ["anthropic/claude-sonnet", "openai/gpt-6"]);
        assert_eq!(settings.sticky_minutes, 30);
    }

    #[test]
    fn invalid_values_name_the_field() {
        for (config, error) in [
            (
                json!({"fallback_models": "openai/gpt-6"}),
                "fallback_models:",
            ),
            (json!({"fallback_models": ["gpt-6"]}), "fallback_models:"),
            (
                json!({"fallback": {"sticky_minutes": 0}}),
                "fallback.sticky_minutes:",
            ),
            (
                json!({"fallback": {"sticky_minutes": 2000}}),
                "fallback.sticky_minutes:",
            ),
            (json!({"fallback": {"sticky": 5}}), "fallback.sticky:"),
        ] {
            let message = FallbackSettings::from_config(&config).unwrap_err();
            assert!(message.starts_with(error), "{config}: {message}");
        }
    }
}
