//! Workflow lookups for the HTTP API (`workflows` → Saved and bundled workflows, Model tiers
//! and mixing, Sandboxed QuickJS runtime), resolved by the runtime's shared start rules.

use std::path::Path;

use cyber_core::env::ProcessEnv;
use cyber_server::http::{ApiError, WorkflowLocation};
use cyber_server::runtime::workflow_start::{self, LibrarySources, StartError, WorkflowLibrary};
use cyber_workflow::WorkflowModule;
use serde_json::Value;

/// Engine limits and tiers resolved against the Location's `model_roles`.
pub(super) fn location(config: &Value) -> Result<WorkflowLocation, ApiError> {
    workflow_start::location(config).map_err(ApiError::invalid)
}

/// The Workflow library of a Location. `CYBER_DISABLE_PROJECT_CONFIG` skips project
/// directories; `workflows.bundled: false` and `CYBER_DISABLE_BUNDLED_WORKFLOWS` hide the
/// bundled Workflows.
pub(super) fn library(global_config_dir: &Path, location: &Path, config: &Value) -> WorkflowLibrary {
    let sources = LibrarySources::of(config, &ProcessEnv);
    WorkflowLibrary::of(workflow_start::discover(
        global_config_dir,
        location,
        sources,
    ))
}

/// The Workflow `name` of a Location's library (see [`library`]).
pub(super) fn saved(
    global_config_dir: &Path,
    location: &Path,
    name: &str,
    config: &Value,
) -> Result<WorkflowModule, ApiError> {
    let sources = LibrarySources::of(config, &ProcessEnv);
    workflow_start::saved(global_config_dir, location, name, sources).map_err(|error| match error {
        StartError::NotFound(_) => ApiError::not_found("WorkflowNotFoundError", error.to_string()),
        other => ApiError::invalid(other.to_string()),
    })
}

/// A refused preparation as the HTTP API reports it: invalid args as their diagnostic,
/// an unresolvable review target as `422 ReviewTargetError`, missing rules as
/// `403 PermissionDenied`.
pub(super) fn prepare_error(error: cyber_tools::PrepareError) -> ApiError {
    match error {
        cyber_tools::PrepareError::Start(error) => cyber_server::http::workflow_start_error(error),
        cyber_tools::PrepareError::Review(error) => ApiError::review_target(error),
        cyber_tools::PrepareError::Forbidden(message) => {
            ApiError::forbidden(message).with_reason("PermissionDenied")
        }
        cyber_tools::PrepareError::Unavailable(message) => ApiError::invalid(message),
    }
}
