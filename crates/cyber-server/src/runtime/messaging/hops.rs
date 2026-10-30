//! Chain hops of message-driven runs (`cross-session-messaging` → Message chain limit;
//! design section 8). A run's hops is the largest `hops` among the messages and idle
//! notices promoted into it, and 0 for a run none joined; a message sent from it carries
//! one more. Hops live in memory next to the MCP depths, scoped to one run (Drain), and a
//! Drain that continues a run after a restart restores them from the promoted rows.
//!
//! Input the model causes inherits the hops of the run that caused it, recorded with
//! `messaging.chain.inherited.1` and read like a notice's: a background Job records its
//! handback's hops when it starts (its Monitor lines read them), and a prompt a parent
//! delegates to a subagent carries the parent's. So only user input restarts a chain at
//! 0, and a run past the limit cannot escape it through a Job or a subagent.

use std::sync::PoisonError;

use cyber_store::NewEvent;
use tokio_util::sync::CancellationToken;

use super::records::{CHAIN_INHERITED, ChainInherited};
use crate::runtime::events::event;
use crate::runtime::model::{InputStatus, SessionState};
use crate::runtime::{Admission, Inner, PromptCheck, Receipt, Runtime, RuntimeError};

impl Inner {
    /// The cancellation token that identifies the Session's running Drain.
    pub(crate) fn run_token(&self, session_id: &str) -> Option<CancellationToken> {
        self.drains
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session_id)
            .map(|entry| entry.cancel.clone())
    }

    /// Raise the hops of the Session's running Drain to at least `hops`; a no-op when the
    /// Session is idle.
    pub(crate) fn raise_message_hops(&self, session_id: &str, hops: u32) {
        let Some(run) = self.run_token(session_id) else {
            return;
        };
        let mut runs = self
            .message_hops
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let entry = runs
            .entry(session_id.to_owned())
            .or_insert_with(|| (run.clone(), 0));
        if entry.0 != run {
            *entry = (run, 0);
        }
        entry.1 = entry.1.max(hops);
    }

    /// The hops of the Session's running Drain (0 when idle or when no message joined it).
    pub(crate) fn run_message_hops(&self, session_id: &str) -> u32 {
        let Some(run) = self.run_token(session_id) else {
            return 0;
        };
        self.message_hops
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session_id)
            .filter(|(token, _)| token == &run)
            .map_or(0, |(_, hops)| *hops)
    }
}

/// The hops of the rows `ids` promotes, if any of them is a received message or notice.
pub(crate) fn promoted_hops(state: &SessionState, ids: &[String]) -> Option<u32> {
    ids.iter().filter_map(|id| state.chain_hops(id)).max()
}

/// The MCP depth the rows `ids` carry into the run that promotes them: the largest
/// sender depth among the received messages, if any (`mcp-server` → MCP recursion guard).
pub(crate) fn promoted_depth(state: &SessionState, ids: &[String]) -> Option<u32> {
    ids.iter()
        .filter_map(|id| state.message_chain(id))
        .map(|chain| chain.depth)
        .max()
}

/// The rows promoted into the run a Drain continues: the message and notice rows promoted
/// since the last promoted row that is neither, newest first.
fn continued_rows(state: &SessionState) -> impl Iterator<Item = &str> {
    let mut promoted: Vec<_> = state
        .inbox
        .iter()
        .filter(|row| row.status == InputStatus::Promoted)
        .filter_map(|row| row.promoted_seq.map(|seq| (seq, row.message_id.as_str())))
        .collect();
    promoted.sort_unstable_by_key(|(seq, _)| std::cmp::Reverse(*seq));
    promoted
        .into_iter()
        .map(|(_, id)| id)
        .take_while(|id| state.chain_hops(id).is_some())
}

/// The MCP depth of the run a Drain continues: the largest sender depth among the
/// messages promoted into it, so a resumed run keeps its recursion guard.
pub(crate) fn restored_depth(state: &SessionState) -> u32 {
    continued_rows(state)
        .filter_map(|id| state.message_chain(id))
        .map(|chain| chain.depth)
        .max()
        .unwrap_or(0)
}

/// The hops of the run a Drain continues: the largest `hops` among the message rows
/// promoted since the last promoted row that is not a message (user input starts a run
/// at 0), so a run cut short by a restart keeps its chain.
pub(crate) fn restored_hops(state: &SessionState) -> u32 {
    continued_rows(state)
        .filter_map(|id| state.chain_hops(id))
        .max()
        .unwrap_or(0)
}

