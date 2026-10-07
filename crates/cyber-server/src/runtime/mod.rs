//! The durable session runtime (`session-runtime`).
//!
//! Input is admitted into a durable inbox before anything runs. A Drain per Session
//! promotes input at Safe Boundaries and runs Turns until nothing is eligible. Every fact
//! the model sees is committed before it is acted on, so a restart rebuilds state by replay.

mod ancestry;
pub use ancestry::AncestorAuthority;
mod auto;
mod bus;
mod compaction;
mod context;
mod drain;
mod events;
mod fork;
mod host;
mod jobs;
mod names;
mod subtask;
pub use jobs::{Job, JobAdmission, JobAttempt, JobStatus, JobUsage};
pub use names::ChildExecution;
mod location;
mod model;
mod requests;
mod rewind;
mod selection;
mod shutdown;
mod structured;
pub use structured::StructuredSchema;
mod title;
mod view;
mod worktree_output;
pub use worktree_output::{SessionSetupSink, SetupChannel, SetupUpdate};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};

use cyber_llm::{Content, RetryPolicy};
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreError, StoredEvent};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Notify, RwLock, broadcast};
use tokio_util::sync::CancellationToken;

pub use auto::{AutoDecision, AutoEffect, AutoReview};
pub use bus::LiveEvent;
pub use compaction::CompactionConfig;
pub use context::{ContextInputs, Observed as ContextObservation, base_prompt};
pub use events::{CompactionTrigger, registry as event_registry};
pub use host::{
    AgentInference, CatalogResolver, FileDiff, Invocation, LocationGuard, LocationLease,
    ModelResolver, NoSnapshots, NoTools, Reconciliation, ResolvedModel, RestoreError, Snapshot,
    Snapshots, ToolDef, ToolHost, ToolOutcome, TurnContext,
};
pub use model::{
    AssistantEntry, CallState, CallStatus, Delivery, Entry, InboxRow, InputStatus, RetrySafety,
    RevertState, RevertTarget, SessionInfo, SessionState, Sourced, StepSnapshot, TaskState, Totals,
};
pub use requests::{
    Asker, PendingKind, PendingRequest, PermissionAsk, PermissionReply, Question, QuestionOption,
    QuestionReply, RequestOrigin, RequestRoute,
};
pub use view::{INTERRUPTED, UNKNOWN};

use bus::Bus;
use events::*;

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
    #[error("ContextInitializationBlocked: {}", .0.join(", "))]
    ContextBlocked(Vec<String>),
    #[error("{0}")]
    Model(String),
    #[error("{0}")]
    Compaction(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("corrupt session history: {0}")]
    Corrupt(String),
    #[error("RewindConflictError: changed since the snapshot and cannot be merged: {}", .0.join(", "))]
    RewindConflict(Vec<String>),
}

