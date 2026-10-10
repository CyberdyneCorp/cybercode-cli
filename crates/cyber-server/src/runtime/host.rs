//! What the runtime needs from the outside: models and tools.

use std::sync::Arc;

use cyber_llm::catalog::{BuildInputs, Catalog, Cost, ModelRef, ModelRole, role_ref};
use cyber_llm::{Adapter, LlmRequest, ToolSpec};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::model::{CallState, RetrySafety, SessionInfo};

/// An owned Location admission. Disposal without settlement must retain recovery evidence.
pub trait LocationGuard: Send {
    fn settle(self: Box<Self>) -> Result<(), String>;

    /// Acknowledge native settlement and retain ownership through a consuming commit.
    fn settle_retained(self: Box<Self>) -> Result<Box<dyn Send>, String> {
        Err("Location host does not support retained native settlement".into())
    }
}

pub struct LocationLease {
    pub worktree_id: Option<String>,
    guard: Option<Box<dyn LocationGuard>>,
}

impl LocationLease {
    pub fn guarded(worktree_id: Option<String>, guard: Box<dyn LocationGuard>) -> Self {
        Self {
            worktree_id,
            guard: Some(guard),
        }
    }
    pub fn unmanaged() -> Self {
        Self {
            worktree_id: None,
            guard: None,
        }
    }

    pub fn managed(id: String, guard: Box<dyn LocationGuard>) -> Self {
        Self {
            worktree_id: Some(id),
            guard: Some(guard),
        }
    }

    pub fn settle(self) -> Result<(), String> {
        self.guard.map_or(Ok(()), |guard| guard.settle())
    }

    /// Keep acknowledged host ownership alive until the returned proof is dropped.
    pub fn settle_retained(self) -> Result<Box<dyn Send>, String> {
        self.guard.map_or_else(
            || Ok(Box::new(()) as Box<dyn Send>),
            |guard| guard.settle_retained(),
        )
    }
}

/// Host-owned managed child reporting/cleanup, retained until the Drain settles it.
pub trait ChildContinuation: Send {
    fn settle(
        self: Box<Self>,
        completed: bool,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<Option<Value>, String>>;
}

/// Trusted profile defaults and its final request layer, resolved from one snapshot.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentInference {
    pub permission_mode: Option<String>,
    pub steps: Option<u64>,
    pub model: Option<String>,
    pub variant: Option<String>,
    pub request: cyber_llm::catalog::RequestOverlay,
}

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
    /// Known provider credential environment names; includes unavailable providers.
    fn credential_env_names(&self) -> Vec<String> {
        Vec::new()
    }

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
    fn credential_env_names(&self) -> Vec<String> {
        self.catalog
            .providers
            .values()
            .flat_map(|provider| provider.env.iter().cloned())
            .collect()
    }

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

/// Scope is assigned by registration authority, never inferred from a tool name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolScope {
    Builtin,
    Plugin,
    Mcp,
    Session,
}
impl ToolScope {
    pub fn deferrable(self) -> bool {
        matches!(self, Self::Plugin | Self::Mcp)
    }
}

/// A tool offered to the model for one Turn.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub scope: ToolScope,
    /// Runtime projection marks an advertised name whose schema has not been loaded.
    pub deferred: bool,
    /// Opaque native registration identity, retained from advertisement through dispatch.
    pub registration: Option<String>,
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
    pub registration: Option<String>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct SkillSuggestion {
    pub name: String,
    pub description: String,
}

