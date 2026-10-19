//! The durable session runtime (`session-runtime`).
//!
//! Input is admitted into a durable inbox before anything runs. A Drain per Session
//! promotes input at Safe Boundaries and runs Turns until nothing is eligible. Every fact
//! the model sees is committed before it is acted on, so a restart rebuilds state by replay.

mod activity;
mod admission_authority;
pub use admission_authority::AdmissionAuthority;
mod ancestry;
pub use ancestry::AncestorAuthority;
mod auto;
mod auto_override;
pub use auto_override::AutoOverrideReceipt;
pub mod auto_statistics;
pub mod browser_evidence;
mod budget;
mod bus;
mod child_ownership;
mod child_usage;
pub use browser_evidence::{
    BrowserArtifact, BrowserEvidence, BrowserEvidenceReport, BrowserEvidenceView, EvidenceStatus,
    Viewport,
};
mod client_hooks;
pub use client_hooks::{ClientHook, ClientHooks};
mod compaction;
pub use child_usage::ChildrenUsage;
mod context;
mod context_meter;
mod continuation;
mod delegations;
mod deletion;
pub mod deliveries;
pub use deliveries::{FilesSent, Notification, SEND_LIMIT_BYTES, SEND_LIMIT_FILES, SentFile};
mod drain;
mod events;
mod files;
mod fork;
mod forked_skills;
mod hook_tests;
mod hooks;
mod lifecycle;
mod lifecycle_events;
pub use hook_tests::{
    SyntheticHookExecution, start_synthetic_hook_execution, try_start_synthetic_hook_execution,
};
pub use hooks::{
    HookExecution, HookExecutionIo, HookExecutionRecord, HookExecutionResult, HookExecutionStatus,
    hook_execution_page, hook_last_run,
};
pub mod goals;
mod handoff;
mod host;
pub mod instructions;
mod jobs;
pub mod loops;
mod names;
pub mod teams;
pub use handoff::Handoff;
mod process_jobs;
pub use process_jobs::{JobIo, ProcessJob, ProcessReservation, ProcessWork};
pub mod pty;
pub use pty::{Pty, PtyAttach, PtyChunk, PtyCursor, PtyFeed, PtyInput, PtySpec};
mod pr_links;
pub use pr_links::{PR_LINKED, PrLink};
mod preparation;
mod subtask;
mod subtree_reopen;
mod subtree_stop;
pub use delegations::{Delegation, DelegationPhase, DelegationStatus};
pub use jobs::{Job, JobAdmission, JobAttempt, JobStatus, JobUsage};
pub use names::ChildExecution;
pub use subtask::UserSubtask;
pub use subtree_reopen::SubtreeReopenReport;
pub use subtree_stop::{SubtreeStopReport, SubtreeStopStatus};
mod location;
mod mcp;
mod mcp_depth;
pub mod messaging;
pub mod peers;
pub use mcp_depth::RunMarker;
mod memory;
pub use mcp::{
    McpConnectionObserver, McpConnectionOwner, McpConnectionPhase, McpConnectionRecord,
    McpConnectionStatus, McpServerStatus, McpStatusUpdate, McpToolsChanged, mcp_connections,
};
pub use memory::{
    MemoryAdmission, MemoryChange, MemoryRecoveryAdmission, MemoryRecoveryIdentity,
    MemoryRecoveryRequestStatus, MemoryRequestStatus, MemoryWrite, MemoryWriteOwner,
    memory_http_request_id,
};
pub mod deferred_tools;
mod model;
mod requests;
mod rewind;
mod sandbox_events;
pub use sandbox_events::SandboxEvent;
mod selection;
pub mod settlement;
mod shutdown;
pub mod side_chat;
pub use side_chat::SideAnswer;
mod structured;
pub use structured::StructuredSchema;
mod recap;
pub use recap::{NOTHING_TO_RECAP, Recap};
mod title;
mod usage;
mod usage_breakdown;
mod usage_insights;
mod usage_ledger;
mod usage_limits;
mod usage_origin;
pub use usage_limits::{LimitReading, UsageLimits, UsageWindow, WindowPeriod};
mod usage_stats;
pub use usage_breakdown::{BudgetLimit, BudgetScope, BudgetUsage, ModelUsage, SessionUsage};
pub use usage_insights::{
    InsightBasis, InsightFactor, InsightFactorKind, InsightWindow, InsightsRequest,
    LARGE_CONTEXT_TOKENS, LONG_RUNNING_MS, SUBAGENT_HEAVY_SHARE, SubagentGroupKind, SubagentShare,
    UsageInsights,
};
pub use usage_stats::{
    FavoriteModel, LongestSession, MAX_PERIOD_DAYS, Period, StatsDay, StatsModel, StatsOrigin,
    StatsRequest, StatsTotals, UsageStats,
};
mod view;
pub use usage::{
    RunAgentUsage, RunUsageGroup, RunUsageReport, ScopeUsage, UsageAmount, UsageReport,
};
mod worktree_events;
pub use worktree_events::{WorktreeChange, WorktreeExit, WorktreeNotice, relocates};
mod worktree_output;
pub use worktree_output::{SessionSetupSink, SetupChannel, SetupUpdate};
mod workflow_control;
mod workflow_host;
mod workflow_progress;
mod workflow_queue;
mod workflow_recovery;
mod workflow_replay;
mod workflow_results;
mod workflow_scheduler;
pub mod workflow_start;
mod workflow_summary;
mod workflows;
pub use workflow_control::{AGENTS_EXITING, ResumeWorkflowRun};
pub use workflow_host::{MAX_CHECKPOINT_BYTES, MAX_RUN_RECORD_BYTES, MAX_RUN_RECORDS};
pub use workflow_progress::PROGRESS_INTERVAL_MS;
pub use workflow_recovery::{InterruptedResume, RefusedResume, WorkflowRelaunch};
pub use workflow_results::{INLINE_RESULT_BYTES, attempt_handback_message_id, handback_message_id};
pub use workflow_scheduler::{Closed, RunSlots, Slot as WorkflowSlot, Waiter, WorkflowScheduler};
pub use workflow_summary::{
    AgentCounts, DEFAULT_RUN_PAGE, MAX_RUN_PAGE, RunFilter, RunPage, WorkflowRunProgress,
    WorkflowRunSummary,
};
pub use workflows::{
    RunShare, RunStarter, StartWorkflowRun, WORKFLOW_AGENT_STARTS_RUN, WorkflowAgent,
    WorkflowLogEntry, WorkflowRun, WorkflowRunStatus, config_digest,
};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};

use cyber_llm::{Content, RetryPolicy};
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreError, StoredEvent};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Notify, RwLock, broadcast};
use tokio_util::sync::CancellationToken;

pub use auto::{AutoDecision, AutoEffect, AutoReview, AutoReviewStage};
pub use bus::LiveEvent;
pub use compaction::CompactionConfig;
pub use context::{ContextInputs, Observed as ContextObservation, base_prompt};
pub use context_meter::ContextMeter;
pub use events::{AuxiliaryUsage, CompactionTrigger, registry as event_registry};
pub use host::{
    AgentInference, CatalogResolver, CheckOutcome, ChildContinuation, FileDecision, FileDiff,
    HookEffects, Invocation, LifecycleHook, LifecycleVerdict, LocationGuard, LocationLease,
    ModelResolver, NoSnapshots, NoTools, Reconciliation, ResolvedModel, RestoreError, RuleEffect,
    RunWorktree, SessionEnvelope, SkillSuggestion, Snapshot, Snapshots, TeamWorktree, ToolDef,
    ToolHost, ToolOutcome, ToolScope, TurnContext, TurnWorktree, WorkflowAgentOutput,
    WorkflowAgentRequest,
};
pub use model::{
    AssistantEntry, CallState, CallStatus, Delivery, Entry, InboxRow, InputStatus, LastStep,
    RetrySafety, RevertState, RevertTarget, SessionInfo, SessionState, Sourced, StepSnapshot,
    TaskState, Totals,
};
pub use requests::{
    Asker, NO_WORKFLOW_APPROVER, PendingKind, PendingRequest, PermissionAsk, PermissionReply,
    Question, QuestionOption, QuestionReply, RequestOrigin, RequestRoute,
};
pub use view::{INTERRUPTED, UNKNOWN};

