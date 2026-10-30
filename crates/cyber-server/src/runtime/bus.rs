//! Live events for attached clients: durable events as they commit, plus live-only
//! fragments (deltas, retries, usage) that are never persisted.

use cyber_llm::Usage;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveEvent {
    MemoryUpdated {
        update: super::MemoryChange,
        seq: i64,
    },
    /// Committed independent MCP ownership observations, never Session events.
    McpStatusChanged {
        update: super::McpStatusUpdate,
        seq: i64,
    },
    /// `mcp.tools.changed.1`: a server's tools or prompts were published or refreshed;
    /// never durable.
    McpToolsChanged {
        update: super::McpToolsChanged,
    },
    /// Transient hook diagnostics; raw IO is not persisted by the live bus.
    HookNotice {
        session_id: String,
        hook_id: String,
        message: String,
    },
    /// A hook receipt was admitted or settled: an internal summary for the telemetry
    /// exporter, with no IO. Every client stream drops it, and no API schema has it.
    #[schemars(skip)]
    HookReceipt {
        session_id: String,
        receipt: super::HookReceipt,
    },
    /// A child request notification on an ancestor's live stream, never its history.
    RequestRouted {
        session_id: String,
        kind: String,
        data: Value,
    },
    WorktreeSetup {
        session_id: String,
        worktree_id: String,
        call_id: String,
        update: super::SetupUpdate,
    },
    Durable {
        session_id: String,
        seq: i64,
        kind: String,
        data: Value,
    },
    TextDelta {
        session_id: String,
        message_id: String,
        text: String,
    },
    ReasoningDelta {
        session_id: String,
        message_id: String,
        text: String,
    },
    ToolInputDelta {
        session_id: String,
        call_id: String,
        arguments: String,
    },
    /// `session.tool.progress`: a dispatched call's live status, never durable. The
    /// `agent` tool reports `queued` while waiting for an `agents.max_concurrent` slot
    /// and `running` once it holds one.
    ToolProgress {
        session_id: String,
        call_id: String,
        status: String,
    },
    /// `job.output.1`: output a process Job wrote, never durable (`background-tasks` →
    /// Background job events). The full output is in the Job's log file.
    JobOutput {
        session_id: String,
        job_id: String,
        text: String,
    },
    /// `session.retry`: the attempt number and delay before it. `discarded` means the
    /// failed attempt of `message_id` had streamed output: clients drop its live text,
    /// reasoning and tool input, since the request is replayed from the start.
    Retry {
        session_id: String,
        message_id: String,
        attempt: u32,
        delay_ms: u64,
        error: String,
        discarded: bool,
    },
    /// `session.usage`: usage of the last step and context-window utilization.
    Usage {
        session_id: String,
        usage: Usage,
        context_tokens: u64,
        context_limit: u64,
        utilization: f64,
        /// The context window meter after this step.
        context: super::ContextMeter,
    },
    /// A Drain stopped on an error the user must address.
    Error {
        session_id: String,
        kind: String,
        message: String,
    },
    /// `messaging.outcome`: the outcome of a message this Session sent to a peer changed
    /// (`released`, `dropped`, `expired`), next to its durable record; never durable.
    MessagingOutcome {
        session_id: String,
        message_id: String,
        /// The address the message was sent to (`X@P`).
        to: String,
        outcome: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Idle {
        session_id: String,
    },
    Deleted {
        session_id: String,
    },
    /// `workflow.run.progress`: a run's counts, spend, status or phase changed; never
    /// durable, at most every 250 ms per run, on the instance stream of `directory`.
    WorkflowProgress {
        directory: String,
        progress: super::WorkflowRunProgress,
    },
    /// `installation.update_available.1`: the background check found a newer release;
    /// never durable, on every instance stream.
    InstallationUpdateAvailable {
        notice: super::InstallationNotice,
    },
    /// `installation.updated.1`: a silent patch upgrade replaced the binary; never
    /// durable, on every instance stream.
    InstallationUpdated {
        notice: super::InstallationNotice,
    },
    /// `worktree.created.1`, `worktree.removed.1` or `worktree.kept.1`; never durable,
    /// on the instance stream of `directory`, the Location that asked.
    Worktree {
        directory: String,
        kind: String,
        worktree: super::WorktreeNotice,
    },
}

#[derive(Clone)]
pub struct Bus {
    sender: broadcast::Sender<LiveEvent>,
}

impl Bus {
    pub fn new() -> Self {
        Self {
            sender: broadcast::channel(4096).0,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LiveEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: LiveEvent) {
        if let LiveEvent::Error {
            session_id,
            kind,
            message,
        } = &event
        {
            cyber_core::log::error(
                "runtime",
                message,
                serde_json::json!({ "session_id": session_id, "kind": kind }),
            );
        }
        // No subscribers is fine.
        let _ = self.sender.send(event);
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}
