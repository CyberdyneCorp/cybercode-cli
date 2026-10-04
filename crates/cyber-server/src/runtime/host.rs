//! What the runtime needs from the outside: models and tools.

use std::sync::Arc;

use cyber_llm::catalog::{BuildInputs, Catalog, Cost, ModelRef, ModelRole, role_ref};
use cyber_llm::{Adapter, LlmRequest, ToolSpec};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::model::{CallState, RetrySafety};

/// A model ready to stream.
pub struct ResolvedModel {
    pub adapter: Arc<dyn Adapter>,
    /// Request template (model ID, reasoning variant, output cap, overlays).
    pub template: LlmRequest,
    pub provider: String,
    pub model: String,
    /// Context window in tokens; 0 when unknown.
    pub context_limit: u64,
    pub cost: Option<Cost>,
    /// The model edits better with `apply_patch` than with `edit`/`write`.
    pub prefers_apply_patch: bool,
}

pub trait ModelResolver: Send + Sync {
    fn resolve(&self, model_ref: &str) -> Result<ResolvedModel, String>;
    /// The configured model for a role, following the role fallbacks.
    fn role(&self, role: ModelRole) -> Option<String>;
}

/// Resolution through the catalog and the user's configuration.
pub struct CatalogResolver {
    catalog: Catalog,
    config: Value,
}

impl CatalogResolver {
    pub fn new(data: &Value, config: Value, env: &dyn cyber_core::env::EnvSource) -> Self {
        let catalog = Catalog::build(&BuildInputs {
            data,
            config: &config,
            env,
        });
        Self { catalog, config }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}

impl ModelResolver for CatalogResolver {
    fn resolve(&self, model_ref: &str) -> Result<ResolvedModel, String> {
        let r = ModelRef::parse(model_ref).map_err(|e| e.to_string())?;
        let target = self.catalog.resolve(&r, None).map_err(|e| e.to_string())?;
        let (_, model) = self.catalog.find(&r).map_err(|e| e.to_string())?;
        Ok(ResolvedModel {
            adapter: Arc::from(target.adapter()),
            template: target.request,
            provider: r.provider.clone(),
            model: r.model.clone(),
            context_limit: model.limits.context,
            cost: model.cost.clone(),
            prefers_apply_patch: model.capabilities.prefers_apply_patch,
        })
    }

    fn role(&self, role: ModelRole) -> Option<String> {
        role_ref(&self.config, role)
    }
}

/// A tool offered to the model for one Turn.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub spec: ToolSpec,
    pub retry_safety: RetrySafety,
    pub concurrency_safe: bool,
}

/// What tool materialization depends on for one Turn (`tool-registry` → Per-turn materialization).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnContext {
    pub session_id: String,
    pub directory: String,
    pub agent: String,
    pub mode: String,
    pub prefers_apply_patch: bool,
    /// The Session ruleset.
    pub rules: Value,
}

/// The invocation context (`tool-registry` → Invocation context).
#[derive(Debug, Clone)]
pub struct Invocation {
    pub session_id: String,
    pub directory: String,
    pub agent: String,
    pub mode: String,
    pub message_id: String,
    pub call_id: String,
    pub name: String,
    pub input: Value,
    pub attempt: u32,
    /// Stable across retries of the same call, for idempotent external APIs.
    pub operation_key: String,
    /// Ask the user for permission or answers.
    pub asker: super::requests::Asker,
    /// The Session ruleset.
    pub rules: Value,
}

/// How a tool run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    Ok(String),
    /// An expected failure; the message is shown to the model.
    Failed(String),
    /// Stopped by the abort signal before finishing.
    Aborted,
    /// A defect; the detail is logged, never shown to the model.
    Crashed(String),
}

/// Read-only reconciliation of an outcome that became unknown.
#[derive(Debug, Clone, PartialEq)]
pub enum Reconciliation {
    Succeeded { evidence: String },
    NotApplied { evidence: String },
    Unknown,
}

pub trait ToolHost: Send + Sync {
    fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef>;

    /// Run a call. Implementations stop within 2 s once `cancel` fires.
    fn execute(&self, call: Invocation, cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome>;

    /// Run a user shell command (`!`) in a Location under the sandbox, without prompts.
    fn shell(
        &self,
        _directory: &str,
        _session_id: &str,
        _command: &str,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Err("shell commands are not supported by this host".to_string()) })
    }

    /// Extra Context Sources this host contributes for a Session, such as `core/skills`.
    fn context_sources(&self, _turn: &TurnContext) -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::new()
    }

    /// Establish, without side effects, whether a dispatched call took effect.
    /// `directory` is the Session's Location, for resolving relative paths.
    fn reconcile(&self, _directory: &str, _call: &CallState) -> BoxFuture<'_, Reconciliation> {
        Box::pin(async { Reconciliation::Unknown })
    }
}

/// A host with no tools.
pub struct NoTools;

impl ToolHost for NoTools {
    fn definitions(&self, _turn: &TurnContext) -> Vec<ToolDef> {
        Vec::new()
    }

    fn execute(&self, _call: Invocation, _cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async { ToolOutcome::Failed("no tools are available".into()) })
    }
}

/// A recorded working tree (`snapshots-checkpoints`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Snapshot {
    pub tree: String,
    /// Untracked files left out for size, relative to the worktree.
    #[serde(default)]
    pub skipped: Vec<String>,
}

/// One file of a diff between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FileDiff {
    pub file: String,
    /// `added`, `deleted` or `modified`.
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    pub patch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RestoreError {
    #[error("RewindConflictError: changed since the snapshot and cannot be merged: {}", .0.join(", "))]
    Conflict(Vec<String>),
    #[error("{0}")]
    Failed(String),
}

/// Working-tree snapshots for a Location.
pub trait Snapshots: Send + Sync {
    /// Record the current tree; `None` when snapshots are off or unsupported here.
    fn track(&self, directory: &str) -> BoxFuture<'_, Result<Option<Snapshot>, String>>;

    /// Paths that differ between two snapshots.
    fn changed(
        &self,
        directory: &str,
        from: &str,
        to: &str,
    ) -> BoxFuture<'_, Result<Vec<String>, String>>;

    fn diff(
        &self,
        directory: &str,
        from: &str,
        to: &str,
    ) -> BoxFuture<'_, Result<Vec<FileDiff>, String>>;

    /// Make the working tree match `target` for every path that differs between `recorded`
    /// (the last state the system wrote) and `target`, preserving later edits by three-way
    /// merge. Returns the paths changed.
    fn restore(
        &self,
        directory: &str,
        target: &str,
        recorded: &str,
    ) -> BoxFuture<'_, Result<Vec<String>, RestoreError>>;
}

/// Snapshots disabled.
pub struct NoSnapshots;

impl Snapshots for NoSnapshots {
    fn track(&self, _directory: &str) -> BoxFuture<'_, Result<Option<Snapshot>, String>> {
        Box::pin(async { Ok(None) })
    }

    fn changed(&self, _: &str, _: &str, _: &str) -> BoxFuture<'_, Result<Vec<String>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn diff(&self, _: &str, _: &str, _: &str) -> BoxFuture<'_, Result<Vec<FileDiff>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn restore(
        &self,
        _: &str,
        _: &str,
        _: &str,
    ) -> BoxFuture<'_, Result<Vec<String>, RestoreError>> {
        Box::pin(async { Err(RestoreError::Failed("snapshots are disabled".into())) })
    }
}
