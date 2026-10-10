//! Explicit user delegation shares child ownership without creating a parent Turn.
use super::delegations::Control;
use super::{Job, JobStatus, Runtime, RuntimeError};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct UserSubtask {
    pub skill_command: Option<super::SkillCommand>,
    /// Internal durable admission identity; public request bodies cannot set this.
    pub admission_id: Option<String>,
    pub prompt: String,
    pub agent: Option<String>,
    pub attachments: Vec<cyber_llm::Content>,
    pub max_steps: Option<u32>,
}

impl Runtime {
    pub async fn subtask(&self, session_id: &str, prompt: &str) -> Result<Job, RuntimeError> {
        self.subtask_with_agent(session_id, prompt, None).await
    }

    pub async fn subtask_with_agent(
        &self,
        session_id: &str,
        prompt: &str,
        agent: Option<String>,
    ) -> Result<Job, RuntimeError> {
        self.subtask_request(
            session_id,
            UserSubtask {
                skill_command: None,
                admission_id: None,
                prompt: prompt.into(),
                agent,
                attachments: Vec::new(),
                max_steps: None,
            },
        )
        .await
    }

    pub async fn subtask_request(
        &self,
        session_id: &str,
        request: UserSubtask,
    ) -> Result<Job, RuntimeError> {
        self.subtask_request_owned(session_id, request, CancellationToken::new())
            .await
    }

    /// Keep awaiting this operation after cancelling its owner so creation/setup
    /// settlement is acknowledged. A returned Job transfers ownership to the caller.
    pub async fn subtask_request_owned(
        &self,
        session_id: &str,
        request: UserSubtask,
        owner: CancellationToken,
    ) -> Result<Job, RuntimeError> {
        if owner.is_cancelled() {
            return Err(RuntimeError::Invalid("Subtask interrupted".into()));
        }
        if request.max_steps == Some(0) {
            return Err(RuntimeError::Invalid(
                "Subtask max_steps must be positive".into(),
            ));
        }
        if request.prompt.trim().is_empty() {
            return Err(RuntimeError::Invalid(
                "subtask prompt must not be empty".into(),
            ));
        }
        let identity = cyber_core::ids::new_id("op");
        let (send, receive) = tokio::sync::oneshot::channel();
        let (accepted, acceptance) = tokio::sync::oneshot::channel();
        let control = self
            .start_owned_subtask(
                session_id,
                &identity,
                request,
                owner.clone(),
                ReplyChannel { send, acceptance },
            )
            .await?;
        let on_disposal = control.stop.clone().drop_guard();
        self.watch_subtask_owner(session_id, &identity, owner.clone(), control.clone());
        let result = receive.await.map_err(|_| {
            RuntimeError::Invalid(format!("Subtask owner was lost; reconcile {identity}"))
        })?;
        if owner.is_cancelled() {
            control.stop.cancel();
        }
        let _ = accepted.send(());
        control.done.cancelled().await;
        on_disposal.disarm();
        let job = result.and_then(|job| self.job(&job.id))?;
        if owner.is_cancelled() && job.status == JobStatus::Running {
            return Err(RuntimeError::Invalid(format!(
                "Subtask cancellation was not acknowledged; inspect {}",
                job.id
            )));
        }
        Ok(job)
    }

    fn watch_subtask_owner(
        &self,
        parent: &str,
        identity: &str,
        owner: CancellationToken,
        control: Arc<Control>,
    ) {
        let weak = self.downgrade();
        let parent = parent.to_owned();
        let identity = identity.to_owned();
        let task = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = owner.cancelled() => control.stop.cancel(),
                _ = control.stop.cancelled() => {},
                _ = control.done.cancelled() => return,
            }
            if control.done.is_cancelled() {
                return;
            }
            if let Some(runtime) = weak.upgrade() {
                let _ = runtime.cancel_delegation(&parent, &identity).await;
            }
        });
        let mut tasks = self
            .inner
            .background
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tasks.retain(|task| !task.is_finished());
        tasks.push(task);
    }
}

pub(super) struct ReplyChannel {
    pub send: tokio::sync::oneshot::Sender<Result<Job, RuntimeError>>,
    pub acceptance: tokio::sync::oneshot::Receiver<()>,
}

pub(super) async fn deliver_reply(
    runtime: &Runtime,
    parent: &str,
    identity: &str,
    result: Result<Job, RuntimeError>,
    reply: ReplyChannel,
    control: &Control,
) -> Result<(), RuntimeError> {
    let job = result.as_ref().ok().cloned();
    let sent = reply.send.send(result).is_ok();
    let accepted = if sent {
        tokio::select! {
            biased;
            _ = control.stop.cancelled() => false,
            accepted = reply.acceptance => accepted.is_ok(),
        }
    } else {
        false
    };
    if !accepted && let Some(job) = job {
        // finish_delegation already verified the recorded parent and child identities.
        if job.session_id == parent {
            runtime
                .finish_delegation(parent, identity, Ok(job), true)
                .await?;
        }
    }
    Ok(())
}
