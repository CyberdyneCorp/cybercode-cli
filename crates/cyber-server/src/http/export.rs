//! Session export (`storage-events` → Session export): `GET /sessions/{id}/export`
//! returns the Session info and totals, its messages with their parts and tool calls, its
//! Goals and its Workflow Run summaries, and with `include_children=true` the same for
//! every child Session, recursively. The messages are the active path, or with
//! `branches=all` every branch in creation order. The top-level `attachments` array
//! carries the bytes of every blob the messages reference. `sanitize=true` omits it and
//! redacts text, paths, URLs, tool inputs and outputs and metadata (see [`sanitize`]).
//!
//! Session import (`storage-events` → Session import): `POST /sessions/import` creates a
//! new Session in the request's Location from a JSON export; a newer `version` is
//! `ExportTooNewError`, and blobs that do not verify refuse the import before any write.

mod sanitize;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State, rejection::JsonRejection},
    http::{StatusCode, request::Parts},
    routing::{get, post},
};
use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AppState, envelope::Data, envelope::location, error::ApiError, sessions::Message};
use crate::runtime::attachments::ExportedBlob;
use crate::runtime::goals::Goal;
use crate::runtime::{
    Entry, ImportDocument, ListFilter, Runtime, RuntimeError, SessionInfo, Totals,
    WorkflowRunSummary,
};

/// The export format version; an import refuses a newer one (`ExportTooNewError`).
pub const EXPORT_VERSION: u32 = 1;
/// Child Sessions and Workflow Runs read per page.
const PAGE: u32 = 200;
/// The largest import document (its blobs are base64).
pub(super) const IMPORT_BODY_LIMIT: usize = 256 * 1024 * 1024;

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
    /// The blobs the messages (children's included) reference, once each; only on the
    /// top-level export, and omitted by `sanitize=true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<ExportedBlob>>,
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
    Router::new()
        .route("/sessions/{session_id}/export", get(export))
        .route(
            "/sessions/import",
            post(import).layer(DefaultBodyLimit::max(IMPORT_BODY_LIMIT)),
        )
}

async fn export(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Json<Data<Value>>, ApiError> {
    let all = super::sessions::all_branches(query.branches.as_deref())?;
    let mut built = build(&state.runtime, id, query.include_children, all).await?;
    if !query.sanitize {
        built.attachments = Some(blobs(&state.runtime, &built)?);
    }
    let mut value =
        serde_json::to_value(built).map_err(|e| RuntimeError::Corrupt(e.to_string()))?;
    if query.sanitize {
        mark_sanitized(&mut value);
        sanitize::Sanitizer::default().apply(&mut value);
    }
    Ok(Json(Data { data: value }))
}

/// The blobs `export` and its children reference.
fn blobs(runtime: &Runtime, export: &SessionExport) -> Result<Vec<ExportedBlob>, RuntimeError> {
    let store = runtime.attachments()?;
    let mut out = Vec::new();
    let mut pending = vec![export];
    while let Some(export) = pending.pop() {
        let parts = export
            .messages
            .iter()
            .flat_map(|message| match &message.entry {
                Entry::User { parts, .. } => parts.as_slice(),
                _ => &[],
            });
        store.exported(&export.session.id, parts, &mut out);
        pending.extend(export.children.iter().flatten());
    }
    Ok(out)
}

/// `POST /sessions/import`: a new Session in the request's Location from an export.
async fn import(
    State(state): State<AppState>,
    parts: Parts,
    document: Result<Json<Value>, JsonRejection>,
) -> Result<(StatusCode, Json<Data<SessionInfo>>), ApiError> {
    let Json(document) = document.map_err(|rejection| match rejection.status() {
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "MessageTooLargeError",
            "The import document exceeds 256 MiB",
        ),
        _ => ApiError::invalid(rejection.body_text()),
    })?;
    let directory = location(&parts, &state.options.default_directory)?;
    let version = document["version"].as_u64().unwrap_or_default();
    if version > u64::from(EXPORT_VERSION) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ExportTooNewError",
            format!("The export is version {version}; this cyber reads up to {EXPORT_VERSION}"),
        ));
    }
    let document: ImportDocument = serde_json::from_value(document)
        .map_err(|error| ApiError::invalid(format!("not a Session export: {error}")))?;
    let info = state
        .runtime
        .import_session(directory.display().to_string(), document)
        .await?;
    Ok((StatusCode::CREATED, Json(Data { data: info })))
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
            attachments: None,
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
