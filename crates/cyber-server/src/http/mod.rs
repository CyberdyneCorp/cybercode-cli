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
mod intelligence;
mod jobs;
mod jsonrpc;
mod mcp;
mod mcp_serve;
mod memory;
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
pub use idempotency::request_fingerprint;
pub use intelligence::{FormatterStatus, LspState, LspStatus, LspTransition};
pub use mcp_serve::{McpServeClock, McpServeSessions};
pub use memory::{
    MemoryEdit, MemoryRecoveryConfirm, MemoryRecoveryView, MemoryScope, RecoverMemory,
    validate_edit_fingerprint,
};
#[cfg(unix)]
pub use serve::serve_unix;
pub use serve::{EmbeddedClient, PeerAddr, serve_tcp};
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
    /// `builtin`, `skill`, `command` or `mcp` (a server prompt, named `<server>:<prompt>`).
    pub source: String,
    pub argument_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Static command origins only; broader source/permission inspection remains separate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<cyber_core::commands::CommandProvenance>,
}

/// A discovered or skipped skill (`skills-commands` → Skill and command inspection).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SkillInfo {
    /// The skill name; for a skipped skill, its directory name.
    pub name: String,
    pub description: String,
    /// The skill directory, the skipped `SKILL.md`, or `builtin:<name>` for an embedded skill.
    pub source: String,
    pub scope: cyber_core::skills::SkillOrigin,
    /// `loaded`, or `invalid` for a skill that was skipped.
    pub status: String,
    pub user_invocable: bool,
    pub model_invocable: bool,
    pub argument_hint: Option<String>,
    /// The `skill` permission for the requested agent (`allow`, `ask` or `deny`); absent when
    /// the skill was skipped.
    pub permission: Option<String>,
    /// Parse errors and duplicate notices (`replaces skill <name> from <path>`).
    pub diagnostics: Vec<String>,
}

/// A command definition that was skipped or could not be read.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommandDiagnostic {
    pub path: PathBuf,
    pub reason: String,
}

/// Skills for an agent plus command discovery diagnostics, for `/skills`, `cyber skills`,
/// `cyber commands list` and `cyber doctor`.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct SkillCatalog {
    pub agent: String,
    pub skills: Vec<SkillInfo>,
    pub command_issues: Vec<CommandDiagnostic>,
    /// Skills the agent's profile preloads (`skills: [..]`) that are unknown, denied for
    /// the agent or not model-invocable, and so are not preloaded.
    pub preload_issues: Vec<String>,
}

/// An agent clients can select.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    /// `primary`, `subagent` or `all`.
    pub mode: String,
}

/// The Location's own defaults for new Sessions, as its configuration states them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocationDefaults {
    /// The configured `mode`.
    pub mode: Option<String>,
    /// The configured `default_agent`.
    pub default_agent: Option<String>,
}