/// The record that makes input `message_id` carry `hops` into the run that promotes it;
/// none at hop 0, where a run starts anyway.
pub(crate) fn inherited_chain(message_id: &str, hops: u32) -> Vec<NewEvent> {
    if hops == 0 {
        return Vec::new();
    }
    let inherited = ChainInherited {
        message_id: message_id.to_owned(),
        hops,
    };
    vec![event(CHAIN_INHERITED, &inherited)]
}

impl Runtime {
    /// The record giving a prompt delegated to `child` (a new or resumed subagent, a
    /// Workflow agent) the hops its ancestors' running Drains carry, or, for a Workflow
    /// agent, those its run was started with; it names the admission, which gets an ID
    /// here when it has none.
    pub(crate) async fn delegated_chain(
        &self,
        child: &str,
        admission: &mut Admission,
    ) -> Vec<NewEvent> {
        let hops = self
            .effective_message_hops(child)
            .max(self.workflow_message_hops(child).await);
        if hops == 0 {
            return Vec::new();
        }
        let id = admission
            .message_id
            .get_or_insert_with(|| cyber_core::ids::new_id("msg"));
        inherited_chain(id, hops)
    }

    /// The hops a background Job's input carries: those its handback was recorded with
    /// when the Job started (0 when it started outside a message chain).
    pub(crate) async fn job_message_hops(&self, session_id: &str, job_id: &str) -> u32 {
        let Ok(handle) = self.inner.handle(session_id).await else {
            return 0;
        };
        let state = handle.state.lock().await;
        state
            .chain_hops(&super::super::jobs::handback_id(job_id))
            .unwrap_or(0)
    }

    /// The hops the Workflow run `session_id` belongs to was started with (recorded on
    /// its summary's ID in the owner Session); 0 outside a run.
    async fn workflow_message_hops(&self, session_id: &str) -> u32 {
        let Ok(Some(run)) = self.workflow_run_of(session_id).await else {
            return 0;
        };
        let Ok(Some(owner)) = self.workflow_run(&run).map(|run| run.map(|r| r.session_id)) else {
            return 0;
        };
        let Ok(handle) = self.inner.handle(&owner).await else {
            return 0;
        };
        let state = handle.state.lock().await;
        state
            .chain_hops(&crate::runtime::workflow_results::handback_message_id(&run))
            .unwrap_or(0)
    }

