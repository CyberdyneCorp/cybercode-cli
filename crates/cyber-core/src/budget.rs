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

/// `max_agents` of a Workflow Run when no source sets it (`workflows` → Run budgets).
pub const DEFAULT_MAX_AGENTS: u64 = 200;
/// `max_wall_seconds` of a Workflow Run when no source sets it.
pub const DEFAULT_RUN_WALL_SECONDS: f64 = 14_400.0;

/// A Workflow Run budget: the canonical [`Budget`] plus the workflow-specific `max_agents`,
/// serialized as one flat object.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct RunBudget {
    #[serde(flatten)]
    pub budget: Budget,
    pub max_agents: u64,
}

impl<'de> Deserialize<'de> for RunBudget {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = serde_json::Map::<String, Value>::deserialize(deserializer)?;
        let max_agents = fields
            .remove("max_agents")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| serde::de::Error::missing_field("max_agents"))?;
        let budget =
            Budget::deserialize(Value::Object(fields)).map_err(serde::de::Error::custom)?;
        Ok(Self { budget, max_agents })
    }
}

impl RunBudget {
    /// Resolve a run budget from `budgets.run` in configuration, then `meta.budget`, then
    /// the invocation's budget. Configuration and the defaults (`max_agents`,
    /// `max_wall_seconds`) are a ceiling: each later layer may only lower a limit, a larger
    /// value keeps the current one, and `null` for a limit already set is refused, so
    /// neither a script nor the model can widen what the user configured. Reserved
    /// enforcement requires bounded provider requests and durable admission.
    pub fn resolve(config: &Value, layers: &[Option<&Value>]) -> Result<Self, String> {
        let mut merged = serde_json::Map::from_iter([
            ("max_agents".to_owned(), Value::from(DEFAULT_MAX_AGENTS)),
            (
                "max_wall_seconds".to_owned(),
                Value::from(DEFAULT_RUN_WALL_SECONDS),
            ),
        ]);
        if let Some(fields) = object("budgets.run", config.pointer("/budgets/run"))? {
            merged.extend(fields.clone());
        }
        for layer in layers {
            if let Some(fields) = object("budget", *layer)? {
                tighten(&mut merged, fields)?;
            }
        }
        Self::from_fields(merged)
    }

    /// This budget with `layer`'s fields replacing its own: an explicit override by the
    /// authenticated client, such as a raised `max_cost_usd` on resume or a CLI start's
    /// `--max-agents`. `null` removes a limit.
    pub fn overridden(&self, layer: &Value) -> Result<Self, String> {
        let Value::Object(mut merged) = serde_json::to_value(self).map_err(|e| e.to_string())?
        else {
            return Err("budget: expected an object".into());
        };
        if let Some(fields) = object("budget", Some(layer))? {
            merged.extend(fields.clone());
        }
        Self::from_fields(merged)
    }

    fn from_fields(mut merged: serde_json::Map<String, Value>) -> Result<Self, String> {
        let max_agents = match merged.remove("max_agents") {
            None => DEFAULT_MAX_AGENTS,
            Some(value) => value
                .as_u64()
                .ok_or("budget.max_agents: expected a nonnegative integer")?,
        };
        let mut budget: Budget =
            serde_json::from_value(Value::Object(merged)).map_err(|e| format!("budget: {e}"))?;
        budget.validate().map_err(|e| format!("budget.{e}"))?;
        budget
            .max_wall_seconds
            .get_or_insert(DEFAULT_RUN_WALL_SECONDS);
        Ok(Self { budget, max_agents })
    }
}

/// A budget source: absent, `null` or an object.
fn object<'a>(
    name: &str,
    layer: Option<&'a Value>,
) -> Result<Option<&'a serde_json::Map<String, Value>>, String> {
    match layer {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(fields)) => Ok(Some(fields)),
        Some(_) => Err(format!("{name}: expected an object")),
    }
}

