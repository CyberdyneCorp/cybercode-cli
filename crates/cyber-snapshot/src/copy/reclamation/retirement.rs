//! Exact private metadata retirement after durable payload completion.
mod native;
use super::*;
use cyber_server::runtime::snapshot_reclamation::{self as records, Receipt, metadata};

pub(crate) fn retire(
    store: Store,
    workspace: tokio::sync::OwnedMutexGuard<()>,
    config: Arc<crate::ConfigFn>,
    policy: Arc<dyn Policy>,
    receipt: Receipt,
) -> Result<u64, String> {
    let record = policy
        .record(&receipt)?
        .ok_or("Missing completed payload cleanup")?;
    if !record.completed {
        return Err("Metadata retirement needs observed payload completion".into());
    }
    if record.retired {
        return Ok(0);
    }
    let origin = receipt.origin().map_err(storage::fail)?;
    let settlement = policy
        .receipt(origin)?
        .ok_or("Missing metadata retirement settlement")?;
    if settlement.receipt != receipt || record.plan.receipt != receipt {
        return Err("Metadata retirement receipt changed".into());
    }
    if !due(config.as_ref(), &store.root, settlement.settled_at_ms) {
        return Ok(0);
    }
    for owner in receipt.owners().map_err(storage::fail)? {
        if owner.lifetime.owner.worktree_id.is_some()
            || owner.lifetime.checkout_resources_digest(&[])? != owner.resources_digest
        {
            return Err("Managed checkout metadata retirement is not yet available".into());
        }
    }
    if record.metadata.is_none() {
        verify_initial(&store, &record)?;
    }
    let prepared =
        native::Prepared::new(store, workspace, config, settlement.settled_at_ms, record)?;
    let plan = prepared.plan.clone();
    let payload = prepared.payload.clone();
    policy.retire(payload, plan, Box::new(move || prepared.delete()))
}

fn verify_initial(store: &Store, retained: &records::Record) -> Result<(), String> {
    let receipt = &retained.plan.receipt;
    for owner in receipt.owners().map_err(storage::fail)? {
        store.verify_worker_namespace(owner.origin)?;
        let (_, journal) = store.journal(&owner.origin.binding.journal_id)?;
        if journal.origin.as_ref() != Some(owner.origin) {
            return Err("Metadata retirement journal owner differs".into());
        }
        let (digest, admitted) = initial_journal(store, retained, &journal, owner.origin)?;
        if store.worker_digest(Some(owner.origin))?.as_ref() != Some(&digest) {
            return Err("Metadata retirement intent differs from durable settlement".into());
        }
        store.verify_reclamation_phase_admission(
            owner.lifetime,
            owner.origin,
            owner.resources_digest,
            admitted,
        )?;
    }
    Ok(())
}
fn initial_journal(
    store: &Store,
    retained: &records::Record,
    journal: &Journal,
    origin: &cyber_server::runtime::SnapshotOrigin,
) -> Result<(String, bool), String> {
    match &retained.plan.receipt {
        Receipt::Restore(receipt) if ordinary(journal) => {
            Ok((receipt.intent_digest.clone(), !journal.changes.is_empty()))
        }
        Receipt::Preparation(receipt) if cancelled::verify(store, receipt, journal)? => Ok((
            retained
                .plan
                .intent_digest
                .clone()
                .ok_or("Missing cancellation intent digest")?,
            false,
        )),
        Receipt::Recovery(receipt) => {
            let (_, resolution) = store.journal(&receipt.resolution_id)?;
            let (_, original) = store.journal(&receipt.journal_id)?;
            store.verify_settlement(&original)?;
            store.verify_receipt(resolution.origin.as_ref(), receipt)?;
            if !paired::eligible(&resolution)
                || original.settled_by.as_deref() != Some(&receipt.resolution_id)
            {
                return Err("Metadata retirement recovery pair differs".into());
            }
            if origin == &receipt.original {
                Ok((
                    resolution
                        .resolution
                        .as_ref()
                        .ok_or("Missing recovery intent")?
                        .original_digest
                        .clone(),
                    true,
                ))
            } else {
                Ok((
                    receipt
                        .resolution_digest
                        .clone()
                        .ok_or("Missing resolution digest")?,
                    !resolution.changes.is_empty(),
                ))
            }
        }
        _ => Err("Metadata retirement journal is not acknowledged".into()),
    }
}
