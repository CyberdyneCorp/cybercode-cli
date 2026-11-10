//! Reclaim only persisted, fully prepared target inodes after cancellation.
use super::*;
use cyber_server::runtime::{SnapshotPreparationReceipt, snapshot_reclamation::Receipt};

pub(super) fn eligible(record: &Journal) -> bool {
    record.version >= 3
        && record.origin.is_some()
        && record.ownership.is_some()
        && record.applied.is_empty()
        && !record.acknowledged
        && record.settled_by.is_none()
        && (!record.completed || record.changes.is_empty())
        && record.context.is_some()
}
pub(super) fn verify(
    store: &Store,
    receipt: &SnapshotPreparationReceipt,
    record: &Journal,
) -> Result<bool, String> {
    let origin = receipt
        .origin
        .as_ref()
        .ok_or("Preparation cleanup has no journal origin")?;
    Ok(eligible(record)
        && record.origin.as_ref() == Some(origin)
        && store
            .preparation_cancellation(&origin.binding.journal_id)?
            .as_ref()
            == Some(receipt))
}
pub(super) fn reclaim(
    store: Store,
    workspace: tokio::sync::OwnedMutexGuard<()>,
    config: Arc<crate::ConfigFn>,
    policy: Arc<dyn Policy>,
    receipt: SnapshotPreparationReceipt,
) -> Result<u64, String> {
    let origin = receipt
        .origin
        .as_ref()
        .ok_or("Preparation cleanup has no journal origin")?;
    let (journal, record) = store.journal(&origin.binding.journal_id)?;
    if !verify(&store, &receipt, &record)? {
        return Ok(0);
    }
    let Some(settlement) = policy.receipt(origin)? else {
        return Ok(0);
    };
    if settlement.receipt != Receipt::Preparation(Box::new(receipt.clone())) {
        return Err("Foreign cancellation cleanup receipt".into());
    }
    if !due(config.as_ref(), &store.root, settlement.settled_at_ms) {
        return Ok(0);
    }
    let existing = policy.record(&settlement.receipt)?;
    if existing.as_ref().is_some_and(|record| record.completed) {
        return Ok(0);
    }
    let prepared = prepare(
        store,
        workspace,
        config,
        journal,
        record,
        settlement,
        existing.map(|record| record.plan),
    )?;
    if prepared.plan.files.is_empty() {
        return Ok(0);
    }
    policy.reclaim(prepared.plan.clone(), Box::new(move || prepared.delete()))
}
fn prepare(
    store: Store,
    workspace: tokio::sync::OwnedMutexGuard<()>,
    config: Arc<crate::ConfigFn>,
    journal: Dir,
    record: Journal,
    settlement: Settlement,
    frozen: Option<Plan>,
) -> Result<Prepared, String> {
    let Receipt::Preparation(receipt) = &settlement.receipt else {
        return Err("Missing preparation receipt".into());
    };
    receipt.checkout_acknowledgement_digest()?;
    let origin = receipt
        .origin
        .as_ref()
        .ok_or("Missing preparation journal origin")?;
    store.verify_worker_namespace(origin)?;
    let lifetime = store.lock_lifetime(&receipt.lifetime)?;
    let digest = receipt
        .checkout_resources_digest
        .as_deref()
        .ok_or("Missing consumed preparation resources")?;
    let checkouts = checkouts::Checkouts::acquire(
        &store,
        &lifetime,
        &receipt.lifetime,
        digest,
        &checkouts::receipt_digest(&settlement.receipt)?,
    )?;
    let workers = storage::existing_child(&store.namespace, "workers", true)?;
    let phase_name = receipt
        .lifetime
        .source_phase
        .as_deref()
        .ok_or("Missing preparation phase")?;
    let phase = MetadataPin::open(&workers, phase_name)?;
    store.verify_reclamation_phase_admission(&receipt.lifetime, origin, digest, false)?;
    phase.verify(&workers, phase_name)?;
    let intent_digest = store
        .worker_digest(Some(origin))?
        .ok_or("Missing prepared intent digest")?;
    let files = payloads(&store, &journal, &record, frozen.as_ref())?;
    let intent = MetadataPin::open(&journal, "intent.json")?;
    let cancellation = Some(MetadataPin::open(
        &journal,
        super::super::preparation::FILE,
    )?);
    let plan = Plan {
        version: 3,
        intent_digest: Some(intent_digest.clone()),
        receipt: settlement.receipt,
        files,
    };
    plan.validate().map_err(storage::fail)?;
    if frozen.as_ref().is_some_and(|frozen| frozen != &plan) {
        return Err("Preparation cleanup differs from retained plan".into());
    }
    let prepared = Prepared {
        checkouts,
        store,
        journal,
        intent,
        cancellation,
        phase,
        workers,
        plan,
        lifetime,
        workspace,
        config,
        settled_at_ms: settlement.settled_at_ms,
        intent_digest,
    };
    prepared.verify()?;
    Ok(prepared)
}
fn payloads(
    store: &Store,
    journal: &Dir,
    record: &Journal,
    frozen: Option<&Plan>,
) -> Result<Vec<Payload>, String> {
    let ownership = record
        .ownership
        .as_ref()
        .ok_or("Missing persisted prepared identities")?;
    let planned = frozen
        .into_iter()
        .flat_map(|plan| &plan.files)
        .map(|file| (file.name.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let mut files = Vec::new();
    for (index, change) in record.changes.iter().enumerate() {
        let name = format!("stage_{index}");
        let image = ownership::observe_image(store, Some(journal), std::ffi::OsStr::new(&name))?;
        let previous = planned.get(name.as_str()).copied();
        match (&change.after, image) {
            (Some(after), Some(image))
                if image.entry == *after
                    && ownership.prepared_identity(index) == Some(image.identity.as_str()) =>
            {
                files.push(Payload {
                    journal: None,
                    name,
                    identity: image.identity,
                    generation: image.generation,
                    hash: image.entry.object,
                    bytes: image.entry.length,
                })
            }
            (Some(after), None)
                if previous.is_some_and(|file| {
                    file.hash == after.object
                        && file.bytes == after.length
                        && ownership.prepared_identity(index) == Some(file.identity.as_str())
                }) =>
            {
                files.push(previous.cloned().ok_or("Missing frozen prepared leaf")?)
            }
            (None, None) if previous.is_none() => {}
            _ => return Err("Prepared cancellation payload lacks persisted ownership".into()),
        }
    }
    Ok(files)
}
