//! Goal routes (`goals` → Goal management commands): `/api/v1/sessions/{id}/goals`.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch, post},
};
use cyber_core::budget::Budget;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AppState, envelope::Data, error::ApiError};
use crate::runtime::goals::{Goal, GoalEdit, GoalRequest};

type Result<T> = std::result::Result<T, ApiError>;

/// `GET .../goals`: the current Goal, the queue in order and the most recent ended ones.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GoalsView {
    pub current: Option<Goal>,
    pub queue: Vec<Goal>,
    /// Ended Goals, newest first, at most 20.
    pub ended: Vec<Goal>,
}

/// How `POST .../goals` places the new Goal.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalMode {
    /// Replace the current Goal and start working on this one.
    #[default]
    Set,
    /// Append to the queue; it activates at once when no Goal is current.
    Add,
}

/// `POST .../goals`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateGoal {
    pub condition: String,
    #[serde(default)]
    pub check: Option<String>,
    #[serde(default)]
    pub budget: Option<Budget>,
    #[serde(default)]
    pub mode: GoalMode,
}

/// `PATCH .../goals/{goal_id}`; `check: null` removes the check.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditGoal {
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub check: Option<Option<String>>,
    #[serde(default)]
    pub budget: Option<Budget>,
}

/// `PATCH .../goals/order`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GoalOrder {
    pub goal_ids: Vec<String>,
}

/// A present field (even `null`) becomes `Some`, so `null` can clear a value.
fn present<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

pub(super) fn routes() -> Router<AppState> {
    let base = "/sessions/{session_id}/goals";
    Router::new()
        .route(base, get(list).post(create).delete(clear_all))
        .route(&format!("{base}/order"), patch(reorder))
        .route(&format!("{base}/pause"), post(pause))
        .route(&format!("{base}/resume"), post(resume))
        .route(&format!("{base}/skip"), post(skip))
        .route(
            &format!("{base}/{{goal_id}}"),
            get(show).patch(edit).delete(clear_one),
        )
}

async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<GoalsView>>> {
    let goals = state.runtime.goals(&id).await?;
    let current = goals.current().cloned();
    let queue = goals
        .queue
        .iter()
        .filter_map(|q| goals.goals.get(q).cloned())
        .collect();
    let mut ended: Vec<Goal> = goals
        .goals
        .values()
        .filter(|g| g.status.is_terminal())
        .cloned()
        .collect();
    ended.sort_by_key(|g| std::cmp::Reverse(g.ended_ms));
    ended.truncate(20);
    Ok(Json(Data {
        data: GoalsView {
            current,
            queue,
            ended,
        },
    }))
}

async fn create(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CreateGoal>,
) -> Result<(StatusCode, Json<Data<Goal>>)> {
    let request = GoalRequest {
        condition: body.condition,
        check: body.check,
        budget: body.budget,
    };
    let goal = match body.mode {
        GoalMode::Set => state.runtime.set_goal(&id, request).await?,
        GoalMode::Add => state.runtime.add_goal(&id, request).await?,
    };
    Ok((StatusCode::CREATED, Json(Data { data: goal })))
}

async fn show(
    State(state): State<AppState>,
    Path((id, goal_id)): Path<(String, String)>,
) -> Result<Json<Data<Goal>>> {
    let goals = state.runtime.goals(&id).await?;
    let goal = goals.goals.get(&goal_id).cloned().ok_or_else(|| {
        ApiError::from(crate::runtime::RuntimeError::GoalNotFound(goal_id.clone()))
    })?;
    Ok(Json(Data { data: goal }))
}

async fn edit(
    State(state): State<AppState>,
    Path((id, goal_id)): Path<(String, String)>,
    Json(body): Json<EditGoal>,
) -> Result<Json<Data<Goal>>> {
    let edit = GoalEdit {
        condition: body.condition,
        check: body.check,
        budget: body.budget,
    };
    Ok(Json(Data {
        data: state.runtime.edit_goal(&id, &goal_id, edit).await?,
    }))
}

async fn clear_one(
    State(state): State<AppState>,
    Path((id, goal_id)): Path<(String, String)>,
) -> Result<Json<Data<Vec<String>>>> {
    Ok(Json(Data {
        data: state.runtime.clear_goals(&id, Some(&goal_id)).await?,
    }))
}

async fn clear_all(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Vec<String>>>> {
    Ok(Json(Data {
        data: state.runtime.clear_goals(&id, None).await?,
    }))
}

async fn reorder(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<GoalOrder>,
) -> Result<Json<Data<Vec<String>>>> {
    Ok(Json(Data {
        data: state.runtime.reorder_goals(&id, body.goal_ids).await?,
    }))
}

async fn pause(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Data<Goal>>> {
    Ok(Json(Data {
        data: state.runtime.pause_goal(&id).await?,
    }))
}

async fn resume(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Data<Goal>>> {
    Ok(Json(Data {
        data: state.runtime.resume_goal(&id).await?,
    }))
}

/// The next Goal it activated, or `null` when the queue was empty.
async fn skip(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Option<Goal>>>> {
    Ok(Json(Data {
        data: state.runtime.skip_goal(&id).await?,
    }))
}
