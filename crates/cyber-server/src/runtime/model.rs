//! Session state as a fold of durable events. Everything the runtime decides is derived
//! from this state, so a restart rebuilds exactly what was acknowledged.

use std::collections::{BTreeMap, BTreeSet};

use cyber_llm::{Content, Opaque, OpaqueKind, Usage};
use cyber_store::StoredEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::events::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Promoted at the next Safe Boundary, even while the Drain continues.
    Steer,
    /// Promoted one at a time when the Session would otherwise go idle.
    Queue,
    /// Recorded but not promotable until released.
    Hold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum InputStatus {
    Pending,
    Held,
    Promoted,
    Refused,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InboxRow {
    #[serde(skip)]
    pub forwarded_skill: bool,
    #[serde(skip)]
    pub skill_command: Option<super::SkillCommand>,
    pub message_id: String,
    pub parts: Vec<Content>,
    pub delivery: Delivery,
    pub source: String,
    pub status: InputStatus,
    pub digest: String,
    pub admitted_seq: i64,
    pub promoted_seq: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionInfo {
    pub id: String,
    pub title: String,
    pub directory: String,
    /// Exact managed checkout incarnation admitted at creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    /// The unmanaged Location `exit_worktree` returns to, while the Session runs in
    /// a worktree it entered with `enter_worktree`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_origin: Option<String>,
    /// The Location a root Session in its own managed worktree was started from (a
    /// forked Goal, a teammate): Session listings of that Location include it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_directory: Option<String>,
    pub parent_id: Option<String>,
    /// Durable child identity within its parent, independent of title and agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_name: Option<String>,
    /// The Workflow Run (`run_` ID) whose `agent()` call created this child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
    pub agent: String,
    pub model: String,
    pub mode: String,
    pub created_ms: i64,
    #[serde(default)]
    pub archived: bool,
    /// The title is still the generated default.
    #[serde(default)]
    pub default_title: bool,
    /// Session ruleset (`permissions-modes`), in the `permissions` config shape.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub rules: Value,
    /// Step limit for this Session's Drains, on top of the runtime-wide limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<cyber_core::budget::Budget>,
    /// Session-level `messaging.inbound` override (`cross-session-messaging` → Inbound
    /// controls): it replaces the Location's default; sender patterns still win.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaging: Option<super::messaging::InboundOverride>,
    /// The team this Session is a member of (`agent-teams`), set at creation and never
    /// changed. Teammates are root Sessions, so they outlive the lead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<super::teams::TeamTag>,
    /// The Session's own output style (`system-context` → Output style selection);
    /// `None` follows the Location's configuration. Only primary Sessions have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_style: Option<String>,
}

/// One entry of model-visible history. Answers are about half of all entries, so boxing
/// them would cost an allocation each to save little memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum Entry {
    User {
        id: String,
        parts: Vec<Content>,
        source: String,
        /// Time of the durable promotion event (`session-runtime` → Assistant
        /// message timing); null only for entries recorded before it was kept.
        #[serde(default)]
        promoted_ms: Option<i64>,
    },
    Assistant(AssistantEntry),
    /// A Mid-Conversation System Message. Context updates belong to one epoch.
    System {
        id: String,
        text: String,
        epoch: Option<u32>,
    },
}

impl Entry {
    pub fn id(&self) -> &str {
        match self {
            Self::User { id, .. } | Self::System { id, .. } => id,
            Self::Assistant(a) => &a.id,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AssistantEntry {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub text: String,
    /// The text of every reasoning item, for display.
    pub reasoning: String,
    /// Reasoning items in stream order (`session-runtime` → Reasoning items).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_items: Vec<ReasoningBlock>,
    /// The one signature of an entry copied before reasoning items; read, never written.
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub signature: Option<String>,
    pub calls: Vec<String>,
    pub finished: bool,
    pub error: Option<String>,
    /// The `session.step.failed.1` kind, such as `interrupted`.
    #[serde(default)]
    pub error_kind: Option<String>,
    /// The agent pinned by `session.step.started.1`.
    #[serde(default)]
    pub agent: Option<String>,
    /// Time of `session.step.started.1`.
    #[serde(default)]
    pub started_ms: Option<i64>,
    /// Time of `session.step.ended.1` or `session.step.failed.1`; null while running.
    #[serde(default)]
    pub ended_ms: Option<i64>,
}

/// One reasoning item of an answer: its text, the provider state to replay with it, and
/// how many of the answer's tool calls the provider produced before it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReasoningBlock {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opaque: Option<Opaque>,
    #[serde(default)]
    pub after_calls: usize,
}

impl AssistantEntry {
    /// Apply one `session.reasoning.ended.1`. An event without `index` comes from before
    /// reasoning items: it is the answer's whole reasoning, ahead of its calls, and its
    /// `signature` belongs to the answer's provider and model.
    fn on_reasoning(&mut self, c: ContentEnded) {
        let legacy = c.index.is_none();
        let opaque = c.opaque.or_else(|| self.signed(c.signature));
        if legacy {
            self.reasoning_items.clear();
        }
        let after_calls = if legacy { 0 } else { self.calls.len() };
        self.reasoning_items.push(ReasoningBlock {
            text: c.text,
            opaque,
            after_calls,
        });
        self.reasoning = self.reasoning_text();
    }

    /// A copied entry from before reasoning items holds its reasoning as one item.
    fn upgrade_legacy_reasoning(&mut self) {
        let signature = self.signature.take();
        if self.reasoning_items.is_empty() && !self.reasoning.is_empty() {
            let opaque = self.signed(signature);
            self.reasoning_items.push(ReasoningBlock {
                text: self.reasoning.clone(),
                opaque,
                after_calls: 0,
            });
        }
    }

    fn signed(&self, signature: Option<String>) -> Option<Opaque> {
        signature.map(|data| Opaque {
            provider: self.provider.clone(),
            model: self.model.clone(),
            ..Opaque::new(OpaqueKind::Signature, data)
        })
    }

    fn reasoning_text(&self) -> String {
        let texts: Vec<&str> = self
            .reasoning_items
            .iter()
            .map(|item| item.text.as_str())
            .filter(|text| !text.is_empty())
            .collect();
        texts.join("\n\n")
    }

    /// Replace every opaque value with the redaction marker, for what leaves the store.
    pub fn redact_opaque(&mut self) {
        for item in &mut self.reasoning_items {
            if let Some(opaque) = &mut item.opaque {
                opaque.redact();
            }
        }
    }

    /// Drop every opaque value, for history from an untrusted document.
    pub fn strip_opaque(&mut self) {
        self.signature = None;
        self.reasoning_items.retain_mut(|item| {
            item.opaque = None;
            !item.text.is_empty()
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetrySafety {
    ReadOnly,
    Idempotent,
    Reconcile,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CallStatus {
    /// Recorded from the stream, not yet dispatched.
    Called,
    /// Dispatched; outcome not yet recorded.
    Dispatched,
    Ok,
    Error,
    /// Known not to have run to completion; safe to try again.
    Interrupted,
    /// Dispatched, and whether its side effects happened is unknown.
    OutcomeUnknown,
}

impl CallStatus {
    pub fn is_settled(self) -> bool {
        !matches!(self, Self::Called | Self::Dispatched)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CallState {
    #[serde(skip)]
    pub skill_activation: Option<cyber_core::skills::SkillActivation>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::events::present_value"
    )]
    pub structured_output: Option<Value>,
    pub call_id: String,
    pub message_id: String,
    pub name: String,
    pub arguments: String,
    pub input: Option<Value>,
    /// Provider state of this call (`session-runtime` → Reasoning items).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opaque: Option<Opaque>,
    pub retry_safety: RetrySafety,
    pub attempt: u32,
    pub status: CallStatus,
    pub output: Option<String>,
    /// The tool host's notes on the settled run (`builtin-tools` → apply_patch through
    /// the shell).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    /// Secrets hidden from `output` before it reached the model (`tool-registry` → Tool
    /// output redaction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redactions: Option<cyber_core::redact::Redactions>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Epoch {
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub reminded_skills: std::collections::BTreeSet<String>,
    /// Canonical paths of nested instruction files attached this epoch.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub reminded_instructions: std::collections::BTreeSet<String>,
    pub number: u32,
    pub baseline: String,
    pub snapshot: BTreeMap<String, String>,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prefix: Option<String>,
    /// See [`EpochStarted::family`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// See [`EpochStarted::prompt_version`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_version: Option<u32>,
}

impl Epoch {
    /// Whether this epoch's prefix still serves a Turn on `provider` with a `family`
    /// model and base prompt `version`. An epoch from before families compares by
    /// provider only, so a restart never rebuilds its prefix; one with a family but no
    /// version was built with version 1.
    pub(crate) fn serves(&self, provider: &str, family: &str, version: u32) -> bool {
        self.provider == provider
            && self
                .family
                .as_deref()
                .is_none_or(|built| built == family && self.prompt_version.unwrap_or(1) == version)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Compacted {
    pub summary_id: String,
    pub summary: String,
    /// Index into `entries` of the first retained entry.
    pub tail_start: usize,
    pub strip_media: bool,
}

/// A user statement with its source, kept outside generated prose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Sourced {
    pub message_id: String,
    pub text: String,
}

/// Durable task state (`compaction` → Durable task state), derived from promoted input.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskState {
    pub version: u32,
    pub objective: Option<Sourced>,
    /// Every user instruction, in order, with its source message ID.
    pub instructions: Vec<Sourced>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Totals {
    pub usage: Usage,
    pub cost: f64,
    /// Steps whose price was unknown (`unpriced`), so `cost` is a lower bound.
    pub unpriced_steps: u32,
    pub steps: u32,
}

/// What a rewind restores (`snapshots-checkpoints` → Three-phase revert).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RevertTarget {
    Code,
    Conversation,
    Both,
}

impl RevertTarget {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "code" => Some(Self::Code),
            "conversation" => Some(Self::Conversation),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    pub fn code(self) -> bool {
        self != Self::Conversation
    }
}

/// A staged revert: the conversation boundary and the trees involved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RevertState {
    /// The user message rewound to; it and everything after it go on commit.
    pub message_id: String,
    pub target: RevertTarget,
    /// The working tree before the first stage.
    pub baseline: Option<String>,
    /// The tree the working tree was restored to.
    pub applied: Option<String>,
    /// Unified diff from the baseline to the restored tree.
    pub diff: String,
}

/// The model and usage of the last finished step.
#[derive(Debug, Clone, PartialEq)]
pub struct LastStep {
    pub provider: String,
    pub model: String,
    pub usage: Usage,
}

/// Snapshots around one step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StepSnapshot {
    pub step_id: String,
    /// The user message the step answers.
    pub user_message_id: Option<String>,
    pub pre: Option<String>,
    pub post: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct SessionState {
    #[serde(skip)]
    pub(crate) loaded_tools: BTreeSet<String>,
    /// Pull requests this Session is linked to (`session.pr.linked.1`).
    #[serde(skip)]
    pub(crate) pull_requests: BTreeSet<u64>,
    /// Times of the newest notifications (`session.notification.sent.1`).
    #[serde(skip)]
    pub(crate) notifications: Vec<i64>,
    /// Files delivered to the user by ID (`session.files.sent.1`).
    #[serde(skip)]
    pub(crate) sent_files: BTreeMap<String, super::deliveries::SentFile>,
    /// Browser verification evidence, oldest first (`session.browser.evidence.1`).
    #[serde(skip)]
    pub(crate) browser_evidence: Vec<super::browser_evidence::BrowserEvidence>,
    #[serde(skip)]
    pub(crate) first_turn_started: bool,
    /// SessionStart's durable marker was replayed; it never runs again.
    #[serde(skip)]
    pub(crate) session_started: bool,
    /// The step count after which a SessionStart or UserPromptSubmit `continue: false`
    /// ends the Drain.
    #[serde(skip)]
    pub(crate) hook_stop_step: Option<u32>,
    #[serde(skip)]
    pub(crate) child_requested_inputs: BTreeSet<String>,
    /// UserPromptSubmit context of unpromoted rows, admitted just before each row.
    #[serde(skip)]
    pub(crate) hook_contexts: BTreeMap<String, String>,
    /// UserPromptSubmit stop requests of unpromoted rows, recorded with each row.
    #[serde(skip)]
    pub(crate) hook_stops: BTreeMap<String, String>,
    /// Resolved resource mentions of unpromoted rows, prepended to each row's parts.
    #[serde(skip)]
    pub(crate) resource_contexts: BTreeMap<String, Vec<String>>,
    #[serde(skip)]
    pub(super) child_pause_seq: i64,
    #[serde(skip)]
    pub(crate) child_continuation_error: Option<String>,
    #[serde(skip)]
    pub(crate) child_continuation_unknown: bool,
    #[serde(skip)]
    pub(super) child_worktree_setup_pending: bool,
    #[serde(skip)]
    pub(super) child_worktree: Option<cyber_core::worktrees::Managed>,
    /// The Session created the worktree it entered with `enter_worktree`.
    #[serde(skip)]
    pub(super) entered_created: bool,
    /// The checkout the Session entered with `enter_worktree` and has not exited; boxed,
    /// as the state is held across many awaits.
    #[serde(skip)]
    pub(super) entered_worktree: Option<Box<cyber_core::worktrees::Managed>>,
    #[serde(skip)]
    pub(crate) result: super::structured::ResultState,
    pub info: SessionInfo,
    #[serde(skip)]
    pub(crate) model_selection: super::selection::ModelSelection,
    #[serde(skip)]
    pub(crate) selection_revision: i64,
    #[serde(skip)]
    pub(crate) mode_default_pending: bool,
    pub last_seq: i64,
    pub inbox: Vec<InboxRow>,
    pub entries: Vec<Entry>,
    pub calls: BTreeMap<String, CallState>,
    pub epoch: Option<Epoch>,
    /// Set by compaction and switches; the next Turn renders a fresh baseline.
    pub epoch_stale: bool,
    pub compacted: Option<Compacted>,
    pub task: TaskState,
    pub totals: Totals,
    #[serde(skip)]
    pub children_usage: super::ChildrenUsage,
    pub steps_since_input: u64,
    /// A row the user wrote ([`USER_SOURCES`]) was promoted since the last finished step,
    /// so the next model call is the user's (`provider-catalog` → GitHub Copilot
    /// subscription provider). Derived from durable events on replay.
    #[serde(skip)]
    pub user_input_pending: bool,
    pub open_step: Option<String>,
    /// The origin of the last started step; its end, failure and in-Turn usage copy it.
    #[serde(skip)]
    pub(crate) step_origin: super::usage_origin::StepOrigin,
    /// Last Turn mode, retained while its tool groups settle.
    pub turn_mode: Option<String>,
    #[serde(skip)]
    pub(crate) turn_agent: Option<String>,
    /// The last finished step's request, for the context meter; cleared by compaction.
    /// Boxed: Session state is held in deeply nested futures, so it stays small.
    #[serde(skip)]
    pub last_step: Option<Box<LastStep>>,
    pub steps: Vec<StepSnapshot>,
    /// File diffs by user message.
    pub diffs: BTreeMap<String, Vec<super::host::FileDiff>>,
    pub revert: Option<RevertState>,
    /// The latest `session.recap.generated.1`.
    pub recap: Option<super::recap::Recap>,
    /// Sequence of the latest finished (ended or failed) step; 0 before the first.
    pub finished_seq: i64,
    /// Received cross-session messages by inbox `message_id`, with the depth and hops
    /// their promotion carries into the run that reads them.
    #[serde(skip)]
    pub(crate) message_chains: BTreeMap<String, super::messaging::MessageChain>,
    /// Chain hops of fired idle notices and of input the model caused (a Job's handback
    /// or Monitor line, a delegated prompt, a Workflow summary), by inbox `message_id`.
    pub(crate) notice_hops: BTreeMap<String, u32>,
    /// The Session's loops by `lop_` ID (`loops-scheduling`), folded from `loop.*` events.
    pub loops: BTreeMap<String, super::loops::LoopState>,
    /// Loops with a tick coalesced behind their running iteration (overlap `coalesce`);
    /// not durable: a restart ends the iteration and the missed-run policy applies.
    #[serde(skip)]
    pub(crate) loop_coalesced: BTreeSet<String>,
    /// The Session's Goals (`goals`), folded from `session.goal.*` events.
    pub goals: super::goals::Goals,
    /// Every projected message with its parent; `entries` is its active path.
    #[serde(skip)]
    pub(super) branches: super::branches::BranchTree,
}

const MAX_INSTRUCTION_CHARS: usize = 2000;
/// Input sources a person wrote, invoked or scheduled: prompts (TUI, HTTP, MCP `prompt`),
/// MCP server prompts the user ran as a slash command, and `/loop` iterations. Every
/// other source (`session` handbacks, monitors, messages, `channel`, `team`, `workflow`,
/// `hook`, `auto_override`, `shell`) is the agent's.
pub(crate) const USER_SOURCES: [&str; 3] = ["user", super::MCP_PROMPT_SOURCE, "loop"];
/// Digest prefix of a Stop-hook continuation row; public digests are `sha256:`.
pub(crate) const STOP_CONTINUATION_DIGEST: &str = "hook:Stop:";

impl SessionState {
    pub(super) fn ensure_worktree_ready(&self) -> Result<(), super::RuntimeError> {
        if self.child_continuation_unknown {
            return Err(super::RuntimeError::Invalid(
                "Child continuation outcome is unknown; recovery is required".into(),
            ));
        }
        if self.child_worktree_setup_pending {
            return Err(super::RuntimeError::Invalid(
                "Isolated child setup is incomplete; recovery is required".into(),
            ));
        }
        Ok(())
    }

    /// Provider completion precedes tool dispatch; the Turn remains active through its batch.
    pub fn active_skill_scopes(
        &self,
        message_id: Option<&str>,
    ) -> Vec<cyber_core::skills::SkillActivation> {
        let latest = self.entries.iter().rev().find_map(|entry| match entry {
            Entry::Assistant(assistant) => Some(assistant.id.as_str()),
            _ => None,
        });
        let Some(message) = message_id.or(latest) else {
            return Vec::new();
        };
        if latest != Some(message)
            || !self
                .calls
                .values()
                .any(|call| call.message_id == message && !call.status.is_settled())
        {
            return Vec::new();
        }
        let mut scopes: Vec<_> = self
            .calls
            .values()
            .filter(|call| call.status == CallStatus::Ok && call.message_id == message)
            .filter_map(|call| call.skill_activation.clone())
            .collect();
        if let Some(row) = self.current_command_row() {
            let first = self
                .steps
                .iter()
                .find(|step| step.user_message_id.as_deref() == Some(&row.message_id));
            if first.is_some_and(|step| step.step_id == message) {
                scopes.push(
                    row.skill_command
                        .as_ref()
                        .expect("command row")
                        .activation
                        .clone(),
                );
            }
        }
        scopes
    }

    fn current_command_row(&self) -> Option<&InboxRow> {
        let message = self.current_user_message()?;
        self.inbox
            .iter()
            .find(|row| row.message_id == message && row.skill_command.is_some())
    }

    pub(crate) fn boundary_command_model(&self, continue_tools: bool) -> Option<&str> {
        let pending = self.pending(Delivery::Steer).last().or_else(|| {
            (!continue_tools)
                .then(|| self.pending(Delivery::Queue).next())
                .flatten()
        });
        match pending {
            Some(row) => row.skill_command.as_ref()?.model.as_deref(),
            None => self.command_model(),
        }
    }

    /// A command model applies through its tool loop, then expires at its final text reply.
    pub fn command_model(&self) -> Option<&str> {
        let row = self.current_command_row()?;
        let user = self
            .entries
            .iter()
            .position(|entry| entry.id() == row.message_id)?;
        let last = self
            .entries
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, entry)| match entry {
                Entry::Assistant(assistant) if index > user => Some(assistant),
                _ => None,
            });
        if last.is_some_and(|assistant| assistant.finished && assistant.calls.is_empty()) {
            return None;
        }
        row.skill_command.as_ref()?.model.as_deref()
    }

    pub fn child_worktree_setup_pending(&self) -> bool {
        self.child_worktree_setup_pending
    }

    pub fn child_worktree(&self) -> Option<&cyber_core::worktrees::Managed> {
        self.child_worktree.as_ref()
    }

    pub fn output_schema(&self) -> Option<&super::StructuredSchema> {
        self.result.schema.as_ref()
    }

    pub fn structured_result(&self) -> Option<&Value> {
        self.result.returned.as_ref().map(|(_, value)| value)
    }

    pub fn structured_attempt_rejected(&self) -> bool {
        self.result.rejected
    }

    pub fn structured_result_error(&self) -> &str {
        self.result.error.as_ref().map_or(
            "return_result was not called with a valid result",
            |(_, error)| error.as_str(),
        )
    }
    /// Schemas selected in this Session; this does not grant execution permission.
    pub fn loaded_tool_names(&self) -> &BTreeSet<String> {
        &self.loaded_tools
    }

    pub fn new(info: SessionInfo) -> Self {
        Self {
            loaded_tools: BTreeSet::new(),
            pull_requests: BTreeSet::new(),
            notifications: Vec::new(),
            sent_files: BTreeMap::new(),
            browser_evidence: Vec::new(),
            first_turn_started: false,
            session_started: false,
            hook_stop_step: None,
            child_requested_inputs: BTreeSet::new(),
            hook_contexts: BTreeMap::new(),
            hook_stops: BTreeMap::new(),
            resource_contexts: BTreeMap::new(),
            child_pause_seq: -1,
            child_continuation_error: None,
            child_continuation_unknown: false,
            child_worktree_setup_pending: false,
            child_worktree: None,
            entered_created: false,
            entered_worktree: None,
            result: super::structured::ResultState::default(),
            model_selection: super::selection::ModelSelection::explicit(info.model.clone()),
            selection_revision: -1,
            mode_default_pending: false,
            info,
            last_seq: -1,
            inbox: Vec::new(),
            entries: Vec::new(),
            calls: BTreeMap::new(),
            epoch: None,
            epoch_stale: false,
            compacted: None,
            task: TaskState::default(),
            totals: Totals::default(),
            children_usage: Default::default(),
            steps_since_input: 0,
            user_input_pending: false,
            open_step: None,
            step_origin: Default::default(),
            turn_mode: None,
            turn_agent: None,
            last_step: None,
            steps: Vec::new(),
            diffs: BTreeMap::new(),
            revert: None,
            recap: None,
            finished_seq: 0,
            message_chains: BTreeMap::new(),
            notice_hops: BTreeMap::new(),
            loops: BTreeMap::new(),
            loop_coalesced: BTreeSet::new(),
            goals: Default::default(),
            branches: Default::default(),
        }
    }

    pub fn effective_mode(&self, running: bool) -> &str {
        if running {
            self.turn_mode.as_deref().unwrap_or(&self.info.mode)
        } else {
            &self.info.mode
        }
    }

    pub fn effective_agent(&self, running: bool) -> &str {
        if running {
            self.turn_agent.as_deref().unwrap_or(&self.info.agent)
        } else {
            &self.info.agent
        }
    }

    pub fn pending_mode(&self, running: bool) -> Option<&str> {
        (self.effective_mode(running) != self.info.mode).then_some(self.info.mode.as_str())
    }

    /// Rebuild from the full event history.
    pub fn replay(events: &[StoredEvent]) -> Result<Self, String> {
        let first = events.first().ok_or("session has no events")?;
        let created: Created = decode(first)?;
        let mut state = Self::new(created.info.clone());
        if let Some(selection) = created.selection {
            state.model_selection = selection;
        }
        state.mode_default_pending = created.mode_default_pending;
        state.result.schema = created
            .output_schema
            .map(super::StructuredSchema::new)
            .transpose()?;
        if let Some(context) = created.fork_context {
            state.epoch = context.epoch;
            state.epoch_stale = context.epoch_stale;
            state.compacted = context.compacted;
            state.task = context.task;
        }
        state.child_worktree_setup_pending = created.child_worktree_setup_pending;
        state.child_worktree = created.child_worktree;
        state.entries = created.history;
        for entry in &mut state.entries {
            if let Entry::Assistant(a) = entry {
                a.upgrade_legacy_reasoning();
            }
        }
        state.attach_history();
        state.calls = created
            .calls
            .into_iter()
            .map(|c| (c.call_id.clone(), c))
            .collect();
        state.last_seq = first.seq;
        for event in &events[1..] {
            state.apply(event)?;
        }
        Ok(state)
    }

    pub fn apply(&mut self, e: &StoredEvent) -> Result<(), String> {
        self.last_seq = e.seq;
        let kind = e
            .kind
            .rsplit_once('.')
            .map_or(e.kind.as_str(), |(base, _)| base);
        match kind {
            "session.tools.loaded" => {
                let loaded: super::deferred_tools::Loaded = decode(e)?;
                super::deferred_tools::validate_names(&loaded.names)?;
                self.loaded_tools.extend(loaded.names);
            }
            "session.child.input_paused" => {
                self.child_requested_inputs.clear();
                self.child_pause_seq = e.seq;
            }
            "session.prompt.admitted" => self.on_admitted(decode(e)?, e.seq),
            "session.inbox.updated" => self.on_inbox_updated(decode(e)?),
            "session.prompt.promoted" => self.on_promoted(decode::<Promoted>(e)?, e.seq, e.time_ms),
            "session.prompt.skill_forwarded" => {
                self.turn_mode = None;
                self.turn_agent = None;
                let promoted: Promoted = decode(e)?;
                self.child_requested_inputs.remove(&promoted.message_id);
                self.hook_contexts.remove(&promoted.message_id);
                self.hook_stops.remove(&promoted.message_id);
                self.resource_contexts.remove(&promoted.message_id);
                if let Some(row) = self
                    .inbox
                    .iter_mut()
                    .find(|row| row.message_id == promoted.message_id)
                {
                    row.forwarded_skill = true;
                    row.status = InputStatus::Promoted;
                    row.promoted_seq = Some(e.seq);
                }
            }
            _ if kind.starts_with("loop.") => super::loops::records::fold(&mut self.loops, e)?,
            _ if kind.starts_with("session.goal.") => {
                super::goals::records::fold(&mut self.goals, e)?;
            }
            _ if kind.starts_with("messaging.") => {
                super::messaging::records::fold(
                    &mut self.message_chains,
                    &mut self.notice_hops,
                    e,
                )?;
            }
            _ => self.apply_runtime(kind, e)?,
        }
        Ok(())
    }

    /// Held cross-session messages in the inbox, awaiting release or drop.
    pub fn held_messages(&self) -> usize {
        self.count_messages(InputStatus::Held)
    }

    /// Admitted cross-session messages waiting for promotion.
    pub fn queued_messages(&self) -> usize {
        self.count_messages(InputStatus::Pending)
    }

    /// The depth and hops of a received message, by inbox `message_id`.
    pub fn message_chain(&self, message_id: &str) -> Option<super::messaging::MessageChain> {
        self.message_chains.get(message_id).copied()
    }

    /// The chain hops an inbox row carries into the run that promotes it: a received
    /// message's, a fired idle notice's or an inherited chain's
    /// (`messaging.chain.inherited.1`); `None` for any other row.
    pub fn chain_hops(&self, message_id: &str) -> Option<u32> {
        self.message_chains
            .get(message_id)
            .map(|chain| chain.hops)
            .or_else(|| self.notice_hops.get(message_id).copied())
    }

    fn count_messages(&self, status: InputStatus) -> usize {
        self.inbox
            .iter()
            .filter(|row| row.status == status && self.message_chains.contains_key(&row.message_id))
            .count()
    }

    fn apply_runtime(&mut self, kind: &str, e: &StoredEvent) -> Result<(), String> {
        match kind {
            "session.worktree.rebound" => {
                let binding: super::names::Rebound = decode(e)?;
                if self.child_worktree.as_ref() != Some(&binding.from) {
                    return Err("Worktree rebound differs from the previous binding".into());
                }
                self.info.directory = binding.to.path.display().to_string();
                self.info.worktree_id = Some(binding.to.id.clone());
                self.child_worktree = Some(binding.to);
                self.child_worktree_setup_pending = true;
            }
            "session.worktree.entered" => {
                let entered: super::worktree_events::Entered = decode(e)?;
                if self.info.worktree_id.is_some() || self.info.worktree_origin.is_some() {
                    return Err("Worktree entered from a managed checkout".into());
                }
                self.info.directory = entered.to.path.display().to_string();
                self.info.worktree_id = Some(entered.to.id.clone());
                self.info.worktree_origin = Some(entered.from.path);
                self.entered_created = entered.created;
                self.entered_worktree = Some(Box::new(entered.to));
            }
            "session.worktree.exited" => {
                let exited: super::worktree_events::Exited = decode(e)?;
                if self.info.worktree_id.as_deref() != Some(&exited.from.id)
                    || self.info.worktree_origin.as_deref() != Some(&exited.to.path)
                {
                    return Err("Worktree exit differs from the entered checkout".into());
                }
                self.info.directory = exited.to.path;
                self.info.worktree_id = None;
                self.info.worktree_origin = None;
                self.entered_created = false;
                self.entered_worktree = None;
            }
            "session.worktree.setup_ready" => {
                let ready: super::names::SetupReady = decode(e)?;
                if self.info.worktree_id.as_deref() != Some(&ready.worktree_id) {
                    return Err("Setup acknowledgement differs from the current checkout".into());
                }
                self.child_worktree_setup_pending = false;
            }
            "session.context.epoch_started" => self.on_epoch(decode(e)?),
            "session.context.updated" => self.on_context_updated(decode(e)?),
            "session.system.added" => self.on_system(decode(e)?),
            "session.hooks.session_started" => {
                let marker: super::events::SessionStarted = decode(e)?;
                self.session_started = true;
                self.hook_stop_step = marker.stop_after_step;
            }
            "session.step.started" => self.on_step_started(decode(e)?, e.time_ms),
            "session.text.ended" | "session.reasoning.ended" => self.on_content(kind, decode(e)?),
            "session.tool.called" => self.on_called(decode(e)?),
            "session.tool.dispatched" => self.on_dispatched(decode(e)?),
            "session.tool.settled" => self.on_settled(decode(e)?),
            "session.step.ended" => {
                self.on_step_ended(decode(e)?, e.time_ms, e.data["usage_accounted"] != true);
                self.finished_seq = e.seq;
            }
            "usage.recorded" => {
                let usage: super::events::AuxiliaryUsage = decode(e)?;
                let before = self.totals.unpriced_steps;
                self.add_hidden(&usage.usage, usage.cost);
                if let Some(change) = e.data["reservation_unpriced_increment"].as_i64() {
                    self.totals.unpriced_steps = before.saturating_add_signed(
                        i32::try_from(change).unwrap_or(if change < 0 {
                            i32::MIN
                        } else {
                            i32::MAX
                        }),
                    );
                }
            }
            "session.step.failed" => {
                self.on_step_failed(decode(e)?, e.time_ms);
                self.finished_seq = e.seq;
            }
            _ => self.apply_meta(kind, e)?,
        }
        Ok(())
    }

    fn apply_snapshots(&mut self, kind: &str, e: &StoredEvent) -> Result<(), String> {
        match kind {
            "snapshot.taken" => {
                let s: SnapshotTaken = decode(e)?;
                if let Some(step) = self
                    .steps
                    .iter_mut()
                    .rev()
                    .find(|st| st.step_id == s.step_id)
                {
                    step.post = Some(s.tree);
                }
            }
            "session.diff" => {
                let d: DiffComputed = decode(e)?;
                self.diffs.insert(d.message_id, d.diffs);
            }
            "session.reverted" => self.on_reverted(decode(e)?),
            "session.pr.linked" => {
                if let Some(number) = e.data["number"].as_u64() {
                    self.pull_requests.insert(number);
                }
            }
            "session.notification.sent" => {
                super::deliveries::remember_notification(&mut self.notifications, e.time_ms);
            }
            "session.files.sent" => {
                let sent: super::deliveries::FilesSent = decode(e)?;
                self.sent_files
                    .extend(sent.files.into_iter().map(|f| (f.id.clone(), f)));
            }
            "session.browser.evidence" => self.browser_evidence.push(decode(e)?),
            "session.branch.checked_out" => self.on_checked_out(decode(e)?)?,
            "session.branch.labeled" => self.on_labeled(decode(e)?),
            "session.recap.generated" => {
                let recap: super::recap::RecapGenerated = decode(e)?;
                // Recaps stored before control characters were stripped stay safe.
                self.recap = Some(super::recap::Recap {
                    text: super::recap::printable(&recap.text),
                    model: recap.model,
                    through_seq: recap.through_seq,
                    generated_ms: e.time_ms,
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn on_reverted(&mut self, r: Reverted) {
        match r.phase {
            RevertPhase::Stage => {
                self.revert = Some(RevertState {
                    message_id: r.message_id,
                    target: r.target,
                    baseline: r.baseline,
                    applied: r.applied,
                    diff: r.diff,
                });
            }
            RevertPhase::Clear => self.revert = None,
            RevertPhase::Commit => {
                self.revert = None;
                if r.target != RevertTarget::Code {
                    self.truncate_from(&r.message_id);
                }
            }
        }
    }

    /// A committed conversation rewind: the user message and everything after it leave
    /// the active path and stay as an inactive branch, so the tip moves to its parent.
    /// Input admitted after the message is dropped, as before branches existed.
    fn truncate_from(&mut self, message_id: &str) {
        let Some(index) = self
            .entries
            .iter()
            .position(|e| matches!(e, Entry::User { id, .. } if id == message_id))
        else {
            return;
        };
        let boundary = self
            .inbox
            .iter()
            .find(|r| r.message_id == message_id)
            .map_or(i64::MAX, |r| r.admitted_seq);
        self.stash_from(index);
        self.inbox.retain(|r| r.admitted_seq < boundary);
    }

    /// The user message the next step answers.
    pub fn current_user_message(&self) -> Option<&str> {
        self.entries.iter().rev().find_map(|e| match e {
            Entry::User { id, .. } => Some(id.as_str()),
            _ => None,
        })
    }

    fn apply_meta(&mut self, kind: &str, e: &StoredEvent) -> Result<(), String> {
        if matches!(
            kind,
            "session.agent.switched" | "session.model.switched" | "session.mode.switched"
        ) {
            self.selection_revision = e.seq;
        }
        match kind {
            "session.child.continuation_settled" => {
                self.child_continuation_error = e.data["error"].as_str().map(str::to_owned);
                self.child_continuation_unknown = e.data["unknown"].as_bool().unwrap_or(false);
            }
            "session.subagent.resumed" => {
                self.child_continuation_error = None;
                self.child_continuation_unknown = false;
                let resumed: super::names::Resumed = decode(e)?;
                if let Some(name) = resumed.name {
                    self.info.subagent_name = Some(name);
                }
                if let Some(schema) = resumed.output_schema {
                    self.result.schema = Some(super::StructuredSchema::new(schema)?);
                }
                self.result.returned = None;
                self.result.error = None;
                self.result.rejected = false;
            }
            "permission.replied" if self.result.schema.is_some() => {
                if matches!(
                    serde_json::from_value::<super::PermissionReply>(e.data["reply"].clone()),
                    Ok(super::PermissionReply::Reject { message: None })
                ) {
                    self.result.rejected = true;
                }
            }
            "permission.auto_decided" => {
                let decision: super::auto::AutoDecision = decode(e)?;
                if let Some(usage) = decision.usage.filter(|_| e.data["usage_accounted"] != true) {
                    self.add_hidden(&usage, decision.cost);
                }
            }
            "session.compaction.completed" => {
                self.on_compacted(decode(e)?, e.data["usage_accounted"] != true)
            }
            "session.title.generated" | "session.renamed" => {
                let titled: Titled = decode(e)?;
                if let Some(usage) = titled.usage.filter(|_| e.data["usage_accounted"] != true) {
                    self.add_hidden(&usage, titled.cost);
                }
                self.info.title = titled.title;
                self.info.default_title = false;
            }
            "session.archived" => self.info.archived = decode::<Archived>(e)?.archived,
            "session.messaging.updated" => {
                self.info.messaging = decode::<super::events::MessagingUpdated>(e)?.messaging;
            }
            "session.agent.switched" => self.info.agent = decode::<Switched>(e)?.to,
            "session.model.switched" => {
                let switched: Switched = decode(e)?;
                self.info.model = switched.to.clone();
                if !switched.automatic {
                    self.model_selection = super::selection::ModelSelection::explicit(switched.to);
                }
            }
            "session.mode.switched" => {
                self.info.mode = decode::<Switched>(e)?.to;
                self.mode_default_pending = false;
            }
            "session.output_style.switched" => {
                self.info.output_style = decode::<super::events::StyleSwitched>(e)?.to;
            }
            // Started/failed compaction and other markers carry no state.
            _ => self.apply_snapshots(kind, e)?,
        }
        Ok(())
    }

    fn on_admitted(&mut self, a: Admitted, seq: i64) {
        if a.wake && self.info.parent_id.is_some() && a.delivery != Delivery::Hold {
            self.child_requested_inputs.insert(a.message_id.clone());
        }
        let status = if a.delivery == Delivery::Hold {
            InputStatus::Held
        } else {
            InputStatus::Pending
        };
        if let Some(context) = a.hook_context {
            self.hook_contexts.insert(a.message_id.clone(), context);
        }
        if let Some(stop) = a.hook_stop {
            self.hook_stops.insert(a.message_id.clone(), stop);
        }
        if !a.resources.is_empty() {
            self.resource_contexts
                .insert(a.message_id.clone(), a.resources);
        }
        self.inbox.push(InboxRow {
            forwarded_skill: false,
            skill_command: a.skill_command,
            message_id: a.message_id,
            parts: a.parts,
            delivery: a.delivery,
            source: a.source,
            status,
            digest: a.digest,
            admitted_seq: seq,
            promoted_seq: None,
        });
    }

    fn on_inbox_updated(&mut self, u: InboxUpdated) {
        let Some(index) = self.inbox.iter().position(|r| r.message_id == u.message_id) else {
            return;
        };
        match u.action {
            InboxAction::Removed => {
                self.child_requested_inputs.remove(&u.message_id);
                self.hook_contexts.remove(&u.message_id);
                self.hook_stops.remove(&u.message_id);
                self.resource_contexts.remove(&u.message_id);
                self.inbox.remove(index);
            }
            InboxAction::Refused => {
                self.child_requested_inputs.remove(&u.message_id);
                self.hook_contexts.remove(&u.message_id);
                self.hook_stops.remove(&u.message_id);
                self.resource_contexts.remove(&u.message_id);
                self.inbox[index].status = InputStatus::Refused;
            }
            InboxAction::Edited | InboxAction::Released => {
                if u.action == InboxAction::Released && self.info.parent_id.is_some() {
                    self.child_requested_inputs.insert(u.message_id.clone());
                }
                let row = &mut self.inbox[index];
                if let Some(parts) = u.parts {
                    // Edited parts were re-checked; their context replaces the original.
                    match u.hook_context {
                        Some(context) => self.hook_contexts.insert(u.message_id.clone(), context),
                        None => self.hook_contexts.remove(&u.message_id),
                    };
                    match u.hook_stop {
                        Some(stop) => self.hook_stops.insert(u.message_id.clone(), stop),
                        None => self.hook_stops.remove(&u.message_id),
                    };
                    self.resource_contexts.remove(&u.message_id);
                    if !u.resources.is_empty() {
                        self.resource_contexts
                            .insert(u.message_id.clone(), u.resources);
                    }
                    row.parts = parts;
                }
                if let Some(delivery) = u.delivery {
                    row.delivery = delivery;
                    row.status = if delivery == Delivery::Hold {
                        InputStatus::Held
                    } else {
                        InputStatus::Pending
                    };
                }
            }
        }
    }

    fn on_promoted(&mut self, p: Promoted, seq: i64, time_ms: i64) {
        self.child_requested_inputs.remove(&p.message_id);
        self.hook_contexts.remove(&p.message_id);
        // The Turn that consumes this row is the last one of the Drain.
        if self.hook_stops.remove(&p.message_id).is_some() {
            self.hook_stop_step = Some(self.totals.steps + 1);
        }
        let resources = self
            .resource_contexts
            .remove(&p.message_id)
            .unwrap_or_default();
        let Some(row) = self.inbox.iter_mut().find(|r| r.message_id == p.message_id) else {
            return;
        };
        row.status = InputStatus::Promoted;
        row.promoted_seq = Some(seq);
        let text = text_of(&row.parts);
        // Attached resources are untrusted user-turn data ahead of the prompt; the
        // task instructions keep only what the user wrote.
        let parts = resources
            .into_iter()
            .map(|text| Content::Text { text })
            .chain(row.parts.iter().cloned())
            .collect();
        let entry = Entry::User {
            id: row.message_id.clone(),
            parts,
            source: row.source.clone(),
            promoted_ms: Some(time_ms),
        };
        if row.source == "user" && !text.is_empty() {
            let sourced = Sourced {
                message_id: row.message_id.clone(),
                text: truncate(&text, MAX_INSTRUCTION_CHARS),
            };
            if self.task.objective.is_none() {
                self.task.objective = Some(sourced.clone());
            }
            self.task.instructions.push(sourced);
            self.task.version += 1;
        }
        // A Stop continuation keeps the step count so max_steps stays a ceiling.
        if !row.digest.starts_with(STOP_CONTINUATION_DIGEST) {
            self.steps_since_input = 0;
            self.user_input_pending |= USER_SOURCES.contains(&row.source.as_str());
        }
        self.push_entry(entry);
        self.result.rejected = false;
    }

    fn on_epoch(&mut self, e: EpochStarted) {
        self.epoch = Some(Epoch {
            reminded_skills: Default::default(),
            reminded_instructions: Default::default(),
            number: e.epoch,
            baseline: e.baseline,
            snapshot: e.snapshot,
            provider: e.provider,
            system_prefix: e.system_prefix,
            family: e.family,
            prompt_version: e.prompt_version,
        });
        self.epoch_stale = false;
    }

    fn on_context_updated(&mut self, c: ContextUpdated) {
        if let Some(epoch) = &mut self.epoch {
            epoch.snapshot = c.snapshot;
        }
        let epoch = self.epoch.as_ref().map(|e| e.number);
        self.push_entry(Entry::System {
            id: c.message_id,
            text: c.text,
            epoch,
        });
    }

    fn on_system(&mut self, s: SystemAdded) {
        self.push_entry(Entry::System {
            id: s.message_id,
            text: s.text,
            epoch: None,
        });
    }

    fn on_step_started(&mut self, s: StepStarted, time_ms: i64) {
        self.first_turn_started = true;
        self.turn_mode = Some(s.mode.unwrap_or_else(|| self.info.mode.clone()));
        let agent = s.agent.unwrap_or_else(|| self.info.agent.clone());
        self.turn_agent = Some(agent.clone());
        self.open_step = Some(s.message_id.clone());
        self.step_origin = s.origin;
        self.steps.push(StepSnapshot {
            step_id: s.message_id.clone(),
            user_message_id: self.current_user_message().map(str::to_string),
            pre: s.snapshot.clone(),
            post: None,
        });
        self.push_entry(Entry::Assistant(AssistantEntry {
            id: s.message_id,
            provider: s.provider,
            model: s.model,
            agent: Some(agent),
            started_ms: Some(time_ms),
            ..AssistantEntry::default()
        }));
    }

    fn assistant(&mut self, id: &str) -> Option<&mut AssistantEntry> {
        self.entries.iter_mut().rev().find_map(|e| match e {
            Entry::Assistant(a) if a.id == id => Some(a),
            _ => None,
        })
    }

    fn on_content(&mut self, kind: &str, c: ContentEnded) {
        let reasoning = kind == "session.reasoning.ended";
        if let Some(a) = self.assistant(&c.message_id) {
            if reasoning {
                a.on_reasoning(c);
            } else {
                a.text = c.text;
            }
        }
    }

    fn on_called(&mut self, c: ToolCalled) {
        if let Some(a) = self.assistant(&c.message_id) {
            a.calls.push(c.call_id.clone());
        }
        self.calls.insert(
            c.call_id.clone(),
            CallState {
                skill_activation: None,
                structured_output: None,
                call_id: c.call_id,
                message_id: c.message_id,
                name: c.name,
                arguments: c.arguments,
                input: c.input,
                opaque: c.opaque,
                retry_safety: c.retry_safety,
                attempt: 0,
                status: CallStatus::Called,
                output: None,
                metadata: None,
                redactions: None,
            },
        );
    }

    fn on_dispatched(&mut self, d: ToolDispatched) {
        if let Some(call) = self.calls.get_mut(&d.call_id) {
            call.status = CallStatus::Dispatched;
            call.attempt = d.attempt;
        }
    }

    fn on_settled(&mut self, s: ToolSettled) {
        if let Some(call) = self.calls.get_mut(&s.call_id) {
            if matches!(s.status, CallStatus::Ok | CallStatus::Error)
                && let Some(epoch) = &mut self.epoch
            {
                epoch.reminded_skills.extend(s.skill_reminders);
                epoch.reminded_instructions.extend(s.instruction_reminders);
            }
            call.skill_activation = if s.status == CallStatus::Ok {
                s.skill_activation
            } else {
                None
            };
            call.status = s.status;
            call.output = Some(s.output);
            call.structured_output = s.structured_output;
            call.metadata = s.metadata;
            call.redactions = s.redactions;
            self.result.settle(call);
        }
    }

    fn on_step_ended(&mut self, s: StepEnded, time_ms: i64, account_usage: bool) {
        if let Some(a) = self.assistant(&s.message_id) {
            a.finished = true;
            a.ended_ms = Some(time_ms);
            let (provider, model) = (a.provider.clone(), a.model.clone());
            self.last_step = Some(Box::new(LastStep {
                provider,
                model,
                usage: s.usage,
            }));
        }
        self.open_step = None;
        self.totals.steps += 1;
        if account_usage {
            self.add_hidden(&s.usage, s.cost);
        }
        self.steps_since_input = self.steps_since_input.saturating_add(1);
        self.user_input_pending = false;
    }

    fn on_step_failed(&mut self, f: StepFailed, time_ms: i64) {
        if let Some(a) = self.assistant(&f.message_id) {
            a.error = Some(f.message);
            a.error_kind = Some(f.kind);
            a.ended_ms = Some(time_ms);
        }
        self.open_step = None;
    }

    /// Usage of hidden calls (title, compaction, permission review) counts toward totals but not steps.
    fn add_hidden(&mut self, usage: &cyber_llm::Usage, cost: Option<f64>) {
        self.totals.usage.add(usage);
        match cost {
            Some(cost) => self.totals.cost += cost,
            None => self.totals.unpriced_steps += 1,
        }
    }

    fn on_compacted(&mut self, c: CompactionCompleted, account_usage: bool) {
        if account_usage {
            self.add_hidden(&c.usage, c.cost);
        }
        // An empty tail (the newest entry alone exceeded the budget) points past the end.
        let tail_start = match c.tail_start_id.as_str() {
            "" => self.entries.len(),
            id => match self.entries.iter().position(|e| e.id() == id) {
                Some(index) => index,
                // The summarized path is no longer active: fail closed, so a sibling's
                // summary never reaches the model, now or after a replay.
                None => return,
            },
        };
        self.compacted = Some(Compacted {
            summary_id: c.message_id,
            summary: c.summary,
            tail_start,
            strip_media: c.trigger == CompactionTrigger::Overflow,
        });
        self.record_summary();
        self.epoch_stale = true;
        self.last_step = None;
    }

    /// Pending (not held) inbox rows of one delivery, in admission order.
    pub fn pending(&self, delivery: Delivery) -> impl Iterator<Item = &InboxRow> {
        self.inbox
            .iter()
            .filter(move |r| r.status == InputStatus::Pending && r.delivery == delivery)
    }

    /// Calls whose outcome is unknown; mutating execution pauses until they are resolved.
    pub fn unresolved(&self) -> Vec<&CallState> {
        self.calls
            .values()
            .filter(|c| c.status == CallStatus::OutcomeUnknown)
            .collect()
    }

    pub fn input(&self, message_id: &str) -> Option<&InboxRow> {
        self.inbox.iter().find(|r| r.message_id == message_id)
    }
}

pub fn text_of(parts: &[Content]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Content::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

fn decode<T: serde::de::DeserializeOwned>(e: &StoredEvent) -> Result<T, String> {
    serde_json::from_value(e.data.clone()).map_err(|err| format!("{} seq {}: {err}", e.kind, e.seq))
}
