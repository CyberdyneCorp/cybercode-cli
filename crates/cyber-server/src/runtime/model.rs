//! Session state as a fold of durable events. Everything the runtime decides is derived
//! from this state, so a restart rebuilds exactly what was acknowledged.

use std::collections::{BTreeMap, BTreeSet};

use cyber_llm::{Content, Usage};
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
    pub parent_id: Option<String>,
    /// Durable child identity within its parent, independent of title and agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_name: Option<String>,
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
}

/// One entry of model-visible history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    User {
        id: String,
        parts: Vec<Content>,
        source: String,
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
    pub reasoning: String,
    pub signature: Option<String>,
    pub calls: Vec<String>,
    pub finished: bool,
    pub error: Option<String>,
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
    pub retry_safety: RetrySafety,
    pub attempt: u32,
    pub status: CallStatus,
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Epoch {
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub reminded_skills: std::collections::BTreeSet<String>,
    pub number: u32,
    pub baseline: String,
    pub snapshot: BTreeMap<String, String>,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prefix: Option<String>,
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
    #[serde(skip)]
    pub(crate) first_turn_started: bool,
    #[serde(skip)]
    pub(crate) child_requested_inputs: BTreeSet<String>,
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
    pub open_step: Option<String>,
    /// Last Turn mode, retained while its tool groups settle.
    pub turn_mode: Option<String>,
    #[serde(skip)]
    pub(crate) turn_agent: Option<String>,
    pub steps: Vec<StepSnapshot>,
    /// File diffs by user message.
    pub diffs: BTreeMap<String, Vec<super::host::FileDiff>>,
    pub revert: Option<RevertState>,
}