/// Apply a layer that may only lower limits: a number replaces a larger one, `null`
/// cannot remove a limit already set. Other fields (`enforcement`) and limits not set yet
/// are taken as given, and validated when the budget is built.
fn tighten(
    merged: &mut serde_json::Map<String, Value>,
    fields: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    for (name, value) in fields {
        let current = merged.get(name).filter(|current| !current.is_null());
        match (current.and_then(Value::as_f64), value) {
            (Some(_), Value::Null) => {
                return Err(format!(
                    "budget.{name}: a limit set by configuration or an earlier source cannot be removed"
                ));
            }
            (Some(limit), Value::Number(lower)) if lower.as_f64().is_some_and(|v| v >= limit) => {}
            _ => {
                merged.insert(name.clone(), value.clone());
            }
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn run_budgets_layer_config_meta_and_invocation_with_defaults() {
        let none = RunBudget::resolve(&json!({}), &[]).unwrap();
        assert_eq!(none.max_agents, DEFAULT_MAX_AGENTS);
        assert_eq!(none.budget.max_wall_seconds, Some(DEFAULT_RUN_WALL_SECONDS));
        let config = json!({"budgets":{"run":{"max_cost_usd":5.0,"max_agents":10}}});
        let meta = json!({"max_agents": 4, "max_tokens": 100});
        let invocation = json!({"max_cost_usd": 1.5});
        let layered = RunBudget::resolve(&config, &[Some(&meta), Some(&invocation)]).unwrap();
        assert_eq!(layered.max_agents, 4);
        assert_eq!(layered.budget.max_tokens, Some(100));
        assert_eq!(layered.budget.max_cost_usd, Some(1.5));
        let text = serde_json::to_value(&layered).unwrap();
        assert_eq!(text["max_agents"], 4);
        assert_eq!(serde_json::from_value::<RunBudget>(text).unwrap(), layered);
        for bad in [
            json!({"max_agents": -1}),
            json!({"max_cost_usd": -1}),
            json!({"bogus": 1}),
            json!(3),
        ] {
            assert!(
                RunBudget::resolve(&json!({}), &[Some(&bad)]).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn configuration_and_defaults_are_a_ceiling_only_the_client_raises() {
        let config = json!({"budgets":{"run":{"max_cost_usd":5.0,"max_agents":10}}});
        // Later layers only lower a limit.
        let ceiling = RunBudget::resolve(
            &config,
            &[
                Some(&json!({"max_cost_usd": 9.0, "max_agents": 1_000_000})),
                Some(&json!({"max_wall_seconds": 1e9, "max_tokens": 7})),
            ],
        )
        .unwrap();
        assert_eq!(ceiling.budget.max_cost_usd, Some(5.0));
        assert_eq!(ceiling.max_agents, 10);
        assert_eq!(
            ceiling.budget.max_wall_seconds,
            Some(DEFAULT_RUN_WALL_SECONDS)
        );
        assert_eq!(ceiling.budget.max_tokens, Some(7));
        let defaults = RunBudget::resolve(&json!({}), &[Some(&json!({"max_agents": 500}))]);
        assert_eq!(defaults.unwrap().max_agents, DEFAULT_MAX_AGENTS);
        for removed in [
            json!({"max_cost_usd": null}),
            json!({"max_agents": null}),
            json!({"max_wall_seconds": null}),
        ] {
            let error = RunBudget::resolve(&config, &[None, Some(&removed)]).unwrap_err();
            assert!(error.contains("cannot be removed"), "{removed}: {error}");
        }
        // A limit set only by meta.budget cannot be removed by the invocation either.
        assert!(
            RunBudget::resolve(
                &json!({}),
                &[
                    Some(&json!({"max_tokens": 5})),
                    Some(&json!({"max_tokens": null}))
                ]
            )
            .is_err()
        );
        // An explicit override by the authenticated client may raise or remove a limit.
        let raised = ceiling
            .overridden(&json!({"max_cost_usd": 50.0, "max_tokens": null, "max_agents": 300}))
            .unwrap();
        assert_eq!(raised.budget.max_cost_usd, Some(50.0));
        assert_eq!(raised.budget.max_tokens, None);
        assert_eq!(raised.max_agents, 300);
    }
}
