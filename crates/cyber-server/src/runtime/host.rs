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

    /// The latest usage window readings the providers of resolved models reported
    /// (`observability-costs` → Provider-reported limits); empty when none reported.
    fn limits(&self) -> Vec<cyber_llm::limits::LimitReading> {
        Vec::new()
    }
}

/// Resolution through the catalog and the user's configuration. The catalog is rebuilt by
/// [`CatalogResolver::reload`] when a provider login changes; resolved models keep theirs.
pub struct CatalogResolver {
    catalog: std::sync::RwLock<Arc<Catalog>>,
    /// The catalog source data the catalog was built from, kept for reloads.
    data: Value,
    config: Value,
    limits: cyber_llm::limits::LimitStore,
}

impl CatalogResolver {
    pub fn new(data: &Value, config: Value, env: &dyn cyber_core::env::EnvSource) -> Self {
        let catalog = Self::build(data, &config, env);
        Self {
            catalog: std::sync::RwLock::new(catalog),
            data: data.clone(),
            config,
            limits: Default::default(),
        }
    }

    fn build(data: &Value, config: &Value, env: &dyn cyber_core::env::EnvSource) -> Arc<Catalog> {
        Arc::new(Catalog::build(&BuildInputs { data, config, env }))
    }

    pub fn catalog(&self) -> Arc<Catalog> {
        Arc::clone(&self.catalog.read().unwrap_or_else(|e| e.into_inner()))
    }

    /// Rebuild the catalog from the same source data and configuration, picking up a
    /// stored provider login that appeared or went away (`provider-catalog` → ChatGPT
    /// subscription provider: Reload on login).
    pub fn reload(&self, env: &dyn cyber_core::env::EnvSource) {
        let catalog = Self::build(&self.data, &self.config, env);
        *self.catalog.write().unwrap_or_else(|e| e.into_inner()) = catalog;
    }
}

impl ModelResolver for CatalogResolver {
    fn credential_env_names(&self) -> Vec<String> {
        self.catalog()
            .providers
            .values()
            .flat_map(|provider| provider.env.iter().cloned())
            .collect()
    }

    fn resolve(&self, model_ref: &str) -> Result<ResolvedModel, String> {
        let r = ModelRef::parse(model_ref).map_err(|e| e.to_string())?;
        let catalog = self.catalog();
        let mut target = catalog.resolve(&r, None).map_err(|e| e.to_string())?;
        target.endpoint.limits = Some(self.limits.sink(&r.provider));
        let (_, model) = catalog.find(&r).map_err(|e| e.to_string())?;
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

    fn limits(&self) -> Vec<cyber_llm::limits::LimitReading> {
        self.limits.readings()
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

/// How a loop's `--until` check ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    /// `None` when the process was killed by a signal or timed out.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// The last lines of its merged output.
    pub tail: String,
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
    pub worktree: TurnWorktree,
}

/// Which worktree tools a Turn may offer (`worktrees` → Enter and exit tools).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TurnWorktree {
    /// An unmanaged Location: `enter_worktree` only.
    #[default]
    Unmanaged,
    /// Entered with `enter_worktree`: `exit_worktree` only.
    Entered,
    /// Started in a managed checkout, an isolated child included: neither.
    Isolated,
}

impl TurnWorktree {
    pub fn of(info: &super::SessionInfo) -> Self {
        match (&info.worktree_origin, &info.worktree_id) {
            (Some(_), _) => Self::Entered,
            (None, Some(_)) => Self::Isolated,
            (None, None) => Self::Unmanaged,
        }
    }
}

/// A path's permission decision for browsing (`server-api` → File browsing routes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileDecision {
    Allow,
    Ask,
    Deny,
}

/// The effect of the permission rules on one action and resource (`permissions-modes` →
/// Permission rules), before any Mode applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleEffect {
    Allow,
    Ask,
    Deny,
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

/// Session-derived hook envelope identity (`hooks` → Supported events): the
/// project and workspace of the Session's Location, captured once per Location,
/// and the delegating parent of a child Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEnvelope {
    /// `prj_<hex>`, or `global` outside git.
    pub project_id: String,
    /// The checkout root of a git Location.
    pub workspace: Option<String>,
    pub parent_session_id: Option<String>,
    /// The Workflow Run of a workflow agent's Session (`hooks` → Hooks in subagents and
    /// workflows).
    pub workflow_run_id: Option<String>,
}