pub struct RuntimeOptions {
    pub store: Arc<Store>,
    pub resolver: Arc<dyn ModelResolver>,
    pub tools: Arc<dyn ToolHost>,
    pub global_config_dir: PathBuf,
    pub shell: String,
    /// Load `CLAUDE.md` where a directory has no `AGENTS.md`.
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

#[derive(Debug, Clone, Default)]
pub struct CreateSession {
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
    pub title: Option<String>,
    /// Session ruleset in the `permissions` config shape.
    pub rules: Option<serde_json::Value>,
    pub max_steps: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct Admission {
    pub message_id: Option<String>,
    pub parts: Vec<Content>,
    pub delivery: Delivery,
    /// `user`, `session`, `channel`, `loop`, `goal`, `workflow` or `hook`.
    pub source: String,
    /// `false` admits without waking the Session.
    pub resume: bool,
}

impl Admission {
    pub fn text(text: impl Into<String>, delivery: Delivery) -> Self {
        Self {
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
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub directory: String,
    pub parent_id: Option<String>,
    pub model: String,
    pub archived: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub cost: f64,
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
}

struct DrainEntry {
    cancel: CancellationToken,
    follow_up: bool,
}

pub(crate) struct Inner {
    pub store: Arc<Store>,
    pub resolver: Arc<dyn ModelResolver>,
    pub tools: Arc<dyn ToolHost>,
    pub bus: Bus,
    pub options: RuntimeOptions,
    sessions: StdMutex<HashMap<String, Arc<Handle>>>,
    drains: StdMutex<HashMap<String, DrainEntry>>,
    idle: Notify,
    lifecycle: RwLock<()>,
    shutdown_lock: Mutex<()>,
    closed: CancellationToken,
    background: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
    jobs: StdMutex<HashMap<String, Arc<jobs::Control>>>,
    job_admission: Arc<Mutex<()>>,
    child_executions: StdMutex<HashMap<String, std::sync::Weak<Mutex<()>>>>,
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
            idle: Notify::new(),
            lifecycle: RwLock::new(()),
            shutdown_lock: Mutex::new(()),
            closed: CancellationToken::new(),
            background: StdMutex::default(),
            jobs: StdMutex::default(),
            job_admission: Arc::new(Mutex::new(())),
            child_executions: StdMutex::default(),
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

    pub fn subscribe(&self) -> broadcast::Receiver<LiveEvent> {
        self.inner.bus.subscribe()
    }

    /// Create a Session, or return the existing one unchanged when `id` is taken.
    pub async fn create_session(&self, req: CreateSession) -> Result<SessionInfo, RuntimeError> {
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
            return Ok(existing.state.lock().await.info.clone());
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
            parent_id: req.parent_id,
            subagent_name: req.subagent_name,
            agent: req.agent.unwrap_or_else(|| "build".into()),
            model: req.model,
            mode: req.mode.unwrap_or_else(|| "default".into()),
            created_ms: chrono::Utc::now().timestamp_millis(),
            archived: false,
            rules: req.rules.unwrap_or_default(),
            max_steps: req.max_steps,
        };
        self.ancestors(&info).await?;
        let defaults = if mode_is_default || selection.agent_default {
            self.inner
                .tools
                .agent_inference(&drain::turn_context_for_info(&info, false))
        } else {
            Ok(AgentInference::default())
        };
        let mode_default_pending = mode_is_default && defaults.is_err();
        if mode_is_default && let Ok(options) = &defaults {
            info.mode = options
                .permission_mode
                .clone()
                .unwrap_or_else(|| "default".into());
            selection::validate_mode(&info.mode)?;
        }
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
        self.inner
            .create(
                info,
                history,
                calls,
                req.fork_from,
                CreationOptions {
                    child_worktree_setup_pending: req.child_worktree_setup_pending,
                    child_worktree: req.child_worktree,
                    fork_context,
                    selection,
                    mode_default_pending,
                    output_schema: req.output_schema.map(|s| s.schema().clone()),
                },
            )
            .await
    }

    /// Durably admit input (`session-runtime` → Durable prompt admission, Exact-retry idempotency).
    pub async fn admit(
        &self,
        session_id: &str,
        admission: Admission,
    ) -> Result<Receipt, RuntimeError> {
        self.admit_attempt(session_id, admission, None).await
    }

    async fn admit_attempt(
        &self,
        session_id: &str,
        admission: Admission,
        attempt: Option<names::Resumed>,
    ) -> Result<Receipt, RuntimeError> {
        let _admission = self.inner.open().await?;
        let handle = self.inner.handle(session_id).await?;
        handle.state.lock().await.ensure_worktree_ready()?;
        self.inner.commit_staged_revert(&handle).await?;
        let message_id = admission
            .message_id
            .clone()
            .unwrap_or_else(|| cyber_core::ids::new_id("msg"));
        let digest = digest(&admission.parts, admission.delivery);
        let receipt = {
            let mut state = handle.state.lock().await;
            state.ensure_worktree_ready()?;
            if let Some(receipt) = existing_receipt(&state, &message_id, &digest)? {
                return Ok(receipt);
            }
            let payload = Admitted {
                message_id: message_id.clone(),
                parts: admission.parts,
                delivery: admission.delivery,
                source: admission.source,
                digest,
            };
            let mut events = Vec::new();
            if let Some(attempt) = attempt {
                events.push(event(events::RESUMED, &attempt));
            }
            events.push(event(ADMITTED, &payload));
            let stored = self.inner.commit_locked(&mut state, events)?;
            receipt_for(
                &state,
                &message_id,
                stored.last().expect("admission event").seq,
            )
        };
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
        self.update_input(session_id, message_id, InboxAction::Edited, parts, delivery)
            .await
    }

    /// Withdraw an unpromoted `queue` or `hold` row; it never reaches the model.
    pub async fn remove_input(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<(), RuntimeError> {
        self.update_input(session_id, message_id, InboxAction::Removed, None, None)
            .await
    }

    /// Release a held row as `steer` or `queue`, waking the Session.
    pub async fn release(
        &self,
        session_id: &str,
        message_id: &str,
        delivery: Delivery,
    ) -> Result<(), RuntimeError> {
        let _admission = self.inner.open().await?;
        if delivery == Delivery::Hold {
            return Err(RuntimeError::Invalid(
                "release a held input as steer or queue".into(),
            ));
        }
        self.update_input(
            session_id,
            message_id,
            InboxAction::Released,
            None,
            Some(delivery),
        )
        .await?;
        self.inner.start_drain(session_id, false);
        Ok(())
    }

    /// Refuse a held row; it is kept as `refused` and never promoted.
    pub async fn refuse(&self, session_id: &str, message_id: &str) -> Result<(), RuntimeError> {
        self.update_input(session_id, message_id, InboxAction::Refused, None, None)
            .await
    }

    async fn update_input(
        &self,
        session_id: &str,
        message_id: &str,
        action: InboxAction,
        parts: Option<Vec<Content>>,
        delivery: Option<Delivery>,
    ) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let row = state
            .input(message_id)
            .ok_or_else(|| RuntimeError::Invalid(format!("no inbox row {message_id}")))?;
        check_inbox_action(row, action)?;
        let payload = InboxUpdated {
            message_id: message_id.into(),
            action,
            parts,
            delivery,
        };
        self.inner
            .commit_locked(&mut state, vec![event(INBOX_UPDATED, &payload)])?;
        Ok(())
    }

    /// Start a Drain when idle, or record one coalesced follow-up when one is running.
    pub async fn wake(&self, session_id: &str) -> Result<(), RuntimeError> {
        let _admission = self.inner.open().await?;
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
        let _admission = self.inner.open().await?;
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
    pub async fn interrupt(&self, session_id: &str) -> Result<(), RuntimeError> {
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
        self.inner.abandon_requests(session_id).await?;
        Ok(())
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

    /// A copy of the Session's current state.
    pub async fn state(&self, session_id: &str) -> Result<SessionState, RuntimeError> {
        Ok(self
            .inner
            .handle(session_id)
            .await?
            .state
            .lock()
            .await
            .clone())
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
        let mut state = handle.state.lock().await;
        if !state.model_selection.agent_default && state.model_selection.reference == model {
            return Ok(());
        }
        let payload = Switched {
            from: state.info.model.clone(),
            to: model.into(),
            automatic: false,
        };
        self.inner
            .commit_locked(&mut state, vec![event(MODEL_SWITCHED, &payload)])?;
        Ok(())
    }

    pub async fn switch_agent(&self, session_id: &str, agent: &str) -> Result<(), RuntimeError> {
        self.switch(session_id, AGENT_SWITCHED, agent, |i| &i.agent)
            .await
    }

    pub async fn switch_mode(&self, session_id: &str, mode: &str) -> Result<(), RuntimeError> {
        selection::validate_mode(mode)?;
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

    pub async fn rename(&self, session_id: &str, title: &str) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        self.inner.commit_locked(
            &mut state,
            vec![event(
                RENAMED,
                &Titled {
                    title: title.into(),
                    usage: None,
                    cost: None,
                },
            )],
        )?;
        Ok(())
    }

    /// Archived Sessions are hidden from default lists but stay resumable.
    pub async fn archive(&self, session_id: &str, archived: bool) -> Result<(), RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        self.inner
            .commit_locked(&mut state, vec![event(ARCHIVED, &Archived { archived })])?;
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
            parent_id: None,
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

    /// Delete a Session, its child Sessions, inbox rows and history.
    pub async fn delete(&self, session_id: &str) -> Result<(), RuntimeError> {
        let _jobs = self.inner.job_admission.lock().await;
        self.inner.handle(session_id).await?;
        let ids = self.inner.descendants(session_id)?;
        if self.is_running(session_id) {
            return Err(RuntimeError::Busy(session_id.into()));
        }
        self.cancel_session_jobs(&ids).await?;
        if ids.iter().any(|id| self.is_running(id)) {
            return Err(RuntimeError::Busy(session_id.into()));
        }
        let doomed = ids.clone();
        self.inner.store.transaction(move |tx| {
            for id in &doomed {
                tx.execute("DELETE FROM event WHERE aggregate_id = ?1", [id])?;
                tx.execute("DELETE FROM event_sequence WHERE aggregate_id = ?1", [id])?;
                tx.execute("DELETE FROM session WHERE id = ?1", [id])?;
            }
            Ok(())
        })?;
        let mut sessions = self
            .inner
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for id in ids {
            sessions.remove(&id);
            self.inner
                .bus
                .publish(LiveEvent::Deleted { session_id: id });
        }
        Ok(())
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
            let last = sessions.last().expect("non-empty");
            format!("{}:{}", last.updated_at, last.id)
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

fn digest(parts: &[Content], delivery: Delivery) -> String {
    let body = serde_json::json!({ "parts": parts, "delivery": delivery });
    format!("sha256:{:x}", Sha256::digest(body.to_string()))
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
        "SELECT id, title, directory, parent_id, model, archived, created_at, updated_at, cost FROM session
         WHERE (?1 IS NULL OR directory = ?1)
           AND (?2 IS NULL OR parent_id = ?2)
           AND (?3 = 0 OR parent_id IS NULL)
           AND (?4 = 1 OR archived = 0)
           AND (?5 IS NULL OR instr(lower(title), lower(?5)) > 0)
           AND (updated_at < ?6 OR (updated_at = ?6 AND id < ?7) OR ?7 = '')
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
                limit
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
                })
            },
        )?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

struct CreationOptions {
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
        if events.is_empty() {
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
        Ok(self.handle(&id).await?.state.lock().await.info.clone())
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
        if self.closed.is_cancelled() {
            return;
        }
        if let Some(entry) = drains.get_mut(id) {
            entry.follow_up = true;
            return;
        }
        let cancel = CancellationToken::new();
        drains.insert(
            id.into(),
            DrainEntry {
                cancel: cancel.clone(),
                follow_up: false,
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
        Ok(self.store.read(move |conn| {
            let mut out = vec![root.clone()];
            let mut frontier = vec![root];
            while let Some(parent) = frontier.pop() {
                let mut stmt = conn.prepare("SELECT id FROM session WHERE parent_id = ?1")?;
                let children: Vec<String> = stmt
                    .query_map([&parent], |r| r.get(0))?
                    .collect::<Result<_, _>>()?;
                frontier.extend(children.iter().cloned());
                out.extend(children);
            }
            Ok(out)
        })?)
    }
}