use bus::Bus;
use events::*;
use lifecycle::PromptEffects;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("SessionNotFoundError: {0}")]
    SessionNotFound(String),
    #[error(
        "PromptConflictError: message {0} was already admitted with different content or delivery"
    )]
    PromptConflict(String),
    #[error("SessionBusyError: session {0} is running")]
    Busy(String),
    #[error("ServerShuttingDownError: the server is shutting down")]
    ShuttingDown,
    #[error("InvalidRequestError: {0}")]
    Invalid(String),
    #[error("BudgetExceededError: scope {scope}: {limit}")]
    BudgetExceeded { scope: String, limit: String },
    #[error("ConflictError: {0}")]
    Conflict(String),
    #[error("McpRequiredError: {0}")]
    McpRequired(String),
    #[error("ContextInitializationBlocked: {}", .0.join(", "))]
    ContextBlocked(Vec<String>),
    #[error("{0}")]
    Model(String),
    #[error("{0}")]
    Compaction(String),
    #[error(transparent)]
    Store(StoreError),
    #[error("corrupt session history: {0}")]
    Corrupt(String),
    #[error("RewindConflictError: changed since the snapshot and cannot be merged: {}", .0.join(", "))]
    RewindConflict(Vec<String>),
    #[error("HookError: {0}")]
    Hook(String),
    /// A message address that resolves to no target (`cross-session-messaging`).
    #[error(transparent)]
    Address(messaging::AddressError),
    /// A failure of the direct peer transport (`PeerUnreachableError`, `PeerAuthError`,
    /// `PeerNotConfiguredError`, `PeerScopeError`).
    #[error(transparent)]
    Peer(messaging::PeerError),
    /// A permission the user refused or did not answer in time, such as `remote.attach`
    /// (`PermissionDeniedError`); nothing was done.
    #[error("PermissionDeniedError: {0}")]
    PermissionDenied(String),
    /// A message text over the content limit; nothing was sent.
    #[error("MessageTooLargeError: the message is {size} bytes; the limit is {limit}")]
    MessageTooLarge { size: usize, limit: usize },
    /// A capability the configuration, the managed policy or the environment disables
    /// (`messaging`); nothing was written.
    #[error("ServiceUnavailableError: {0} is disabled")]
    Disabled(&'static str),
    /// An edit of a received cross-session message: it can be released or dropped, not
    /// rewritten (`ConflictError`, reason `message_immutable`).
    #[error("ConflictError: {0} is another Session's message; release or drop it instead")]
    MessageImmutable(String),
    /// A per-caller quota is full (`watch_quota`); nothing was written.
    #[error("RateLimitedError: {message}")]
    RateLimited {
        reason: &'static str,
        message: String,
    },
    /// No live Goal of the Session has this ID, or none is in the needed state (`goals`).
    #[error("GoalNotFoundError: no {0} goal in this Session")]
    GoalNotFound(String),
    /// The pending queue already holds `goals.max_queue` Goals.
    #[error("GoalQueueFullError: the goal queue is full ({0})")]
    GoalQueueFull(u64),
    /// An empty or over-long condition.
    #[error("GoalConditionTooLongError: {0}")]
    GoalCondition(String),
    /// No active loop of the Session has this ID (`loops-scheduling`).
    #[error("LoopNotFoundError: no active loop {0} in this Session")]
    LoopNotFound(String),
    /// The Session already has `loops.max_per_session` active loops.
    #[error("LoopLimitError: Scheduled task limit reached ({0})")]
    LoopLimit(u64),
    /// A feature the Location's configuration leaves off (`agent-teams` → Feature flag);
    /// nothing was written.
    #[error("FeatureDisabledError: {0}")]
    FeatureDisabled(&'static str),
    /// No team has this ID (`agent-teams`).
    #[error("TeamNotFoundError: No team {0}")]
    TeamNotFound(String),
    /// A team already has `teams.max_teammates` teammates; nothing was created.
    #[error("TeamLimitError: Team size limit reached ({0})")]
    TeamLimit(u32),
    /// A claim of a team task another member is working on; nothing changed.
    #[error("TaskClaimedError: Task already claimed by {0}")]
    TaskClaimed(String),
    /// A team task change the dependency rule refuses (an unfinished dependency, a
    /// cycle, a task of another team); nothing changed.
    #[error("TaskDependencyError: {0}")]
    TaskDependency(String),
    /// The target refuses this caller (a watch of a Session that refuses its messages,
    /// with the refusal reason); nothing was written.
    #[error("ForbiddenError: {message}")]
    Forbidden {
        reason: &'static str,
        message: String,
    },
}

pub struct RuntimeOptions {
    pub store: Arc<Store>,
    pub resolver: Arc<dyn ModelResolver>,
    pub tools: Arc<dyn ToolHost>,
    pub global_config_dir: PathBuf,
    pub shell: String,
    /// Load `CLAUDE.md` where a directory has no `AGENTS.md` (`compat.claude`).
    pub claude_compat: bool,
    pub compaction: CompactionConfig,
    pub retry: RetryPolicy,
    /// Step limit per Drain for primary agents (`None` is unlimited).
    pub max_steps: Option<u32>,
    /// Fixed date for deterministic tests; `None` uses the host's local date.
    pub today: Option<String>,
    /// Whether a client can answer permission requests and questions. When false, requests
    /// resolve immediately as `Unattended` and never block.
    pub interactive: bool,
    /// Working-tree snapshots; [`NoSnapshots`] disables code rewind.
    pub snapshots: Arc<dyn Snapshots>,
}

impl From<StoreError> for RuntimeError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Projector { reason, .. }
                if reason == admission_authority::STALE
                    || reason == admission_authority::CLOSED =>
            {
                Self::Conflict(reason)
            }
            error => Self::Store(error),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CreateSession {
    /// Internal source authority captured before asynchronous child preparation.
    pub admission_authority: Option<AdmissionAuthority>,
    pub budget: Option<cyber_core::budget::Budget>,
    pub child_worktree_setup_pending: bool,
    /// Internal binding for a child-created isolated checkout.
    pub child_worktree: Option<cyber_core::worktrees::Managed>,
    pub output_schema: Option<StructuredSchema>,
    /// Copy this source into a child whose declared parent is the source.
    pub fork_from: Option<String>,
    pub id: Option<String>,
    pub directory: String,
    /// Expected checkout identity for an internally provisioned managed Session.
    pub worktree_id: Option<String>,
    pub model: String,
    /// The supplied model is a fallback; the selected agent may replace it.
    pub model_is_default: bool,
    pub agent: Option<String>,
    pub mode: Option<String>,
    pub parent_id: Option<String>,
    pub subagent_name: Option<String>,
    /// The Workflow Run whose `agent()` call created this child.
    pub workflow_run_id: Option<String>,
    pub title: Option<String>,
    /// Session ruleset in the `permissions` config shape.
    pub rules: Option<serde_json::Value>,
    pub max_steps: Option<u32>,
    /// Session-level `messaging.inbound` override, committed with the Session
    /// (`cyber exec` refuses messages with `non_interactive`).
    pub messaging: Option<messaging::InboundOverride>,
    /// The team tag of a team member Session (`agent-teams`); only for root Sessions.
    pub team: Option<teams::TeamTag>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkillCommand {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fork: bool,
    pub activation: cyber_core::skills::SkillActivation,
    pub model: Option<String>,
    /// Structured-result schema for the forked child (the bundled review's findings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
    /// The originating command when planning depends on the environment, so a
    /// retry is matched by request instead of being planned again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<CommandRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommandRequest {
    pub name: String,
    pub arguments: String,
    /// The delivery originally requested, set at admission; a later release
    /// does not change it.
    pub delivery: Delivery,
}

/// A planned command or skill template (`skills-commands` → Shell output injection, File
/// and agent references). Only the trusted command service sets it; a public prompt body
/// cannot, so plain prompts never run shell commands or attach template references.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptTemplate {
    /// `command` or `skill`.
    pub kind: String,
    pub name: String,
    /// A project-controlled template runs shell commands only once `digest` is approved.
    pub project: bool,
    pub digest: String,
    /// The template's `` !`cmd` `` spans in the first text part, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injections: Vec<TemplateInjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TemplateInjection {
    pub start: usize,
    pub end: usize,
    pub command: String,
}

impl PromptTemplate {
    /// Replace each recorded span of the first text part with its output, given in span
    /// order. A span that no longer covers its command fails closed.
    pub fn apply(&self, parts: &[Content], outputs: &[String]) -> Result<Vec<Content>, String> {
        let mismatch = || "command template no longer matches its plan".to_string();
        if outputs.len() != self.injections.len() {
            return Err(mismatch());
        }
        let mut parts = parts.to_vec();
        let Some(Content::Text { text }) = parts.first_mut() else {
            return if self.injections.is_empty() {
                Ok(parts)
            } else {
                Err(mismatch())
            };
        };
        let mut expanded = String::with_capacity(text.len());
        let mut cursor = 0;
        for (injection, output) in self.injections.iter().zip(outputs) {
            let span = injection
                .start
                .checked_sub(cursor)
                .and_then(|_| text.get(injection.start..injection.end))
                .filter(|span| *span == format!("!`{}`", injection.command))
                .ok_or_else(mismatch)?;
            expanded.push_str(&text[cursor..injection.start]);
            expanded.push_str(output);
            cursor = injection.start + span.len();
        }
        expanded.push_str(&text[cursor..]);
        *text = expanded;
        Ok(parts)
    }
}

#[derive(Debug, Clone)]
pub struct CommandPlan {
    pub text: String,
    pub skill: Option<SkillCommand>,
    /// Set for command and skill templates, which expand at admission.
    pub template: Option<PromptTemplate>,
    /// The admission source: `user`, or [`MCP_PROMPT_SOURCE`] for server-supplied
    /// prompt text that must never become a user instruction.
    pub source: &'static str,
}

/// The source of an MCP prompt command's turn: the text is the server's untrusted
/// `<mcp_prompt>` frame, so it is not recorded as a task objective or instruction.
pub const MCP_PROMPT_SOURCE: &str = "mcp_prompt";

/// Why a command could not be planned; nothing is admitted in either case.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandPlanError {
    #[error("{0}")]
    Forbidden(String),
    #[error(transparent)]
    Review(#[from] cyber_core::review::TargetError),
    /// The host could not run its planning helper.
    #[error("{0}")]
    Unavailable(String),
    /// The command's arguments do not fit it, such as a missing required MCP prompt
    /// argument.
    #[error("{0}")]
    Invalid(String),
    /// The MCP server could not produce the prompt; nothing is admitted.
    #[error("{0}")]
    McpUnavailable(String),
}

impl From<String> for CommandPlanError {
    fn from(message: String) -> Self {
        Self::Forbidden(message)
    }
}

impl CommandPlan {
    pub fn plain(text: String) -> Self {
        Self {
            text,
            skill: None,
            template: None,
            source: "user",
        }
    }
    /// A server prompt's frame: admitted as the user turn, never as an instruction.
    pub fn mcp_prompt(text: String) -> Self {
        Self {
            source: MCP_PROMPT_SOURCE,
            ..Self::plain(text)
        }
    }
    pub fn admission(mut self, message_id: Option<String>, delivery: Delivery) -> Admission {
        if let Some(request) = self.skill.as_mut().and_then(|skill| skill.request.as_mut()) {
            request.delivery = delivery;
        }
        Admission {
            skill_command: self.skill,
            template: self.template,
            message_id,
            parts: vec![Content::Text { text: self.text }],
            delivery,
            source: self.source.into(),
            resume: true,
        }
    }
}

/// Whether an admission still owes UserPromptSubmit, or carries its checked context.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptCheck {
    Run,
    Checked(Option<String>),
}

#[derive(Debug, Clone)]
pub struct Admission {
    /// Captured by the trusted command service; public prompt bodies cannot supply this.
    pub skill_command: Option<SkillCommand>,
    /// Captured by the trusted command service; expanded once, before commit.
    pub template: Option<PromptTemplate>,
    pub message_id: Option<String>,
    pub parts: Vec<Content>,
    pub delivery: Delivery,
    /// `user`, `session`, `channel`, `loop`, `goal`, `workflow`, `hook` or `mcp_prompt`.
    pub source: String,
    /// `false` admits without waking the Session.
    pub resume: bool,
}

