//! The HTTP API (`server-api`): `/api/v1` routes over the durable runtime.
//!
//! The router is transport-independent. TCP, the Unix socket and the in-process embedded
//! transport mark each request with a [`Transport`], which decides authentication.

mod catalog;
mod envelope;
mod error;
mod events;
mod guard;
mod idempotency;
pub mod openapi;
pub mod remote_tools;
pub mod rpc;
mod serve;
mod sessions;

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
pub use serve::{EmbeddedClient, serve_tcp, serve_unix};

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
    pub runtime: Runtime,
    /// Tools registered by connected clients over JSON-RPC.
    pub remote_tools: Arc<remote_tools::RemoteTools>,
    pub store: Arc<Store>,
    pub services: Arc<dyn Services>,
    pub options: Arc<HttpOptions>,
}

/// The full `/api/v1` router.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .merge(sessions::routes())
        .merge(events::routes())
        .merge(catalog::routes())
        .route("/ws", axum::routing::get(rpc::upgrade))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            idempotency::layer,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            guard::layer,
        ));
    Router::new()
        .nest("/api/v1", api)
        .layer(
            CompressionLayer::new()
                .compress_when(DefaultPredicate::new().and(SizeAbove::new(1024))),
        )
        .with_state(state)
}
