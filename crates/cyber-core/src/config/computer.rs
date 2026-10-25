use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ComputerSettings {
    pub enabled: bool,
    pub retention_days: u64,
}
impl ComputerSettings {
    pub fn from_global(config: &Value) -> Result<Self, String> {
        #[derive(Deserialize, Default)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            retention_days: Option<u64>,
        }
        let raw = match config.get("computer") {
            Some(value) => serde_json::from_value::<Raw>(value.clone())
                .map_err(|_| "computer: expected { retention_days: 0..365 }".to_owned())?,
            None => Raw::default(),
        };
        let days = raw.retention_days.unwrap_or(1);
        if days > 365 {
            return Err("computer.retention_days must be 0..365".into());
        }
        let enabled = config
            .pointer("/features/computer_use")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && config.pointer("/policy/features/lock/computer_use") != Some(&Value::Bool(false));
        Ok(Self {
            enabled,
            retention_days: days,
        })
    }
}

pub(super) fn layer_issue(label: &str, layer: &Value) -> Option<String> {
    let here = layer.get("computer").is_some()
        || layer.pointer("/features/computer_use").is_some()
        || layer.pointer("/experimental/computer_use").is_some();
    let nested = layer
        .get("profiles")
        .and_then(Value::as_object)
        .is_some_and(|profiles| {
            profiles
                .values()
                .any(|profile| layer_issue(label, profile).is_some())
        });
    (here||nested).then(||format!("computer and features.computer_use are accepted only in trusted top-level user/managed global configuration, not {label}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_defaults_are_private_and_disabled() {
        assert_eq!(
            ComputerSettings::from_global(&json!({})).unwrap(),
            ComputerSettings {
                enabled: false,
                retention_days: 1
            }
        );
        assert!(!ComputerSettings::from_global(&json!({"features":{"computer_use":true},"policy":{"features":{"lock":{"computer_use":false}}}})).unwrap().enabled);
        assert!(
            ComputerSettings::from_global(&json!({"computer":{"retention_days":366}})).is_err()
        );
        assert!(ComputerSettings::from_global(&json!({"computer":{"retention_days":0}})).is_ok());
        assert!(
            layer_issue(
                "project:fixture",
                &json!({"profiles":{"unsafe":{"features":{"computer_use":true}}}})
            )
            .is_some()
        );
    }
}
