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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_phase: Option<String>,
}

impl SnapshotLifetimeBinding {
    /// Versioned digest of the complete ordered acquisition, bound to this lifetime.
    pub fn checkout_resources_digest(
        &self,
        resources: &[cyber_core::worktrees::CheckoutBinding],
    ) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let owner = self.checkout_owner()?;
        if resources.len() > 4096
            || self
                .owner
                .worktree_id
                .as_ref()
                .is_some_and(|id| !resources.iter().any(|resource| &resource.worktree_id == id))
            || self.owner.worktree_id.is_none() && !resources.is_empty()
        {
            return Err("Snapshot checkout resource set is incomplete or exceeds limits".into());
        }
        let mut seen = std::collections::HashSet::new();
        for resource in resources {
            if resource.version != 2
                || resource.owner_id != self.owner.activity_id
                || resource.lifetime.as_ref() != Some(&owner)
                || !seen.insert((&resource.common_dir, &resource.worktree_id))
            {
                return Err(
                    "Snapshot checkout resource set has foreign ownership or duplicates".into(),
                );
            }
        }
        let bytes = serde_json::to_vec(&("snapshot-checkout-resources-v1", self, resources))
            .map_err(|error| error.to_string())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub fn checkout_owner(&self) -> Result<cyber_core::worktrees::CheckoutOwner, String> {
        use sha2::{Digest, Sha256};
        if self.version != 2 || self.source_phase.is_none() {
            return Err("Legacy lifetime has no checkout binding authority".into());
        }
        let bytes = serde_json::to_vec(&("snapshot-checkout-lifetime-v1", self))
            .map_err(|error| error.to_string())?;
        Ok(cyber_core::worktrees::CheckoutOwner {
            activity_id: self.owner.activity_id.clone(),
            session_id: self.owner.session_id.clone(),
            session_created_id: self.owner.session_created_id.clone(),
            directory: self.owner.directory.clone(),
            lifetime_digest: format!("{:x}", Sha256::digest(bytes)),
        })
    }
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
            if binding.version != 2
                || binding.source_phase.as_deref().is_none_or(|phase| {
                    !phase.starts_with("phase_")
                        || phase.len() > 128
                        || !phase
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                })
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

/// The actual worker retains this lock through all resource acquisition and work.
/// It conveys no authority to settle checkout, process or binary-document owners.
pub trait SnapshotLifetime: Send {
    fn binding(&self) -> &SnapshotLifetimeBinding;
    /// Synchronous native metadata verification; may run inside the Store writer.
    /// It must not call the runtime/Store, launch processes or perform source effects.
    fn verify(&self) -> Result<(), String>;
    /// Publish the complete acquired set before origin/effect dispatch. This is
    /// native metadata only; it cannot acknowledge claims or reenter the Store.
    fn bind_checkouts(
        &self,
        _bindings: &[cyber_core::worktrees::CheckoutBinding],
    ) -> Result<(), String> {
        Err("Durable checkout resource publication is unavailable".into())
    }
    /// Missing is not an empty complete resource set and grants no new authority.
    fn checkout_bindings(
        &self,
    ) -> Result<Option<Vec<cyber_core::worktrees::CheckoutBinding>>, String> {
        Ok(None)
    }
    /// Persist native admission before a source callback. A legacy lifetime may
    /// never grant the new positive pre-effect cancellation proof.
    fn admit_source(&self, _origin: &super::SnapshotOrigin) -> Result<(), String> {
        Err("Native source admission is unavailable".into())
    }
    /// Positive native source-phase proof, not merely an unlocked lifetime file.
    fn verify_preparation(&self, _origin: Option<&super::SnapshotOrigin>) -> Result<(), String> {
        Err("Native preparation phase proof is unavailable".into())
    }
    /// Read-only candidate check against retained native preparation receipts.
    /// Called at the primary writer; no primary Store reentry or metadata writes.
    fn prepare_preparation_acknowledgement(
        &self,
        _receipt: &super::SnapshotPreparationReceipt,
    ) -> Result<(), String> {
        Err("Native preparation receipt verification is unavailable".into())
    }
    /// Metadata publication only, after observed durable preparation cancellation.
    fn acknowledge_preparation(
        &mut self,
        receipt: &super::SnapshotPreparationReceipt,
    ) -> Result<(), String> {
        if receipt.origin.is_some() {
            return Err("Preparation journal acknowledgement is unavailable".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod checkout_encoding_tests {
    use super::*;
    #[test]
    fn checkout_digest_encoding_has_a_stable_versioned_known_answer() {
        let binding = SnapshotLifetimeBinding {
            version: 2,
            owner: SnapshotLifetimeRequest {
                activity_id: "op_fixture".into(),
                session_id: "ses_fixture".into(),
                session_created_id: "evt_fixture".into(),
                directory: "/work".into(),
                worktree_id: None,
            },
            root_identity: "root".into(),
            namespace_identity: "namespace".into(),
            directory_identity: "directory".into(),
            file_identity: "file".into(),
            source_phase: Some("phase_fixture".into()),
        };
        // Pins the framed v1 encoding, rather than recomputing it with this helper.
        assert_eq!(
            binding.checkout_owner().unwrap().lifetime_digest,
            "66239118d7327f0739d4e468da0b7d4c547e0a752cfd9680e6e5f7f2469af802"
        );
        assert_eq!(
            binding.checkout_resources_digest(&[]).unwrap(),
            "a36ba5baced567a6dde73bc41c6e83c49017ba049b43fd59a5607b6204b5c5e7"
        );
        let mut receipt = super::super::SnapshotPreparationReceipt {
            activity_id: "op_fixture".into(),
            request_id: "cancel".into(),
            confirmation: "a".repeat(64),
            lifetime: binding.clone(),
            native_sequence: 0,
            session_sequence: 3,
            origin: None,
            source_admitted: Some(false),
            checkout_resources_digest: Some(binding.checkout_resources_digest(&[]).unwrap()),
        };
        assert_eq!(
            receipt.checkout_acknowledgement_digest().unwrap(),
            "9d475e87059bfef3f53d128c9be8029e36b8815289ad5a0cde8f7d5614938a4e"
        );
        receipt.source_admitted = None;
        assert!(receipt.checkout_acknowledgement_digest().is_err());
        receipt.source_admitted = Some(false);
        receipt.checkout_resources_digest = None;
        assert!(receipt.checkout_acknowledgement_digest().is_err());
        let mut changed = binding.clone();
        changed.file_identity.push_str("_replacement");
        assert_ne!(
            changed.checkout_resources_digest(&[]).unwrap(),
            binding.checkout_resources_digest(&[]).unwrap()
        );
        changed.version = 1;
        assert!(changed.checkout_resources_digest(&[]).is_err());
    }
}
