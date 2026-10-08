//! Validated auto-mode controls shared by config loading and tool admission.
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AutoModeSettings {
    pub rules: AutoModeRules,
    pub policy: String,
    pub classify_read_only: bool,
    pub fallback: AutoFallback,
}
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AutoModeRules {
    pub always_block: Vec<AutoPattern>,
    pub always_allow: Vec<AutoPattern>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoPattern {
    pub action: String,
    pub resource: String,
}
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoFallback {
    #[default]
    Ask,
    Deny,
}
impl AutoModeSettings {
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let Some(value) = config.pointer("/permissions/auto_mode") else {
            return Ok(Self::default());
        };
        let settings: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!("permissions.auto_mode: {error}"))?;
        if settings.policy.len() > 65_536 {
            return Err("permissions.auto_mode.policy exceeds 65536 bytes".into());
        }
        if settings
            .rules
            .always_allow
            .iter()
            .chain(&settings.rules.always_block)
            .any(|rule| rule.action.trim().is_empty() || rule.resource.trim().is_empty())
        {
            return Err(
                "permissions.auto_mode.rules requires nonempty action/resource patterns".into(),
            );
        }
        Ok(settings)
    }
}
impl AutoPattern {
    pub fn matches(&self, action: &str, resource: &str) -> bool {
        crate::wildcard::matches(&self.action, action)
            && crate::wildcard::matches(&self.resource, resource)
    }
}
