//! The one tool-registry deferral threshold, shared across registration scopes.
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeferredToolSettings {
    pub threshold_tokens: usize,
}
impl Default for DeferredToolSettings {
    fn default() -> Self {
        Self {
            threshold_tokens: 10_000,
        }
    }
}
impl DeferredToolSettings {
    pub fn from_config(value: &Value) -> Result<Self, String> {
        let Some(output) = value.get("tool_output") else {
            return Ok(Self::default());
        };
        let output = output.as_object().ok_or("tool_output must be an object")?;
        let Some(threshold) = output.get("deferred_threshold_tokens") else {
            return Ok(Self::default());
        };
        let threshold_tokens = threshold.as_u64().and_then(|value| usize::try_from(value).ok())
            .ok_or("tool_output.deferred_threshold_tokens must be a nonnegative integer that fits this platform")?;
        Ok(Self { threshold_tokens })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn default_and_only_documented_key() {
        assert_eq!(
            DeferredToolSettings::from_config(&json!({}))
                .unwrap()
                .threshold_tokens,
            10_000
        );
        assert_eq!(
            DeferredToolSettings::from_config(&json!({"tool_output":{"max_bytes":200}}))
                .unwrap()
                .threshold_tokens,
            10_000
        );
        assert_eq!(
            DeferredToolSettings::from_config(&json!({"mcp":{"deferred_threshold_tokens":1}}))
                .unwrap()
                .threshold_tokens,
            10_000
        );
        for threshold in [0, 1, 10000, 50000] {
            assert_eq!(
                DeferredToolSettings::from_config(
                    &json!({"tool_output":{"deferred_threshold_tokens":threshold}})
                )
                .unwrap()
                .threshold_tokens,
                threshold
            );
        }
    }
    #[test]
    fn rejects_invalid_values_without_echoing_them() {
        for threshold in [
            json!(-1),
            json!(1.5),
            json!("private-value"),
            json!(null),
            json!(true),
        ] {
            let error = DeferredToolSettings::from_config(
                &json!({"tool_output":{"deferred_threshold_tokens":threshold}}),
            )
            .unwrap_err();
            assert!(!error.contains("private-value"));
        }
        assert!(DeferredToolSettings::from_config(&json!({"tool_output":[]})).is_err());
    }
}
