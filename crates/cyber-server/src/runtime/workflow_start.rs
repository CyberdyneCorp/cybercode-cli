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
use cyber_core::env::EnvSource;
use cyber_llm::catalog::{ModelRole, tier_role_ref};
use cyber_workflow::{Diagnostic, Entry, Limits, Listing, Origin, Scope, WorkflowModule};
use serde::Serialize;
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

/// Which Workflow sources a Location's library includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibrarySources {
    /// Project directories; `false` under `CYBER_DISABLE_PROJECT_CONFIG`.
    pub project: bool,
    /// The bundled Workflows: `workflows.bundled` and not
    /// `CYBER_DISABLE_BUNDLED_WORKFLOWS`.
    pub bundled: bool,
}

impl LibrarySources {
    /// The sources a Location's configuration and the environment allow.
    pub fn of(config: &Value, env: &dyn EnvSource) -> Self {
        let setting = WorkflowSettings::from_config(config).map_or(true, |s| s.bundled);
        Self {
            project: !env.flag("CYBER_DISABLE_PROJECT_CONFIG"),
            bundled: cyber_workflow::bundled::enabled(
                setting,
                env.flag(cyber_workflow::bundled::DISABLE_ENV),
            ),
        }
    }
}

/// The Workflows a Location can run by name: the bundled ones, then global, then project
/// directories from the project root down to the Location, each overriding the earlier.
pub fn discover(global_config_dir: &Path, location: &Path, sources: LibrarySources) -> Listing {
    let project_root = cyber_core::config::project_root(location);
    let project = sources
        .project
        .then_some((location, project_root.as_path()));
    cyber_workflow::discover_library(
        &cyber_workflow::roots(global_config_dir, project),
        sources.bundled,
    )
}

/// The Workflow `name` of a Location's library (see [`discover`]).
pub fn saved(
    global_config_dir: &Path,
    location: &Path,
    name: &str,
    sources: LibrarySources,
) -> Result<WorkflowModule, StartError> {
    discover(global_config_dir, location, sources)
        .find(name)
        .cloned()
        .ok_or_else(|| StartError::NotFound(name.to_owned()))
}

/// The Workflow library of a Location (`workflows` → Workflow library over HTTP): every
/// entry a run can start by name, and the modules that failed to load.
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct WorkflowLibrary {
    pub workflows: Vec<LibraryEntry>,
    pub issues: Vec<LibraryIssue>,
}

/// One library entry, as its `meta` declares it.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct LibraryEntry {
    pub name: String,
    pub description: String,
    /// Declared phase titles.
    pub phases: Vec<String>,
    /// `meta.args`: the JSON Schema of the run's `args`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    /// `meta.budget`: the module's default run budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budgets: Option<Value>,
    /// `meta.estimate`: how many agents a run starts, in words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimate: Option<String>,
    /// `meta.tools`: the read-only tools the script calls directly.
    pub tools: Vec<String>,
    pub origin: LibraryOrigin,
    /// `sha256:<hex>` of the source.
    pub digest: String,
    /// The definitions this entry replaces, lowest precedence first.
    pub shadowed: Vec<String>,
    /// The module source, when asked for (`source=true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Where a library entry comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LibraryOrigin {
    /// `<config>/workflows`.
    Global { path: String },
    /// A `.cyber/workflows` directory of the project.
    Project { path: String },
    /// Embedded in the binary.
    Bundled { name: String },
}

/// A module that failed to load, with its diagnostic.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct LibraryIssue {
    pub path: String,
    pub diagnostic: Diagnostic,
}

impl WorkflowLibrary {
    pub fn of(listing: Listing) -> Self {
        Self {
            workflows: listing.workflows.into_iter().filter_map(entry).collect(),
            issues: listing
                .issues
                .into_iter()
                .map(|issue| LibraryIssue {
                    path: issue.path.display().to_string(),
                    diagnostic: issue.diagnostic,
                })
                .collect(),
        }
    }
}

