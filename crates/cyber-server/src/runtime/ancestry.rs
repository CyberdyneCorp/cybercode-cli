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
    /// `session_id` followed by its ancestors' IDs, immediate parent first; a missing
    /// ancestor or a cycle ends the chain there.
    pub async fn lineage(&self, session_id: &str) -> Result<Vec<String>, RuntimeError> {
        let mut lineage = vec![session_id.to_owned()];
        let mut parent = {
            let handle = self.inner.handle(session_id).await?;
            handle.state.lock().await.info.parent_id.clone()
        };
        while let Some(id) = parent.take() {
            if lineage.contains(&id) {
                break;
            }
            let Ok(handle) = self.inner.handle(&id).await else {
                break;
            };
            parent = handle.state.lock().await.info.parent_id.clone();
            lineage.push(id);
        }
        Ok(lineage)
    }

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