    /// Record that input `message_id` the model caused in `session_id`, admitted later,
    /// carries the hops of the Session's running chain (a Workflow run's summary).
    pub(crate) async fn record_caused_input(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<(), RuntimeError> {
        let events = inherited_chain(message_id, self.effective_message_hops(session_id));
        if events.is_empty() {
            return Ok(());
        }
        let handle = self.inner.handle(session_id).await?;
        self.inner.commit(&handle, events).await?;
        Ok(())
    }

    /// Admit input the model caused at `hops` (a Monitor line): it carries them into the
    /// run that promotes it. Hop 0 is an ordinary admission.
    pub(crate) async fn admit_caused(
        &self,
        session_id: &str,
        mut admission: Admission,
        hops: u32,
    ) -> Result<Receipt, RuntimeError> {
        if hops == 0 {
            return self.admit(session_id, admission).await;
        }
        let id = admission
            .message_id
            .get_or_insert_with(|| cyber_core::ids::new_id("msg"))
            .clone();
        let check = if admission.source == "user" {
            PromptCheck::Run
        } else {
            PromptCheck::Checked(None)
        };
        self.admit_attempt_with(
            session_id,
            admission,
            None,
            check,
            inherited_chain(&id, hops),
        )
        .await
    }

    /// The hops of the Session's running Drain.
    pub fn run_message_hops(&self, session_id: &str) -> u32 {
        self.inner.run_message_hops(session_id)
    }

    /// The hops a message sent by the Session continues from: the largest running hops of
    /// the Session and its ancestors (a subagent runs inside its parent's run).
    pub fn effective_message_hops(&self, session_id: &str) -> u32 {
        std::iter::once(session_id.to_owned())
            .chain(self.ancestor_ids(session_id))
            .map(|id| self.run_message_hops(&id))
            .max()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::runtime::Delivery;
    use crate::runtime::messaging::MessageChain;
    use crate::runtime::model::{InboxRow, SessionInfo};

    fn state(
        rows: &[(&str, Option<i64>, Option<u32>)],
        notice: Option<(&str, u32)>,
    ) -> SessionState {
        let info: SessionInfo = serde_json::from_value(json!({
            "id": "ses_t", "title": "t", "directory": "/r", "parent_id": null,
            "agent": "build", "model": "test/main", "mode": "default", "created_ms": 1,
        }))
        .unwrap();
        let mut state = SessionState::new(info);
        for (id, promoted, hops) in rows {
            state.inbox.push(InboxRow {
                forwarded_skill: false,
                skill_command: None,
                message_id: (*id).into(),
                parts: Vec::new(),
                delivery: Delivery::Steer,
                source: "session".into(),
                status: if promoted.is_some() {
                    InputStatus::Promoted
                } else {
                    InputStatus::Pending
                },
                digest: String::new(),
                admitted_seq: 0,
                promoted_seq: *promoted,
                untrusted: None,
            });
            if let Some(hops) = hops {
                let chain = MessageChain {
                    depth: 0,
                    hops: *hops,
                };
                state.message_chains.insert((*id).into(), chain);
            }
        }
        if let Some((id, hops)) = notice {
            state.notice_hops.insert(id.into(), hops);
        }
        state
    }

    #[test]
    fn promotion_takes_the_largest_hops_of_messages_and_notices() {
        let state = state(
            &[
                ("msg_user", None, None),
                ("msgx_a", None, Some(2)),
                ("msgx_b", None, Some(5)),
            ],
            Some(("msgx_watch_wch_1", 7)),
        );
        let ids = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        assert_eq!(promoted_hops(&state, &ids(&["msg_user"])), None);
        assert_eq!(
            promoted_hops(&state, &ids(&["msg_user", "msgx_a", "msgx_b"])),
            Some(5)
        );
        assert_eq!(
            promoted_hops(&state, &ids(&["msgx_a", "msgx_watch_wch_1"])),
            Some(7)
        );
    }

    #[test]
    fn a_continued_run_restores_the_hops_promoted_since_the_last_user_input() {
        // Messages promoted after the user's prompt belong to the run being continued.
        let continued = state(
            &[
                ("msgx_old", Some(3), Some(8)),
                ("msg_user", Some(5), None),
                ("msgx_a", Some(7), Some(4)),
                ("msgx_b", Some(9), Some(2)),
                ("msgx_pending", None, Some(6)),
            ],
            None,
        );
        assert_eq!(restored_hops(&continued), 4);
        // A run the user's input joined last starts again at 0.
        let user_last = state(
            &[("msgx_a", Some(3), Some(4)), ("msg_user", Some(5), None)],
            None,
        );
        assert_eq!(restored_hops(&user_last), 0);
        assert_eq!(restored_hops(&state(&[], None)), 0);
    }

    #[test]
    fn promotion_and_a_continued_run_carry_the_largest_message_depth() {
        let mut continued = state(
            &[
                ("msgx_old", Some(3), Some(1)),
                ("msg_user", Some(5), None),
                ("msgx_a", Some(7), Some(1)),
                ("msgx_b", Some(9), Some(1)),
            ],
            Some(("msgx_watch_wch_1", 1)),
        );
        for (id, depth) in [("msgx_old", 3), ("msgx_a", 1), ("msgx_b", 2)] {
            continued.message_chains.get_mut(id).unwrap().depth = depth;
        }
        let ids = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        // Only received messages carry a depth; user input and notices carry none.
        assert_eq!(promoted_depth(&continued, &ids(&["msg_user"])), None);
        assert_eq!(
            promoted_depth(&continued, &ids(&["msg_user", "msgx_watch_wch_1"])),
            None
        );
        assert_eq!(
            promoted_depth(&continued, &ids(&["msgx_a", "msgx_b"])),
            Some(2)
        );
        // The depth promoted before the user's prompt belongs to an earlier run.
        assert_eq!(restored_depth(&continued), 2);
        let user_last = state(
            &[("msgx_a", Some(3), Some(1)), ("msg_user", Some(5), None)],
            None,
        );
        assert_eq!(restored_depth(&user_last), 0);
    }
}
