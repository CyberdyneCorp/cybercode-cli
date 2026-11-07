//! A committed recovery pair owns two private inventories, never current source.
use super::*;
use cyber_server::runtime::{
    SnapshotOrigin, SnapshotRestoreCheckout, snapshot_reclamation::Receipt,
};

struct Owner {
    origin: SnapshotOrigin,
    journal: Dir,
    intent: MetadataPin,
    digest: String,
    phase: MetadataPin,
    lifetime: super::super::lifetime::Lifetime,
}
struct PreparedPair {
    store: Store,
    owners: Vec<Owner>,
    workers: Dir,
    plan: Plan,
    workspace: tokio::sync::OwnedMutexGuard<()>,
    config: Arc<crate::ConfigFn>,
    settled_at_ms: i64,
}
pub(super) fn eligible(record: &Journal) -> bool {
    record.completed
        && record.acknowledged
        && record.resolution.is_some()
        && record.settled_by.is_none()
        && record.origin.is_some()
        && record
            .context
            .as_ref()
            .is_some_and(|context| context.git.is_none())
        && record.backend != Some(JournalBackend::Git)
}
pub(super) fn reclaim(
    store: Store,
    workspace: tokio::sync::OwnedMutexGuard<()>,
    config: Arc<crate::ConfigFn>,
    policy: Arc<dyn Policy>,
    journal: Dir,
    record: Journal,
) -> Result<u64, String> {
    let origin = record.origin.as_ref().ok_or("Missing recovery origin")?;
    let Some(settlement) = policy.receipt(origin)? else {
        return Ok(0);
    };
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

fn owner(
    store: &Store,
    workers: &Dir,
    origin: &SnapshotOrigin,
    checkout: &SnapshotRestoreCheckout,
    journal: Dir,
    digest: String,
    admitted: bool,
) -> Result<Owner, String> {
    store.verify_worker_namespace(origin)?;
    if store.worker_digest(Some(origin))?.as_ref() != Some(&digest) {
        return Err("Recovery cleanup intent differs".into());
    }
    let lifetime = store.lock_lifetime(&checkout.lifetime)?;
    let resources = lifetime
        .checkout_bindings()?
        .ok_or("Missing recovery checkout inventory")?;
    if !resources.is_empty()
        || checkout.resources_digest != checkout.lifetime.checkout_resources_digest(&resources)?
    {
        return Err("Managed recovery cleanup requires retained resource guards".into());
    }
    let name = checkout
        .lifetime
        .source_phase
        .as_deref()
        .ok_or("Missing recovery phase")?;
    let phase = MetadataPin::open(workers, name)?;
    store.verify_reclamation_phase_admission(
        &checkout.lifetime,
        origin,
        &checkout.resources_digest,
        admitted,
    )?;
    phase.verify(workers, name)?;
    let intent = MetadataPin::open(&journal, "intent.json")?;
    Ok(Owner {
        origin: origin.clone(),
        journal,
        intent,
        digest,
        phase,
        lifetime,
    })
}

fn prepare(
    store: Store,
    workspace: tokio::sync::OwnedMutexGuard<()>,
    config: Arc<crate::ConfigFn>,
    journal: Dir,
    record: Journal,
    settlement: Settlement,
    frozen: Option<Plan>,
) -> Result<PreparedPair, String> {
    let Receipt::Recovery(receipt) = &settlement.receipt else {
        return Err("Recovery cleanup requires its paired receipt".into());
    };
    receipt.checkout_acknowledgement_digest()?;
    let checkout = receipt
        .checkout
        .as_ref()
        .ok_or("Missing paired checkout proof")?;
    let original_checkout = checkout
        .original
        .as_deref()
        .ok_or("Missing original checkout proof")?;
    let resolution = record
        .resolution
        .as_ref()
        .ok_or("Missing resolution intent")?;
    let (parent_dir, parent) = store.journal(&receipt.journal_id)?;
    store.verify_settlement(&parent)?;
    store.verify_receipt(record.origin.as_ref(), receipt)?;
    if !eligible(&record)
        || parent.origin.as_ref() != Some(&receipt.original)
        || parent.settled_by.as_deref() != Some(receipt.resolution_id.as_str())
        || parent
            .context
            .as_ref()
            .is_none_or(|context| context.git.is_some())
    {
        return Err("Recovery cleanup pair or backend changed".into());
    }
    unmanaged(&store)?;
    let workers = storage::existing_child(&store.namespace, "workers", true)?;
    let original = owner(
        &store,
        &workers,
        &receipt.original,
        original_checkout,
        parent_dir,
        resolution.original_digest.clone(),
        true,
    )?;
    let resolved = owner(
        &store,
        &workers,
        &checkout.resolution_origin,
        &checkout.resolution,
        journal,
        receipt
            .resolution_digest
            .clone()
            .ok_or("Missing resolution digest")?,
        !record.changes.is_empty(),
    )?;
    let mut files = original_payloads(&store, &original, &parent, resolution, frozen.as_ref())?;
    files.extend(resolution_payloads(
        &store,
        &resolved,
        &record,
        frozen.as_ref(),
    )?);
    let plan = Plan {
        version: 2,
        receipt: settlement.receipt,
        files,
    };
    plan.validate().map_err(storage::fail)?;
    if frozen.as_ref().is_some_and(|frozen| frozen != &plan) {
        return Err("Recovery cleanup plan differs from retained request".into());
    }
    let prepared = PreparedPair {
        store,
        owners: vec![original, resolved],
        workers,
        plan,
        workspace,
        config,
        settled_at_ms: settlement.settled_at_ms,
    };
    prepared.verify()?;
    Ok(prepared)
}

fn inventory(journal: &Dir, count: usize) -> Result<(), String> {
    let known = (0..count)
        .map(|index| format!("stage_{index}"))
        .chain(std::iter::once("intent.json".into()))
        .collect::<BTreeSet<_>>();
    if storage::names(journal, super::super::ENTRY_LIMIT * 3)?
        .iter()
        .any(|name| !known.contains(name))
    {
        return Err("Unknown recovery payload or construction entry".into());
    }
    Ok(())
}
fn planned<'a>(frozen: Option<&'a Plan>, owner: &Owner) -> BTreeMap<&'a str, &'a Payload> {
    frozen
        .into_iter()
        .flat_map(|plan| &plan.files)
        .filter(|file| file.journal.as_deref() == Some(owner.origin.binding.journal_id.as_str()))
        .map(|file| (file.name.as_str(), file))
        .collect()
}