impl SessionEnvelope {
    /// The envelope of an event owned by `session_id` with its captured Location,
    /// agent and Mode.
    pub fn identity(
        &self,
        session_id: &str,
        directory: &str,
        agent: &str,
        mode: &str,
    ) -> cyber_core::hooks::HookIdentity {
        cyber_core::hooks::HookIdentity {
            session_id: session_id.into(),
            location: cyber_core::hooks::HookLocation {
                directory: directory.into(),
                workspace: self.workspace.clone(),
            },
            project_id: self.project_id.clone(),
            agent: agent.into(),
            mode: mode.into(),
            parent_session_id: self.parent_session_id.clone(),
            workflow_run_id: self.workflow_run_id.clone(),
        }
    }

    /// Whether a hook event carries exactly this Session-derived identity.
    pub fn matches(&self, identity: &cyber_core::hooks::HookIdentity) -> bool {
        identity.project_id == self.project_id
            && identity.location.workspace == self.workspace
            && identity.parent_session_id == self.parent_session_id
    }
}

/// A runtime-owned lifecycle event for host hook dispatch (`hooks` → Stop hooks).
/// The runtime captures the Session identity; the host builds the envelope and,
/// only when a definition exists, adds the project identity from
/// [`super::Runtime::session_envelope`], which hook admission requires.
#[derive(Debug, Clone)]
pub struct LifecycleHook {
    pub session_id: String,
    pub directory: String,
    pub agent: String,
    pub mode: String,
    /// A runtime lifecycle event name, such as `Stop` or `PostToolBatch`.
    pub kind: String,
    /// The matcher subject.
    pub subject: String,
    pub fields: serde_json::Map<String, Value>,
    /// The Workflow Run a `WorkflowRunStart`/`WorkflowRunEnd` event belongs to; the
    /// envelope carries it as `workflow_run_id`.
    pub workflow_run_id: Option<String>,
}

/// The merged decision of a lifecycle event and the catalog's continuation cap.
#[derive(Debug, Clone, Default)]
pub struct LifecycleVerdict {
    pub decision: cyber_core::hooks::HookDecision,
    pub max_stop_continuations: usize,
    /// A definition exists for the event. False means nothing was dispatched, so
    /// the runtime records nothing durable for it.
    pub configured: bool,
}

/// What tool-event hooks (PreToolUse, PermissionRequest, PostToolUse,
/// PostToolUseFailure) left for the runtime to apply when their call settles.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookEffects {
    /// `[hook <id>]`-tagged `additional_context`, in declared order.
    pub context: Option<String>,
    /// A `continue: false` request with its `[hook <id>]`-tagged stop reason.
    pub stop: Option<String>,
    /// What a PreToolUse `updated_input` changed the call into, so the transcript
    /// shows the input that actually ran (`hooks` → Decision schema).
    pub rewrite: Option<String>,
}

impl HookEffects {
    /// Accumulate one event's merged decision: context concatenates in order and
    /// the first stop request sticks.
    pub fn absorb(&mut self, decision: &cyber_core::hooks::HookDecision) {
        if let Some(context) = &decision.additional_context {
            match &mut self.context {
                Some(existing) => {
                    existing.push('\n');
                    existing.push_str(context);
                }
                None => self.context = Some(context.clone()),
            }
        }
        if decision.continuation == Some(false) && self.stop.is_none() {
            self.stop = Some(
                decision
                    .stop_reason
                    .clone()
                    .unwrap_or_else(|| "continue: false".into()),
            );
        }
    }

    /// Record one accepted input rewrite; a re-issued call's later rewrite follows
    /// the earlier one.
    pub fn record_rewrite(&mut self, text: String) {
        match &mut self.rewrite {
            Some(existing) => {
                existing.push('\n');
                existing.push_str(&text);
            }
            None => self.rewrite = Some(text),
        }
    }
}

