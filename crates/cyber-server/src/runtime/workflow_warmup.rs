//! Prompt-cache warm-up for fan-out (`workflows` → Prompt-cache warm-up for fan-out).
//!
//! Agents of a run that share a cacheable prefix (the same agent profile, model and result
//! schema, so the same system prompt and tools) are grouped by that prefix. The first agent
//! of a group starts alone; the others wait until its child Session's first provider
//! response begins, so they read the prefix the leader's request cached. The group stays
//! released for the rest of the attempt. A leader that settles, fails or is cancelled
//! before responding releases the group too, so nothing waits on it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cyber_store::Store;
use rusqlite::OptionalExtension;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{Notify, broadcast};

use super::LiveEvent;

/// One prefix group of a run.
#[derive(Default)]
pub(super) struct Warmup {
    released: AtomicBool,
    notify: Notify,
}

impl Warmup {
    pub(super) fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub(super) fn is_released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }

    /// Resolves once the group is released.
    pub(super) async fn released(&self) {
        loop {
            let notified = self.notify.notified();
            futures::pin_mut!(notified);
            notified.as_mut().enable();
            if self.is_released() {
                return;
            }
            notified.await;
        }
    }
}

/// Releases its group when dropped: a leader's call ended, however it ended.
pub(super) struct Release(pub Arc<Warmup>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// What an agent call is in its prefix group.
pub(super) enum Role {
    /// The first of its group: starts at once and releases the others.
    Leader(Arc<Warmup>),
    /// Waits for the leader's first response.
    Follower(Arc<Warmup>),
}

/// The prefix groups of one attempt.
#[derive(Default)]
pub(super) struct Groups(std::cell::RefCell<HashMap<String, Arc<Warmup>>>);

impl Groups {
    /// The role of a new agent call with this prefix.
    pub(super) fn join(
        &self,
        agent: Option<&str>,
        model: Option<&str>,
        schema: Option<&Value>,
    ) -> Role {
        let mut hash = Sha256::new();
        for part in [agent.unwrap_or("general"), model.unwrap_or("")] {
            hash.update(part.as_bytes());
            hash.update([0]);
        }
        hash.update(schema.map(Value::to_string).unwrap_or_default().as_bytes());
        let key = format!("{:x}", hash.finalize());
        let mut groups = self.0.borrow_mut();
        match groups.get(&key) {
            Some(group) => Role::Follower(Arc::clone(group)),
            None => {
                let group = Arc::new(Warmup::default());
                groups.insert(key, Arc::clone(&group));
                Role::Leader(group)
            }
        }
    }
}

/// Release `group` at the first provider output of the owner's child Session named
/// `agent_id`: a text, reasoning or tool-input delta, or usage. Ends when the group is
/// released some other way or the bus closes.
pub(super) async fn release_on_first_response(
    mut events: broadcast::Receiver<LiveEvent>,
    store: Arc<Store>,
    owner: String,
    agent_id: String,
    group: Arc<Warmup>,
) {
    let mut others: HashSet<String> = HashSet::new();
    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            () = group.released() => return,
        };
        let session = match event {
            Ok(event) => match responding(&event) {
                Some(session) => session.to_owned(),
                None => continue,
            },
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        if others.contains(&session) {
            continue;
        }
        if is_child(&store, &session, &owner, &agent_id) {
            group.release();
            return;
        }
        others.insert(session);
    }
}

/// The Session of a live event that shows a provider response has begun.
fn responding(event: &LiveEvent) -> Option<&str> {
    match event {
        LiveEvent::TextDelta { session_id, .. }
        | LiveEvent::ReasoningDelta { session_id, .. }
        | LiveEvent::ToolInputDelta { session_id, .. }
        | LiveEvent::Usage { session_id, .. } => Some(session_id),
        _ => None,
    }
}

fn is_child(store: &Store, session: &str, owner: &str, agent_id: &str) -> bool {
    let (session, owner, agent) = (session.to_owned(), owner.to_owned(), agent_id.to_owned());
    store
        .read(move |db| {
            Ok(db
                .query_row(
                    "SELECT 1 FROM session WHERE id = ?1 AND parent_id = ?2 AND subagent_name = ?3",
                    [session, owner, agent],
                    |_| Ok(()),
                )
                .optional()?
                .is_some())
        })
        .unwrap_or(false)
}
