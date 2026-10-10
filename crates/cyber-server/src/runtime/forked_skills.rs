//! Inbox forwarding retains retry identity without exposing forked instructions to the parent.
use super::events::{Promoted, SKILL_FORWARDED, event};
use super::model::{Delivery, InboxRow, SessionState};
use super::{
    Admission, Delegation, DelegationStatus, Handle, Inner, Runtime, RuntimeError, UserSubtask,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

impl Runtime {
    pub fn skill_command_delegation_id(session: &str, message: &str) -> String {
        format!(
            "op_skill_{:x}",
            Sha256::digest(format!("{session}\0{message}"))
        )
    }

    /// Terminal pre-admission outcomes use the inbox's existing retry fence.
    pub(super) async fn notify_fork_admission(
        &self,
        delegation: &Delegation,
    ) -> Result<bool, RuntimeError> {
        if delegation.job_id.is_some()
            || !matches!(
                delegation.status,
                DelegationStatus::Failed | DelegationStatus::Cancelled
            )
        {
            return Ok(false);
        }
        let message = format!("msg_skill_handback_{}", delegation.id);
        let state = self.state(&delegation.session_id).await?;
        if state.input(&message).is_some() {
            return Ok(false);
        }
        let Some(skill) = state.inbox.iter().find_map(|row| {
            (row.forwarded_skill
                && Self::skill_command_delegation_id(&delegation.session_id, &row.message_id)
                    == delegation.id)
                .then_some(row.skill_command.as_ref())
                .flatten()
        }) else {
            return Ok(false);
        };
        // Setup errors may include private instructions; expose only stable diagnostics.
        let mut admission = Admission::text(
            format!(
                "Forked skill admission handback: {}",
                serde_json::json!({"skill":skill.activation.name,"delegation_id":delegation.id,"status":delegation.status})
            ),
            Delivery::Queue,
        );
        admission.message_id = Some(message);
        admission.source = "session".into();
        self.admit(&delegation.session_id, admission).await?;
        Ok(true)
    }

    async fn dispatch_forwarded_skill(
        &self,
        session: &str,
        row: InboxRow,
    ) -> Result<bool, RuntimeError> {
        let id = Self::skill_command_delegation_id(session, &row.message_id);
        if let Some(delegation) = self.delegation(session, &id)? {
            if delegation.status == DelegationStatus::Unknown {
                return Err(RuntimeError::Invalid(format!(
                    "Forked skill admission needs reconciliation: {id}"
                )));
            }
            return self.notify_fork_admission(&delegation).await;
        }
        let prompt = row
            .parts
            .iter()
            .filter_map(|part| match part {
                cyber_llm::Content::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.start_delegation(
            session,
            &id,
            UserSubtask {
                agent: row
                    .skill_command
                    .as_ref()
                    .and_then(|skill| skill.agent.clone()),
                skill_command: row.skill_command,
                admission_id: None,
                prompt,
                attachments: Vec::new(),
                max_steps: None,
            },
        )
        .await?;
        Ok(true)
    }
}

impl Inner {
    fn claim_fork_commands(
        &self,
        state: &mut SessionState,
        continue_tools: bool,
    ) -> Result<bool, RuntimeError> {
        self.ensure_admission_open(&state.info.id)?;
        let mut pending: Vec<_> = state.pending(Delivery::Steer).collect();
        if pending.is_empty() && !continue_tools {
            pending.extend(state.pending(Delivery::Queue).next());
        }
        let forks: Vec<_> = pending
            .into_iter()
            .filter(|row| row.skill_command.as_ref().is_some_and(|skill| skill.fork))
            .cloned()
            .collect();
        for row in &forks {
            if let Some(model) = row
                .skill_command
                .as_ref()
                .and_then(|skill| skill.model.as_ref())
            {
                self.resolve_selection(
                    &state.info,
                    &super::selection::ModelSelection::explicit(model.clone()),
                )?;
            }
        }
        if forks.is_empty() {
            return Ok(false);
        }
        self.commit_locked(
            state,
            forks
                .iter()
                .map(|row| {
                    event(
                        SKILL_FORWARDED,
                        &Promoted {
                            message_id: row.message_id.clone(),
                        },
                    )
                })
                .collect(),
        )?;
        Ok(true)
    }

    pub(super) async fn forward_skill_commands(
        self: &Arc<Self>,
        handle: &Arc<Handle>,
        continue_tools: bool,
        cancel: &CancellationToken,
    ) -> Result<bool, RuntimeError> {
        if cancel.is_cancelled() {
            return Ok(false);
        }
        let (session, mut dispatched, rows) = {
            let mut state = handle.state.lock().await;
            let forwarded = self.claim_fork_commands(&mut state, continue_tools)?;
            (
                state.info.id.clone(),
                forwarded,
                state
                    .inbox
                    .iter()
                    .filter(|row| row.forwarded_skill)
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        };
        let runtime = Runtime {
            inner: self.clone(),
        };
        for row in rows {
            if cancel.is_cancelled() {
                break;
            }
            dispatched |= runtime.dispatch_forwarded_skill(&session, row).await?;
        }
        Ok(dispatched)
    }
}
