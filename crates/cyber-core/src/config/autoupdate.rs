//! The `autoupdate` key (`installation-upgrade` → Background update policy).

use std::collections::BTreeMap;

use serde_json::Value;

/// `true` upgrades patch releases silently, `false` never checks, and `"notify"` (the
/// default) only announces a newer version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoUpdate {
    On,
    Off,
    #[default]
    Notify,
}

impl AutoUpdate {
    pub fn from_config(config: &Value) -> Result<Self, String> {
        match config.get("autoupdate") {
            None | Some(Value::Null) => Ok(Self::default()),
            Some(Value::Bool(true)) => Ok(Self::On),
            Some(Value::Bool(false)) => Ok(Self::Off),
            Some(Value::String(value)) if value == "notify" => Ok(Self::Notify),
            Some(other) => Err(format!(
                "autoupdate: expected true, false or \"notify\", got {other}"
            )),
        }
    }

    /// `policy.autoupdate` when organization policy (a `managed:` layer) set it; a user
    /// layer writing `policy` does not count. Malformed policy sets `false`.
    pub fn managed(config: &Value, sources: &BTreeMap<String, String>) -> Option<Self> {
        sources
            .get("/policy/autoupdate")
            .filter(|source| source.starts_with("managed:"))?;
        Self::from_config(config.get("policy")?).ok()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn accepts_the_three_values_and_defaults_to_notify() {
        let parse = |value: Value| AutoUpdate::from_config(&json!({ "autoupdate": value }));
        assert_eq!(parse(json!(true)), Ok(AutoUpdate::On));
        assert_eq!(parse(json!(false)), Ok(AutoUpdate::Off));
        assert_eq!(parse(json!("notify")), Ok(AutoUpdate::Notify));
        assert_eq!(AutoUpdate::from_config(&json!({})), Ok(AutoUpdate::Notify));
    }

    #[test]
    fn rejects_other_values() {
        for value in [json!("yes"), json!(1), json!({})] {
            let error = AutoUpdate::from_config(&json!({ "autoupdate": value })).unwrap_err();
            assert!(error.starts_with("autoupdate: expected"), "{error}");
        }
    }
}
