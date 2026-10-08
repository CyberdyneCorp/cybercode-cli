//! Live events for attached clients: durable events as they commit, plus live-only
//! fragments (deltas, retries, usage) that are never persisted.

use cyber_llm::Usage;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveEvent {
    /// Transient hook diagnostics; raw IO is not persisted by the live bus.
    HookNotice {
        session_id: String,
        hook_id: String,
        message: String,
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
    /// `session.retry`: the attempt number and delay before it.
    Retry {
        session_id: String,
        attempt: u32,
        delay_ms: u64,
        error: String,
    },
    /// `session.usage`: usage of the last step and context-window utilization.
    Usage {
        session_id: String,
        usage: Usage,
        context_tokens: u64,
        context_limit: u64,
        utilization: f64,
    },
    /// A Drain stopped on an error the user must address.
    Error {
        session_id: String,
        kind: String,
        message: String,
    },
    Idle {
        session_id: String,
    },
    Deleted {
        session_id: String,
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