fn payload(owner: &Owner, name: String, image: ownership::ImageObservation) -> Payload {
    Payload {
        journal: Some(owner.origin.binding.journal_id.clone()),
        name,
        identity: image.identity,
        generation: image.generation,
        hash: image.entry.object,
        bytes: image.entry.length,
    }
}
fn original_payloads(
    store: &Store,
    owner: &Owner,
    parent: &Journal,
    resolution: &super::super::resolution::Resolution,
    frozen: Option<&Plan>,
) -> Result<Vec<Payload>, String> {
    let outcome = resolution
        .outcome
        .as_ref()
        .ok_or("Missing immutable recovery outcome")?;
    if outcome.files.len() != parent.changes.len() {
        return Err("Incomplete recovery slot inventory".into());
    }
    inventory(&owner.journal, parent.changes.len())?;
    let mut files = Vec::new();
    let planned = planned(frozen, owner);
    for (index, slots) in outcome.files.iter().enumerate() {
        let name = format!("stage_{index}");
        let actual =
            ownership::observe_image(store, Some(&owner.journal), std::ffi::OsStr::new(&name))?;
        let expected = slots.stage.as_ref();
        let file = expected
            .cloned()
            .map(|image| payload(owner, name.clone(), image));
        if expected.is_some_and(|image| image.entry.kind != super::super::ImageKind::File)
            || (actual.as_ref() != expected
                && !(actual.is_none() && file.as_ref() == planned.get(name.as_str()).copied()))
        {
            return Err("Original recovery private slot differs from acknowledged outcome".into());
        }
        if let Some(file) = file {
            files.push(file);
        }
    }
    Ok(files)
}
fn resolution_payloads(
    store: &Store,
    owner: &Owner,
    record: &Journal,
    frozen: Option<&Plan>,
) -> Result<Vec<Payload>, String> {
    inventory(&owner.journal, record.changes.len())?;
    let ownership = record
        .ownership
        .as_ref()
        .ok_or("Missing resolution payload ownership")?;
    let mut files = Vec::new();
    let planned = planned(frozen, owner);
    for (index, change) in record.changes.iter().enumerate() {
        let name = format!("stage_{index}");
        let actual =
            ownership::observe_image(store, Some(&owner.journal), std::ffi::OsStr::new(&name))?;
        let previous = planned.get(name.as_str()).copied();
        match (&change.before, actual) {
            (Some(before), Some(image))
                if before.kind == super::super::ImageKind::File && image.entry == *before =>
            {
                let pin = storage::open(&owner.journal, Path::new(&name))?;
                ownership.verify_original(index, &pin)?;
                files.push(payload(owner, name, image));
            }
            (Some(before), None)
                if previous.is_some_and(|file| {
                    file.hash == before.object
                        && file.bytes == before.length
                        && ownership.original_identity(index) == Some(file.identity.as_str())
                }) =>
            {
                files.push(
                    previous
                        .cloned()
                        .ok_or("Missing planned resolution payload")?,
                )
            }
            (None, None) if previous.is_none() => {}
            _ => return Err("Resolution displaced payload lacks original ownership".into()),
        }
    }
    Ok(files)
}
impl PreparedPair {
    fn routes(&self) -> Result<(), String> {
        let _reader = &self.workspace;
        if !due(self.config.as_ref(), &self.store.root, self.settled_at_ms) {
            return Err("Recovery retention no longer permits cleanup".into());
        }
        unmanaged(&self.store)?;
        for owner in &self.owners {
            owner.lifetime.verify()?;
            self.store.verify_worker_namespace(&owner.origin)?;
            owner.intent.verify(&owner.journal, "intent.json")?;
            owner.phase.verify(
                &self.workers,
                owner
                    .lifetime
                    .binding()
                    .source_phase
                    .as_deref()
                    .ok_or("Missing paired phase")?,
            )?;
        }
        Ok(())
    }
    fn verify(&self) -> Result<(), String> {
        self.routes()?;
        for owner in &self.owners {
            if self.store.worker_digest(Some(&owner.origin))?.as_ref() != Some(&owner.digest) {
                return Err("Paired cleanup intent changed".into());
            }
            let known = self
                .plan
                .files
                .iter()
                .filter(|file| {
                    file.journal.as_deref() == Some(owner.origin.binding.journal_id.as_str())
                })
                .map(|file| file.name.as_str())
                .chain(std::iter::once("intent.json"))
                .collect::<BTreeSet<_>>();
            if storage::names(&owner.journal, super::super::ENTRY_LIMIT * 3)?
                .iter()
                .any(|name| !known.contains(name.as_str()))
            {
                return Err("Unplanned entry appeared before paired cleanup".into());
            }
        }
        let (_, parent) = self
            .store
            .journal(&self.owners[0].origin.binding.journal_id)?;
        self.store.verify_settlement(&parent)
    }
    fn delete(self) -> Result<u64, String> {
        self.verify()?;
        for file in &self.plan.files {
            self.pin(file)?;
        }
        for owner in &self.owners {
            let directory = owner
                .journal
                .try_clone()
                .map_err(storage::fail)?
                .into_std_file();
            let mut generation = ownership::generation(&directory)?;
            for file in self.plan.files.iter().filter(|file| {
                file.journal.as_deref() == Some(owner.origin.binding.journal_id.as_str())
            }) {
                self.routes()?;
                if ownership::generation(&directory)? != generation {
                    return Err("Paired cleanup directory changed".into());
                }
                if let Some(_pin) = self.pin(file)? {
                    owner
                        .journal
                        .remove_file(&file.name)
                        .map_err(storage::fail)?;
                    storage::sync(&owner.journal)?;
                    generation = ownership::generation(&directory)?;
                }
            }
        }
        self.verify()?;
        self.plan.total_bytes().map_err(storage::fail)
    }
    fn pin(&self, file: &Payload) -> Result<Option<std::fs::File>, String> {
        let owner = self
            .owners
            .iter()
            .find(|owner| file.journal.as_deref() == Some(owner.origin.binding.journal_id.as_str()))
            .ok_or("Foreign cleanup journal")?;
        match owner.journal.symlink_metadata(&file.name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage::fail(error)),
            _ => {}
        }
        let pin = storage::open(&owner.journal, Path::new(&file.name))?;
        if storage::identity(&pin)? != file.identity
            || ownership::generation(&pin)? != file.generation
        {
            return Err("Paired cleanup payload changed".into());
        }
        Ok(Some(pin))
    }
}
