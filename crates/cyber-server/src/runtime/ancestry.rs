//! Read-only parent-chain validation; no child lock is held during ancestor lookup.

use super::{Runtime, RuntimeError, SessionInfo};
use std::collections::HashSet;

impl Runtime {
    /// Return immediate parent first, refusing missing parents and ancestry cycles.
    pub async fn ancestors(&self, info: &SessionInfo) -> Result<Vec<SessionInfo>, RuntimeError> {
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
            let info = handle.state.lock().await.info.clone();
            parent = info.parent_id.clone();
            ancestors.push(info);
        }
        Ok(ancestors)
    }
}
