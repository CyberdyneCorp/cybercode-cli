//! Local-owner account API. These routes never accept collaboration hub credentials
//! as a substitute for the local server's transport authentication.
use super::{ApiError, AppState, envelope::Data};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

type Result<T> = std::result::Result<T, ApiError>;
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AccountView {
    pub logged_in: bool,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub account: Option<cyber_account::Account>,
    pub storage: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct LoginOptions {
    #[serde(default)]
    pub no_browser: bool,
    #[serde(default)]
    pub reauthenticate: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "AccountLoginStatus")]
pub enum LoginStatus {
    Pending,
    Complete,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "AccountLoginAttempt")]
pub struct LoginAttempt {
    pub attempt_id: String,
    pub status: LoginStatus,
    pub authorization_url: Option<String>,
    pub user_code: Option<String>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
}
#[derive(Deserialize, JsonSchema)]
#[schemars(rename = "AccountLoginCode")]
pub struct LoginCode {
    pub redirect_url: String,
}
#[derive(Default, Deserialize, JsonSchema)]
pub struct LogoutOptions {
    #[serde(default)]
    pub all: bool,
}

pub trait AccountService: Send + Sync {
    fn current(&self) -> BoxFuture<'_, Result<AccountView>>;
    fn start(&self, options: LoginOptions) -> BoxFuture<'_, Result<LoginAttempt>>;
    fn attempt(&self, id: &str) -> Result<LoginAttempt>;
    fn submit(&self, id: &str, redirect: &str) -> Result<LoginAttempt>;
    fn logout(&self, all: bool) -> BoxFuture<'_, Result<()>>;
}
fn service(state: &AppState) -> Result<Arc<dyn AccountService>> {
    state.services.account().ok_or_else(|| {
        ApiError::service_unavailable("account", "account login is unavailable in this host")
    })
}
pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/account", get(current).delete(logout))
        .route("/account/login", post(start))
        .route("/account/login/{id}", get(attempt))
        .route("/account/login/{id}/code", post(submit))
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
async fn current(State(state): State<AppState>) -> Result<Json<Data<AccountView>>> {
    Ok(Json(Data {
        data: service(&state)?.current().await?,
    }))
}
async fn start(
    State(state): State<AppState>,
    options: Option<Json<LoginOptions>>,
) -> Result<(StatusCode, Json<Data<LoginAttempt>>)> {
    Ok((
        StatusCode::CREATED,
        Json(Data {
            data: service(&state)?
                .start(options.map(|Json(value)| value).unwrap_or_default())
                .await?,
        }),
    ))
}
async fn attempt(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Data<LoginAttempt>>> {
    Ok(Json(Data {
        data: service(&state)?.attempt(&id)?,
    }))
}
async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(code): Json<LoginCode>,
) -> Result<Json<Data<LoginAttempt>>> {
    Ok(Json(Data {
        data: service(&state)?.submit(&id, &code.redirect_url)?,
    }))
}
async fn logout(
    State(state): State<AppState>,
    Query(options): Query<LogoutOptions>,
) -> Result<StatusCode> {
    service(&state)?.logout(options.all).await?;
    Ok(StatusCode::NO_CONTENT)
}