/// One Workflow `agent()` call, spawned as a child Session of the run's owner Session
/// (`workflows` → agent() call). The runtime is granted `slot` by the server-wide fair
/// scheduler of Workflow agents; the host keeps it until the child has settled.
pub struct WorkflowAgentRequest {
    pub run_id: String,
    /// The `agt_` ID of this call; also the child's subagent name.
    pub agent_id: String,
    /// Stable for this call across relaunches of the run: the replay key and its ordinal.
    pub call_key: String,
    /// The owner Session; its `mode` is the run Mode, a ceiling for the child.
    pub owner: TurnContext,
    pub prompt: String,
    /// Agent profile (default `general`).
    pub agent: Option<String>,
    /// A resolved model reference; `None` inherits the owner's model.
    pub model: Option<String>,
    /// JSON Schema the child's result must match.
    pub schema: Option<Value>,
    pub label: Option<String>,
    /// Continue the existing child Session named `agent_id` (an agent the run started
    /// before it was interrupted) instead of creating one.
    pub resume: bool,
    /// Run the child in its own managed worktree (`worktrees` → Workflow agent isolation).
    pub worktree: Option<RunWorktree>,
    pub slot: super::workflow_scheduler::Slot,
}

/// The managed worktree of an isolated Workflow agent: `run-<ulid>-<index>` on branch
/// `cyber/run-<ulid>/<index>`, where `<index>` is the agent's 1-based start ordinal in
/// the run. It is kept until the run is archived or `cyber worktree prune` runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunWorktree {
    pub name: String,
    pub branch: String,
}

impl RunWorktree {
    pub fn new(run_id: &str, index: u64) -> Self {
        let id = run_id
            .strip_prefix("run_")
            .unwrap_or(run_id)
            .to_ascii_lowercase();
        Self {
            name: format!("run-{id}-{index}"),
            branch: format!("cyber/run-{id}/{index}"),
        }
    }
}

/// The managed worktree of a teammate (`agent-teams` → Team composition):
/// `team-<ulid>-<member>` on branch `cyber/team-<ulid>/<member>`, based on the lead's
/// current HEAD and kept until the team is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamWorktree {
    pub name: String,
    pub branch: String,
}

impl TeamWorktree {
    pub fn new(team_id: &str, member: &str) -> Self {
        let id = team_id
            .strip_prefix("team_")
            .unwrap_or(team_id)
            .to_ascii_lowercase();
        Self {
            name: format!("team-{id}-{member}"),
            branch: format!("cyber/team-{id}/{member}"),
        }
    }
}