/// How a tool run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    Ok(String),
    SkillLoaded {
        output: String,
        activation: cyber_core::skills::SkillActivation,
    },
    /// Suggestions are deduplicated and persisted atomically with the tool settlement.
    SkillSuggestions {
        failed: bool,
        output: String,
        value: Option<Value>,
        skills: Vec<SkillSuggestion>,
    },
    /// Human/model-visible text plus the authoritative typed value.
    Structured {
        output: String,
        value: Value,
    },
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
    /// Start shared Location services without waiting for their readiness.
    fn open_location(&self, _info: &SessionInfo) {}

    /// Refresh services within a Turn without reopening explicitly closed actors.
    fn refresh_location(&self, info: &SessionInfo) {
        self.open_location(info);
    }

    /// Readiness of required Location services before this Session's first Turn.
    /// Cancelling the wait does not assert shared native service shutdown.
    fn wait_for_required_mcp(
        &self,
        _info: &SessionInfo,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<(), super::RuntimeError>> {
        Box::pin(async { Ok(()) })
    }

    /// Join and settle shared native services after runtime work has stopped.
    fn shutdown(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    /// Finalize the complete reminder and tool text before recording delivered skill names.
    fn finalize_skill_output(
        &self,
        _directory: &str,
        mut output: String,
        reminder: &str,
    ) -> Result<String, String> {
        output.push_str(reminder);
        Ok(output)
    }

    fn session_budget(
        &self,
        _directory: &str,
    ) -> Result<Option<cyber_core::budget::Budget>, String> {
        Ok(None)
    }

    /// Selected agent request options from trusted, validated Location configuration.
    fn request_overlay(
        &self,
        _turn: &TurnContext,
    ) -> Result<cyber_llm::catalog::RequestOverlay, String> {
        Ok(cyber_llm::catalog::RequestOverlay::default())
    }

    /// Hosts implementing only the existing request hook retain its behavior.
    fn agent_inference(&self, turn: &TurnContext) -> Result<AgentInference, String> {
        Ok(AgentInference {
            request: self.request_overlay(turn)?,
            ..Default::default()
        })
    }

    /// Admit Location use before recovery, context, snapshots or tools. Only fresh
    /// Session creation may establish a new checkout binding.
    fn claim_location<'a>(
        &'a self,
        _info: &'a SessionInfo,
        _creating: bool,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<LocationLease, String>> {
        Box::pin(async { Ok(LocationLease::unmanaged()) })
    }

    /// Prepare an existing child under its exclusive owner. Opening a client view grants no bypass.
    fn prepare_child_continuation<'a>(
        &'a self,
        _parent: &'a SessionInfo,
        child: &'a super::SessionState,
        _owner: &'a super::ChildExecution,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<Box<dyn ChildContinuation>>, String>> {
        Box::pin(async move {
            if child.child_worktree().is_some() {
                Err("Managed child continuation is not supported by this host".into())
            } else {
                Ok(None)
            }
        })
    }

    fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef>;

    fn deferred_tool_settings(
        &self,
        _turn: &TurnContext,
    ) -> Result<cyber_core::config::DeferredToolSettings, String> {
        Ok(cyber_core::config::DeferredToolSettings::default())
    }

    /// Run a call. Implementations stop within 2 s once `cancel` fires.
    fn execute(&self, call: Invocation, cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome>;

    /// An explicit user request for a forked background child. Deny rules still apply.
    fn subtask(
        &self,
        _turn: TurnContext,
        _prompt: String,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<super::Job, String>> {
        Box::pin(async { Err("subtasks are not supported by this host".into()) })
    }

    /// Explicit named user delegation. Omission preserves the legacy forked subtask.
    fn subtask_with_agent(
        &self,
        turn: TurnContext,
        prompt: String,
        agent: Option<String>,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<super::Job, String>> {
        if agent.is_some() {
            Box::pin(async { Err("named user delegation is not supported by this host".into()) })
        } else {
            self.subtask(turn, prompt, cancel)
        }
    }

    /// Opt in only when user dispatch records the durable launch marker before effects.
    fn durable_user_delegation(&self) -> bool {
        false
    }

    fn subtask_request(
        &self,
        turn: TurnContext,
        request: super::UserSubtask,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<super::Job, String>> {
        if !request.attachments.is_empty() || request.max_steps.is_some() {
            Box::pin(async { Err("typed user delegation is not supported by this host".into()) })
        } else {
            self.subtask_with_agent(turn, request.prompt, request.agent, cancel)
        }
    }

    /// Run a user shell command (`!`) in a Location under the sandbox, without prompts.
    fn shell(
        &self,
        _directory: &str,
        _session_id: &str,
        _command: &str,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Err("shell commands are not supported by this host".to_string()) })
    }

    /// Cancellation-owned shell execution. Legacy hosts remain bounded by runtime disposal.
    fn shell_owned(
        &self,
        directory: &str,
        session_id: &str,
        command: &str,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<String, String>> {
        self.shell(directory, session_id, command)
    }

    /// Extra Context Sources this host contributes for a Session, such as `core/skills`.
    fn context_sources(&self, _turn: &TurnContext) -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::new()
    }

    /// Typed observations distinguish removal from temporary source unavailability.
    fn context_observations(
        &self,
        turn: &TurnContext,
    ) -> std::collections::BTreeMap<String, super::context::Observed> {
        self.context_sources(turn)
            .into_iter()
            .map(|(key, value)| (key, super::context::Observed::Value(value)))
            .collect()
    }

    /// Owned asynchronous observation for sources requiring authority or blocking IO.
    fn context_observations_owned<'a>(
        &'a self,
        turn: &'a TurnContext,
    ) -> BoxFuture<'a, std::collections::BTreeMap<String, super::context::Observed>> {
        Box::pin(async move { self.context_observations(turn) })
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
