//! Moving a running foreground command to the background (`background-tasks` →
//! Background commands from the user: Ctrl+B). The tool that runs the command registers
//! a [`Handoff`] for its call; the user's request signals it, and the tool then hands its
//! process to a process Job and settles the call with the Job ID.
use std::sync::PoisonError;

use tokio_util::sync::CancellationToken;

use super::{Runtime, WeakRuntime};

pub(super) struct Entry {
    call_id: String,
    token: CancellationToken,
}

/// A registered foreground call; dropping it withdraws the offer.
pub struct Handoff {
    runtime: WeakRuntime,
    session_id: String,
    call_id: String,
    token: CancellationToken,
}

impl Handoff {
    /// Cancelled when the user asks to move this call to the background.
    pub fn requested(&self) -> &CancellationToken {
        &self.token
    }
}

impl Drop for Handoff {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.upgrade() else {
            return;
        };
        let mut handoffs = runtime
            .inner
            .handoffs
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(entries) = handoffs.get_mut(&self.session_id) {
            entries.retain(|entry| entry.call_id != self.call_id);
            if entries.is_empty() {
                handoffs.remove(&self.session_id);
            }
        }
    }
}

impl Runtime {
    /// Offer the foreground call `call_id` of `session_id` for a move to the background.
    pub fn offer_handoff(&self, session_id: &str, call_id: &str) -> Handoff {
        let token = CancellationToken::new();
        self.inner
            .handoffs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(session_id.to_owned())
            .or_default()
            .push(Entry {
                call_id: call_id.to_owned(),
                token: token.clone(),
            });
        Handoff {
            runtime: self.downgrade(),
            session_id: session_id.to_owned(),
            call_id: call_id.to_owned(),
            token,
        }
    }

    /// Ask every foreground command of the Session to move to the background and return
    /// the calls asked. Empty when none is running.
    pub fn request_handoff(&self, session_id: &str) -> Vec<String> {
        let handoffs = self
            .inner
            .handoffs
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        handoffs
            .get(session_id)
            .into_iter()
            .flatten()
            .filter(|entry| !entry.token.is_cancelled())
            .map(|entry| {
                entry.token.cancel();
                entry.call_id.clone()
            })
            .collect()
    }
}