/// What the server needs beyond the runtime: catalogs and Location-level lookups.
pub trait Services: Send + Sync {
    fn transition_lsp(
        &self,
        _location: PathBuf,
        _reload: bool,
    ) -> BoxFuture<'_, Result<(), ApiError>> {
        Box::pin(async {
            let mut error = ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                "LSP lifecycle is unavailable in this host",
            );
            error.body.service = Some("lsp".into());
            Err(error)
        })
    }
    fn lsp_status(&self, _location: PathBuf) -> BoxFuture<'_, Result<Vec<LspStatus>, ApiError>> {
        Box::pin(async {
            let mut error = ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                "LSP status is unavailable in this host",
            );
            error.body.service = Some("lsp".into());
            Err(error)
        })
    }
    fn memory_recovery(
        &self,
        _runtime: Runtime,
        _directory: PathBuf,
        _scope: MemoryScope,
    ) -> BoxFuture<'_, Result<Option<MemoryRecoveryView>, ApiError>> {
        Box::pin(async { Err(memory::unavailable()) })
    }
    fn memory_recover(
        &self,
        _runtime: Runtime,
        _confirmation: MemoryRecoveryConfirm,
    ) -> BoxFuture<'_, Result<crate::runtime::MemoryChange, ApiError>> {
        Box::pin(async { Err(memory::unavailable()) })
    }

    fn memory_edit_review(
        &self,
        _directory: PathBuf,
        _scope: MemoryScope,
        _name: String,
    ) -> BoxFuture<'_, Result<cyber_core::memory::MemoryEditReview, ApiError>> {
        Box::pin(async { Err(memory::unavailable()) })
    }
    fn memory_edit(
        &self,
        _runtime: Runtime,
        _edit: MemoryEdit,
    ) -> BoxFuture<'_, Result<crate::runtime::MemoryChange, ApiError>> {
        Box::pin(async { Err(memory::unavailable()) })
    }
    fn memory_list(
        &self,
        _directory: PathBuf,
        _scope: MemoryScope,
    ) -> BoxFuture<'_, Result<cyber_core::memory::MemoryCatalog, ApiError>> {
        Box::pin(async { Err(memory::unavailable()) })
    }
    fn memory_read(
        &self,
        _directory: PathBuf,
        _scope: MemoryScope,
        _name: String,
    ) -> BoxFuture<'_, Result<cyber_core::memory::MemoryDocument, ApiError>> {
        Box::pin(async { Err(memory::unavailable()) })
    }
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
    fn formatters(
        &self,
        _location: PathBuf,
    ) -> BoxFuture<'_, Result<Vec<FormatterStatus>, ApiError>> {
        Box::pin(async {
            let mut error = ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                "Formatter status is unavailable in this host",
            );
            error.body.service = Some("formatters".into());
            Err(error)
        })
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
    /// Reconnect one server waiting for the user (`needs_auth`, `needs_client_registration`)
    /// or running on a logged-out credential, returning its status observed right after
    /// re-admission.
    fn reconnect_mcp(
        &self,
        _location: PathBuf,
        _name: String,
    ) -> BoxFuture<'_, Result<crate::runtime::McpServerStatus, ApiError>> {
        Box::pin(async {
            let mut error = ApiError::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailableError",
                "MCP reconnect is unavailable in this host",
            );
            error.body.service = Some("mcp".into());
            Err(error)
        })
    }
    /// MCP serve settings from the global configuration, read for every MCP serve
    /// request. Hosts without them do not serve MCP.
    fn mcp_serve(&self) -> Result<cyber_core::config::McpServeSettings, ApiError> {
        Err(ApiError::service_unavailable(
            "mcp_serve",
            "MCP serve is unavailable in this host",
        ))
    }
    /// The configured `mode` and `default_agent` of a Location; hosts without
    /// configuration have none.
    fn location_defaults(&self, _location: &Path) -> Result<LocationDefaults, ApiError> {
        Ok(LocationDefaults::default())
    }
    fn models(&self, location: &Path) -> BoxFuture<'_, Result<Vec<ModelInfo>, String>>;
    /// The configured default model for new Sessions in a Location.
    fn default_model(&self, location: &Path) -> Option<String>;
    fn agents(&self, location: &Path) -> Vec<AgentInfo>;
    fn tools(&self, turn: &TurnContext) -> Vec<ToolDef>;
    fn commands(&self, location: &Path) -> Vec<CommandInfo>;
    /// Skills with source, scope, permission effect for `agent` and diagnostics.
    fn skills(&self, _location: &Path, _agent: &str) -> Result<SkillCatalog, ApiError> {
        let mut error = ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "ServiceUnavailableError",
            "Skill inspection is unavailable in this host",
        );
        error.body.service = Some("skills".into());
        Err(error)
    }
    /// Files and directories under the Location matching `query`, for `@` mentions.
    fn find_files(&self, location: &Path, query: &str, limit: usize) -> Vec<String>;
    fn command_plan<'a>(
        &'a self,
        turn: &'a TurnContext,
        name: &'a str,
        arguments: &'a str,
    ) -> BoxFuture<'a, Result<Option<crate::runtime::CommandPlan>, crate::runtime::CommandPlanError>>
    {
        Box::pin(async move {
            Ok(self
                .expand_command(Path::new(&turn.directory), name, arguments)
                .map(crate::runtime::CommandPlan::plain))
        })
    }
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
    router_with(state, Arc::new(McpServeSessions::new()))
}

/// The full `/api/v1` router with its MCP serve session table; listeners sharing one
/// router share MCP sessions.
pub fn router_with(state: AppState, mcp_serve: Arc<McpServeSessions>) -> Router {
    let api = Router::new()
        .merge(sessions::routes())
        .merge(children::router())
        .merge(jobs::routes())
        .merge(hooks::routes())
        .merge(mcp::routes(mcp_serve))
        .merge(intelligence::routes())
        .merge(memory::routes())
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
