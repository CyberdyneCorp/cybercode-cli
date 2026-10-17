//! What a Workflow Run starts from (`workflows` → Saved and bundled workflows, Model tiers
//! and mixing, Run budgets, Model-authored workflows): the module (a saved name or an
//! inline script), the Location's engine limits and model tiers, and the layered run
//! budget. The HTTP API and the model-facing `workflow` tool resolve runs through here, so
//! both start the same run from the same request.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use cyber_core::budget::RunBudget;
use cyber_core::config::{TierTarget, WorkflowSettings};
use cyber_llm::catalog::{ModelRole, tier_role_ref};
use cyber_workflow::{Diagnostic, Limits, WorkflowModule};
use serde_json::{Value, json};

use super::workflows::RunShare;

/// The Workflow settings of a Location that a run needs.
#[derive(Debug, Clone, Default)]
pub struct WorkflowLocation {
    pub limits: Limits,
    /// Tier name → resolved model reference; `None` inherits the owner's model.
    pub tiers: BTreeMap<String, Option<String>>,
    /// `budgets.run` of the Location's configuration (`null` when unset).
    pub run_budget: Value,
    /// `workflows.max_concurrent_per_run`: the share of a run that sets none.
    pub max_concurrent_per_run: Option<u32>,
}

/// Why a run cannot start.
#[derive(Debug, Clone, PartialEq)]
pub enum StartError {
    /// No saved Workflow has this name in the Location.
    NotFound(String),
    /// The request or configuration is invalid.
    Invalid(String),
    /// The module does not load (`ParseError`, `UnsupportedSyntax`, …).
    Module(Diagnostic),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "No saved Workflow named {name:?}"),
            Self::Invalid(message) => f.write_str(message),
            Self::Module(diagnostic) => write!(f, "{diagnostic}"),
        }
    }
}

/// Engine limits and tiers resolved against the Location's `model_roles`.
pub fn location(config: &Value) -> Result<WorkflowLocation, String> {
    let settings = WorkflowSettings::from_config(config)?;
    let tiers = settings
        .tiers
        .iter()
        .map(|(name, target)| (name.clone(), tier_model(config, target)))
        .collect();
    Ok(WorkflowLocation {
        limits: Limits {
            max_memory_bytes: settings.max_memory_mb as usize * 1024 * 1024,
            script_cpu: Duration::from_secs(u64::from(settings.script_cpu_seconds)),
        },
        tiers,
        run_budget: config
            .pointer("/budgets/run")
            .cloned()
            .unwrap_or(Value::Null),
        max_concurrent_per_run: settings.max_concurrent_per_run,
    })
}

/// A tier's model reference; `None` when no model is configured for its role or the
/// default, so the agent inherits its parent's model.
pub fn tier_model(config: &Value, target: &TierTarget) -> Option<String> {
    match target {
        TierTarget::Model(reference) => Some(reference.clone()),
        TierTarget::Role(role) => {
            ModelRole::parse(role).and_then(|role| tier_role_ref(config, role))
        }
    }
}

/// The saved Workflow `name` of a Location: global, then project directories from the
/// project root down to the Location. `project_config: false` skips project directories
/// (`CYBER_DISABLE_PROJECT_CONFIG`).
pub fn saved(
    global_config_dir: &Path,
    location: &Path,
    name: &str,
    project_config: bool,
) -> Result<WorkflowModule, StartError> {
    let project_root = cyber_core::config::project_root(location);
    let project = project_config.then_some((location, project_root.as_path()));
    let listing = cyber_workflow::discover(&cyber_workflow::roots(global_config_dir, project));
    listing
        .find(name)
        .cloned()
        .ok_or_else(|| StartError::NotFound(name.to_owned()))
}

/// An inline module, checked like a saved one.
pub fn inline(script: String) -> Result<WorkflowModule, StartError> {
    WorkflowModule::inline(script).map_err(StartError::Module)
}

/// `budgets.run`, then `meta.budget`, then the invocation's budget fields. The
/// configuration (and the defaults) is a ceiling that the script and the invocation may
/// only lower: this is the budget of a run the model starts with the `workflow` tool.
pub fn budget(
    location: &WorkflowLocation,
    module: &WorkflowModule,
    invocation: Option<&Value>,
) -> Result<RunBudget, StartError> {
    let config = json!({"budgets": {"run": location.run_budget}});
    RunBudget::resolve(&config, &[module.meta.raw.get("budget"), invocation])
        .map_err(StartError::Invalid)
}

/// The budget of a run the authenticated client starts (HTTP API, CLI): `budgets.run`
/// and `meta.budget` as for [`budget`], then the client's fields as an explicit override
/// that may also raise a limit, like a resume's.
pub fn client_budget(
    location: &WorkflowLocation,
    module: &WorkflowModule,
    invocation: Option<&Value>,
) -> Result<RunBudget, StartError> {
    let resolved = budget(location, module, None)?;
    match invocation {
        None => Ok(resolved),
        Some(layer) => resolved.overridden(layer).map_err(StartError::Invalid),
    }
}

