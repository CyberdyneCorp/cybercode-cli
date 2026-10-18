//! Sandbox denials and escalations (`sandbox` → Sandbox events and audit), recorded by
//! the tool host on the calling Session.

use serde_json::Value;

use super::events::event;
use super::{Asker, RuntimeError};

/// `{ session_id, call_id, operation, target }` of a command the OS sandbox refused.
pub const DENIED: &str = "sandbox.denied.1";
/// `{ session_id, call_id, command, justification }` of an approved escalation.
pub const ESCALATED: &str = "sandbox.escalated.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxEvent {
    Denied,
    Escalated,
}

impl SandboxEvent {
    pub fn kind(self) -> &'static str {
        match self {
            Self::Denied => DENIED,
            Self::Escalated => ESCALATED,
        }
    }
}

impl Asker {
    /// Record a sandbox event on this call's Session. Without a runtime (detached
    /// tests and one-off hosts) there is nothing to record.
    pub async fn record_sandbox(
        &self,
        kind: SandboxEvent,
        data: Value,
    ) -> Result<(), RuntimeError> {
        let Some(inner) = self.inner.upgrade() else {
            return Ok(());
        };
        let handle = inner.handle(&self.session_id).await?;
        inner
            .commit(&handle, vec![event(kind.kind(), &data)])
            .await?;
        Ok(())
    }
}