/// A settled Workflow agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowAgentOutput {
    pub session_id: String,
    /// The final answer; empty for a schema-governed child.
    pub text: String,
    /// The schema-validated result, when a schema was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<Value>,
    /// Known cost of the child and its descendants, in USD.
    pub cost: f64,
    pub tokens: u64,
    /// The branch of an isolated child's worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
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
    /// The Session ended (archived or deleted): stop what it owns outside the runtime,
    /// such as its browser and dev servers. Runs after SessionEnd hooks observed it.
    fn end_session(&self, _session_id: &str) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
    /// A digest of the Location's resolved configuration, compared at each Safe
    /// Boundary to fire `ConfigChange`; `None` (no configuration resolver) never fires.
    fn config_digest(&self, _directory: &str) -> Option<String> {
        None
    }

    /// `compat.claude` of the Location's resolved config (`CLAUDE.md` loads in place);
    /// `None` uses the server-wide [`super::RuntimeOptions::claude_compat`].
    fn claude_compat(&self, _directory: &str) -> Option<bool> {
        None
    }

    /// The Location's `read` permission rules as a gate on project instruction files and
    /// their imports; `None` (a host without a permission policy) loads every one.
    fn instruction_read_gate(&self, _directory: &str) -> Option<super::instructions::ReadGate> {
        None
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

    /// Whether a Session of `agent` in `directory` may run in `mode` (`permissions-modes`
    /// → bypass mode; org `policy.modes.disable`). Checked on every Mode a Session takes:
    /// creation, the deferred agent default and each switch. Hosts without a policy
    /// admit every Mode.
    fn admit_mode(&self, _directory: &str, _agent: &str, _mode: &str) -> Result<(), String> {
        Ok(())
    }

    /// `budgets.daily`: the per-machine daily cap the Usage breakdown reports.
    fn daily_budget(&self, _directory: &str) -> Result<Option<cyber_core::budget::Budget>, String> {
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
        if !request.attachments.is_empty()
            || request.max_steps.is_some()
            || request.skill_command.is_some()
        {
            Box::pin(async { Err("typed user delegation is not supported by this host".into()) })
        } else {
            self.subtask_with_agent(turn, request.prompt, request.agent, cancel)
        }
    }

    /// Whether a Workflow agent with this profile (default `general`) edits files in a
    /// Location a worktree can isolate: a git checkout, and a tool set that offers an edit
    /// tool (`workflows` → Write isolation default). Hosts without agent profiles report
    /// false.
    fn workflow_agent_writes(&self, _directory: &str, _agent: Option<&str>) -> bool {
        false
    }

    /// Spawn a Workflow agent and wait for its result (`workflows` → agent() call). Failures
    /// are typed: `AgentFailed`, `SchemaMismatch` or `PermissionDenied`.
    fn spawn_workflow_agent(
        &self,
        _request: WorkflowAgentRequest,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<WorkflowAgentOutput, cyber_workflow::Diagnostic>> {
        Box::pin(async {
            Err(cyber_workflow::Diagnostic::new(
                cyber_workflow::DiagnosticKind::AgentFailed,
                "workflow agents are not supported by this host",
            ))
        })
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

    /// Start a user shell command (`& cmd`) as a background process Job of the Session,
    /// under the sandbox and without prompts, and return the Job ID.
    fn shell_background(
        &self,
        _directory: &str,
        _session_id: &str,
        _command: &str,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Err("background shell commands are not supported by this host".into()) })
    }

    /// The team settings of a Location (`agent-teams` → Feature flag): `None` while the
    /// `teams` feature is off there. Callers treat an error as off.
    fn team_settings(
        &self,
        _directory: &str,
    ) -> Result<Option<cyber_core::teams::TeamSettings>, String> {
        Ok(None)
    }

    /// Whether `agent` at `directory` can edit: its tool set offers `edit`, `write`,
    /// `apply_patch` or `bash` (`agent-teams` → isolation default). An agent that cannot
    /// be resolved counts as an editor.
    fn team_editor(&self, _directory: &str, _agent: &str) -> bool {
        true
    }

    /// Create the teammate Session `request` in its own managed `worktree`, based on the
    /// lead's current HEAD and authorized under the lead's permissions.
    fn create_team_session<'a>(
        &'a self,
        _lead: &'a SessionInfo,
        _request: super::CreateSession,
        _worktree: &'a TeamWorktree,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<SessionInfo, String>> {
        Box::pin(async { Err("team worktrees are not supported by this host".to_string()) })
    }

    /// Loop limits and the `budgets.loop` default of a Location (`loops-scheduling`).
    fn loop_settings(
        &self,
        _directory: &str,
    ) -> Result<
        (
            cyber_core::loops::LoopSettings,
            Option<cyber_core::budget::Budget>,
        ),
        String,
    > {
        Ok((cyber_core::loops::LoopSettings::default(), None))
    }

    /// Run a loop's `--until` check like a user `!` command (sandboxed, never asking)
    /// and report its exit code.
    fn check(
        &self,
        _directory: &str,
        _session_id: &str,
        _command: &str,
        _timeout_ms: u64,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<CheckOutcome, String>> {
        Box::pin(async { Err("checks are not supported by this host".to_string()) })
    }

    /// Plan a loop iteration's `/command` prompt as the command route does; `None` when
    /// no command or skill has that name.
    fn loop_command_plan<'a>(
        &'a self,
        _turn: &'a TurnContext,
        _name: &'a str,
        _arguments: &'a str,
    ) -> BoxFuture<'a, Result<Option<super::CommandPlan>, super::CommandPlanError>> {
        Box::pin(async { Ok(None) })
    }

    /// Cross-session messaging settings of a Location (`cross-session-messaging` →
    /// Disable switch, Inbound controls); `None` is the process scope (the environment,
    /// the managed policy and the global configuration). Callers treat an error as
    /// disabled.
    fn messaging_settings(
        &self,
        _directory: Option<&str>,
    ) -> Result<cyber_core::config::MessagingSettings, String> {
        Ok(cyber_core::config::MessagingSettings::default())
    }

    /// The enabled `peers.remotes` entry `name` of the user configuration with its token
    /// (`cross-session-messaging` → Direct peer transport); `None` when no enabled remote
    /// has that name. An error (unreadable user config, no stored token) sends nothing.
    fn peer_target(&self, _name: &str) -> Result<Option<super::messaging::PeerTarget>, String> {
        Ok(None)
    }

    /// What a Session's permission rules (config, agent and Session rules, without its
    /// Mode) say about `action` on `resource` as a boundary, such as `remote.attach` on
    /// `peer:laptop` (`cross-session-messaging` → Peer attach): any deny denies, and only
    /// an explicit user-level allow of that action allows (never a blanket or wildcard
    /// allow, nor a repository, agent or Session rule). A host without rules asks.
    fn permission_effect(
        &self,
        _turn: &TurnContext,
        _action: &str,
        _resource: &str,
    ) -> Result<RuleEffect, String> {
        Ok(RuleEffect::Ask)
    }

    /// The names of the enabled `peers.remotes` entries of the user configuration, in
    /// config order (`cross-session-messaging` → Peer watches and listing).
    fn peer_names(&self) -> Result<Vec<String>, String> {
        Ok(Vec::new())
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

    /// Resolve `@<server>:<uri>` resource mentions in a user prompt before admission
    /// (`mcp` → Resources). Each block is either an attached, escaped resource or a
    /// visible notice; they ride with the admission as untrusted user-turn data. Hosts
    /// without MCP recognize no mentions.
    fn resolve_prompt_resources<'a>(
        &'a self,
        _turn: &'a TurnContext,
        _parts: &'a [cyber_llm::Content],
        _asker: super::Asker,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async { Vec::new() })
    }

    /// `action` (`read` or `list`) decisions for project paths under the Session's rules
    /// and Mode, without asking (`server-api` → File browsing routes). A Turn context
    /// without a Session ID uses the Location's rules. Hosts without permission rules
    /// deny every path.
    fn file_decisions<'a>(
        &'a self,
        _turn: &'a TurnContext,
        _action: &'a str,
        paths: &'a [std::path::PathBuf],
    ) -> BoxFuture<'a, Vec<FileDecision>> {
        Box::pin(async move { vec![FileDecision::Deny; paths.len()] })
    }

    /// Authorize `action` on `path` as the `read` tool would, through the normal
    /// permission request when the rules say `ask`. Hosts without permission rules
    /// refuse.
    fn authorize_file<'a>(
        &'a self,
        _turn: &'a TurnContext,
        _action: &'a str,
        _path: &'a std::path::Path,
        _asker: super::Asker,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Err("File access is unavailable in this host".into()) })
    }

    /// Expand a planned command template once, before admission commits
    /// (`skills-commands` → Shell output injection, File and agent references): each
    /// `` !`cmd` `` span becomes its sandboxed output or a visible notice, and `@`
    /// references become attached parts. The result replaces the admitted parts. Hosts
    /// without a sandboxed shell fail closed: nothing runs and nothing is attached.
    fn expand_prompt_template<'a>(
        &'a self,
        _turn: &'a TurnContext,
        template: &'a super::PromptTemplate,
        parts: &'a [cyber_llm::Content],
        _asker: super::Asker,
        _cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<cyber_llm::Content>, String>> {
        Box::pin(async move {
            let unavailable = vec!["[unavailable: sandbox]".to_string(); template.injections.len()];
            template.apply(parts, &unavailable)
        })
    }

    /// Dispatch a runtime lifecycle hook. Hosts without hooks observe nothing and
    /// return no decision; an error means a fail_closed handler was unavailable.
    fn lifecycle_hooks(
        &self,
        _event: LifecycleHook,
        _cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<LifecycleVerdict, String>> {
        Box::pin(async { Ok(LifecycleVerdict::default()) })
    }

    /// Take, once, the hook effects recorded for a call. The runtime commits them
    /// atomically with the call's settlement. Hosts without hooks record none.
    fn take_hook_effects(&self, _session_id: &str, _call_id: &str) -> HookEffects {
        HookEffects::default()
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
