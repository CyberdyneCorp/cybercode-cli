//! Session export (`storage-events` → Session export): `GET /sessions/{id}/export`
//! returns the Session info and totals, its messages with their parts and tool calls, its
//! Goals and its Workflow Run summaries, and with `include_children=true` the same for
//! every child Session, recursively. The messages are the active path, or with
//! `branches=all` every branch in creation order. `sanitize=true` redacts text, paths,
//! URLs, tool inputs and outputs and metadata (see [`sanitize`]).

mod sanitize;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AppState, envelope::Data, error::ApiError, sessions::Message};
use crate::runtime::goals::Goal;
use crate::runtime::{ListFilter, Runtime, RuntimeError, SessionInfo, Totals, WorkflowRunSummary};

/// The export format version; an import refuses a newer one (`ExportTooNewError`).
pub const EXPORT_VERSION: u32 = 1;
/// Child Sessions and Workflow Runs read per page.
const PAGE: u32 = 200;

/// One exported Session.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SessionExport {
    pub version: u32,
    pub exported_ms: i64,
    /// Text, paths, URLs, tool inputs and outputs and metadata are
    /// `[redacted:<kind>:<n>]` markers; structure, IDs, counts and timings are kept.
    pub sanitized: bool,
    pub session: SessionInfo,
    pub totals: Totals,
    /// `active` (the active path) or `all` (every branch, in creation order).
    pub branches: String,
    /// The messages in order, with their parts and, for answers, their tool calls; each
    /// `parent_message_id` links the branch tree.
    pub messages: Vec<Message>,
    /// Every Goal of the Session, oldest first, with its evaluations.
    pub goals: Vec<Goal>,
    pub workflow_runs: Vec<WorkflowRunSummary>,
    /// Child Sessions, oldest first; present with `include_children=true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<SessionExport>>,
}

/// `GET .../export` options.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ExportQuery {
    #[serde(default)]
    pub sanitize: bool,
    #[serde(default)]
    pub include_children: bool,
    /// `all` exports every branch instead of the active path.
    #[serde(default)]
    pub branches: Option<String>,
}

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/sessions/{session_id}/export", get(export))
}

async fn export(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Json<Data<Value>>, ApiError> {
    let all = super::sessions::all_branches(query.branches.as_deref())?;
    let built = build(&state.runtime, id, query.include_children, all).await?;
    let mut value =
        serde_json::to_value(built).map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
    if query.sanitize {
        mark_sanitized(&mut value);
        sanitize::Sanitizer::default().apply(&mut value);
    }
    Ok(Json(Data { data: value }))
}

/// Set `sanitized` on the export and every child export.
fn mark_sanitized(export: &mut Value) {
    export["sanitized"] = true.into();
    if let Some(children) = export["children"].as_array_mut() {
        children.iter_mut().for_each(mark_sanitized);
    }
}

/// The export of `id`, with its children and every branch when asked.
fn build(
    runtime: &Runtime,
    id: String,
    children: bool,
    all: bool,
) -> BoxFuture<'_, Result<SessionExport, RuntimeError>> {
    Box::pin(async move {
        let state = runtime.state(&id).await?;
        let messages = super::sessions::listed(&state, all)
            .into_iter()
            .map(|entry| super::sessions::message_of(&state, entry))
            .collect();
        let mut goals: Vec<Goal> = state.goals.goals.values().cloned().collect();
        goals.sort_by_key(|goal| goal.created_ms);
        let mut export = SessionExport {
            version: EXPORT_VERSION,
            exported_ms: chrono::Utc::now().timestamp_millis(),
            sanitized: false,
            session: state.info.clone(),
            totals: state.totals.clone(),
            branches: if all { "all" } else { "active" }.into(),
            messages,
            goals,
            workflow_runs: runs(runtime, &id)?,
            children: None,
        };
        if children {
            let mut nested = Vec::new();
            for child in child_ids(runtime, &id)? {
                nested.push(build(runtime, child, true, all).await?);
            }
            export.children = Some(nested);
        }
        Ok(export)
    })
}

/// Summaries of every Workflow Run the Session started.
fn runs(runtime: &Runtime, id: &str) -> Result<Vec<WorkflowRunSummary>, RuntimeError> {
    let mut filter = crate::runtime::RunFilter {
        session_id: Some(id.to_owned()),
        limit: Some(PAGE),
        ..Default::default()
    };
    let mut all = Vec::new();
    loop {
        let page = runtime.workflow_run_page(&filter)?;
        all.extend(page.runs);
        match page.next {
            Some(next) => filter.cursor = Some(next),
            None => return Ok(all),
        }
    }
}

/// The direct child Sessions of `id`, archived ones included, oldest first.
fn child_ids(runtime: &Runtime, id: &str) -> Result<Vec<String>, RuntimeError> {
    let mut filter = ListFilter {
        parent: Some(id.to_owned()),
        include_archived: true,
        limit: Some(PAGE),
        ..ListFilter::default()
    };
    let mut rows = Vec::new();
    loop {
        let page = runtime.list(&filter)?;
        rows.extend(page.sessions);
        match page.next {
            Some(next) => filter.cursor = Some(next),
            None => break,
        }
    }
    rows.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    Ok(rows.into_iter().map(|row| row.id).collect())
}