/// A run's share of the agent slots (`workflows` → Fair agent scheduling): the
/// invocation's `max_concurrent`, else `meta.max_concurrent`, else
/// `workflows.max_concurrent_per_run`; `None` for the whole server-wide limit, which also
/// caps it when the run starts. Each source must be a positive integer: a bad invocation
/// value is an invalid request, a bad `meta` value an `ExecutionFailure`.
pub fn share(
    location: &WorkflowLocation,
    module: &WorkflowModule,
    invocation: Option<&Value>,
) -> Result<RunShare, StartError> {
    if let Some(value) = invocation.filter(|value| !value.is_null()) {
        return positive(value)
            .map(|n| RunShare::requested(Some(n)))
            .ok_or_else(|| {
                StartError::Invalid(format!(
                    "max_concurrent must be a positive integer, got {value}"
                ))
            });
    }
    let max_concurrent = match module.meta.raw.get("max_concurrent") {
        Some(value) => Some(positive(value).ok_or_else(|| {
            StartError::Module(Diagnostic::execution(format!(
                "meta.max_concurrent must be a positive integer, got {value}"
            )))
        })?),
        None => location.max_concurrent_per_run,
    };
    Ok(RunShare {
        requested: None,
        max_concurrent,
    })
}

/// A positive integer, saturated to `u32`.
fn positive(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .filter(|n| *n > 0)
        .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tiers_follow_model_roles_and_verify_falls_back_to_default() {
        let config = json!({"model_roles":{"default":"p/big","small":"p/small"}});
        let tiers = location(&config).unwrap().tiers;
        assert_eq!(tiers["fast"].as_deref(), Some("p/small"));
        assert_eq!(tiers["smart"].as_deref(), Some("p/big"));
        assert_eq!(tiers["verify"].as_deref(), Some("p/big"));
        let advised = json!({"model":"p/big","model_roles":{"advisor":"p/wise"},
            "workflows":{"tiers":{"cheap":"p/tiny","judge":"role:advisor"}}});
        let tiers = location(&advised).unwrap().tiers;
        assert_eq!(tiers["verify"].as_deref(), Some("p/wise"));
        assert_eq!(tiers["judge"].as_deref(), Some("p/wise"));
        assert_eq!(tiers["cheap"].as_deref(), Some("p/tiny"));
        assert_eq!(tiers["fast"].as_deref(), Some("p/big"));
        assert_eq!(location(&json!({})).unwrap().tiers["verify"], None);
    }

    #[test]
    fn limits_come_from_the_workflows_section() {
        let limits = location(&json!({"workflows":{"max_memory_mb":32,"script_cpu_seconds":5}}))
            .unwrap()
            .limits;
        assert_eq!(limits.max_memory_bytes, 32 * 1024 * 1024);
        assert_eq!(limits.script_cpu, Duration::from_secs(5));
        assert!(location(&json!({"workflows":{"bogus":1}})).is_err());
    }

    #[test]
    fn run_budgets_layer_configuration_meta_and_invocation() {
        let module = WorkflowModule::inline(
            "export const meta = {name: \"b\", description: \"d\", budget: {max_turns: 4, max_agents: 3}};\nexport default async () => 1;".to_owned(),
        )
        .unwrap();
        let location = WorkflowLocation {
            run_budget: json!({"max_cost_usd": 2.0, "max_turns": 9}),
            ..Default::default()
        };
        let resolved = budget(&location, &module, Some(&json!({"max_agents": 0}))).unwrap();
        assert_eq!(resolved.budget.max_cost_usd, Some(2.0));
        assert_eq!(resolved.budget.max_turns, Some(4));
        assert_eq!(resolved.max_agents, 0);
        // The model's invocation cannot lift the configured caps.
        let widened = json!({"max_cost_usd": null});
        assert!(budget(&location, &module, Some(&widened)).is_err());
        let raised = budget(&location, &module, Some(&json!({"max_cost_usd": 100.0}))).unwrap();
        assert_eq!(raised.budget.max_cost_usd, Some(2.0));
        // The authenticated client overrides explicitly.
        let client = client_budget(&location, &module, Some(&json!({"max_cost_usd": 100.0})));
        assert_eq!(client.unwrap().budget.max_cost_usd, Some(100.0));
    }

    #[test]
    fn the_share_layers_invocation_meta_and_configuration() {
        let plain = WorkflowModule::inline(
            "export const meta = {name: \"s\", description: \"d\"};\nexport default async () => 1;"
                .to_owned(),
        )
        .unwrap();
        let declared = WorkflowModule::inline(
            "export const meta = {name: \"s\", description: \"d\", max_concurrent: 4};\nexport default async () => 1;".to_owned(),
        )
        .unwrap();
        let configured = WorkflowLocation {
            max_concurrent_per_run: Some(6),
            ..Default::default()
        };
        let resolved = |max_concurrent| {
            Ok(RunShare {
                requested: None,
                max_concurrent,
            })
        };
        assert_eq!(
            share(&WorkflowLocation::default(), &plain, None),
            resolved(None)
        );
        assert_eq!(share(&configured, &plain, None), resolved(Some(6)));
        assert_eq!(share(&configured, &declared, None), resolved(Some(4)));
        // Only the invocation's own value is kept for a later resume.
        assert_eq!(
            share(&configured, &declared, Some(&json!(2))),
            Ok(RunShare::requested(Some(2)))
        );
        assert_eq!(
            share(&configured, &declared, Some(&Value::Null)),
            resolved(Some(4))
        );
        for bad in [json!(0), json!(-1), json!(1.5), json!("3")] {
            assert!(
                matches!(
                    share(&configured, &plain, Some(&bad)),
                    Err(StartError::Invalid(_))
                ),
                "{bad}"
            );
            let source = format!(
                "export const meta = {{name: \"s\", description: \"d\", max_concurrent: {bad}}};\nexport default async () => 1;"
            );
            let module = WorkflowModule::inline(source).unwrap();
            match share(&configured, &module, None) {
                Err(StartError::Module(error)) => {
                    assert_eq!(error.kind, cyber_workflow::DiagnosticKind::ExecutionFailure);
                }
                other => panic!("{bad}: {other:?}"),
            }
        }
    }
}