fn entry(entry: Entry) -> Option<LibraryEntry> {
    let module = entry.module;
    let origin = match &module.origin {
        Origin::Saved { scope, path } => {
            let path = path.display().to_string();
            match scope {
                Scope::Global => LibraryOrigin::Global { path },
                Scope::Project => LibraryOrigin::Project { path },
            }
        }
        Origin::Bundled { name } => LibraryOrigin::Bundled { name: name.clone() },
        Origin::Inline => return None,
    };
    let raw = &module.meta.raw;
    Some(LibraryEntry {
        name: module.meta.name.clone(),
        description: module.meta.description.clone(),
        phases: module.meta.phases.iter().map(|p| p.title.clone()).collect(),
        args: module.meta.args.clone(),
        budgets: raw.get("budget").cloned(),
        estimate: raw
            .get("estimate")
            .and_then(Value::as_str)
            .map(str::to_owned),
        tools: meta_tools(&module),
        origin,
        digest: module.digest.clone(),
        shadowed: entry
            .shadowed
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        source: Some(module.source.clone()),
    })
}

/// An invocation's `tools`: the direct read-only tools it allowlists besides
/// `meta.tools`; mutating tools are refused.
pub fn invocation_tools(value: Option<&Value>) -> Result<Vec<String>, StartError> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => cyber_workflow::meta::tool_list(value).ok_or_else(|| {
            StartError::Invalid("tools must be an array naming read, glob or grep".into())
        }),
    }
}

/// `meta.tools`: the direct read-only tools a module allowlists.
pub fn meta_tools(module: &WorkflowModule) -> Vec<String> {
    module.meta.tools.clone()
}

/// Check a run's `args` against the module's `meta.args` JSON Schema before any run record
/// exists (`workflows` → Bundled workflow contracts). The first violation fails as
/// `ExecutionFailure: args.<path> …`; a module without `meta.args` takes any object.
pub fn validate_args(module: &WorkflowModule, args: &Value) -> Result<(), StartError> {
    if !args.is_object() {
        return Err(args_error("args", "expected an object"));
    }
    let Some(schema) = &module.meta.args else {
        return Ok(());
    };
    let validator = jsonschema::validator_for(schema).map_err(|error| {
        args_error("meta.args", &format!("is not a valid JSON Schema: {error}"))
    })?;
    match validator.iter_errors(args).next() {
        None => Ok(()),
        Some(error) => {
            let path = args_path(&error.instance_path().to_string());
            Err(args_error(&path, &error.to_string()))
        }
    }
}

/// `args.a.0.b` for the JSON pointer `/a/0/b`.
fn args_path(pointer: &str) -> String {
    pointer
        .split('/')
        .skip(1)
        .map(|token| token.replace("~1", "/").replace("~0", "~"))
        .fold("args".to_owned(), |path, token| format!("{path}.{token}"))
}

fn args_error(path: &str, message: &str) -> StartError {
    StartError::Module(Diagnostic::execution(format!("{path}: {message}")))
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
mod args_tests {
    use super::*;

    fn module(args: &str) -> WorkflowModule {
        WorkflowModule::inline(format!(
            "export const meta = {{ name: 'm', description: 'd', args: {args} }};"
        ))
        .unwrap()
    }

    fn message(error: StartError) -> String {
        match error {
            StartError::Module(diagnostic) => diagnostic.to_string(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn args_are_validated_against_meta_args_and_name_their_path() {
        let schema = module(
            "{ type: 'object', properties: { lenses: { type: 'array', items: { enum: ['a', 'b'] } }, n: { type: 'integer', maximum: 3 } }, required: ['n'], additionalProperties: false }",
        );
        assert!(validate_args(&schema, &json!({"n": 2, "lenses": ["a"]})).is_ok());
        let wrong = message(validate_args(&schema, &json!({"n": 2, "lenses": ["c"]})).unwrap_err());
        assert!(
            wrong.starts_with("ExecutionFailure: args.lenses.0: "),
            "{wrong}"
        );
        let big = message(validate_args(&schema, &json!({"n": 9})).unwrap_err());
        assert!(big.starts_with("ExecutionFailure: args.n: "), "{big}");
        let missing = message(validate_args(&schema, &json!({})).unwrap_err());
        assert!(missing.starts_with("ExecutionFailure: args: "), "{missing}");
        assert!(missing.contains("\"n\""), "{missing}");
        let not_object = message(validate_args(&schema, &json!([])).unwrap_err());
        assert_eq!(not_object, "ExecutionFailure: args: expected an object");
    }

    #[test]
    fn a_module_without_meta_args_takes_any_object() {
        let free =
            WorkflowModule::inline("export const meta = { name: 'm', description: 'd' };").unwrap();
        assert!(validate_args(&free, &json!({"anything": [1, 2]})).is_ok());
    }
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
