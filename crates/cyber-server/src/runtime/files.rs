//! Permission decisions for the file browsing routes (`server-api` → File browsing
//! routes): the Session's rules and Mode, or the Location's rules without a Session.

use std::path::{Path, PathBuf};

use super::{FileDecision, Runtime, RuntimeError, TurnContext};

impl Runtime {
    /// The context file routes decide in: the Session's, or a Session-less one for the
    /// Location in `default` mode.
    pub async fn file_turn(
        &self,
        session_id: Option<&str>,
        directory: &Path,
    ) -> Result<TurnContext, RuntimeError> {
        match session_id {
            Some(id) => {
                let info = self.state(id).await?.info;
                Ok(super::drain::turn_context_for_info(&info, false))
            }
            None => Ok(TurnContext {
                session_id: String::new(),
                directory: directory.display().to_string(),
                agent: String::new(),
                mode: "default".into(),
                prefers_apply_patch: false,
                rules: serde_json::Value::Null,
            }),
        }
    }

    /// `action` decisions for `paths`, without asking.
    pub async fn file_decisions(
        &self,
        turn: &TurnContext,
        action: &str,
        paths: &[PathBuf],
    ) -> Vec<FileDecision> {
        self.inner.tools.file_decisions(turn, action, paths).await
    }

    /// Authorize `action` on `path`, raising the normal permission request on `ask`; the
    /// call waits for the reply.
    pub async fn authorize_file(
        &self,
        turn: &TurnContext,
        action: &str,
        path: &Path,
    ) -> Result<(), String> {
        let asker = self
            .operation_asker(&turn.session_id)
            .await
            .map_err(|e| e.to_string())?;
        self.inner
            .tools
            .authorize_file(turn, action, path, asker, self.inner.closed.child_token())
            .await
    }
}
