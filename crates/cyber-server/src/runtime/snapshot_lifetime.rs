//! Exact native lifetime ownership exists before a snapshot reservation is durable.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotLifetimeRequest {
    pub activity_id: String,
    pub session_id: String,
    pub session_created_id: String,
    pub directory: PathBuf,
    pub worktree_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotLifetimeBinding {
    pub version: u32,
    pub owner: SnapshotLifetimeRequest,
    pub root_identity: String,
    pub namespace_identity: String,
    pub directory_identity: String,
    pub file_identity: String,
}

/// The actual worker retains this lock through all resource acquisition and work.
/// It conveys no authority to settle checkout, process or binary-document owners.
pub trait SnapshotLifetime: Send {
    fn binding(&self) -> &SnapshotLifetimeBinding;
    /// Synchronous native metadata verification; may run inside the Store writer.
    /// It must not call the runtime/Store, launch processes or perform source effects.
    fn verify(&self) -> Result<(), String>;
}

impl super::Runtime {
    pub(super) async fn snapshot_lifetime(
        &self,
        state: &super::SessionState,
        activity_id: &str,
    ) -> Result<Option<Box<dyn SnapshotLifetime>>, super::RuntimeError> {
        let created = self
            .inner
            .store
            .read_events(&state.info.id, -1, 1)?
            .events
            .first()
            .ok_or_else(|| {
                super::RuntimeError::Conflict("Missing snapshot Session creation".into())
            })?
            .id
            .clone();
        let request = SnapshotLifetimeRequest {
            activity_id: activity_id.to_owned(),
            session_id: state.info.id.clone(),
            session_created_id: created,
            directory: std::fs::canonicalize(&state.info.directory)
                .map_err(|error| super::RuntimeError::Invalid(error.to_string()))?,
            worktree_id: state.info.worktree_id.clone(),
        };
        let lifetime = self
            .inner
            .options
            .snapshots
            .reserve_worker(request.clone())
            .await
            .map_err(super::RuntimeError::Conflict)?;
        if let Some(lifetime) = &lifetime {
            let binding = lifetime.binding();
            if binding.version != 1
                || binding.owner != request
                || [
                    &binding.root_identity,
                    &binding.namespace_identity,
                    &binding.directory_identity,
                    &binding.file_identity,
                ]
                .iter()
                .any(|identity| identity.is_empty() || identity.len() > 256)
            {
                return Err(super::RuntimeError::Conflict(
                    "Snapshot lifetime ownership differs".into(),
                ));
            }
            lifetime.verify().map_err(super::RuntimeError::Conflict)?;
        }
        Ok(lifetime)
    }
}
