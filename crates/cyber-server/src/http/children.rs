//! Direct child threads, independent of their worktree Location.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    AppState,
    envelope::{Cursor, Data},
    error::ApiError,
};
use crate::runtime::{Entry, InputStatus, ListFilter, Runtime, SessionInfo, SessionState};

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Running,
    Waiting,
    Completed,
    Failed,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChildThread {
    pub session: SessionInfo,
    pub status: ThreadStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChildThreads {
    pub parent_id: String,
    pub data: Vec<ChildThread>,
    pub cursor: Cursor,
}

#[derive(Default, Deserialize)]
struct QueryOptions {
    limit: Option<u32>,
    cursor: Option<String>,
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/sessions/{id}/children", get(list))
}

async fn list(
    State(app): State<AppState>,
    Path(parent): Path<String>,
    Query(query): Query<QueryOptions>,
) -> Result<Json<Data<ChildThreads>>, ApiError> {
    app.runtime.state(&parent).await?;
    let page = app.runtime.list(&ListFilter {
        parent: Some(parent.clone()),
        limit: query.limit,
        cursor: query.cursor,
        include_archived: true,
        ..Default::default()
    })?;
    let mut data = Vec::with_capacity(page.sessions.len());
    for row in page.sessions {
        let state = app.runtime.state(&row.id).await?;
        let (status, reason) = observed(&app.runtime, &state);
        data.push(ChildThread {
            session: state.info,
            status,
            reason,
        });
    }
    Ok(Json(Data {
        data: ChildThreads {
            parent_id: parent,
            data,
            cursor: Cursor {
                previous: None,
                next: page.next,
            },
        },
    }))
}

fn observed(runtime: &Runtime, state: &SessionState) -> (ThreadStatus, Option<String>) {
    if runtime
        .pending_requests(Some(&state.info.id))
        .iter()
        .any(|r| r.session_id == state.info.id)
    {
        return (
            ThreadStatus::Waiting,
            Some("Waiting for a user reply".into()),
        );
    }
    if runtime.is_running(&state.info.id) {
        return (ThreadStatus::Running, None);
    }
    if state.child_worktree_setup_pending() {
        return failed("Worktree setup recovery is required");
    }
    if !state.unresolved().is_empty() {
        return failed("Tool outcome is unknown; recovery is required");
    }
    let Some(input) = state.inbox.last() else {
        return (
            ThreadStatus::Waiting,
            Some("No child prompt has been admitted".into()),
        );
    };
    match input.status {
        InputStatus::Pending | InputStatus::Held => {
            return (
                ThreadStatus::Waiting,
                Some("Prompt is pending or held".into()),
            );
        }
        InputStatus::Refused => return failed("Prompt was refused"),
        InputStatus::Promoted => {}
    }
    terminal(state, &input.message_id)
}

fn failed(reason: impl Into<String>) -> (ThreadStatus, Option<String>) {
    (ThreadStatus::Failed, Some(reason.into()))
}

fn terminal(state: &SessionState, prompt: &str) -> (ThreadStatus, Option<String>) {
    let start = state
        .entries
        .iter()
        .position(|entry| matches!(entry, Entry::User { id, .. } if id == prompt));
    let answer = start.and_then(|start| {
        state.entries[start + 1..].iter().rev().find_map(|entry| {
            if let Entry::Assistant(answer) = entry {
                Some(answer)
            } else {
                None
            }
        })
    });
    let Some(answer) = answer else {
        return failed("Stopped without a child answer");
    };
    if let Some(error) = &answer.error {
        return failed(error.clone());
    }
    let structured = state.structured_result().is_some()
        && answer.calls.iter().any(|id| {
            state.calls.get(id).is_some_and(|call| {
                call.name == "return_result"
                    && call.status == crate::runtime::CallStatus::Ok
                    && call.structured_output.is_some()
            })
        });
    if state.output_schema().is_some() && !structured {
        return failed(state.structured_result_error());
    }
    if structured || (answer.finished && answer.calls.is_empty()) {
        (ThreadStatus::Completed, None)
    } else {
        failed("Stopped without a completed child answer")
    }
}
