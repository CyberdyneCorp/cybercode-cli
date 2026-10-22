//! Workflow lookups for the HTTP API (`workflows` → Saved and bundled workflows, Model tiers
//! and mixing, Sandboxed QuickJS runtime), resolved by the runtime's shared start rules.

use std::path::Path;

use cyber_core::env::{EnvSource, ProcessEnv};
use cyber_server::http::{ApiError, WorkflowLocation};
use cyber_server::runtime::workflow_start::{self, StartError, WorkflowLibrary};
use cyber_workflow::WorkflowModule;
use serde_json::Value;

/// Engine limits and tiers resolved against the Location's `model_roles`.
pub(super) fn location(config: &Value) -> Result<WorkflowLocation, ApiError> {
    workflow_start::location(config).map_err(ApiError::invalid)
}

/// The Workflow library of a Location. `CYBER_DISABLE_PROJECT_CONFIG` skips project
/// directories.
pub(super) fn library(global_config_dir: &Path, location: &Path) -> WorkflowLibrary {
    let project = !ProcessEnv.flag("CYBER_DISABLE_PROJECT_CONFIG");
    WorkflowLibrary::of(workflow_start::discover(global_config_dir, location, project))
}

/// The saved Workflow `name` of a Location. `CYBER_DISABLE_PROJECT_CONFIG` skips project
/// directories.
pub(super) fn saved(
    global_config_dir: &Path,
    location: &Path,
    name: &str,
) -> Result<WorkflowModule, ApiError> {
    let project = !ProcessEnv.flag("CYBER_DISABLE_PROJECT_CONFIG");
    workflow_start::saved(global_config_dir, location, name, project).map_err(|error| match error {
        StartError::NotFound(_) => ApiError::not_found("WorkflowNotFoundError", error.to_string()),
        other => ApiError::invalid(other.to_string()),
    })
}
