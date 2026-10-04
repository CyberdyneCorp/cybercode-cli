//! Health, OpenAPI, Location, catalogs and pending requests.

use axum::extract::{Query, State};
use axum::http::request::Parts;
use axum::routing::get;
use axum::{Json, Router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::envelope::{Data, Located, LocationInfo, location};
use super::error::ApiError;
use super::{AgentInfo, AppState, CommandInfo, ModelInfo};
use crate::runtime::{PendingKind, PendingRequest, TurnContext};

type Result<T> = std::result::Result<T, ApiError>;

#[derive(Debug, Serialize, JsonSchema)]
pub struct Health {
    pub healthy: bool,
    pub version: String,
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub features: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    /// `read_only`, `idempotent`, `reconcile` or `never`.
    pub retry_safety: crate::runtime::RetrySafety,
    pub concurrency_safe: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct ToolsQuery {
    pub agent: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FindQuery {
    pub query: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RequestsQuery {
    pub session_id: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi))
        .route("/location", get(location_info))
        .route("/models", get(models))
        .route("/agents", get(agents))
        .route("/tools", get(tools))
        .route("/tools/schema", get(tools))
        .route("/commands", get(commands))
        .route("/fs/find", get(find))
        .route("/permissions/requests", get(permission_requests))
        .route("/questions/requests", get(question_requests))
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        healthy: true,
        version: state.options.version.clone(),
        api_version: "v1".into(),
        features: state.options.features.clone(),
    })
}

async fn openapi(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(super::openapi::document(&state.options.version))
}

async fn location_info(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Data<LocationInfo>>> {
    let directory = location(&parts, &state.options.default_directory)?;
    Ok(Json(Data {
        data: LocationInfo::of(&directory),
    }))
}

async fn models(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Vec<ModelInfo>>>> {
    let directory = location(&parts, &state.options.default_directory)?;
    let data = state
        .services
        .models(&directory)
        .await
        .map_err(ApiError::unknown)?;
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn agents(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Vec<AgentInfo>>>> {
    let directory = location(&parts, &state.options.default_directory)?;
    Ok(Json(Located {
        data: state.services.agents(&directory),
        location: LocationInfo::of(&directory),
    }))
}

async fn tools(
    State(state): State<AppState>,
    parts: Parts,
    Query(q): Query<ToolsQuery>,
) -> Result<Json<Located<Vec<ToolInfo>>>> {
    let directory = location(&parts, &state.options.default_directory)?;
    let turn = TurnContext {
        session_id: String::new(),
        directory: directory.display().to_string(),
        agent: q.agent.unwrap_or_else(|| "build".into()),
        mode: q.mode.unwrap_or_else(|| "default".into()),
        prefers_apply_patch: false,
        rules: serde_json::Value::Null,
    };
    let data = state
        .services
        .tools(&turn)
        .into_iter()
        .map(|d| ToolInfo {
            name: d.spec.name,
            description: d.spec.description,
            input_schema: d.spec.input_schema,
            retry_safety: d.retry_safety,
            concurrency_safe: d.concurrency_safe,
        })
        .collect();
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

async fn commands(
    State(state): State<AppState>,
    parts: Parts,
) -> Result<Json<Located<Vec<CommandInfo>>>> {
    let directory = location(&parts, &state.options.default_directory)?;
    Ok(Json(Located {
        data: state.services.commands(&directory),
        location: LocationInfo::of(&directory),
    }))
}

async fn find(
    State(state): State<AppState>,
    parts: Parts,
    Query(q): Query<FindQuery>,
) -> Result<Json<Located<Vec<String>>>> {
    let directory = location(&parts, &state.options.default_directory)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let data = state
        .services
        .find_files(&directory, q.query.as_deref().unwrap_or(""), limit);
    Ok(Json(Located {
        location: LocationInfo::of(&directory),
        data,
    }))
}

fn pending(state: &AppState, session: Option<&str>, permission: bool) -> Vec<PendingRequest> {
    state
        .runtime
        .pending_requests(session)
        .into_iter()
        .filter(|r| matches!(r.kind, PendingKind::Permission(_)) == permission)
        .collect()
}

async fn permission_requests(
    State(state): State<AppState>,
    Query(q): Query<RequestsQuery>,
) -> Json<Data<Vec<PendingRequest>>> {
    Json(Data {
        data: pending(&state, q.session_id.as_deref(), true),
    })
}

async fn question_requests(
    State(state): State<AppState>,
    Query(q): Query<RequestsQuery>,
) -> Json<Data<Vec<PendingRequest>>> {
    Json(Data {
        data: pending(&state, q.session_id.as_deref(), false),
    })
}
