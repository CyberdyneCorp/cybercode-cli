//! Cross-session messages in the target inbox (design section 6): a held message is
//! released or dropped through the ordinary inbox routes, never edited. The change commits
//! with its `in` record (`messaging.message.released.1` / `.dropped.1`), and the sender's
//! `out` record follows (recovered at startup like a send). Deleting a Session drops its
//! held messages first and notifies their senders with `session_deleted`.

use std::sync::Arc;

use cyber_store::NewEvent;

use super::records::{DROPPED, Direction, MessageDropped, MessageReleased, RELEASED};
use crate::runtime::events::{INBOX_UPDATED, InboxAction, InboxUpdated, event};
use crate::runtime::{Delivery, Runtime, RuntimeError, SessionState};

/// The reason senders see when the target Session is deleted with their message held.
pub const SESSION_DELETED: &str = "session_deleted";

/// The `in` record a change to inbox row `message_id` commits with, when the row is a
/// received cross-session message: release and drop are recorded, an edit is refused
/// (another Session's words are released or dropped, never rewritten).
pub(crate) fn inbox_record(
    state: &SessionState,
    message_id: &str,
    action: InboxAction,
    delivery: Option<Delivery>,
) -> Result<Option<NewEvent>, RuntimeError> {
    if !state.message_chains.contains_key(message_id) {
        return Ok(None);
    }
    let record = match action {
        InboxAction::Edited => return Err(RuntimeError::MessageImmutable(message_id.into())),
        InboxAction::Released => event(
            RELEASED,
            &MessageReleased {
                message_id: message_id.into(),
                direction: Direction::In,
                delivery: delivery.unwrap_or(Delivery::Steer),
            },
        ),
        InboxAction::Removed | InboxAction::Refused => event(
            DROPPED,
            &MessageDropped {
                message_id: message_id.into(),
                direction: Direction::In,
                reason: None,
            },
        ),
    };
    Ok(Some(record))
}

impl Runtime {
    /// Drop every held message of the Sessions `ids` with reason `session_deleted`, then
    /// notify their senders. Returns the target send locks, held until the deletion
    /// ends, so no message is admitted after its held messages were dropped.
    pub(crate) async fn drop_held_messages(
        &self,
        ids: &[String],
    ) -> Result<Vec<tokio::sync::OwnedMutexGuard<()>>, RuntimeError> {
        let mut locks = Vec::with_capacity(ids.len());
        for id in ids {
            locks.push(Arc::clone(&self.message_lock(id)).lock_owned().await);
        }
        for id in ids {
            for message_id in self.drop_held(id).await? {
                self.complete_sent(Some(&message_id)).await?;
            }
        }
        Ok(locks)
    }

    /// Remove a Session's held messages from its inbox with their `dropped` records, in
    /// one commit. Returns their IDs.
    async fn drop_held(&self, session_id: &str) -> Result<Vec<String>, RuntimeError> {
        let handle = self.inner.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        let held: Vec<String> = state
            .inbox
            .iter()
            .filter(|row| row.status == crate::runtime::InputStatus::Held)
            .filter(|row| state.message_chains.contains_key(&row.message_id))
            .map(|row| row.message_id.clone())
            .collect();
        if held.is_empty() {
            return Ok(held);
        }
        let events = held
            .iter()
            .flat_map(|message_id| {
                let removed = InboxUpdated {
                    message_id: message_id.clone(),
                    action: InboxAction::Removed,
                    parts: None,
                    delivery: None,
                    hook_context: None,
                    hook_stop: None,
                    resources: Vec::new(),
                    untrusted: None,
                };
                let dropped = MessageDropped {
                    message_id: message_id.clone(),
                    direction: Direction::In,
                    reason: Some(SESSION_DELETED.into()),
                };
                [event(INBOX_UPDATED, &removed), event(DROPPED, &dropped)]
            })
            .collect();
        self.inner.commit_locked(&mut state, events)?;
        Ok(held)
    }
}
