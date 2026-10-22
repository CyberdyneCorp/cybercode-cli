//! Provider login routes (`provider-credentials` → Provider login API): clients drive the
//! ChatGPT subscription sign-in through the server, which stores the login where its
//! model calls are made and reloads its catalog when the login changes.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AppState, envelope::Data, error::ApiError};

type Result<T> = std::result::Result<T, ApiError>;

/// Where a login attempt stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginStatus {
    Pending,
    Complete,
    Failed,
}

/// One sign-in attempt. Never carries a token, the PKCE verifier or the `state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoginAttempt {
    /// A `pla_` ID.
    pub attempt_id: String,
    pub provider: String,
    pub status: LoginStatus,
    /// Open in a browser to authorize.
    pub authorization_url: String,
    /// The server is waiting for the browser's redirect on localhost:1455; without it the
    /// user pastes the redirect URL to the `code` route.
    pub callback: bool,
    /// Unix seconds after which a pending attempt fails.
    pub expires_at: u64,
    /// The signed-in account, when complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The plan's model IDs, when complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// Why the model list is missing from a complete attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// Why a failed attempt failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `POST /api/v1/providers/:provider/login/:attempt_id/code`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginCode {
    /// The full URL the browser was redirected to.
    pub redirect_url: String,
}

/// `DELETE /api/v1/providers/:provider/login`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Logout {
    /// A stored login was removed.
    pub removed: bool,
}

/// The host side of provider sign-in.
pub trait ProviderLogins: Send + Sync {
    fn start<'a>(&'a self, provider: &'a str) -> BoxFuture<'a, Result<LoginAttempt>>;
    fn attempt(&self, provider: &str, attempt_id: &str) -> Result<LoginAttempt>;
    fn submit_code<'a>(
        &'a self,
        provider: &'a str,
        attempt_id: &'a str,
        redirect_url: &'a str,
    ) -> BoxFuture<'a, Result<LoginAttempt>>;
    fn cancel(&self, provider: &str, attempt_id: &str) -> Result<()>;
    fn logout(&self, provider: &str) -> Result<bool>;
}

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/providers/{provider}/login", post(start).delete(logout))
        .route(
            "/providers/{provider}/login/{attempt_id}",
            get(attempt).delete(cancel),
        )
        .route(
            "/providers/{provider}/login/{attempt_id}/code",
            post(submit_code),
        )
}

fn logins(state: &AppState) -> Result<Arc<dyn ProviderLogins>> {
    state
        .services
        .provider_logins()
        .ok_or_else(|| ApiError::service_unavailable("provider", "provider login is not available"))
}

async fn start(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> Result<(StatusCode, Json<Data<LoginAttempt>>)> {
    let data = logins(&state)?.start(&provider).await?;
    Ok((StatusCode::CREATED, Json(Data { data })))
}

async fn attempt(
    State(state): State<AppState>,
    Path((provider, id)): Path<(String, String)>,
) -> Result<Json<Data<LoginAttempt>>> {
    let data = logins(&state)?.attempt(&provider, &id)?;
    Ok(Json(Data { data }))
}

async fn submit_code(
    State(state): State<AppState>,
    Path((provider, id)): Path<(String, String)>,
    Json(body): Json<LoginCode>,
) -> Result<Json<Data<LoginAttempt>>> {
    let data = logins(&state)?
        .submit_code(&provider, &id, &body.redirect_url)
        .await?;
    Ok(Json(Data { data }))
}

async fn cancel(
    State(state): State<AppState>,
    Path((provider, id)): Path<(String, String)>,
) -> Result<StatusCode> {
    logins(&state)?.cancel(&provider, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn logout(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> Result<Json<Data<Logout>>> {
    let removed = logins(&state)?.logout(&provider)?;
    Ok(Json(Data {
        data: Logout { removed },
    }))
}
