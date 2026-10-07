//! Explicit user creation of a fresh managed worktree Session.

use axum::extract::State;
use axum::http::{StatusCode, request::Parts};
use axum::routing::get;
use axum::{Json, Router};
use cyber_core::worktrees::{Managed, Name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::envelope::{Located, LocationInfo, location};
use super::sessions::{CreateBody, Session, session};
use super::{ApiError, AppState};
use crate::runtime::{CreateSession, SessionInfo};

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CreateWorktreeBody {
    /// Generated when omitted.
    pub name: Option<String>,
    /// Correlates live setup updates with this request.
    pub call_id: Option<String>,
    #[serde(default)]
    pub session: CreateBody,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SetupStatus {
    Completed,
    Failed { index: usize, code: Option<i32> },
    Error { message: String },
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WorktreeInfo {
    pub id: String,
    pub name: String,
    pub path: std::path::PathBuf,
    pub branch: String,
    pub base: String,
}

impl From<Managed> for WorktreeInfo {
    fn from(managed: Managed) -> Self {
        Self {
            id: managed.id,
            name: managed.name,
            path: managed.path,
            branch: managed.branch,
            base: managed.base,
        }
    }
}

pub struct StartedWorktree {
    pub worktree: WorktreeInfo,
    pub session: SessionInfo,
    pub setup: SetupStatus,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CreatedWorktree {
    pub worktree: WorktreeInfo,
    pub session: Session,
    pub setup: SetupStatus,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WorktreeEntry {
    Ready {
        worktree: WorktreeInfo,
        dirty: bool,
        ahead: u64,
        behind: u64,
        sessions: Vec<crate::runtime::SessionRow>,
    },
    Pending {
        worktree: WorktreeInfo,
    },
    Invalid {
        name: String,
        message: String,
    },
}

pub(super) fn routes() -> Router<AppState> {
    Router::new().route("/worktrees", get(list).post(create))
}

async fn list(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Vec<WorktreeEntry>>>, ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    let mut entries = state.services.list_worktrees(directory.clone()).await?;
    attach_sessions(&state, &mut entries).await?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data: entries,
    }))
}

async fn attach_sessions(state: &AppState, entries: &mut [WorktreeEntry]) -> Result<(), ApiError> {
    let mut by_directory = session_locations(state)?;
    for entry in entries {
        if let WorktreeEntry::Ready {
            worktree, sessions, ..
        } = entry
        {
            sessions.clear();
            for row in by_directory.remove(&worktree.path).unwrap_or_default() {
                if state.runtime.worktree_binding(&row.id).await?.as_deref() == Some(&worktree.id) {
                    sessions.push(row);
                }
            }
        }
    }
    Ok(())
}

fn session_locations(
    state: &AppState,
) -> Result<std::collections::HashMap<std::path::PathBuf, Vec<crate::runtime::SessionRow>>, ApiError>
{
    let mut result: std::collections::HashMap<_, Vec<_>> = std::collections::HashMap::new();
    let mut filter = crate::runtime::ListFilter {
        limit: Some(200),
        include_archived: true,
        ..Default::default()
    };
    loop {
        let page = state.runtime.list(&filter)?;
        for row in page.sessions {
            if let Ok(directory) = std::path::Path::new(&row.directory).canonicalize() {
                result.entry(directory).or_default().push(row);
            }
        }
        filter.cursor = page.next;
        if filter.cursor.is_none() {
            return Ok(result);
        }
    }
}

async fn create(
    State(state): State<AppState>,
    parts: Parts,
    Json(body): Json<CreateWorktreeBody>,
) -> Result<(StatusCode, Json<Located<CreatedWorktree>>), ApiError> {
    let directory = location(&parts, &state.options.default_directory)?;
    let name = body
        .name
        .map(|name| Name::parse(&name))
        .transpose()
        .map_err(ApiError::invalid)?
        .unwrap_or_else(Name::generate);
    if body.session.id.is_some() {
        return Err(ApiError::invalid(
            "Worktree creation requires a fresh Session",
        ));
    }
    let call_id = body
        .call_id
        .unwrap_or_else(|| cyber_core::ids::new_id("call"));
    if call_id.is_empty()
        || call_id.len() > 128
        || !call_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        return Err(ApiError::invalid(
            "call_id must be 1-128 ASCII letters, digits, underscores or hyphens",
        ));
    }
    let model = body
        .session
        .model
        .or_else(|| state.services.default_model(&directory))
        .ok_or_else(|| {
            ApiError::invalid("No model is configured; pass session.model or set model in config")
        })?;
    let request = CreateSession {
        directory: directory.display().to_string(),
        model,
        agent: body.session.agent,
        mode: body.session.mode,
        parent_id: body.session.parent_id,
        title: body.session.title,
        rules: body.session.rules,
        max_steps: body.session.max_steps,
        ..Default::default()
    };
    let started = state
        .services
        .create_worktree(directory, request, name, call_id)
        .await?;
    let data = CreatedWorktree {
        session: session(&state, state.runtime.state(&started.session.id).await?),
        worktree: started.worktree,
        setup: started.setup,
    };
    Ok((
        StatusCode::CREATED,
        Json(Located {
            location: LocationInfo::of(std::path::Path::new(&data.session.info.directory)),
            data,
        }),
    ))
}
