//! Explicit user delegation shares child ownership without creating a parent Turn.
use super::{Job, Runtime, RuntimeError, TurnContext};

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
        if prompt.trim().is_empty() {
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
        self.inner
            .tools
            .subtask_with_agent(turn, prompt.into(), agent, self.inner.closed.child_token())
            .await
            .map_err(RuntimeError::Invalid)
    }
}
