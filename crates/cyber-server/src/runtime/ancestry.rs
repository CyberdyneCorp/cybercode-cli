//! Read-only parent-chain validation; no child lock is held during ancestor lookup.

use super::{Runtime, RuntimeError, SessionInfo};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct AncestorAuthority {
    pub info: SessionInfo,
    pub effective_mode: String,
    pub effective_agent: String,
}

impl Runtime {
    /// Return immediate parent first, refusing missing parents and ancestry cycles.
    pub async fn ancestors(&self, info: &SessionInfo) -> Result<Vec<SessionInfo>, RuntimeError> {
        Ok(self
            .ancestor_authorities(info)
            .await?
            .into_iter()
            .map(|authority| authority.info)
            .collect())
    }

    /// Snapshot each ancestor’s metadata and effective (possibly Turn-pinned) Mode and agent.
    pub async fn ancestor_authorities(
        &self,
        info: &SessionInfo,
    ) -> Result<Vec<AncestorAuthority>, RuntimeError> {
        let mut seen = HashSet::from([info.id.clone()]);
        let mut parent = info.parent_id.clone();
        let mut ancestors = Vec::new();
        while let Some(id) = parent {
            if !seen.insert(id.clone()) {
                return Err(RuntimeError::Invalid(format!(
                    "Session ancestry cycle at {id}"
                )));
            }
            let handle = self.inner.handle(&id).await?;
            let authority = {
                let state = handle.state.lock().await;
                let running = self.is_running(&id);
                AncestorAuthority {
                    info: state.info.clone(),
                    effective_mode: state.effective_mode(running).into(),
                    effective_agent: state.effective_agent(running).into(),
                }
            };
            parent = authority.info.parent_id.clone();
            ancestors.push(authority);
        }
        Ok(ancestors)
    }
}
