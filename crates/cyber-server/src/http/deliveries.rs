//! Side chat and delivered files (`session-runtime` → Side chat, `background-tasks` →
//! Send file to user).

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{ApiError, AppState, envelope::Data};
use crate::runtime::{RuntimeError, SideAnswer};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sessions/{session_id}/side", post(side))
        .route("/sessions/{session_id}/files/{file_id}", get(file))
}

/// `POST /sessions/{id}/side`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SideBody {
    pub question: String,
}

async fn side(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SideBody>,
) -> Result<Json<Data<SideAnswer>>, ApiError> {
    match state.runtime.side_chat(&id, &body.question).await {
        Ok(data) => Ok(Json(Data { data })),
        Err(RuntimeError::Model(message)) => {
            let mut error = ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                format!("The side chat model failed: {message}"),
            );
            error.body.service = Some("model".into());
            Err(error)
        }
        Err(error) => Err(error.into()),
    }
}

/// The bytes of a file the Session delivered, as an attachment.
async fn file(
    State(state): State<AppState>,
    Path((id, file_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let missing = || ApiError::not_found("FileNotFoundError", format!("No sent file {file_id}"));
    let sent = state
        .runtime
        .sent_file(&id, &file_id)
        .await?
        .ok_or_else(missing)?;
    let bytes = tokio::fs::read(&sent.stored_path)
        .await
        .map_err(|_| missing())?;
    let name: String = sent
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Response::builder()
        .header(header::CONTENT_TYPE, sent.media_type)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        )
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(Body::from(bytes))
        .map_err(ApiError::unknown)
}
