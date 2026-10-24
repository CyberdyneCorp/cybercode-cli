//! Local-owner collaboration API. Hub bearer tokens never authenticate these routes.
use super::{ApiError, AppState, envelope::Data};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::Arc;

pub trait CollaborationService: Send + Sync {
    /// The locally pinned, active Session publication; no credential is returned.
    fn agent_scope(&self, _session: &str) -> Option<String> {
        None
    }
    fn agent_call(
        &self,
        _session: String,
        _operation: String,
        _input: Value,
    ) -> BoxFuture<'_, Result<Value, ApiError>> {
        Box::pin(async { Err(ApiError::forbidden("Session is not published to a project")) })
    }
    /// Owner operations; model-facing operations use the separate scoped tool bridge.
    fn call(&self, operation: String, input: Value) -> BoxFuture<'_, Result<Value, ApiError>>;
}
fn service(state: &AppState) -> Result<Arc<dyn CollaborationService>, ApiError> {
    state.services.collaboration().ok_or_else(|| {
        ApiError::service_unavailable(
            "collaboration",
            "project collaboration is unavailable in this host",
        )
    })
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/collaboration/status", get(status))
        .route("/collaboration/participants", get(participants))
        .route("/collaboration/messages", get(messages).post(send))
        .route("/collaboration/tasks", get(tasks).post(create_task))
        .route("/collaboration/tasks/{id}/{action}", post(task_action))
        .route("/collaboration/owner/{operation}", post(owner))
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static("no-store"),
                );
                response
            },
        ))
}
macro_rules! read {
    ($name:ident, $operation:literal) => {
        async fn $name(
            State(state): State<AppState>,
            Query(input): Query<std::collections::BTreeMap<String, String>>,
        ) -> Result<Json<Data<Value>>, ApiError> {
            Ok(Json(Data {
                data: service(&state)?
                    .call($operation.into(), json!(input))
                    .await?,
            }))
        }
    };
}
read!(status, "status");
read!(participants, "participants");
read!(messages, "messages.list");
read!(tasks, "tasks.list");
macro_rules! write {
    ($name:ident, $operation:literal) => {
        async fn $name(
            State(state): State<AppState>,
            Json(input): Json<Value>,
        ) -> Result<Json<Data<Value>>, ApiError> {
            Ok(Json(Data {
                data: service(&state)?.call($operation.into(), input).await?,
            }))
        }
    };
}
write!(send, "messages.send");
write!(create_task, "tasks.create");
async fn task_action(
    State(state): State<AppState>,
    Path((id, action)): Path<(String, String)>,
    Json(mut input): Json<Value>,
) -> Result<Json<Data<Value>>, ApiError> {
    if !matches!(action.as_str(), "claim" | "update" | "reconcile") {
        return Err(ApiError::invalid("unknown task action"));
    }
    input
        .as_object_mut()
        .ok_or_else(|| ApiError::invalid("expected object"))?
        .insert("task".into(), json!(id));
    Ok(Json(Data {
        data: service(&state)?
            .call(format!("tasks.{action}"), input)
            .await?,
    }))
}
async fn owner(
    State(state): State<AppState>,
    Path(operation): Path<String>,
    Json(input): Json<Value>,
) -> Result<Json<Data<Value>>, ApiError> {
    Ok(Json(Data {
        data: service(&state)?.call(operation, input).await?,
    }))
}
