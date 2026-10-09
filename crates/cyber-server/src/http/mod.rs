//! The HTTP API (`server-api`): `/api/v1` routes over the durable runtime.
//!
//! The router is transport-independent. TCP, the Unix socket and the in-process embedded
//! transport mark each request with a [`Transport`], which decides authentication.

mod catalog;
mod children;
mod envelope;
mod error;
mod event_location;
mod events;
mod guard;
mod hooks;
mod idempotency;
mod jobs;
mod mcp;
pub mod openapi;
pub mod remote_tools;
pub mod rpc;
mod serve;
mod service;
mod sessions;
mod usage;
pub mod worktrees;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use cyber_store::Store;
use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::Serialize;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{DefaultPredicate, Predicate, SizeAbove};

use crate::runtime::{Runtime, ToolDef, TurnContext};

pub use envelope::{LocationInfo, ProjectInfo};
pub use error::{ApiError, ErrorBody};
#[cfg(unix)]
pub use serve::serve_unix;
pub use serve::{EmbeddedClient, serve_tcp};
pub use service::ServiceControl;

/// How a request reached the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    /// The Unix socket; peers were checked to be the same OS user at accept.
    Unix,
    /// In-process calls with no listener; authentication is off.
    Embedded,
}

/// A model offered to clients.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ModelInfo {
    /// `provider/model`.
    pub id: String,
    pub provider: String,
    pub name: String,
    pub available: bool,
    pub context_limit: u64,
    pub reasoning: bool,
}

/// A slash command or skill for autocomplete.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    /// `builtin`, `skill` or `command`.
    pub source: String,
    pub argument_hint: Option<String>,
}

/// An agent clients can select.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    /// `primary`, `subagent` or `all`.
    pub mode: String,
}

/// What the server needs beyond the runtime: catalogs and Location-level lookups.
pub trait Services: Send + Sync {
    fn inspect_child_setup(
        &self,
        parent: String,
        child: String,
    ) -> BoxFuture<'_, Result<crate::worktrees::ChildSetupInspection, ApiError>> {
        let _ = (parent, child);
        Box::pin(async {
            Err(ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                "Child setup recovery is unavailable in this host",
            ))
        })
    }
    fn recover_child_setup(
        &self,
        parent: String,
        child: String,
        review: crate::worktrees::SetupRecoveryRequest,
    ) -> BoxFuture<'_, Result<worktrees::StartedWorktree, ApiError>> {
        let _ = (parent, child, review);
        Box::pin(async {
            Err(ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                "Child setup recovery is unavailable in this host",
            ))
        })
    }
    fn list_worktrees(
        &self,
        directory: PathBuf,
    ) -> BoxFuture<'_, Result<Vec<worktrees::WorktreeEntry>, ApiError>> {
        let _ = directory;
        Box::pin(async {
            Err(ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                "Managed worktree listing is unavailable in this host",
            ))
        })
    }
    fn create_worktree(
        &self,
        directory: PathBuf,
        session: crate::runtime::CreateSession,
        name: cyber_core::worktrees::Name,
        call_id: String,
    ) -> BoxFuture<'_, Result<worktrees::StartedWorktree, ApiError>> {
        let _ = (directory, session, name, call_id);
        Box::pin(async {
            Err(ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                "Managed worktree creation is unavailable in this host",
            ))
        })
    }

    fn review_hooks(&self, _location: &Path) -> Result<cyber_core::hooks::HookReview, ApiError> {
        Err(ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "ServiceUnavailableError",
            "Hook configuration review is unavailable in this host",
        ))
    }
    fn trust_hook(&self, _location: &Path, _digest: &str) -> Result<(), ApiError> {
        Err(ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "ServiceUnavailableError",
            "Hook approval is unavailable in this host",
        ))
    }
    fn untrust_hook(&self, _location: &Path, _digest: &str) -> Result<bool, ApiError> {
        Err(ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "ServiceUnavailableError",
            "Hook revocation is unavailable in this host",
        ))
    }
    fn mcp_status(
        &self,
        _location: &Path,
    ) -> Result<Vec<crate::runtime::McpServerStatus>, ApiError> {
        let mut error = ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "ServiceUnavailableError",
            "MCP status is unavailable in this host",
        );
        error.body.service = Some("mcp".into());
        Err(error)
    }
    fn close_mcp(&self, _location: PathBuf) -> BoxFuture<'_, Result<(), ApiError>> {
        Box::pin(async {
            let mut error = ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                "MCP connection close is unavailable in this host",
            );
            error.body.service = Some("mcp".into());
            Err(error)
        })
    }
    fn models(&self, location: &Path) -> BoxFuture<'_, Result<Vec<ModelInfo>, String>>;
    /// The configured default model for new Sessions in a Location.
    fn default_model(&self, location: &Path) -> Option<String>;
    fn agents(&self, location: &Path) -> Vec<AgentInfo>;
    fn tools(&self, turn: &TurnContext) -> Vec<ToolDef>;
    fn commands(&self, location: &Path) -> Vec<CommandInfo>;
    /// Files and directories under the Location matching `query`, for `@` mentions.
    fn find_files(&self, location: &Path, query: &str, limit: usize) -> Vec<String>;
    /// Expand a skill or custom command with its arguments into prompt text.
    fn expand_command(&self, location: &Path, name: &str, arguments: &str) -> Option<String>;
}

pub struct HttpOptions {
    pub version: String,
    /// Basic-auth password for TCP; `None` disables authentication on TCP (tests only).
    pub password: Option<String>,
    pub cors_origins: Vec<String>,
    /// The Location for requests that name none.
    pub default_directory: PathBuf,
    /// Enabled capability groups, reported by `/health`.
    pub features: Vec<String>,
}

#[derive(Clone)]
pub struct AppState {
    /// Present only while this application owns server listeners.
    pub service: Option<ServiceControl>,
    pub runtime: Runtime,
    /// Tools registered by connected clients over JSON-RPC.
    pub remote_tools: Arc<remote_tools::RemoteTools>,
    pub store: Arc<Store>,
    pub services: Arc<dyn Services>,
    pub options: Arc<HttpOptions>,
}

/// Log each request with a request ID, also returned as `x-request-id`.
async fn request_log(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let id = cyber_core::ids::new_id("req");
    let (method, path) = (req.method().clone(), req.uri().path().to_string());
    let started = std::time::Instant::now();
    let mut response = next.run(req).await;
    let level = if path.ends_with("/health") {
        cyber_core::log::Level::Debug
    } else {
        cyber_core::log::Level::Info
    };
    let fields = serde_json::json!({
        "request_id": id, "method": method.as_str(), "path": path,
        "status": response.status().as_u16(), "ms": started.elapsed().as_millis() as u64,
    });
    cyber_core::log::log(level, "http", "request", fields);
    if let Ok(value) = axum::http::HeaderValue::from_str(&id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

/// The full `/api/v1` router.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .merge(sessions::routes())
        .merge(children::router())
        .merge(jobs::routes())
        .merge(hooks::routes())
        .merge(mcp::routes())
        .merge(usage::routes())
        .merge(worktrees::routes())
        .merge(events::routes())
        .merge(catalog::routes())
        .merge(service::routes())
        .route("/ws", axum::routing::get(rpc::upgrade))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            idempotency::layer,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            guard::layer,
        ))
        .layer(axum::middleware::from_fn(request_log));
    Router::new()
        .nest("/api/v1", api)
        .layer(
            CompressionLayer::new()
                .compress_when(DefaultPredicate::new().and(SizeAbove::new(1024))),
        )
        .with_state(state)
}
