//! Read-only reacquisition of every consumed checkout and its published disposition.
use super::*;
use cyber_core::worktrees::{CheckoutAcknowledgement, Managed, Repository, SnapshotClaim};
use cyber_server::runtime::SnapshotLifetimeBinding;
use cyber_server::runtime::snapshot_reclamation::Receipt;

pub(super) struct Checkouts {
    locations: Vec<Managed>,
    claims: Vec<SnapshotClaim>,
}

impl Checkouts {
    pub fn acquire(
        store: &Store,
        lifetime: &super::super::lifetime::Lifetime,
        binding: &SnapshotLifetimeBinding,
        resources_digest: &str,
        receipt_digest: &str,
    ) -> Result<Self, String> {
        let resources = lifetime
            .checkout_bindings()?
            .ok_or("Missing reclamation checkout inventory")?;
        if binding.checkout_resources_digest(&resources)? != resources_digest {
            return Err("Reclamation checkout resource digest changed".into());
        }
        let locations = Repository::managed_locations_at(&store.root).map_err(storage::fail)?;
        if locations.len() != resources.len() {
            return Err("Reclamation checkout inventory changed".into());
        }
        let owner = binding.checkout_owner()?;
        let mut claims = Vec::with_capacity(resources.len());
        for ((repository, managed), resource) in locations.iter().zip(&resources) {
            let mut claim = repository
                .reacquire_snapshot(managed, resource, &owner)
                .map_err(storage::fail)?;
            CheckoutAcknowledgement::new(resource, resources_digest, receipt_digest)
                .and_then(|ack| ack.require_existing(&mut claim))
                .map_err(storage::fail)?;
            claims.push(claim);
        }
        let guards = Self {
            locations: locations.into_iter().map(|(_, managed)| managed).collect(),
            claims,
        };
        guards.verify(store)?;
        Ok(guards)
    }

    pub fn verify(&self, store: &Store) -> Result<(), String> {
        let current = Repository::managed_locations_at(&store.root).map_err(storage::fail)?;
        if !current
            .iter()
            .map(|(_, managed)| managed)
            .eq(&self.locations)
        {
            return Err("Reclamation checkout registration changed".into());
        }
        for claim in &self.claims {
            claim.verify().map_err(storage::fail)?;
        }
        Ok(())
    }
}

pub(super) fn receipt_digest(receipt: &Receipt) -> Result<String, String> {
    match receipt {
        Receipt::Restore(receipt) => receipt.checkout_acknowledgement_digest(),
        Receipt::Recovery(receipt) => receipt.checkout_acknowledgement_digest(),
        Receipt::Preparation(receipt) => receipt.checkout_acknowledgement_digest(),
    }
}
