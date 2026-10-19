//! Loop routes (`loops-scheduling` → Loop status visibility).

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
};
use cyber_core::loops::{LoopCommand, LoopSpec};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{AppState, envelope::Data, error::ApiError};
use crate::runtime::loops::LoopState;

/// `POST /api/v1/sessions/:id/loops`: the `/loop` arguments as typed, or the parsed fields.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum CreateLoop {
    /// `{"command": "5m --max 10 check the deploy"}`, parsed like `/loop`.
    Command {
        command: String,
    },
    Spec(LoopSpec),
}

type Result<T> = std::result::Result<T, ApiError>;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/sessions/{session_id}/loops",
            get(list).post(create).delete(stop_all),
        )
        .route("/sessions/{session_id}/loops/{loop_id}", delete(stop_one))
}

async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Vec<LoopState>>>> {
    Ok(Json(Data {
        data: state.runtime.loops(&id).await?,
    }))
}

async fn create(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CreateLoop>,
) -> Result<(StatusCode, Json<Data<LoopState>>)> {
    let spec = match body {
        CreateLoop::Spec(spec) => spec,
        CreateLoop::Command { command } => match cyber_core::loops::parse(&command) {
            Ok(LoopCommand::Create(spec)) => spec,
            Ok(_) => {
                return Err(ApiError::invalid(
                    "list loops with GET and stop them with DELETE".to_string(),
                ));
            }
            Err(message) => return Err(ApiError::invalid(message)),
        },
    };
    let created = state.runtime.create_loop(&id, spec).await?;
    Ok((StatusCode::CREATED, Json(Data { data: created })))
}

async fn stop_all(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<Vec<String>>>> {
    Ok(Json(Data {
        data: state.runtime.stop_loops(&id, None).await?,
    }))
}

async fn stop_one(
    State(state): State<AppState>,
    Path((id, loop_id)): Path<(String, String)>,
) -> Result<Json<Data<Vec<String>>>> {
    Ok(Json(Data {
        data: state.runtime.stop_loops(&id, Some(&loop_id)).await?,
    }))
}
