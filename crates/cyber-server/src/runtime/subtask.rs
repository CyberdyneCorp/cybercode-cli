//! Explicit user delegation shares child ownership without creating a parent Turn.
use super::{Job, JobStatus, Runtime, RuntimeError, TurnContext};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct UserSubtask {
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
        // Child admission takes its own lifecycle guard; do not hold a nested read
        // across queued admission when shutdown is waiting for the write guard.
        drop(self.inner.open().await?);
        let state = self.state(session_id).await?;
        let running = self.is_running(session_id);
        let turn = TurnContext {
            session_id: session_id.into(),
            directory: state.info.directory.clone(),
            agent: state.effective_agent(running).into(),
            mode: state.effective_mode(running).into(),
            prefers_apply_patch: false,
            rules: state.info.rules.clone(),
        };
        let stop = self.inner.closed.child_token();
        let _on_return = stop.clone().drop_guard();
        let watch = stop.clone();
        let caller = owner.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = caller.cancelled() => watch.cancel(),
                _ = watch.cancelled() => {},
            }
        });
        let result = self
            .inner
            .tools
            .subtask_request(turn, request, stop)
            .await
            .map_err(RuntimeError::Invalid);
        match result {
            Ok(job) if owner.is_cancelled() => {
                let recorded = self.job(&job.id)?;
                if recorded.session_id != session_id || recorded.child_id != job.child_id {
                    return Err(RuntimeError::Corrupt(
                        "Delegated Job ownership changed".into(),
                    ));
                }
                let settled = self.cancel_job(&job.id).await?;
                if settled.status == JobStatus::Running {
                    return Err(RuntimeError::Invalid(format!(
                        "Subtask cancellation was not acknowledged; inspect {}",
                        job.id
                    )));
                }
                Ok(settled)
            }
            result => result,
        }
    }
}
