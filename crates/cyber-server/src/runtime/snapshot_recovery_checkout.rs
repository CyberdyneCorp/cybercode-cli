//! Runtime-owned checkout evidence, distinct from immutable native intent data.
use super::{
    SnapshotOrigin, SnapshotRecoveryCheckoutRole as Role, SnapshotRecoveryReceipt as Receipt,
    SnapshotRestoreCheckout,
};

impl Receipt {
    pub fn checkout_evidence(&self, role: Role) -> Option<&SnapshotRestoreCheckout> {
        let checkout = self.checkout.as_ref()?;
        match role {
            Role::Original => checkout.original.as_deref(),
            Role::Resolution => Some(&checkout.resolution),
        }
    }
    /// A checksum binds metadata to an observed durable pair; it grants no authority alone.
    pub fn checkout_acknowledgement_digest(&self) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let checkout = self
            .checkout
            .as_ref()
            .ok_or("Legacy recovery receipt lacks checkout proof")?;
        self.validate_checkout(&checkout.resolution_origin)?;
        Ok(format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&("snapshot-recovery-checkout-ack-v1", self))
                    .map_err(|error| error.to_string())?
            )
        ))
    }
    pub(super) fn validate_checkout(&self, resolution: &SnapshotOrigin) -> Result<(), String> {
        let checkout = self
            .checkout
            .as_ref()
            .ok_or("Missing recovery checkout proof")?;
        let hash =
            |value: &str| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
        if checkout.version != 1
            || checkout.resolution_origin != *resolution
            || !self.filesystem_complete
            || self
                .resolution_digest
                .as_deref()
                .is_none_or(|value| !hash(value))
            || !hash(&self.confirmation)
            || self.request_id.is_empty()
            || self.request_id.len() > 256
            || self.request_id.contains('\0')
            || self.journal_id != self.original.binding.journal_id
            || self.resolution_id != resolution.binding.journal_id
            || resolution.activity_id
                != super::snapshot_recovery::request_owner(
                    &self.original.session_created_id,
                    &self.request_id,
                )
            || resolution.activity_id == self.original.activity_id
            || self.resolution_id == self.journal_id
            || resolution.session_id != self.original.session_id
            || resolution.session_created_id != self.original.session_created_id
            || resolution.worktree_id != self.original.worktree_id
            || resolution.binding.directory != self.original.binding.directory
            || resolution.binding.root_identity != self.original.binding.root_identity
        {
            return Err("Recovery checkout receipt owner or frame differs".into());
        }
        checkout.resolution.validate(resolution)?;
        if let Some(original) = &checkout.original {
            original.validate(&self.original)?;
        }
        Ok(())
    }
}

pub(super) struct RecoveryClaims {
    pub resolution: Box<dyn super::SnapshotLocationGuard>,
    pub original: Option<Box<dyn super::SnapshotLocationGuard>>,
}
impl RecoveryClaims {
    pub fn verify(&self, receipt: &Receipt) -> Result<(), String> {
        self.resolution.verify()?;
        if let Some(original) = &self.original {
            original.verify()?;
        }
        if receipt.checkout.is_none() {
            return Ok(());
        }
        for role in [Role::Resolution, Role::Original] {
            let claims = match role {
                Role::Resolution => Some(&self.resolution),
                Role::Original => self.original.as_ref(),
            };
            match (receipt.checkout_evidence(role), claims) {
                (Some(evidence), Some(claims))
                    if evidence
                        .lifetime
                        .checkout_resources_digest(&claims.bindings()?)?
                        == evidence.resources_digest => {}
                (None, None) => {}
                _ => return Err("Recovery consuming checkout set differs".into()),
            }
        }
        Ok(())
    }
    pub fn prepare(&mut self, receipt: &Receipt) -> Result<(), String> {
        self.verify(receipt)?;
        if receipt.checkout.is_some() {
            self.resolution
                .prepare_recovery_acknowledgement(receipt, Role::Resolution)?;
            if let Some(original) = &mut self.original {
                original.prepare_recovery_acknowledgement(receipt, Role::Original)?;
            }
        }
        Ok(())
    }
    pub fn acknowledge(&mut self, receipt: &Receipt) -> Result<(), String> {
        self.verify(receipt)?;
        if receipt.checkout.is_some() {
            self.resolution
                .acknowledge_recovery(receipt, Role::Resolution)?;
            if let Some(original) = &mut self.original {
                original.acknowledge_recovery(receipt, Role::Original)?;
            }
        }
        Ok(())
    }
}
