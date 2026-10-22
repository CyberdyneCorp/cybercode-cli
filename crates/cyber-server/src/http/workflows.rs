//! The `workflow` route group (`server-api` → Route groups; `workflows` → Background
//! execution on the server, Run lifecycle states, Phases, logs and checkpoints, Permissions
//! for runs). Runs are started, listed, shown, paused, resumed, stopped and their logs read
//! here; they execute inside the server, independent of the client connection. Starting
//! and resuming honour the owner Session's `workflow.run` deny rules. `GET /workflows` lists
//! the Location's Workflow library. Paths without an operation answer
//! `503 ServiceUnavailableError` with `service: "workflow"`, so clients can tell a missing
//! capability from a missing resource.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{any, get, post};
use axum::{Json, Router};
use cyber_workflow::{Diagnostic, WorkflowModule};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use std::sync::Arc;

use axum::http::request::Parts;
use futures::future::BoxFuture;

use super::envelope::{Cursor, Data, Page, location};
use super::{ApiError, AppState, Services};
pub use crate::runtime::workflow_start::WorkflowLocation;
use crate::runtime::workflow_start::{self, StartError, WorkflowLibrary};
use crate::runtime::{
    InterruptedResume, ResumeWorkflowRun, RunFilter, RunStarter, Runtime, RuntimeError,
    SessionInfo, StartWorkflowRun, TurnContext, WorkflowAgent, WorkflowLogEntry, WorkflowRelaunch,
    WorkflowRun, WorkflowRunStatus, WorkflowRunSummary,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workflows/runs", get(list).post(start))
        .route(
            "/workflows/runs/resume_interrupted",
            post(resume_interrupted),
        )
        .route("/workflows/runs/{id}", get(show))
        .route("/workflows/runs/{id}/pause", post(pause))
        .route("/workflows/runs/{id}/resume", post(resume))
        .route("/workflows/runs/{id}/stop", post(stop))
        .route("/workflows/runs/{id}/logs", get(logs))
        .route("/workflows/runs/{id}/save", post(save))
        .route("/workflows", get(library).fallback(unavailable))
        .route("/workflows/{*rest}", any(unavailable))
}

