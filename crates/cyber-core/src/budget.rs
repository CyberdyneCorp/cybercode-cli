//! The canonical budget shared by Session and orchestration scopes.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Enforcement {
    #[default]
    Soft,
    Reserved,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_seconds: Option<f64>,
    #[serde(default)]
    pub enforcement: Enforcement,
}
impl Budget {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("max_cost_usd", self.max_cost_usd),
            ("max_wall_seconds", self.max_wall_seconds),
        ] {
            if value.is_some_and(|v| !v.is_finite() || v < 0.0) {
                return Err(format!("{name}: expected a finite nonnegative number"));
            }
        }
        Ok(())
    }

    pub fn from_config(config: &Value, scope: &str) -> Result<Option<Self>, String> {
        let Some(value) = config
            .get("budgets")
            .and_then(|v| v.get(scope))
            .filter(|v| !v.is_null())
        else {
            return Ok(None);
        };
        let mut value = value.clone();
        if scope == "run"
            && let Some(map) = value.as_object_mut()
        {
            map.remove("max_agents");
        }
        let budget: Self =
            serde_json::from_value(value).map_err(|e| format!("budgets.{scope}: {e}"))?;
        budget
            .validate()
            .map_err(|e| format!("budgets.{scope}.{e}"))?;
        Ok(Some(budget))
    }
}

pub fn validate_config(config: &Value) -> Vec<String> {
    let Some(value) = config.get("budgets") else {
        return Vec::new();
    };
    let Some(scopes) = value.as_object() else {
        return vec!["budgets: expected an object".into()];
    };
    let mut issues = Vec::new();
    for (scope, value) in scopes {
        if !["session", "run", "goal", "loop", "routine", "team", "daily"].contains(&scope.as_str())
        {
            issues.push(format!("budgets.{scope}: unknown budget scope"));
            continue;
        }
        if scope == "run"
            && let Some(limit) = value.get("max_agents")
            && limit.as_u64().is_none()
        {
            issues.push("budgets.run.max_agents: expected a nonnegative integer".into());
        }
        if let Err(error) = Budget::from_config(config, scope) {
            issues.push(error);
        }
    }
    issues
}