impl Admission {
    /// Exact-retry identity: computed from the parts as submitted, before expansion.
    fn digest(&self) -> String {
        digest(
            &self.parts,
            self.delivery,
            self.skill_command.as_ref(),
            self.template.as_ref(),
        )
    }

    pub fn text(text: impl Into<String>, delivery: Delivery) -> Self {
        Self {
            skill_command: None,
            template: None,
            message_id: None,
            parts: vec![Content::Text { text: text.into() }],
            delivery,
            source: "user".into(),
            resume: true,
        }
    }
}

/// The receipt returned for an admission and for an exact retry of it.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Receipt {
    pub session_id: String,
    pub message_id: String,
    pub delivery: Delivery,
    pub admitted_seq: i64,
    pub status: InputStatus,
}

/// User resolution of an unknown tool outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// The effect happened; `evidence` becomes the result the model sees.
    Succeeded(String),
    /// The effect did not happen.
    NotApplied,
    /// Allow the model to try again without establishing the outcome.
    AuthorizeRetry,
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub directory: Option<String>,
    pub parent: Option<String>,
    pub roots_only: bool,
    pub include_archived: bool,
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    /// Only Sessions linked to this pull request number.
    pub pr: Option<u64>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub directory: String,
    pub parent_id: Option<String>,
    pub agent: String,
    pub model: String,
    /// The selected permission mode.
    pub mode: String,
    pub archived: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub cost: f64,
    #[serde(flatten)]
    pub children_usage: ChildrenUsage,
}