async fn unavailable() -> ApiError {
    ApiError::service_unavailable(
        "workflow",
        "This Workflow operation is not available in this server yet",
    )
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct StartBody {
    /// The owner Session: agents become its children and run under its Mode.
    session_id: String,
    /// A saved Workflow's `meta.name`, discovered from the owner's Location.
    name: Option<String>,
    /// An inline JavaScript module.
    script: Option<String>,
    #[serde(default)]
    args: Option<Value>,
    /// Run budget fields, plus `max_agents`, overriding `budgets.run` and `meta.budget`:
    /// the authenticated client may raise a limit, as on resume.
    #[serde(default)]
    budget: Option<Value>,
    /// The run's share of the server-wide Workflow agent slots, a positive integer capped
    /// at that limit; overrides `meta.max_concurrent` and `workflows.max_concurrent_per_run`.
    #[serde(default)]
    #[schemars(with = "Option<std::num::NonZeroU32>")]
    max_concurrent: Option<Value>,
    /// Direct read-only tools (`read`, `glob`, `grep`) the script may call besides those
    /// its `meta.tools` names.
    #[serde(default)]
    #[schemars(with = "Option<Vec<String>>")]
    tools: Option<Value>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ResumeBody {
    /// Accept a changed script or configuration as a new revision.
    #[serde(default)]
    revision: bool,
    /// Run budget fields to override, such as a raised `max_cost_usd`.
    #[serde(default)]
    budget: Option<Value>,
    /// The run's share of the agent slots for this attempt, as on start; a change needs
    /// no revision. Absent, the run keeps the `max_concurrent` it was last given.
    #[serde(default)]
    #[schemars(with = "Option<std::num::NonZeroU32>")]
    max_concurrent: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
struct Filter {
    /// Comma-separated statuses; every status when absent.
    status: Option<String>,
    session_id: Option<String>,
    /// A Location's directory, or `all`; the request Location when absent.
    directory: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
}

/// A run's summary with its agents.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(super) struct RunDetail {
    #[serde(flatten)]
    summary: WorkflowRunSummary,
    /// Every agent of the run, in start or settlement order.
    agents_detail: Vec<WorkflowAgent>,
}

fn diagnostic(error: Diagnostic) -> ApiError {
    let message = error.to_string();
    ApiError::invalid(message)
        .with_reason(error.kind.as_str())
        .with_field(
            "diagnostic",
            serde_json::to_value(&error).unwrap_or_default(),
        )
}

/// The owner Session as permission rules see it.
fn owner_context(owner: &SessionInfo) -> TurnContext {
    TurnContext {
        session_id: owner.id.clone(),
        directory: owner.directory.clone(),
        agent: owner.agent.clone(),
        mode: owner.mode.clone(),
        prefers_apply_patch: false,
        rules: owner.rules.clone(),
        worktree: Default::default(),
    }
}

fn not_found(id: &str) -> ApiError {
    ApiError::not_found("WorkflowRunNotFoundError", format!("No Workflow Run {id}"))
}

/// Lifecycle refusals keep their message; a replay mismatch names its diagnostic kind.
fn lifecycle(error: RuntimeError) -> ApiError {
    match error {
        RuntimeError::Conflict(message) if message.starts_with("WorkflowReplayMismatch") => {
            ApiError::conflict(message.clone()).with_reason("WorkflowReplayMismatch")
        }
        RuntimeError::Conflict(message) => ApiError::conflict(message),
        error => error.into(),
    }
}

/// A run that cannot start, as the API reports it.
pub fn start_error(error: StartError) -> ApiError {
    match error {
        StartError::Module(error) => diagnostic(error),
        StartError::NotFound(_) => ApiError::not_found("WorkflowNotFoundError", error.to_string()),
        StartError::Invalid(message) => ApiError::invalid(message),
    }
}

async fn start(
    State(state): State<AppState>,
    Json(body): Json<StartBody>,
) -> Result<(StatusCode, Json<Data<WorkflowRun>>), ApiError> {
    let owner = state.runtime.state(&body.session_id).await?.info;
    let location = std::path::PathBuf::from(&owner.directory);
    let module = match (&body.name, body.script) {
        (Some(name), None) => state.services.saved_workflow(&location, name)?,
        (None, Some(script)) => workflow_start::inline(script).map_err(start_error)?,
        _ => {
            return Err(ApiError::invalid(
                "Give exactly one of name (a saved Workflow) or script (an inline module)",
            ));
        }
    };
    let owner_turn = owner_context(&owner);
    state
        .services
        .authorize_workflow_run(&owner_turn, &module)?;
    let args = body
        .args
        .unwrap_or_else(|| Value::Object(Default::default()));
    let args = state
        .services
        .prepare_workflow_args(&owner_turn, &module, args)
        .await?;
    let settings = state.services.workflow_location(&location)?;
    let budget = workflow_start::client_budget(&settings, &module, body.budget.as_ref())
        .map_err(start_error)?;
    let share = workflow_start::share(&settings, &module, body.max_concurrent.as_ref())
        .map_err(start_error)?;
    let tools = workflow_start::invocation_tools(body.tools.as_ref()).map_err(start_error)?;
    let run = state
        .runtime
        .start_workflow_run(StartWorkflowRun {
            module,
            args,
            session_id: owner.id,
            limits: settings.limits,
            tiers: settings.tiers,
            budget,
            share,
            starter: RunStarter::Api,
            tools,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(Data { data: run })))
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SaveBody {
    /// Replace an existing `.cyber/workflows/<name>.js`.
    #[serde(default)]
    force: bool,
}

/// Where a saved script was written.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(super) struct SavedScript {
    path: String,
}

/// Save an inline run's script into the project's `.cyber/workflows/` (`workflows` →
/// Model-authored workflows).
async fn save(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<SaveBody>>,
) -> Result<Json<Data<SavedScript>>, ApiError> {
    if state.runtime.workflow_run(&id)?.is_none() {
        return Err(not_found(&id));
    }
    let force = body.is_some_and(|Json(body)| body.force);
    let path = state
        .runtime
        .save_workflow_script(&id, force)
        .map_err(lifecycle)?;
    Ok(Json(Data {
        data: SavedScript {
            path: path.display().to_string(),
        },
    }))
}

#[derive(Debug, Default, Deserialize)]
struct LibraryQuery {
    /// The Location; the request's when absent.
    directory: Option<String>,
    /// Include each entry's module source.
    #[serde(default)]
    source: bool,
}

/// The Workflow library of a Location (`workflows` → Workflow library over HTTP).
async fn library(
    State(state): State<AppState>,
    Query(query): Query<LibraryQuery>,
    parts: Parts,
) -> Result<Json<WorkflowLibrary>, ApiError> {
    let directory = match query.directory.as_deref() {
        Some(directory) => std::fs::canonicalize(directory)
            .map_err(|_| ApiError::invalid(format!("no such directory: {directory}")))?,
        None => location(&parts, &state.options.default_directory)?,
    };
    let mut library = state.services.workflow_library(&directory)?;
    if !query.source {
        for entry in &mut library.workflows {
            entry.source = None;
        }
    }
    Ok(Json(library))
}

/// `status=running,queued`: each name a run status.
fn statuses(list: Option<&str>) -> Result<Vec<WorkflowRunStatus>, ApiError> {
    list.into_iter()
        .flat_map(|list| list.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| {
            serde_json::from_value(Value::String(name.to_owned()))
                .map_err(|_| ApiError::invalid(format!("unknown Workflow Run status: {name}")))
        })
        .collect()
}

/// `directory=all` is every Location; a directory, that Location; none, the request's.
fn run_directory(
    state: &AppState,
    parts: &Parts,
    directory: Option<&str>,
) -> Result<Option<String>, ApiError> {
    let path = match directory {
        Some("all") => return Ok(None),
        Some(directory) => std::fs::canonicalize(directory)
            .map_err(|_| ApiError::invalid(format!("no such directory: {directory}")))?,
        None => location(parts, &state.options.default_directory)?,
    };
    Ok(Some(path.display().to_string()))
}

/// Run summaries, active runs first and then the newest (`workflows` → Run monitor).
async fn list(
    State(state): State<AppState>,
    Query(filter): Query<Filter>,
    parts: Parts,
) -> Result<Json<Page<WorkflowRunSummary>>, ApiError> {
    if let Some(id) = &filter.session_id {
        state.runtime.state(id).await?;
    }
    let filter = RunFilter {
        statuses: statuses(filter.status.as_deref())?,
        directory: run_directory(&state, &parts, filter.directory.as_deref())?,
        session_id: filter.session_id,
        limit: filter.limit,
        cursor: filter.cursor,
    };
    let page = state
        .runtime
        .workflow_run_page(&filter)
        .map_err(|error| match error {
            RuntimeError::Invalid(message) if message.starts_with("InvalidCursorError") => {
                ApiError::new(StatusCode::BAD_REQUEST, "InvalidCursorError", message)
            }
            error => error.into(),
        })?;
    Ok(Json(Page {
        data: page.runs,
        cursor: Cursor {
            previous: None,
            next: page.next,
        },
    }))
}

fn existing(state: &AppState, id: &str) -> Result<WorkflowRun, ApiError> {
    state.runtime.workflow_run(id)?.ok_or_else(|| not_found(id))
}

async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<RunDetail>>, ApiError> {
    let summary = state
        .runtime
        .workflow_run_summary(&id)?
        .ok_or_else(|| not_found(&id))?;
    let agents_detail = state.runtime.workflow_agents(&id)?;
    Ok(Json(Data {
        data: RunDetail {
            summary,
            agents_detail,
        },
    }))
}

async fn pause(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<WorkflowRun>>, ApiError> {
    existing(&state, &id)?;
    let run = state
        .runtime
        .pause_workflow_run(&id)
        .await
        .map_err(lifecycle)?;
    Ok(Json(Data { data: run }))
}

async fn stop(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<WorkflowRun>>, ApiError> {
    existing(&state, &id)?;
    let run = state
        .runtime
        .stop_workflow_run(&id)
        .await
        .map_err(lifecycle)?;
    Ok(Json(Data { data: run }))
}

/// Relaunch through replay. Saved Workflows, bundled ones (as this build ships them) and
/// an inline script's `script_file` are read again, so a change needs `revision: true`; an
/// inline run whose file is gone runs its stored source.
async fn resume(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<ResumeBody>>,
) -> Result<Json<Data<WorkflowRun>>, ApiError> {
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let run = existing(&state, &id)?;
    // A resume that gives no share keeps the one the run was last given.
    let requested = body
        .max_concurrent
        .filter(|value| !value.is_null())
        .or_else(|| run.max_concurrent.map(Value::from));
    let mut resume = relaunch(
        state.services.as_ref(),
        &state.runtime,
        &run,
        requested.as_ref(),
    )
    .await?;
    resume.budget = body.budget;
    resume.revision = body.revision;
    let run = state
        .runtime
        .resume_workflow_run(resume)
        .await
        .map_err(lifecycle)?;
    Ok(Json(Data { data: run }))
}

/// The next attempt of `run` as a resume runs it: the module read again, the owner's
/// `workflow.run` rules, and the Location's limits, tiers and share. No budget override
/// and no new revision.
async fn relaunch(
    services: &dyn Services,
    runtime: &Runtime,
    run: &WorkflowRun,
    max_concurrent: Option<&Value>,
) -> Result<ResumeWorkflowRun, ApiError> {
    let location = std::path::PathBuf::from(&run.directory);
    let module = match run.origin["kind"].as_str() {
        Some("saved") => services.saved_workflow(&location, &run.name)?,
        Some("bundled") => bundled_module(run)?,
        _ => WorkflowModule::inline(inline_source(runtime, run)?).map_err(diagnostic)?,
    };
    let owner = runtime.state(&run.session_id).await?.info;
    services.authorize_workflow_run(&owner_context(&owner), &module)?;
    let settings = services.workflow_location(&location)?;
    let share = workflow_start::share(&settings, &module, max_concurrent).map_err(start_error)?;
    Ok(ResumeWorkflowRun {
        run_id: run.id.clone(),
        module,
        limits: settings.limits,
        tiers: settings.tiers,
        budget: None,
        revision: false,
        share,
    })
}

/// The bundled Workflow a run started from, as this build ships it.
fn bundled_module(run: &WorkflowRun) -> Result<WorkflowModule, ApiError> {
    let name = run.origin["name"].as_str().unwrap_or(&run.name);
    cyber_workflow::bundled::module(name)
        .cloned()
        .ok_or_else(|| {
            ApiError::not_found(
                "WorkflowNotFoundError",
                format!("This build has no bundled Workflow named {name:?}"),
            )
        })
}

/// How the runtime rebuilds runs after a restart (`workflows` → Recovery of several runs):
/// as a [`resume`] that gives no share does.
pub struct ServicesRelaunch(pub Arc<dyn Services>);

impl WorkflowRelaunch for ServicesRelaunch {
    fn relaunch<'a>(
        &'a self,
        runtime: &'a Runtime,
        run: &'a WorkflowRun,
    ) -> BoxFuture<'a, Result<ResumeWorkflowRun, String>> {
        Box::pin(async move {
            let requested = run.max_concurrent.map(Value::from);
            relaunch(self.0.as_ref(), runtime, run, requested.as_ref())
                .await
                .map_err(|error| error.body.message)
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct InterruptedFilter {
    /// A Location's directory, or `all`; the request Location when absent.
    directory: Option<String>,
}

/// Resume every run a restart interrupted, of the request Location or of every Location
/// (`directory=all`). A run paused by a user stays paused.
async fn resume_interrupted(
    State(state): State<AppState>,
    Query(filter): Query<InterruptedFilter>,
    parts: Parts,
) -> Result<Json<Data<InterruptedResume>>, ApiError> {
    let directory = match filter.directory.as_deref() {
        Some("all") => None,
        Some(directory) => Some(
            std::fs::canonicalize(directory)
                .map_err(|_| ApiError::invalid(format!("no such directory: {directory}")))?,
        ),
        None => Some(location(&parts, &state.options.default_directory)?),
    };
    let report = state
        .runtime
        .resume_interrupted_workflow_runs(directory.as_deref())
        .await?;
    Ok(Json(Data { data: report }))
}

/// The script an inline run relaunches: its `script_file` as the user left it, else the
/// source of its current revision.
fn inline_source(runtime: &Runtime, run: &WorkflowRun) -> Result<String, ApiError> {
    if let Some(file) = &run.script_file {
        match std::fs::read_to_string(file) {
            Ok(source) => return Ok(source),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ApiError::conflict(format!(
                    "The run's script file {file} cannot be read: {error}"
                )));
            }
        }
    }
    runtime
        .workflow_run_source(&run.id)?
        .ok_or_else(|| ApiError::conflict("The run's script source is not stored"))
}

async fn logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Page<WorkflowLogEntry>>, ApiError> {
    existing(&state, &id)?;
    Ok(Json(Page {
        data: state.runtime.workflow_logs(&id)?,
        cursor: Cursor::default(),
    }))
}