const MAX_INSTRUCTION_CHARS: usize = 2000;

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
            first_turn_started: false,
            child_requested_inputs: BTreeSet::new(),
            child_pause_seq: -1,
            child_continuation_error: None,
            child_continuation_unknown: false,
            child_worktree_setup_pending: false,
            child_worktree: None,
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
            open_step: None,
            turn_mode: None,
            turn_agent: None,
            steps: Vec::new(),
            diffs: BTreeMap::new(),
            revert: None,
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
            "session.prompt.promoted" => self.on_promoted(decode::<Promoted>(e)?, e.seq),
            _ => self.apply_runtime(kind, e)?,
        }
        Ok(())
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
            "session.step.started" => self.on_step_started(decode(e)?),
            "session.text.ended" | "session.reasoning.ended" => self.on_content(kind, decode(e)?),
            "session.tool.called" => self.on_called(decode(e)?),
            "session.tool.dispatched" => self.on_dispatched(decode(e)?),
            "session.tool.settled" => self.on_settled(decode(e)?),
            "session.step.ended" => self.on_step_ended(decode(e)?),
            "usage.recorded" => {
                let usage: super::events::AuxiliaryUsage = decode(e)?;
                self.add_hidden(&usage.usage, usage.cost);
            }
            "session.step.failed" => self.on_step_failed(decode(e)?),
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

    /// Drop a user message and everything after it. Unsettled and unknown-outcome calls stay
    /// in `calls` for recovery even though no entry projects them.
    fn truncate_from(&mut self, message_id: &str) {
        let Some(index) = self
            .entries
            .iter()
            .position(|e| matches!(e, Entry::User { id, .. } if id == message_id))
        else {
            return;
        };
        let removed: Vec<String> = self.entries[index..]
            .iter()
            .map(|e| e.id().to_string())
            .collect();
        self.entries.truncate(index);
        if self
            .compacted
            .as_ref()
            .is_some_and(|c| c.tail_start > index)
        {
            self.compacted = None;
            self.epoch_stale = true;
        }
        let boundary = self
            .inbox
            .iter()
            .find(|r| r.message_id == message_id)
            .map_or(i64::MAX, |r| r.admitted_seq);
        self.inbox.retain(|r| r.admitted_seq < boundary);
        self.calls.retain(|_, c| {
            !removed.contains(&c.message_id)
                || matches!(
                    c.status,
                    CallStatus::OutcomeUnknown | CallStatus::Dispatched
                )
        });
        self.result.rewind(&self.calls);
        self.steps.retain(|s| !removed.contains(&s.step_id));
        self.diffs.retain(|id, _| !removed.contains(id));
        self.task
            .instructions
            .retain(|i| !removed.contains(&i.message_id));
        if self
            .task
            .objective
            .as_ref()
            .is_some_and(|o| removed.contains(&o.message_id))
        {
            self.task.objective = self.task.instructions.first().cloned();
        }
        self.task.version += 1;
        self.steps_since_input = 0;
        self.open_step = None;
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
                if let Some(usage) = decision.usage {
                    self.add_hidden(&usage, decision.cost);
                }
            }
            "session.compaction.completed" => self.on_compacted(decode(e)?),
            "session.title.generated" | "session.renamed" => {
                let titled: Titled = decode(e)?;
                if let Some(usage) = titled.usage {
                    self.add_hidden(&usage, titled.cost);
                }
                self.info.title = titled.title;
                self.info.default_title = false;
            }
            "session.archived" => self.info.archived = decode::<Archived>(e)?.archived,
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
        self.inbox.push(InboxRow {
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
                self.inbox.remove(index);
            }
            InboxAction::Refused => {
                self.child_requested_inputs.remove(&u.message_id);
                self.inbox[index].status = InputStatus::Refused;
            }
            InboxAction::Edited | InboxAction::Released => {
                if u.action == InboxAction::Released && self.info.parent_id.is_some() {
                    self.child_requested_inputs.insert(u.message_id.clone());
                }
                let row = &mut self.inbox[index];
                if let Some(parts) = u.parts {
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

    fn on_promoted(&mut self, p: Promoted, seq: i64) {
        self.child_requested_inputs.remove(&p.message_id);
        let Some(row) = self.inbox.iter_mut().find(|r| r.message_id == p.message_id) else {
            return;
        };
        row.status = InputStatus::Promoted;
        row.promoted_seq = Some(seq);
        let text = text_of(&row.parts);
        let entry = Entry::User {
            id: row.message_id.clone(),
            parts: row.parts.clone(),
            source: row.source.clone(),
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
        self.entries.push(entry);
        self.result.rejected = false;
        self.steps_since_input = 0;
    }

    fn on_epoch(&mut self, e: EpochStarted) {
        self.epoch = Some(Epoch {
            reminded_skills: Default::default(),
            number: e.epoch,
            baseline: e.baseline,
            snapshot: e.snapshot,
            provider: e.provider,
            system_prefix: e.system_prefix,
        });
        self.epoch_stale = false;
    }

    fn on_context_updated(&mut self, c: ContextUpdated) {
        if let Some(epoch) = &mut self.epoch {
            epoch.snapshot = c.snapshot;
        }
        let epoch = self.epoch.as_ref().map(|e| e.number);
        self.entries.push(Entry::System {
            id: c.message_id,
            text: c.text,
            epoch,
        });
    }

    fn on_system(&mut self, s: SystemAdded) {
        self.entries.push(Entry::System {
            id: s.message_id,
            text: s.text,
            epoch: None,
        });
    }

    fn on_step_started(&mut self, s: StepStarted) {
        self.first_turn_started = true;
        self.turn_mode = Some(s.mode.unwrap_or_else(|| self.info.mode.clone()));
        self.turn_agent = Some(s.agent.unwrap_or_else(|| self.info.agent.clone()));
        self.open_step = Some(s.message_id.clone());
        self.steps.push(StepSnapshot {
            step_id: s.message_id.clone(),
            user_message_id: self.current_user_message().map(str::to_string),
            pre: s.snapshot.clone(),
            post: None,
        });
        self.entries.push(Entry::Assistant(AssistantEntry {
            id: s.message_id,
            provider: s.provider,
            model: s.model,
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
                a.reasoning = c.text;
                a.signature = c.signature;
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
                structured_output: None,
                call_id: c.call_id,
                message_id: c.message_id,
                name: c.name,
                arguments: c.arguments,
                input: c.input,
                retry_safety: c.retry_safety,
                attempt: 0,
                status: CallStatus::Called,
                output: None,
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
            }
            call.status = s.status;
            call.output = Some(s.output);
            call.structured_output = s.structured_output;
            self.result.settle(call);
        }
    }

    fn on_step_ended(&mut self, s: StepEnded) {
        if let Some(a) = self.assistant(&s.message_id) {
            a.finished = true;
        }
        self.open_step = None;
        self.totals.usage.add(&s.usage);
        self.totals.steps += 1;
        match s.cost {
            Some(cost) => self.totals.cost += cost,
            None => self.totals.unpriced_steps += 1,
        }
        self.steps_since_input = self.steps_since_input.saturating_add(1);
    }

    fn on_step_failed(&mut self, f: StepFailed) {
        if let Some(a) = self.assistant(&f.message_id) {
            a.error = Some(f.message);
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

    fn on_compacted(&mut self, c: CompactionCompleted) {
        self.add_hidden(&c.usage, c.cost);
        let tail_start = self
            .entries
            .iter()
            .position(|e| e.id() == c.tail_start_id)
            .unwrap_or(self.entries.len());
        self.compacted = Some(Compacted {
            summary_id: c.message_id,
            summary: c.summary,
            tail_start,
            strip_media: c.trigger == CompactionTrigger::Overflow,
        });
        self.epoch_stale = true;
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
