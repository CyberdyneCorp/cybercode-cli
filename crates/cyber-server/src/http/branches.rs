//! Session branch routes (`server-api` → Session branch routes): the user-message tree,
//! checkout and labels. `GET .../messages?branches=all` lives with the message routes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::AppState;
use super::envelope::Data;
use super::error::ApiError;
use crate::runtime::{BranchCheckout, BranchNode, CheckedOutBranch, branch_nodes};

type Result<T> = std::result::Result<T, ApiError>;

/// `PUT .../labels/:name`: the message the label marks.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LabelBody {
    pub message_id: String,
}

pub(super) fn routes() -> Router<AppState> {
    let s = "/sessions/{session_id}";
    Router::new()
        .route(&format!("{s}/tree"), get(tree))
        .route(&format!("{s}/checkout"), post(checkout))
        .route(&format!("{s}/labels/{{name}}"), put(label).delete(unlabel))
}

async fn tree(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Vec<BranchNode>>>> {
    let s = state.runtime.state(&id).await?;
    Ok(Json(Data {
        data: branch_nodes(&s),
    }))
}

async fn checkout(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<BranchCheckout>,
) -> Result<Json<Data<CheckedOutBranch>>> {
    Ok(Json(Data {
        data: state.runtime.checkout(&id, &b).await?,
    }))
}

async fn label(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
    Json(b): Json<LabelBody>,
) -> Result<StatusCode> {
    state
        .runtime
        .label_branch(&id, &name, Some(&b.message_id))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unlabel(
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> Result<StatusCode> {
    state.runtime.label_branch(&id, &name, None).await?;
    Ok(StatusCode::NO_CONTENT)
}
