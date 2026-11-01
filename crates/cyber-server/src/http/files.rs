//! File browsing (`server-api` → File browsing routes, in the `fs` route group): one
//! directory level with git status and per-entry `read` decisions, a fuzzy path search
//! and file content. Every path is confined to the project root, and content is served
//! only when the Session's rules allow `read`; an `ask` raises the normal permission
//! request and waits for it. What is served is the file or directory the decision was
//! made on: one swapped in during the wait is refused.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use axum::extract::{Query, State};
use axum::http::request::Parts;
use axum::routing::get;
use axum::{Json, Router};
use cyber_core::files::{self, Content, Entry, Refusal};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::envelope::{Located, LocationInfo, location};
use super::{ApiError, AppState};
use crate::runtime::{FileDecision, TurnContext};

/// Search results returned at most.
const MAX_SEARCH: usize = 200;

#[derive(Debug, Default, Deserialize)]
pub struct FilesQuery {
    /// Project-relative; the project root when empty.
    pub path: Option<String>,
    /// Include dot entries and ignored ones.
    pub hidden: Option<bool>,
    /// Apply this Session's rules and Mode.
    pub session_id: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FileEntry {
    #[serde(flatten)]
    pub entry: Entry,
    /// The `read` decision for this path.
    pub read: FileDecision,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FileListing {
    pub entries: Vec<FileEntry>,
    /// More than 2,000 entries; the rest are left out.
    pub truncated: bool,
    /// Dot and ignored entries left out without `hidden=true`.
    pub hidden: usize,
    /// Changed files in the whole project, by git status.
    pub changed: usize,
    /// The canonical project root that entry paths are relative to; it may be an
    /// ancestor of the Location.
    pub root: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FileHit {
    pub path: String,
    pub read: FileDecision,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FileSearch {
    pub results: Vec<FileHit>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/files", get(list))
        .route("/files/search", get(search))
        .route("/files/content", get(content))
}

/// The decision context and the canonical project root.
pub(super) async fn scope(
    state: &AppState,
    parts: &Parts,
    session_id: Option<&str>,
) -> Result<(TurnContext, PathBuf, LocationInfo), ApiError> {
    let directory = location(parts, &state.options.default_directory)?;
    let turn = state.runtime.file_turn(session_id, &directory).await?;
    let root = cyber_core::config::project_root(Path::new(&turn.directory));
    let root = std::fs::canonicalize(&root)
        .map_err(|_| ApiError::invalid(format!("no such directory: {}", root.display())))?;
    Ok((turn, root, LocationInfo::of(&directory)))
}

fn refused(refusal: Refusal) -> ApiError {
    match refusal {
        Refusal::OutsideProject => {
            ApiError::forbidden("The path is outside the project").with_reason("outside_project")
        }
        Refusal::NotFound => ApiError::not_found("NotFoundError", "No such file or directory"),
        Refusal::WrongKind => ApiError::invalid("The path is not of the expected kind"),
        Refusal::Changed => {
            ApiError::forbidden("The path changed after it was authorized").with_reason("changed")
        }
        Refusal::Io(error) => ApiError::invalid(error),
    }
}

fn denied(decision: FileDecision, message: &str) -> ApiError {
    let decision = match decision {
        FileDecision::Allow => "allow",
        FileDecision::Ask => "ask",
        FileDecision::Deny => "deny",
    };
    ApiError::forbidden(message)
        .with_reason("permission")
        .with_field("decision", decision)
}

/// Allow `action` on `path`: at once, after the user approves an `ask` (only with a
/// Session, which owns the request), or never.
pub(super) async fn permit(
    state: &AppState,
    turn: &TurnContext,
    action: &str,
    path: &Path,
) -> Result<FileDecision, ApiError> {
    let decision = state
        .runtime
        .file_decisions(turn, action, &[path.to_path_buf()])
        .await
        .first()
        .copied()
        .unwrap_or(FileDecision::Deny);
    match decision {
        FileDecision::Allow => Ok(FileDecision::Allow),
        FileDecision::Ask if !turn.session_id.is_empty() => state
            .runtime
            .authorize_file(turn, action, path)
            .await
            .map(|_| FileDecision::Ask)
            .map_err(|message| denied(FileDecision::Ask, &message)),
        _ => Err(denied(decision, &format!("Permission denied: {action}"))),
    }
}

/// Run blocking filesystem work.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Refusal> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| ApiError::invalid("File access was interrupted"))?
        .map_err(refused)
}

async fn list(
    State(state): State<AppState>,
    parts: Parts,
    Query(q): Query<FilesQuery>,
) -> Result<Json<Located<FileListing>>, ApiError> {
    let (turn, root, location) = scope(&state, &parts, q.session_id.as_deref()).await?;
    let rel = q.path.unwrap_or_default();
    let at = root.clone();
    let dir = blocking(move || files::target(&at, &rel)).await?;
    permit(&state, &turn, "list", dir.path()).await?;
    let hidden = q.hidden.unwrap_or(false);
    let at = root.clone();
    let listing = blocking(move || {
        let statuses: BTreeMap<String, char> = cyber_core::vcs::file_statuses(&at);
        files::list_target(&at, &dir, hidden, &statuses)
    })
    .await?;
    let paths: Vec<PathBuf> = listing.entries.iter().map(|e| root.join(&e.path)).collect();
    let decisions = state.runtime.file_decisions(&turn, "read", &paths).await;
    let entries = listing
        .entries
        .into_iter()
        .zip(decisions)
        .map(|(entry, read)| FileEntry { entry, read })
        .collect();
    Ok(Json(Located {
        location,
        data: FileListing {
            entries,
            truncated: listing.truncated,
            hidden: listing.hidden,
            changed: listing.changed,
            root: root.display().to_string(),
        },
    }))
}

async fn search(
    State(state): State<AppState>,
    parts: Parts,
    Query(q): Query<FilesQuery>,
) -> Result<Json<Located<FileSearch>>, ApiError> {
    let (turn, root, location) = scope(&state, &parts, q.session_id.as_deref()).await?;
    let at = root.clone();
    let top = blocking(move || files::target(&at, "")).await?;
    permit(&state, &turn, "list", top.path()).await?;
    let query = q.query.unwrap_or_default();
    let limit = q.limit.unwrap_or(50).clamp(1, MAX_SEARCH);
    let at = root.clone();
    let hits = blocking(move || Ok(files::search(&at, &query, MAX_SEARCH))).await?;
    let mut hits = listable(&state, &turn, &root, hits).await;
    hits.truncate(limit);
    let paths: Vec<PathBuf> = hits.iter().map(|p| root.join(p)).collect();
    let decisions = state.runtime.file_decisions(&turn, "read", &paths).await;
    let results = hits
        .into_iter()
        .zip(decisions)
        .map(|(path, read)| FileHit { path, read })
        .collect();
    Ok(Json(Located {
        location,
        data: FileSearch { results },
    }))
}

/// The hits whose directory may be listed without asking: a name is shown only where
/// `/files` would show it. The root itself was permitted already.
async fn listable(
    state: &AppState,
    turn: &TurnContext,
    root: &Path,
    hits: Vec<String>,
) -> Vec<String> {
    let parent = |hit: &str| root.join(hit).parent().map(Path::to_path_buf);
    let dirs: Vec<PathBuf> = hits
        .iter()
        .filter_map(|hit| parent(hit))
        .filter(|dir| dir != root)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let decisions = state.runtime.file_decisions(turn, "list", &dirs).await;
    let allowed: std::collections::HashSet<PathBuf> = dirs
        .into_iter()
        .zip(decisions)
        .filter(|(_, decision)| *decision == FileDecision::Allow)
        .map(|(dir, _)| dir)
        .collect();
    hits.into_iter()
        .filter(|hit| parent(hit).is_some_and(|dir| dir == root || allowed.contains(&dir)))
        .collect()
}

async fn content(
    State(state): State<AppState>,
    parts: Parts,
    Query(q): Query<FilesQuery>,
) -> Result<Json<Located<Content>>, ApiError> {
    let (turn, root, location) = scope(&state, &parts, q.session_id.as_deref()).await?;
    let rel = q
        .path
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::invalid("path is required"))?;
    let at = root.clone();
    let file = blocking(move || files::target(&at, &rel)).await?;
    permit(&state, &turn, "read", file.path()).await?;
    let data = blocking(move || files::content_target(&file)).await?;
    Ok(Json(Located { location, data }))
}