impl SessionRow {
    /// The `list` cursor that continues after this row.
    pub fn cursor(&self) -> String {
        format!("{}:{}", self.updated_at, self.id)
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SessionPage {
    pub sessions: Vec<SessionRow>,
    pub next: Option<String>,
}

pub(crate) struct Handle {
    pub state: Mutex<SessionState>,
    pub pending_compaction: StdMutex<Option<Option<String>>>,
    pub title_requested: std::sync::atomic::AtomicBool,
    /// Set by a reject without feedback or a dismissed question: stop after the current tool group.
    pub halt: std::sync::atomic::AtomicBool,
    pub location_uncertain: std::sync::atomic::AtomicBool,
    /// Fingerprints of calls since the last promoted input, for doom-loop detection.
    pub recent_calls: StdMutex<Vec<String>>,
    /// Serialized auto reviews and consecutive blocks, reset for each new Drain.
    pub auto_blocks: Mutex<u32>,
    /// Consecutive Stop-hook continuations, reset for each new Drain and user admission.
    pub stop_continuations: std::sync::atomic::AtomicUsize,
    /// Hook envelope identity captured for the Session's current directory.
    pub envelope: StdMutex<Option<(String, SessionEnvelope)>>,
    /// Tagged context from background hooks, committed at the next Safe Boundary.
    pub pending_hook_context: StdMutex<Vec<String>>,
    /// Drain-owned lifecycle hooks in flight; a cancelled boundary waits for them.
    pub drain_hooks: std::sync::atomic::AtomicUsize,
    pub drain_hooks_settled: Notify,
    /// Serializes recap generation, so concurrent requests make one model call.
    pub recap: Mutex<()>,
    /// Digest of the Location configuration this Session last saw at a Safe
    /// Boundary; a different one fires `ConfigChange`.
    pub config_digest: StdMutex<Option<String>>,
    /// The Location lease of the running Drain or idle operation; `enter_worktree`
    /// and `exit_worktree` replace it with one on the new Location.
    pub location_lease: StdMutex<Option<LocationLease>>,
}

impl Handle {
    pub(crate) fn holds_lease(&self) -> bool {
        self.location_lease
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    pub(crate) fn replace_lease(&self, lease: LocationLease) -> Option<LocationLease> {
        self.location_lease
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(lease)
    }

    pub(crate) fn take_lease(&self) -> Option<LocationLease> {
        self.location_lease
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

struct DrainEntry {
    _child_owner: Option<ChildExecution>,
    child_settlement: Option<Box<dyn ChildContinuation>>,
    child_settling: bool,
    cancel: CancellationToken,
    follow_up: bool,
    /// A deletion fence, not a Drain: no Drain or child Job starts for the Session.
    deleting: bool,
}

pub(crate) struct Inner {
    pub store: Arc<Store>,
    pub resolver: Arc<dyn ModelResolver>,
    pub tools: Arc<dyn ToolHost>,
    pub bus: Bus,
    pub options: RuntimeOptions,
    sessions: StdMutex<HashMap<String, Arc<Handle>>>,
    drains: StdMutex<HashMap<String, DrainEntry>>,
    /// MCP serve recursion depth by Session, for the run (Drain) it was raised in.
    mcp_depths: StdMutex<HashMap<String, (CancellationToken, u32)>>,
    /// Cross-session message chain hops by Session, for the run they were raised in.
    message_hops: StdMutex<HashMap<String, (CancellationToken, u32)>>,
    idle: Notify,
    lifecycle: Arc<RwLock<()>>,
    shutdown_lock: Mutex<()>,
    closed: CancellationToken,
    background: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
    jobs: StdMutex<HashMap<String, Arc<jobs::Control>>>,
    job_admission: Arc<Mutex<()>>,
    /// Foreground commands the user may move to the background, by Session.
    handoffs: StdMutex<HashMap<String, Vec<handoff::Entry>>>,
    /// Running PTYs and their attach tickets.
    ptys: pty::Registry,
    delegations: StdMutex<HashMap<String, Arc<delegations::Control>>>,
    child_executions: StdMutex<HashMap<String, std::sync::Weak<Mutex<()>>>>,
    /// Registration ID of the server lock this runtime holds (`adopt_server_lock`).
    server_holder: std::sync::OnceLock<String>,
    child_preparations: StdMutex<HashMap<String, Arc<preparation::Control>>>,
    activities: StdMutex<HashMap<String, Arc<activity::Control>>>,
    subtree_stops: StdMutex<HashMap<String, Arc<subtree_stop::Control>>>,
    subtree_stop_lock: Mutex<()>,
    /// The server-wide fair scheduler of Workflow agent slots (`workflows` → Fair agent
    /// scheduling, Concurrency limit).
    workflow_scheduler: std::sync::OnceLock<workflow_scheduler::WorkflowScheduler>,
    /// The server-wide run queue (`workflows` → Run queue).
    workflow_queue: std::sync::OnceLock<workflow_queue::WorkflowQueue>,
    /// Executing Workflow Run attempts (`workflows` → Run lifecycle states).
    workflow_controls: StdMutex<HashMap<String, Arc<workflow_control::Control>>>,
    /// Set, under `workflow_controls`, when shutdown starts pausing runs: no attempt
    /// launches or is dispatched after it, so every executing attempt is paused.
    workflow_closing: std::sync::atomic::AtomicBool,
    /// One lifecycle lock per run (`workflows` → Concurrent runs): pause, stop, resume and
    /// the record of a script's own end serialize per run, never across runs.
    workflow_locks: StdMutex<HashMap<String, Arc<Mutex<()>>>>,
    /// One send lock per message target (`cross-session-messaging` → Rate limits):
    /// concurrent senders to one Session pass the checks and admission one at a time.
    message_locks: StdMutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Team creation and spawns (`agent-teams` → Team composition): the size check and
    /// the spawn it admits run under this lock, so concurrent spawns cannot exceed
    /// `teams.max_teammates`.
    team_spawn: Mutex<()>,
    /// Team status changes (`agent-teams` → Team budgets, Shutdown and resume): a budget
    /// stop, a stop, its end and a resume read the team's status and record the change
    /// one at a time, so a team is exceeded or stopped once per active period.
    team_status: Mutex<()>,
    /// Peer Sessions of the reachable listing, reused for 15 s (`cross-session-messaging`
    /// → Peer watches and listing).
    peer_listings: StdMutex<messaging::listing::ListingCache>,
    /// Issued, unsettled edit-capable workflow agents per canonical Location, across runs
    /// (`workflows` → Write isolation default).
    workflow_writers: StdMutex<HashMap<String, Arc<std::sync::atomic::AtomicUsize>>>,
    /// `<state>/workflows`: inline scripts and results larger than 1 MiB.
    workflow_dir: std::sync::OnceLock<PathBuf>,
    /// Rebuilds a run's next attempt after a restart (`workflows` → Recovery of several
    /// runs).
    workflow_relaunch: std::sync::OnceLock<Arc<dyn workflow_recovery::WorkflowRelaunch>>,
    /// Set once the `workflow.run.progress` publisher runs.
    workflow_progress: std::sync::OnceLock<()>,
    pub(crate) waiters: StdMutex<Vec<requests::Waiter>>,
    pub(crate) me: std::sync::Weak<Inner>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.closed.cancel();
    }
}

#[derive(Clone)]
pub struct Runtime {
    inner: Arc<Inner>,
}

/// A non-owning callback handle for hosts owned by the runtime itself.
#[derive(Clone)]
pub struct WeakRuntime {
    inner: std::sync::Weak<Inner>,
}

impl WeakRuntime {
    pub fn upgrade(&self) -> Option<Runtime> {
        self.inner.upgrade().map(|inner| Runtime { inner })
    }
}

impl Runtime {
    /// Models for trusted host-owned hidden calls, without starting another Session.
    pub fn model_resolver(&self) -> Arc<dyn ModelResolver> {
        Arc::clone(&self.inner.resolver)
    }

    pub fn downgrade(&self) -> WeakRuntime {
        WeakRuntime {
            inner: Arc::downgrade(&self.inner),
        }
    }

    /// The event registry the store must be opened with.
    pub fn registry() -> EventRegistry {
        events::registry()
    }

    pub fn new(options: RuntimeOptions) -> Self {
        let inner = Arc::new_cyclic(|me| Inner {
            store: Arc::clone(&options.store),
            resolver: Arc::clone(&options.resolver),
            tools: Arc::clone(&options.tools),
            bus: Bus::new(),
            options,
            sessions: StdMutex::default(),
            drains: StdMutex::default(),
            mcp_depths: StdMutex::default(),
            message_hops: StdMutex::default(),
            idle: Notify::new(),
            lifecycle: Arc::new(RwLock::new(())),
            shutdown_lock: Mutex::new(()),
            closed: CancellationToken::new(),
            background: StdMutex::default(),
            jobs: StdMutex::default(),
            job_admission: Arc::new(Mutex::new(())),
            handoffs: StdMutex::default(),
            ptys: pty::Registry::default(),
            delegations: StdMutex::default(),
            child_executions: StdMutex::default(),
            server_holder: std::sync::OnceLock::new(),
            child_preparations: StdMutex::default(),
            activities: StdMutex::default(),
            subtree_stops: StdMutex::default(),
            subtree_stop_lock: Mutex::new(()),
            workflow_scheduler: std::sync::OnceLock::new(),
            workflow_queue: std::sync::OnceLock::new(),
            workflow_controls: StdMutex::default(),
            workflow_closing: std::sync::atomic::AtomicBool::new(false),
            workflow_locks: StdMutex::default(),
            message_locks: StdMutex::default(),
            team_spawn: Mutex::default(),
            team_status: Mutex::default(),
            peer_listings: StdMutex::default(),
            workflow_writers: StdMutex::default(),
            workflow_dir: std::sync::OnceLock::new(),
            workflow_relaunch: std::sync::OnceLock::new(),
            workflow_progress: std::sync::OnceLock::new(),
            waiters: StdMutex::default(),
            me: me.clone(),
        });
        Self { inner }
    }

    /// Requests waiting for a client, optionally owned by or routed to one Session.
    pub fn pending_requests(&self, session_id: Option<&str>) -> Vec<PendingRequest> {
        self.inner.pending(session_id)
    }

    /// Answer a permission request (`once`, `always`, `reject` with optional feedback).
    pub async fn reply_permission(
        &self,
        request_id: &str,
        reply: PermissionReply,
    ) -> Result<(), RuntimeError> {
        self.inner.reply_permission(request_id, reply).await
    }

    /// Answer or dismiss a question.
    pub async fn answer_question(
        &self,
        request_id: &str,
        reply: QuestionReply,
    ) -> Result<(), RuntimeError> {
        self.inner.answer_question(request_id, reply).await
    }

    pub fn memory_http_identity(
        &self,
        key: &str,
        digest: &str,
    ) -> Result<Option<bool>, cyber_store::StoreError> {
        memory::http_identity(&self.inner.store, key, digest)
    }

    /// Read durable HTTP save/delete evidence without replaying its effects.
    pub fn memory_request_status(
        &self,
        key: &str,
    ) -> Result<Option<MemoryRequestStatus>, cyber_store::StoreError> {
        memory::request_status(&self.inner.store, key)
    }

    pub fn memory_write_receipt(
        &self,
        write: &MemoryWrite<'_>,
    ) -> Result<Option<MemoryChange>, cyber_store::StoreError> {
        memory::lookup(&self.inner.store, write)
    }

    pub fn admit_memory_write(
        &self,
        write: MemoryWrite<'_>,
    ) -> Result<MemoryAdmission, cyber_store::StoreError> {
        memory::admit(self.inner.store.clone(), self.inner.bus.clone(), write)
    }

    pub fn review_memory_recovery(
        &self,
        directory: &std::path::Path,
        project_id: &str,
        journal: &cyber_core::memory::MemoryJournalIdentity,
    ) -> Result<MemoryRecoveryAdmission, cyber_store::StoreError> {
        memory::review(&self.inner.store, directory, project_id, journal)
    }

    /// Explicit host-controlled reconciliation; callers retain scope, settings and lifecycle admission.
    pub fn recover_memory_write(
        &self,
        scope: &mut cyber_core::memory::MemoryScope<'_>,
        storage_review: &cyber_core::memory::MemoryRecoveryReview,
        admission_review: &MemoryRecoveryAdmission,
    ) -> Result<MemoryChange, cyber_store::StoreError> {
        memory::recover(
            self.inner.store.clone(),
            self.inner.bus.clone(),
            scope,
            storage_review,
            admission_review,
        )
    }

    pub fn memory_recovery_request_status(
        &self,
        key: &str,
    ) -> Result<Option<MemoryRecoveryRequestStatus>, cyber_store::StoreError> {
        memory::recovery::status(&self.inner.store, key)
    }
    pub fn memory_recovery_http_receipt(
        &self,
        identity: &MemoryRecoveryIdentity,
    ) -> Result<Option<MemoryChange>, cyber_store::StoreError> {
        memory::recovery::receipt(&self.inner.store, identity)
    }
    pub fn recover_memory_write_with_identity(
        &self,
        scope: &mut cyber_core::memory::MemoryScope<'_>,
        storage: &cyber_core::memory::MemoryRecoveryReview,
        admission: &MemoryRecoveryAdmission,
        identity: Option<&MemoryRecoveryIdentity>,
    ) -> Result<MemoryChange, cyber_store::StoreError> {
        memory::recovery::recover_with_identity(
            self.inner.store.clone(),
            self.inner.bus.clone(),
            scope,
            storage,
            admission,
            identity,
        )
    }

    pub fn mcp_status_observer(&self) -> McpConnectionObserver {
        let bus = self.inner.bus.clone();
        Arc::new(move |record, seq| {
            bus.publish(LiveEvent::McpStatusChanged {
                update: record.into(),
                seq,
            });
        })
    }

    /// Publish `mcp.tools.changed.1` after a server's tools or prompts were published or
    /// re-listed.
    pub fn publish_mcp_tools_changed(&self, update: McpToolsChanged) {
        self.inner
            .bus
            .publish(LiveEvent::McpToolsChanged { update });
    }

    /// Publish a dispatched call's live status (`session.tool.progress`), never durable.
    pub fn tool_progress(&self, session: &str, call_id: &str, status: &str) {
        self.inner.bus.publish(LiveEvent::ToolProgress {
            session_id: session.into(),
            call_id: call_id.into(),
            status: status.into(),
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LiveEvent> {
        self.inner.bus.subscribe()
    }

    /// Create a Session, or return the existing one unchanged when `id` is taken.
    /// The first root Session of a Location fires the observational `Setup` hook
    /// once creation is committed and admission released (hook runners take it again).
    pub async fn create_session(&self, req: CreateSession) -> Result<SessionInfo, RuntimeError> {
        let (info, first) = self.create_session_admitted(req).await?;
        if first {
            self.inner.setup(&info.id).await;
        }
        Ok(info)
    }

    /// Create under runtime admission; true when it is the first root Session of
    /// its Location in this store.
    async fn create_session_admitted(
        &self,
        req: CreateSession,
    ) -> Result<(SessionInfo, bool), RuntimeError> {
        let _admission = self.inner.open().await?;
        let id = req
            .id
            .clone()
            .unwrap_or_else(|| cyber_core::ids::new_id("ses"));
        if !cyber_core::ids::has_prefix(&id, "ses") {
            return Err(RuntimeError::Invalid(format!(
                "session IDs start with ses_: {id}"
            )));
        }
        if let Ok(existing) = self.inner.handle(&id).await {
            return Ok((existing.state.lock().await.info.clone(), false));
        }
        if req.output_schema.is_some() && req.parent_id.is_none() {
            return Err(RuntimeError::Invalid(
                "output_schema requires a child Session".into(),
            ));
        }
        if req.subagent_name.as_ref().is_some_and(|name| {
            req.parent_id.is_none() || name.trim().is_empty() || name.len() > 128
        }) {
            return Err(RuntimeError::Invalid(
                "subagent_name requires a parent and 1–128 bytes".into(),
            ));
        }
        if req.workflow_run_id.is_some() && req.parent_id.is_none() {
            return Err(RuntimeError::Invalid(
                "workflow_run_id requires a parent".into(),
            ));
        }
        if let Some(tag) = &req.team {
            if req.parent_id.is_some() {
                return Err(RuntimeError::Invalid(
                    "A team member is a root Session".into(),
                ));
            }
            tag.validate().map_err(RuntimeError::Invalid)?;
        }
        if req.child_worktree_setup_pending && req.child_worktree.is_none() {
            return Err(RuntimeError::Invalid(
                "Pending isolated setup requires a checkout binding".into(),
            ));
        }
        if let Some(managed) = &req.child_worktree
            && (!managed.ready
                || req.parent_id.is_none()
                || req.worktree_id.as_deref() != Some(managed.id.as_str())
                || std::path::Path::new(&req.directory)
                    .canonicalize()
                    .map_err(|e| RuntimeError::Invalid(e.to_string()))?
                    != managed.path)
        {
            return Err(RuntimeError::Invalid(
                "Isolated child binding differs from its owned checkout".into(),
            ));
        }
        let admission_bindings = match req.parent_id.as_deref() {
            Some(parent) => {
                let authority = match &req.admission_authority {
                    Some(authority) => authority.clone(),
                    None => self.capture_child_admission(parent)?,
                };
                authority.verify(self, parent)?;
                Some(authority.bindings)
            }
            None if req.admission_authority.is_some() => {
                return Err(RuntimeError::Invalid(
                    "Child admission authority requires a parent".into(),
                ));
            }
            None => None,
        };
        let mode_is_default = req.mode.is_none();
        let selection = selection::ModelSelection {
            reference: req.model.clone(),
            agent_default: req.model_is_default,
        };
        let mut info = SessionInfo {
            id: id.clone(),
            default_title: req.title.is_none(),
            title: req.title.unwrap_or_else(default_title),
            directory: req.directory,
            worktree_id: req.worktree_id,
            worktree_origin: None,
            parent_id: req.parent_id,
            subagent_name: req.subagent_name,
            workflow_run_id: req.workflow_run_id,
            agent: req.agent.unwrap_or_else(|| "build".into()),
            model: req.model,
            mode: req.mode.unwrap_or_else(|| "default".into()),
            created_ms: chrono::Utc::now().timestamp_millis(),
            archived: false,
            rules: req.rules.unwrap_or_default(),
            max_steps: req.max_steps,
            budget: req.budget,
            messaging: req.messaging,
            team: req.team,
        };
        if info.budget.is_none() {
            info.budget = self
                .inner
                .tools
                .session_budget(&info.directory)
                .map_err(RuntimeError::Invalid)?;
        }
        budget::validate(info.budget.as_ref())?;
        self.ancestors(&info).await?;
        let defaults = if mode_is_default || selection.agent_default {
            self.inner
                .tools
                .agent_inference(&drain::turn_context_for_info(&info, false))
        } else {
            Ok(AgentInference::default())
        };
        let mode_default_pending = self.resolve_mode(&mut info, mode_is_default, &defaults)?;
        if selection.agent_default {
            let options = defaults.map_err(RuntimeError::Invalid)?;
            info.model = self
                .inner
                .resolve_options(&info, &selection, &options)?
                .reference;
        }
        let selection = selection.persisted(&info.model);
        let copied = match &req.fork_from {
            Some(source) => {
                if info.parent_id.as_deref() != Some(source) {
                    return Err(RuntimeError::Invalid(
                        "Fork source must be the child parent".into(),
                    ));
                }
                let state = self.state(source).await?;
                Some(fork::history(&state, state.entries.len(), &state))
            }
            None => None,
        };
        let (history, calls, fork_context) = match copied {
            Some(copy) => (copy.entries, copy.calls, Some(copy.context)),
            None => (Vec::new(), Vec::new(), None),
        };
        let first =
            info.parent_id.is_none() && !self.inner.location_has_sessions(&info.directory)?;
        let info = self
            .inner
            .create(
                info,
                history,
                calls,
                req.fork_from,
                CreationOptions {
                    admission_bindings,
                    child_worktree_setup_pending: req.child_worktree_setup_pending,
                    child_worktree: req.child_worktree,
                    fork_context,
                    selection,
                    mode_default_pending,
                    output_schema: req.output_schema.map(|s| s.schema().clone()),
                },
            )
            .await?;
        Ok((info, first))
    }

    /// A new Session's Mode: the requested one, else the agent's `permission_mode`, admitted
    /// by the host. `true` when the agent cannot be resolved yet: the first Safe Boundary
    /// then applies (and admits) its default.
    fn resolve_mode(
        &self,
        info: &mut SessionInfo,
        mode_is_default: bool,
        defaults: &Result<AgentInference, String>,
    ) -> Result<bool, RuntimeError> {
        if mode_is_default {
            let Ok(options) = defaults else {
                return Ok(true);
            };
            info.mode = options
                .permission_mode
                .clone()
                .unwrap_or_else(|| "default".into());
            selection::validate_mode(&info.mode)?;
        }
        self.admit_mode(&info.directory, &info.agent, &info.mode)?;
        Ok(false)
    }

    /// The `permission_mode` an agent would give a new Session at `directory`, before any
    /// write; `None` when the agent sets none. An unknown or hidden agent is `Invalid`.
    pub fn agent_permission_mode(
        &self,
        directory: &str,
        agent: &str,
    ) -> Result<Option<String>, RuntimeError> {
        let turn = TurnContext {
            session_id: String::new(),
            directory: directory.to_owned(),
            agent: agent.to_owned(),
            mode: "default".into(),
            prefers_apply_patch: false,
            rules: serde_json::Value::Null,
            worktree: Default::default(),
        };
        self.inner
            .tools
            .agent_inference(&turn)
            .map(|options| options.permission_mode)
            .map_err(RuntimeError::Invalid)
    }

    /// Whether a Session of `agent` at `directory` may run in `mode` (the host's
    /// `bypass` gate and org `policy.modes.disable`).
    pub fn admit_mode(&self, directory: &str, agent: &str, mode: &str) -> Result<(), RuntimeError> {
        self.inner
            .tools
            .admit_mode(directory, agent, mode)
            .map_err(RuntimeError::Invalid)
    }

    /// Durably admit input (`session-runtime` → Durable prompt admission, Exact-retry idempotency).
    pub async fn admit(
        &self,
        session_id: &str,
        admission: Admission,
    ) -> Result<Receipt, RuntimeError> {
        let check = if admission.source == "user" {
            PromptCheck::Run
        } else {
            PromptCheck::Checked(None)
        };
        self.admit_attempt(session_id, admission, None, check).await
    }

    /// Admit a delegated child's prompt with the UserPromptSubmit context the
    /// parent-side delegation already obtained; the hook does not run again.
    /// It carries the chain hops of the parent's run (`messaging::hops`).
    pub async fn admit_with_hook_context(
        &self,
        session_id: &str,
        mut admission: Admission,
        hook_context: Option<String>,
    ) -> Result<Receipt, RuntimeError> {
        let inherited = self.delegated_chain(session_id, &mut admission).await;
        self.admit_attempt_with(
            session_id,
            admission,
            None,
            PromptCheck::Checked(hook_context),
            inherited,
        )
        .await
    }

    /// Public user input: UserPromptSubmit runs whatever source the client claims.
    async fn admit_prompt(
        &self,
        session_id: &str,
        admission: Admission,
    ) -> Result<Receipt, RuntimeError> {
        self.admit_attempt(session_id, admission, None, PromptCheck::Run)
            .await
    }

    /// UserPromptSubmit, then template expansion and resource mentions, for input not
    /// yet admitted; an exact retry skips all of them, so nothing runs twice. They run
    /// before the lifecycle guard: every hook runner and permission request re-enters
    /// that guard to admit its receipt, and a shutdown writer queued in between would
    /// deadlock both. The caller rechecks admission and the exact retry once it holds
    /// the guard.
    async fn prompt_hooks(
        &self,
        session_id: &str,
        admission: &Admission,
        message_id: &str,
        digest: &str,
    ) -> Result<(PromptEffects, PromptAttachments), RuntimeError> {
        self.inner.ensure_admission_open(session_id)?;
        let handle = self.inner.handle(session_id).await?;
        {
            let state = handle.state.lock().await;
            state.ensure_worktree_ready()?;
            if existing_receipt(&state, message_id, digest)?.is_some() {
                return Ok((PromptEffects::default(), PromptAttachments::default()));
            }
        }
        // A denied prompt reads no resources and runs no template commands.
        let effects = self
            .inner
            .user_prompt_hooks(&handle, &admission.parts, &admission.source)
            .await?
            .for_prompt(is_forked(admission.skill_command.as_ref()))?;
        Ok((effects, self.prompt_resources(&handle, admission).await?))
    }

    /// Expand a planned template (`skills-commands` → Shell output injection, File and
    /// agent references), then attach the prompt's `@server:uri` resource mentions
    /// (`mcp` → Resources), all under the Session's agent, mode and rules; a permission
    /// ask reaches the attached client. Mentions are read from the unexpanded text, so
    /// command output never triggers a read. A forked skill command hands its expanded
    /// text to a child, so it attaches no resources here.
    async fn prompt_resources(
        &self,
        handle: &Handle,
        admission: &Admission,
    ) -> Result<PromptAttachments, RuntimeError> {
        let parts = match &admission.template {
            Some(template) => Some(self.expand_template(handle, template, admission).await?),
            None => None,
        };
        let resources = if is_forked(admission.skill_command.as_ref()) {
            Vec::new()
        } else {
            self.mention_resources(handle, &admission.parts).await?
        };
        Ok(PromptAttachments { resources, parts })
    }

    async fn expand_template(
        &self,
        handle: &Handle,
        template: &PromptTemplate,
        admission: &Admission,
    ) -> Result<Vec<Content>, RuntimeError> {
        let turn = drain::turn_context_for(&*handle.state.lock().await, false);
        let asker = self.operation_asker(&turn.session_id).await?;
        self.inner
            .tools
            .expand_prompt_template(
                &turn,
                template,
                &admission.parts,
                asker,
                self.inner.closed.child_token(),
            )
            .await
            .map_err(RuntimeError::Invalid)
    }

    async fn mention_resources(
        &self,
        handle: &Handle,
        parts: &[Content],
    ) -> Result<Vec<String>, RuntimeError> {
        let turn = drain::turn_context_for(&*handle.state.lock().await, false);
        let asker = self.operation_asker(&turn.session_id).await?;
        Ok(self
            .inner
            .tools
            .resolve_prompt_resources(&turn, parts, asker, self.inner.closed.child_token())
            .await)
    }

    async fn admit_attempt(
        &self,
        session_id: &str,
        admission: Admission,
        attempt: Option<names::Resumed>,
        check: PromptCheck,
    ) -> Result<Receipt, RuntimeError> {
        self.admit_attempt_with(session_id, admission, attempt, check, Vec::new())
            .await
    }

    /// Admit a cross-session message with its `in` record (`extra`) in the same commit.
    /// A message is not user input: no UserPromptSubmit, template or `@` resolution runs,
    /// so its mentions stay literal text (`cross-session-messaging` → Message content
    /// limits). An exact retry returns the original receipt and commits nothing.
    pub(crate) async fn admit_message(
        &self,
        session_id: &str,
        admission: Admission,
        extra: Vec<NewEvent>,
    ) -> Result<Receipt, RuntimeError> {
        self.admit_attempt_with(
            session_id,
            admission,
            None,
            PromptCheck::Checked(None),
            extra,
        )
        .await
    }

    async fn admit_attempt_with(
        &self,
        session_id: &str,
        admission: Admission,
        attempt: Option<names::Resumed>,
        check: PromptCheck,
        extra: Vec<NewEvent>,
    ) -> Result<Receipt, RuntimeError> {
        let message_id = admission
            .message_id
            .clone()
            .unwrap_or_else(|| cyber_core::ids::new_id("msg"));
        let digest = admission.digest();
        // A denied prompt leaves no durable trace, including a staged revert.
        let prompt = check == PromptCheck::Run;
        let (effects, attachments) = match check {
            PromptCheck::Run => {
                self.prompt_hooks(session_id, &admission, &message_id, &digest)
                    .await?
            }
            PromptCheck::Checked(context) => (
                PromptEffects {
                    context,
                    stop: None,
                },
                PromptAttachments::default(),
            ),
        };
        let _admission = self.inner.open().await?;
        self.inner.ensure_admission_open(session_id)?;
        let admission_bindings = self.callback_admission_bindings(session_id)?;
        if let Some(bindings) = attempt
            .as_ref()
            .and_then(|attempt| attempt.admission_bindings.as_ref())
        {
            self.verify_admission_bindings(bindings)?;
        }
        let handle = self.inner.handle(session_id).await?;
        handle.state.lock().await.ensure_worktree_ready()?;
        self.inner.commit_staged_revert(&handle).await?;
        // Public input resets the Stop count whatever source the client claims.
        let genuine = prompt || admission.source != "hook";
        let receipt = {
            let mut state = handle.state.lock().await;
            state.ensure_worktree_ready()?;
            if let Some(receipt) = existing_receipt(&state, &message_id, &digest)? {
                return Ok(receipt);
            }
            // The state lock makes accepted input visible before settlement chooses cleanup.
            if self.child_is_settling(session_id) {
                return Err(RuntimeError::Busy(session_id.into()));
            }
            let payload = Admitted {
                skill_command: admission.skill_command,
                admission_bindings,
                wake: state.info.parent_id.is_some()
                    && admission.resume
                    && admission.delivery != Delivery::Hold,
                message_id: message_id.clone(),
                parts: attachments.parts.unwrap_or(admission.parts),
                delivery: admission.delivery,
                source: admission.source,
                digest,
                hook_context: effects.context,
                hook_stop: effects.stop,
                resources: attachments.resources,
            };
            let mut events = Vec::new();
            if let Some(attempt) = attempt {
                events.push(event(events::RESUMED, &attempt));
            }
            events.push(event(ADMITTED, &payload));
            let admitted = events.len() - 1;
            events.extend(extra);
            let stored = self.inner.commit_locked(&mut state, events)?;
            receipt_for(&state, &message_id, stored[admitted].seq)
        };
        if genuine {
            handle
                .stop_continuations
                .store(0, std::sync::atomic::Ordering::SeqCst);
        }
        if admission.resume && admission.delivery != Delivery::Hold {
            self.inner.start_drain(session_id, false);
        }
        Ok(receipt)
    }

    /// Edit an unpromoted `queue` or `hold` row.
    pub async fn edit_input(
        &self,
        session_id: &str,
        message_id: &str,
        parts: Option<Vec<Content>>,
        delivery: Option<Delivery>,
    ) -> Result<(), RuntimeError> {
        // Edited parts are user input: UserPromptSubmit re-checks them and their
        // resource mentions are resolved again before commit.
        let (effects, resources) = match &parts {
            Some(parts) => {
                self.edited_prompt_hooks(session_id, message_id, parts)
                    .await?
            }
            None => (PromptEffects::default(), Vec::new()),
        };
        self.update_input(
            session_id,
            message_id,
            InboxAction::Edited,
            parts,
            delivery,
            (effects, resources),
        )
        .await
    }

    async fn edited_prompt_hooks(
        &self,
        session_id: &str,
        message_id: &str,
        parts: &[Content],
    ) -> Result<(PromptEffects, Vec<String>), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let (source, fork) = {
            let state = handle.state.lock().await;
            let row = state
                .input(message_id)
                .ok_or_else(|| RuntimeError::Invalid(format!("no inbox row {message_id}")))?;
            check_inbox_action(row, InboxAction::Edited)?;
            messaging::inbox::inbox_record(&state, message_id, InboxAction::Edited, None)?;
            let fork = row.skill_command.as_ref().is_some_and(|skill| skill.fork);
            (row.source.clone(), fork)
        };
        let effects = self
            .inner
            .user_prompt_hooks(&handle, parts, &source)
            .await?
            .for_prompt(fork)?;
        let resources = if fork {
            Vec::new()
        } else {
            self.mention_resources(&handle, parts).await?
        };
        Ok((effects, resources))
    }

    /// Withdraw an unpromoted `queue` or `hold` row; it never reaches the model. A
    /// dropped cross-session message is recorded in its sender too.
    pub async fn remove_input(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<(), RuntimeError> {
        self.update_input(
            session_id,
            message_id,
            InboxAction::Removed,
            None,
            None,
            (PromptEffects::default(), Vec::new()),
        )
        .await?;
        self.complete_sent(Some(message_id)).await?;
        Ok(())
    }

    /// Release a held row as `steer` or `queue`, waking the Session.
    pub async fn release(
        &self,
        session_id: &str,
        message_id: &str,
        delivery: Delivery,
    ) -> Result<(), RuntimeError> {
        self.inner.ensure_admission_open(session_id)?;
        if delivery == Delivery::Hold {
            return Err(RuntimeError::Invalid(
                "release a held input as steer or queue".into(),
            ));
        }
        if self
            .continue_child_input(session_id, Some((message_id, delivery)), false)
            .await?
        {
            // A released cross-session message: the sender's record follows.
            self.complete_sent(Some(message_id)).await?;
            return Ok(());
        }
        let _admission = self.inner.open().await?;
        self.update_input(
            session_id,
            message_id,
            InboxAction::Released,
            None,
            Some(delivery),
            (PromptEffects::default(), Vec::new()),
        )
        .await?;
        self.inner.start_drain(session_id, false);
        // A released cross-session message: the sender's record follows.
        self.complete_sent(Some(message_id)).await?;
        Ok(())
    }

    /// Refuse a held row; it is kept as `refused` and never promoted.
    pub async fn refuse(&self, session_id: &str, message_id: &str) -> Result<(), RuntimeError> {
        self.update_input(
            session_id,
            message_id,
            InboxAction::Refused,
            None,
            None,
            (PromptEffects::default(), Vec::new()),
        )
        .await?;
        self.complete_sent(Some(message_id)).await?;
        Ok(())
    }

    async fn update_input(
        &self,
        session_id: &str,
        message_id: &str,
        action: InboxAction,
        parts: Option<Vec<Content>>,
        delivery: Option<Delivery>,
        (effects, resources): (PromptEffects, Vec<String>),
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let row = state
            .input(message_id)
            .ok_or_else(|| RuntimeError::Invalid(format!("no inbox row {message_id}")))?;
        check_inbox_action(row, action)?;
        if action == InboxAction::Released {
            state.ensure_worktree_ready()?;
            if self.child_is_settling(session_id) {
                return Err(RuntimeError::Busy(session_id.into()));
            }
        }
        let record = messaging::inbox::inbox_record(&state, message_id, action, delivery)?;
        let payload = InboxUpdated {
            message_id: message_id.into(),
            action,
            parts,
            delivery,
            hook_context: effects.context,
            hook_stop: effects.stop,
            resources,
        };
        // A cross-session message's `in` record commits with the change; the caller then
        // completes the sender's record.
        let events = std::iter::once(event(INBOX_UPDATED, &payload))
            .chain(record)
            .collect();
        self.inner.commit_locked(&mut state, events)?;
        Ok(())
    }

    /// Start a Drain when idle, or record one coalesced follow-up when one is running.
    pub async fn wake(&self, session_id: &str) -> Result<(), RuntimeError> {
        self.inner.ensure_admission_open(session_id)?;
        if self.continue_child_input(session_id, None, false).await? {
            return Ok(());
        }
        let _admission = self.inner.open().await?;
        if self.child_is_settling(session_id) {
            return Err(RuntimeError::Busy(session_id.into()));
        }
        self.inner
            .handle(session_id)
            .await?
            .state
            .lock()
            .await
            .ensure_worktree_ready()?;
        self.inner.start_drain(session_id, false);
        Ok(())
    }

    /// Join an active Drain, or start one that performs at least one Turn.
    pub async fn resume(&self, session_id: &str) -> Result<(), RuntimeError> {
        self.inner.ensure_admission_open(session_id)?;
        let _admission = self.inner.open().await?;
        if self.child_is_settling(session_id) {
            return Err(RuntimeError::Busy(session_id.into()));
        }
        self.inner
            .handle(session_id)
            .await?
            .state
            .lock()
            .await
            .ensure_worktree_ready()?;
        self.inner.start_drain(session_id, true);
        Ok(())
    }

    /// Stop the Drain and abandon owned requests, including idle host operations; inbox rows are kept.
    /// Interrupt hooks then observe it.
    pub async fn interrupt(&self, session_id: &str) -> Result<(), RuntimeError> {
        let running = self.is_running(session_id);
        self.interrupt_for_owner(session_id).await?;
        let handle = self.inner.handle(session_id).await?;
        self.inner.interrupted(&handle, running).await;
        Ok(())
    }

    /// Interrupt a child on behalf of its owner's cancellation or settlement. No
    /// Interrupt hook runs: the owner's SubagentStop/JobEnded observe the outcome,
    /// and the canceller may hold the child while it waits for this.
    pub async fn interrupt_for_owner(&self, session_id: &str) -> Result<(), RuntimeError> {
        let (paused, preparation) = self.pause_requested_child_input(session_id).await;
        let activities = self.inner.cancel_idle_activities(session_id);
        {
            let mut drains = self
                .inner
                .drains
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(entry) = drains.get_mut(session_id) {
                entry.follow_up = false;
                entry.cancel.cancel();
            }
        }
        self.wait_idle(session_id).await;
        self.inner
            .wait_idle_activities(session_id, activities)
            .await?;
        if let Some(preparation) = preparation
            && tokio::time::timeout(
                std::time::Duration::from_secs(3),
                preparation.done.cancelled(),
            )
            .await
            .is_err()
        {
            let handle = self.inner.handle(session_id).await?;
            let message =
                "Child preparation did not acknowledge interruption; recovery is required";
            self.mark_child_preparation_unknown(&handle, message)
                .await?;
            return Err(RuntimeError::Invalid(message.into()));
        }
        self.inner.abandon_requests(session_id).await?;
        paused
    }

    /// Wait until no Drain runs for the Session.
    pub async fn wait_idle(&self, session_id: &str) {
        loop {
            let notified = self.inner.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.is_running(session_id) {
                return;
            }
            notified.await;
        }
    }

    pub fn is_running(&self, session_id: &str) -> bool {
        self.inner
            .drains
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(session_id)
    }

    /// The Sessions whose Drain is running.
    pub fn running_sessions(&self) -> std::collections::HashSet<String> {
        self.inner
            .drains
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// A copy of the Session's current state.
    pub async fn state(&self, session_id: &str) -> Result<SessionState, RuntimeError> {
        let mut state = self
            .inner
            .handle(session_id)
            .await?
            .state
            .lock()
            .await
            .clone();
        state.children_usage = self.children_usage(session_id)?;
        Ok(state)
    }

    pub(crate) async fn worktree_binding(
        &self,
        session_id: &str,
    ) -> Result<Option<String>, RuntimeError> {
        Ok(self
            .inner
            .handle(session_id)
            .await?
            .state
            .lock()
            .await
            .info
            .worktree_id
            .clone())
    }

    /// Compact now when idle, or at the next Safe Boundary when running.
    pub async fn compact(
        &self,
        session_id: &str,
        instructions: Option<String>,
    ) -> Result<(), RuntimeError> {
        self.inner.ensure_admission_open(session_id)?;
        let _admission = self.inner.open().await?;
        let handle = self.inner.handle(session_id).await?;
        if self.is_running(session_id) {
            *handle
                .pending_compaction
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(instructions);
            return Ok(());
        }
        drop(_admission);
        self.inner
            .with_idle_location(
                &handle,
                self.inner
                    .compact(&handle, CompactionTrigger::Manual, instructions),
            )
            .await
    }

    pub async fn switch_model(&self, session_id: &str, model: &str) -> Result<(), RuntimeError> {
        cyber_llm::catalog::ModelRef::parse(model)
            .map_err(|e| RuntimeError::Invalid(e.to_string()))?;
        let handle = self.inner.handle(session_id).await?;
        self.inner.switch_model_hooked(&handle, model).await
    }

    pub async fn switch_agent(&self, session_id: &str, agent: &str) -> Result<(), RuntimeError> {
        self.switch(session_id, AGENT_SWITCHED, agent, |i| &i.agent)
            .await
    }

    pub async fn switch_mode(&self, session_id: &str, mode: &str) -> Result<(), RuntimeError> {
        selection::validate_mode(mode)?;
        // `agent-teams` → Permissions in teams: never above the lead's Mode.
        let handle = self.inner.handle(session_id).await?;
        let ceiling = self.inner.team_ceiling(&handle).await?;
        teams::check_mode(mode, ceiling.as_deref())?;
        self.switch(session_id, MODE_SWITCHED, mode, |i| &i.mode)
            .await
    }

    /// Record a switch that takes effect at the next Turn; switching to the current value is a no-op.
    async fn switch(
        &self,
        session_id: &str,
        kind: &str,
        to: &str,
        current: impl Fn(&SessionInfo) -> &String,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let from = current(&state.info).clone();
        if from == to && !(kind == MODE_SWITCHED && state.mode_default_pending) {
            return Ok(());
        }
        // The Mode stays admitted for whichever agent runs it (its own sandbox profile).
        let (agent, mode) = match kind {
            MODE_SWITCHED => (state.info.agent.as_str(), to),
            _ => (to, state.info.mode.as_str()),
        };
        self.inner
            .tools
            .admit_mode(&state.info.directory, agent, mode)
            .map_err(RuntimeError::Invalid)?;
        self.inner.commit_locked(
            &mut state,
            vec![event(
                kind,
                &Switched {
                    automatic: false,
                    from,
                    to: to.into(),
                },
            )],
        )?;
        Ok(())
    }

    /// A user-set title is trimmed, stripped of control characters and capped at 100
    /// characters; once set, generated titles never replace it.
    pub async fn rename(&self, session_id: &str, title: &str) -> Result<(), RuntimeError> {
        let title = title::user_title(title)
            .ok_or_else(|| RuntimeError::Invalid("title must not be empty".into()))?;
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        self.inner.commit_locked(
            &mut state,
            vec![event(
                RENAMED,
                &Titled {
                    title,
                    usage: None,
                    cost: None,
                    model: None,
                },
            )],
        )?;
        Ok(())
    }

    /// Archived Sessions are hidden from default lists but stay resumable.
    /// Archiving ends the Session for SessionEnd hooks, which only observe it.
    pub async fn archive(&self, session_id: &str, archived: bool) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let ended = {
            let mut state = handle.state.lock().await;
            let ended = archived && !state.info.archived;
            self.inner
                .commit_locked(&mut state, vec![event(ARCHIVED, &Archived { archived })])?;
            ended
        };
        if ended {
            self.inner.session_end(&handle, "archive").await;
            // `cross-session-messaging` → Idle notifications: an archived target exited.
            self.target_archived(session_id).await;
        }
        Ok(())
    }

    /// Resolve a call whose outcome is unknown, lifting the pause on mutating tools.
    pub async fn resolve_unknown(
        &self,
        session_id: &str,
        call_id: &str,
        resolution: Resolution,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let call = state
            .calls
            .get(call_id)
            .ok_or_else(|| RuntimeError::Invalid(format!("no tool call {call_id}")))?;
        if call.status != CallStatus::OutcomeUnknown {
            return Err(RuntimeError::Invalid(format!(
                "tool call {call_id} has a known outcome"
            )));
        }
        let (status, output) = match resolution {
            Resolution::Succeeded(evidence) => (CallStatus::Ok, evidence),
            Resolution::NotApplied => (
                CallStatus::Interrupted,
                "[The user confirmed the call did not take effect]".into(),
            ),
            Resolution::AuthorizeRetry => (
                CallStatus::Interrupted,
                "[The user authorized another attempt]".into(),
            ),
        };
        let payload = ToolSettled {
            skill_activation: None,
            skill_reminders: Vec::new(),
            instruction_reminders: Vec::new(),
            structured_output: None,
            call_id: call_id.into(),
            status,
            output,
            detail: Some("resolved by user".into()),
        };
        self.inner
            .commit_locked(&mut state, vec![event(TOOL_SETTLED, &payload)])?;
        Ok(())
    }

    /// Start a new Context Epoch from current sources (`cyber sessions repair-context`).
    pub async fn repair_context(&self, session_id: &str) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let model = handle.state.lock().await.info.model.clone();
        let resolved = self.inner.resolve(&model)?;
        self.inner
            .with_idle_location(&handle, self.inner.start_epoch(&handle, &resolved.provider))
            .await
    }

    /// Copy history before `before_message` (all of it when `None`) into a new Session.
    pub async fn fork(
        &self,
        session_id: &str,
        before_message: Option<&str>,
    ) -> Result<SessionInfo, RuntimeError> {
        let _admission = self.inner.open().await?;
        let handle = self.inner.handle(session_id).await?;
        let state = handle.state.lock().await.clone();
        let cut = match before_message {
            Some(id) => state
                .entries
                .iter()
                .position(|e| e.id() == id)
                .ok_or_else(|| RuntimeError::Invalid(format!("no message {id}")))?,
            None => state.entries.len(),
        };
        let context = self.fork_boundary(&state, before_message)?;
        let copied = fork::history(&state, cut, &context);
        let forks = self.list(&ListFilter {
            search: Some(format!("{} (fork #", state.info.title)),
            limit: Some(200),
            ..ListFilter::default()
        })?;
        let info = SessionInfo {
            id: cyber_core::ids::new_id("ses"),
            title: format!("{} (fork #{})", state.info.title, forks.sessions.len() + 1),
            subagent_name: None,
            workflow_run_id: None,
            parent_id: None,
            // A fork is a new Session of its own, never a member of the source's team.
            team: None,
            default_title: false,
            created_ms: chrono::Utc::now().timestamp_millis(),
            archived: false,
            ..state.info.clone()
        };
        self.inner
            .create(
                info,
                copied.entries,
                copied.calls,
                Some(session_id.to_string()),
                CreationOptions {
                    admission_bindings: None,
                    child_worktree_setup_pending: false,
                    child_worktree: None,
                    fork_context: Some(copied.context),
                    selection: state.model_selection.persisted(&state.info.model),
                    mode_default_pending: state.mode_default_pending,
                    output_schema: state.result.schema.as_ref().map(|s| s.schema().clone()),
                },
            )
            .await
    }

    /// Sessions ordered by `updated_at` descending, with an opaque cursor.
    pub fn list(&self, filter: &ListFilter) -> Result<SessionPage, RuntimeError> {
        let limit = filter.limit.unwrap_or(50);
        if !(1..=200).contains(&limit) {
            return Err(RuntimeError::Invalid(
                "limit must be between 1 and 200".into(),
            ));
        }
        let cursor = filter.cursor.as_deref().map(parse_cursor).transpose()?;
        let filter = filter.clone();
        let rows = self
            .inner
            .store
            .read(move |conn| query_sessions(conn, &filter, cursor, limit + 1))?;
        let mut sessions = rows;
        let next = (sessions.len() > limit as usize).then(|| {
            sessions.truncate(limit as usize);
            sessions.last().expect("non-empty").cursor()
        });
        Ok(SessionPage { sessions, next })
    }
}

fn default_title() -> String {
    format!(
        "New session - {}",
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    )
}

fn digest(
    parts: &[Content],
    delivery: Delivery,
    skill: Option<&SkillCommand>,
    template: Option<&PromptTemplate>,
) -> String {
    let mut body = serde_json::json!({ "parts": parts, "delivery": delivery });
    if let Some(skill) = skill {
        body["skill_command"] = serde_json::to_value(skill).expect("serializable skill command");
    }
    if let Some(template) = template {
        body["template"] = serde_json::to_value(template).expect("serializable template");
    }
    format!("sha256:{:x}", Sha256::digest(body.to_string()))
}

/// A forked skill command hands its prompt to a child instead of a Turn here.
fn is_forked(skill: Option<&SkillCommand>) -> bool {
    skill.is_some_and(|skill| skill.fork)
}

/// Resource blocks and, for a planned template, its expanded parts, which replace the
/// admitted parts so replay and an exact retry never run anything again.
#[derive(Debug, Default)]
pub(super) struct PromptAttachments {
    pub(super) resources: Vec<String>,
    pub(super) parts: Option<Vec<Content>>,
}

fn existing_receipt(
    state: &SessionState,
    message_id: &str,
    digest: &str,
) -> Result<Option<Receipt>, RuntimeError> {
    if let Some(row) = state.input(message_id) {
        if row.digest == digest {
            return Ok(Some(receipt_for(state, message_id, row.admitted_seq)));
        }
        return Err(RuntimeError::PromptConflict(message_id.into()));
    }
    if state.entries.iter().any(|e| e.id() == message_id) {
        return Err(RuntimeError::PromptConflict(message_id.into()));
    }
    Ok(None)
}

fn receipt_for(state: &SessionState, message_id: &str, seq: i64) -> Receipt {
    let row = state.input(message_id).expect("admitted row");
    Receipt {
        session_id: state.info.id.clone(),
        message_id: message_id.into(),
        delivery: row.delivery,
        admitted_seq: seq,
        status: row.status,
    }
}

fn check_inbox_action(row: &InboxRow, action: InboxAction) -> Result<(), RuntimeError> {
    let editable = matches!(row.status, InputStatus::Pending | InputStatus::Held)
        && row.delivery != Delivery::Steer;
    let allowed = match action {
        InboxAction::Edited | InboxAction::Removed => editable,
        InboxAction::Released | InboxAction::Refused => row.status == InputStatus::Held,
    };
    if allowed {
        Ok(())
    } else {
        Err(RuntimeError::Invalid(format!(
            "inbox row {} cannot be {action:?} in status {:?}",
            row.message_id, row.status
        )))
    }
}

fn parse_cursor(cursor: &str) -> Result<(i64, String), RuntimeError> {
    let invalid = || RuntimeError::Invalid("InvalidCursorError: malformed cursor".into());
    let (at, id) = cursor.split_once(':').ok_or_else(invalid)?;
    Ok((at.parse().map_err(|_| invalid())?, id.to_string()))
}

fn query_sessions(
    conn: &rusqlite::Connection,
    f: &ListFilter,
    cursor: Option<(i64, String)>,
    limit: u32,
) -> Result<Vec<SessionRow>, StoreError> {
    let (cursor_at, cursor_id) = cursor.unwrap_or((i64::MAX, String::new()));
    let mut stmt = conn.prepare(
        "SELECT id, title, directory, parent_id, model, archived, created_at, updated_at, cost, children_cost, children_tokens, children_unpriced_steps, children_usage_complete, children_token_classes, children_token_classes_complete, agent, mode FROM session
         WHERE (?1 IS NULL OR directory = ?1)
           AND (?2 IS NULL OR parent_id = ?2)
           AND (?3 = 0 OR parent_id IS NULL)
           AND (?4 = 1 OR archived = 0)
           AND (?5 IS NULL OR instr(lower(title), lower(?5)) > 0)
           AND (updated_at < ?6 OR (updated_at = ?6 AND id < ?7) OR ?7 = '')
           AND (?9 IS NULL OR EXISTS (SELECT 1 FROM event
                WHERE event.aggregate_id = session.id AND event.type = 'session.pr.linked.1'
                  AND json_extract(event.data, '$.number') = ?9))
         ORDER BY updated_at DESC, id DESC LIMIT ?8",
    )?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                f.directory,
                f.parent,
                i64::from(f.roots_only),
                i64::from(f.include_archived),
                f.search,
                cursor_at,
                cursor_id,
                limit,
                f.pr.and_then(|pr| i64::try_from(pr).ok())
            ],
            |r| {
                Ok(SessionRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    directory: r.get(2)?,
                    parent_id: r.get(3)?,
                    model: r.get(4)?,
                    archived: r.get::<_, i64>(5)? != 0,
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                    cost: r.get(8)?,
                    children_usage: child_usage::read(r, 9)?,
                    agent: r.get(15)?,
                    mode: r.get(16)?,
                })
            },
        )?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

struct CreationOptions {
    admission_bindings: Option<Vec<admission_authority::Binding>>,
    child_worktree_setup_pending: bool,
    child_worktree: Option<cyber_core::worktrees::Managed>,
    fork_context: Option<fork::ForkContext>,
    selection: Option<selection::ModelSelection>,
    mode_default_pending: bool,
    output_schema: Option<serde_json::Value>,
}

impl Inner {
    pub(crate) async fn handle(&self, id: &str) -> Result<Arc<Handle>, RuntimeError> {
        if let Some(h) = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            return Ok(Arc::clone(h));
        }
        let events = self.load_events(id)?;
        // A deleted team lead keeps its `team.*` records, so a stream that does not
        // open with `session.created` is a Session that no longer exists.
        if events
            .first()
            .is_none_or(|first| first.kind != events::CREATED)
        {
            return Err(RuntimeError::SessionNotFound(id.into()));
        }
        let state = SessionState::replay(&events).map_err(RuntimeError::Corrupt)?;
        let handle = Arc::new(Handle {
            title_requested: std::sync::atomic::AtomicBool::new(!state.info.default_title),
            state: Mutex::new(state),
            pending_compaction: StdMutex::default(),
            halt: std::sync::atomic::AtomicBool::new(false),
            location_uncertain: std::sync::atomic::AtomicBool::new(false),
            recent_calls: StdMutex::default(),
            auto_blocks: Mutex::new(0),
            stop_continuations: std::sync::atomic::AtomicUsize::new(0),
            envelope: StdMutex::default(),
            pending_hook_context: StdMutex::default(),
            drain_hooks: Default::default(),
            drain_hooks_settled: Notify::new(),
            recap: Mutex::new(()),
            config_digest: StdMutex::default(),
            location_lease: StdMutex::default(),
        });
        let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(Arc::clone(sessions.entry(id.into()).or_insert(handle)))
    }

    fn load_events(&self, id: &str) -> Result<Vec<StoredEvent>, RuntimeError> {
        let mut events = Vec::new();
        let mut after = -1;
        loop {
            let page = self
                .store
                .read_events(id, after, cyber_store::MAX_PAGE_LIMIT)?;
            after = page.events.last().map_or(after, |e| e.seq);
            events.extend(page.events);
            if !page.has_more {
                return Ok(events);
            }
        }
    }

    async fn create(
        &self,
        mut info: SessionInfo,
        history: Vec<Entry>,
        calls: Vec<CallState>,
        forked_from: Option<String>,
        defaults: CreationOptions,
    ) -> Result<SessionInfo, RuntimeError> {
        let lease = self
            .claim_location(&info, forked_from.is_none(), self.closed.child_token())
            .await?;
        if info.worktree_id.is_some() && info.worktree_id != lease.worktree_id {
            return Err(RuntimeError::Invalid(
                "Managed checkout identity changed".into(),
            ));
        }
        info.worktree_id = lease.worktree_id.clone();
        let id = info.id.clone();
        let payload = Created {
            admission_bindings: defaults.admission_bindings,
            child_worktree_setup_pending: defaults.child_worktree_setup_pending,
            child_worktree: defaults.child_worktree,
            fork_context: defaults.fork_context,
            mode_default_pending: defaults.mode_default_pending,
            selection: defaults.selection,
            output_schema: defaults.output_schema,
            info: info.clone(),
            history,
            calls,
            forked_from,
        };
        match self
            .store
            .append(&id, Expected::Seq(-1), vec![event(CREATED, &payload)])
        {
            Ok(stored) => self.publish(&stored),
            // Created concurrently: return the existing Session unchanged.
            Err(StoreError::Concurrency { .. }) => {}
            Err(e) => {
                lease.settle().map_err(RuntimeError::Invalid)?;
                return Err(e.into());
            }
        }
        lease.settle().map_err(RuntimeError::Invalid)?;
        let info = self.handle(&id).await?.state.lock().await.info.clone();
        self.tools.open_location(&info);
        Ok(info)
    }

    /// Append events with optimistic concurrency, fold them into state, then publish.
    pub(crate) fn commit_locked(
        &self,
        state: &mut SessionState,
        events: Vec<NewEvent>,
    ) -> Result<Vec<StoredEvent>, RuntimeError> {
        let stored = self.record_locked(state, events)?;
        self.publish(&stored);
        Ok(stored)
    }

    /// Record and fold events without exposing them before related state is ready.
    pub(crate) fn record_locked(
        &self,
        state: &mut SessionState,
        events: Vec<NewEvent>,
    ) -> Result<Vec<StoredEvent>, RuntimeError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let stored = self
            .store
            .append(&state.info.id, Expected::Seq(state.last_seq), events)?;
        for e in &stored {
            state.apply(e).map_err(RuntimeError::Corrupt)?;
        }
        Ok(stored)
    }

    pub(crate) async fn commit(
        &self,
        handle: &Handle,
        events: Vec<NewEvent>,
    ) -> Result<Vec<StoredEvent>, RuntimeError> {
        let mut state = handle.state.lock().await;
        self.commit_locked(&mut state, events)
    }

    fn publish(&self, stored: &[StoredEvent]) {
        for e in stored {
            self.bus.publish(LiveEvent::Durable {
                session_id: e.aggregate_id.clone(),
                seq: e.seq,
                kind: e.kind.clone(),
                data: e.data.clone(),
            });
        }
    }

    pub(crate) fn resolve(&self, model: &str) -> Result<ResolvedModel, RuntimeError> {
        self.resolver.resolve(model).map_err(RuntimeError::Model)
    }

    fn start_drain(self: &Arc<Self>, id: &str, forced: bool) {
        let mut drains = self.drains.lock().unwrap_or_else(PoisonError::into_inner);
        if self.closed.is_cancelled() || self.ensure_admission_open(id).is_err() {
            return;
        }
        if let Some(entry) = drains.get_mut(id) {
            if entry.child_settling {
                return;
            }
            entry.follow_up = true;
            return;
        }
        let cancel = CancellationToken::new();
        drains.insert(
            id.into(),
            DrainEntry {
                _child_owner: None,
                child_settlement: None,
                child_settling: false,
                cancel: cancel.clone(),
                follow_up: false,
                deleting: false,
            },
        );
        tokio::spawn(drain::run(Arc::clone(self), id.to_string(), forced, cancel));
    }

    /// Called when a Drain pass ends. Returns whether a coalesced follow-up needs another pass.
    pub(crate) fn finish_pass(&self, id: &str, cancel: &CancellationToken) -> bool {
        let mut drains = self.drains.lock().unwrap_or_else(PoisonError::into_inner);
        let again = drains.get_mut(id).is_some_and(|e| {
            let again = e.follow_up && !cancel.is_cancelled();
            e.follow_up = false;
            again
        });
        if !again {
            drains.remove(id);
            drop(drains);
            self.bus.publish(LiveEvent::Idle {
                session_id: id.into(),
            });
            self.idle.notify_waiters();
        }
        again
    }

    fn descendants(&self, id: &str) -> Result<Vec<String>, RuntimeError> {
        let root = id.to_string();
        self.store
            .read(move |conn| {
                let snapshot = conn.unchecked_transaction()?;
                let mut seen = std::collections::HashSet::from([root.clone()]);
                let mut out = vec![root.clone()];
                let mut frontier = vec![root];
                while let Some(parent) = frontier.pop() {
                    let mut stmt =
                        snapshot.prepare("SELECT id FROM session WHERE parent_id = ?1")?;
                    let children: Vec<String> = stmt
                        .query_map([&parent], |r| r.get(0))?
                        .collect::<Result<_, _>>()?;
                    if children.iter().any(|child| !seen.insert(child.clone())) {
                        return Ok(None);
                    }
                    frontier.extend(children.iter().cloned());
                    out.extend(children);
                }
                snapshot.commit()?;
                Ok(Some(out))
            })?
            .ok_or_else(|| {
                RuntimeError::Corrupt("Session descendant hierarchy contains a cycle".into())
            })
    }
}
