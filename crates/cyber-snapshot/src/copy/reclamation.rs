//! Reclaim only exact acknowledged displaced images; keep intent/lifetime receipts.
mod cancelled;
mod paired;
use super::{
    Store, ownership,
    restore::{Journal, JournalBackend},
    storage,
};
use crate::reclamation::{Policy, Settlement};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use cyber_server::runtime::{
    SnapshotLifetime,
    snapshot_reclamation::{File as Payload, Plan},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

struct Prepared {
    store: Store,
    journal: Dir,
    intent: MetadataPin,
    cancellation: Option<MetadataPin>,
    phase: MetadataPin,
    workers: Dir,
    plan: Plan,
    lifetime: super::lifetime::Lifetime,
    workspace: tokio::sync::OwnedMutexGuard<()>,
    config: Arc<crate::ConfigFn>,
    settled_at_ms: i64,
    intent_digest: String,
}
struct MetadataPin {
    file: std::fs::File,
    identity: String,
    generation: String,
}
impl MetadataPin {
    fn open(dir: &Dir, name: &str) -> Result<Self, String> {
        let file = storage::open(dir, Path::new(name))?;
        storage::validate_lock(&file)?;
        Ok(Self {
            identity: storage::identity(&file)?,
            generation: ownership::generation(&file)?,
            file,
        })
    }
    fn verify(&self, dir: &Dir, name: &str) -> Result<(), String> {
        let route = storage::open(dir, Path::new(name))?;
        storage::validate_lock(&route)?;
        if storage::identity(&route)? != self.identity
            || storage::identity(&self.file)? != self.identity
            || ownership::generation(&route)? != self.generation
            || ownership::generation(&self.file)? != self.generation
        {
            return Err("Reclamation metadata inode or generation changed".into());
        }
        Ok(())
    }
}
fn due(config: &crate::ConfigFn, root: &Path, settled_at_ms: i64) -> bool {
    let value = config(root);
    if value.get("snapshots") == Some(&serde_json::Value::Bool(false)) {
        return false;
    }
    let days = value
        .pointer("/snapshots/retention_days")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(7);
    let elapsed = (super::now() as u128)
        .saturating_mul(1000)
        .saturating_sub(settled_at_ms.max(0) as u128);
    settled_at_ms > 0 && days != 0 && elapsed > u128::from(days).saturating_mul(86400000)
}
fn ordinary(record: &Journal) -> bool {
    record.version >= 3
        && record.completed
        && record.acknowledged
        && record.origin.is_some()
        && record.resolution.is_none()
        && record.settled_by.is_none()
        && record
            .context
            .as_ref()
            .is_some_and(|context| context.git.is_none())
        && record.backend != Some(JournalBackend::Git)
        && record
            .changes
            .iter()
            .any(|change| change.before.as_ref().is_some_and(|entry| entry.length > 0))
}
/// Read existing metadata only. A missing/partial namespace remains protected.
pub(crate) fn locations(data: &Path) -> Result<BTreeSet<String>, String> {
    if !data.exists() {
        return Ok(BTreeSet::new());
    }
    let data = Dir::open_ambient_dir(data, cap_std::ambient_authority()).map_err(storage::fail)?;
    let snapshot = match data.symlink_metadata("snapshot") {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        _ => storage::existing_child(&data, "snapshot", false)?,
    };
    let root = match snapshot.symlink_metadata("global") {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        _ => storage::existing_child(&snapshot, "global", true)?,
    };
    let mut result = BTreeSet::new();
    let mut count = 0usize;
    for name in storage::names(&root, 100000)? {
        if !storage::valid_digest(&name) {
            return Err("Unknown snapshot namespace; reclamation refused".into());
        }
        let namespace = storage::existing_child(&root, &name, true)?;
        let journals = match namespace.symlink_metadata("journals") {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            _ => storage::existing_child(&namespace, "journals", true)?,
        };
        for name in storage::names(&journals, super::ENTRY_LIMIT)? {
            count += 1;
            if count > 100000 {
                return Err("Snapshot reclamation inventory limit exceeded".into());
            }
            let directory = journals.open_dir_nofollow(&name).map_err(storage::fail)?;
            let record: Journal = match directory.symlink_metadata("intent.json") {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                _ => serde_json::from_slice(&storage::read(
                    &directory,
                    "intent.json",
                    super::RECORD_LIMIT,
                )?)
                .map_err(storage::fail)?,
            };
            if ordinary(&record) || paired::eligible(&record) || cancelled::eligible(&record) {
                result.insert(
                    record
                        .location
                        .to_str()
                        .ok_or("Non-UTF8 snapshot Location")?
                        .to_owned(),
                );
            }
        }
    }
    Ok(result)
}

pub(crate) fn reclaim(
    store: Store,
    workspace: tokio::sync::OwnedMutexGuard<()>,
    config: Arc<crate::ConfigFn>,
    policy: Arc<dyn Policy>,
    id: &str,
) -> Result<u64, String> {
    if let Some(receipt) = store.preparation_cancellation(id)? {
        return cancelled::reclaim(store, workspace, config, policy, receipt);
    }
    let (directory, record) = store.journal(id)?;
    if paired::eligible(&record) {
        return paired::reclaim(store, workspace, config, policy, directory, record);
    }
    if !ordinary(&record) {
        return Ok(0);
    }
    let origin = record.origin.as_ref().ok_or("Missing reclamation origin")?;
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
    let frozen = existing.map(|record| record.plan);
    let prepared = prepare(
        store, directory, record, settlement, frozen, workspace, config,
    )?;
    let plan = prepared.plan.clone();
    if plan.files.is_empty() {
        return Ok(0);
    }
    policy.reclaim(plan, Box::new(move || prepared.delete()))
}
impl Store {
    pub(crate) fn reclamation_journals(&self) -> Result<Vec<String>, String> {
        storage::names(&self.journals, super::ENTRY_LIMIT)
    }
}

fn prepare(
    store: Store,
    journal: Dir,
    record: Journal,
    settlement: Settlement,
    frozen: Option<Plan>,
    workspace: tokio::sync::OwnedMutexGuard<()>,
    config: Arc<crate::ConfigFn>,
) -> Result<Prepared, String> {
    let receipt = settlement.receipt;
    let cyber_server::runtime::snapshot_reclamation::Receipt::Restore(restore) = &receipt else {
        return Err("Ordinary cleanup received a recovery receipt".into());
    };
    let origin = &restore.origin;
    store.verify_worker_namespace(origin)?;
    if store.worker_digest(Some(origin))?.as_ref() != Some(&restore.intent_digest) {
        return Err("Reclamation intent differs from committed restore".into());
    }
    let checkout = restore
        .checkout
        .as_ref()
        .ok_or("Legacy restore has no cleanup checkout proof")?;
    let lifetime = store.lock_lifetime(&checkout.lifetime)?;
    let resources = lifetime
        .checkout_bindings()?
        .ok_or("Missing acquired checkout inventory")?;
    if !resources.is_empty()
        || checkout.resources_digest != checkout.lifetime.checkout_resources_digest(&resources)?
    {
        return Err("Managed checkout reclamation requires retained resource guards".into());
    }
    unmanaged(&store)?;
    let ownership = record
        .ownership
        .as_ref()
        .ok_or("Legacy restore has no payload ownership")?;
    let mut files = Vec::new();
    let existing = frozen.as_ref().map(|plan| {
        plan.files
            .iter()
            .map(|file| (file.name.as_str(), file))
            .collect::<BTreeMap<_, _>>()
    });
    let mut known = BTreeSet::from(["intent.json".to_owned()]);
    for (index, change) in record.changes.iter().enumerate() {
        let name = format!("stage_{index}");
        known.insert(name.clone());
        let observed =
            ownership::observe_image(&store, Some(&journal), std::ffi::OsStr::new(&name))?;
        let expected = existing
            .as_ref()
            .and_then(|files| files.get(name.as_str()).copied());
        match (change.before.as_ref(), observed) {
            (Some(before), Some(image))
                if before.kind == super::ImageKind::File && image.entry == *before =>
            {
                let pin = storage::open(&journal, Path::new(&name))?;
                ownership.verify_original(index, &pin)?;
                let file = Payload {
                    journal: None,
                    name: name.clone(),
                    identity: image.identity,
                    generation: image.generation,
                    hash: image.entry.object,
                    bytes: image.entry.length,
                };
                if existing.is_some() && expected != Some(&file) {
                    return Err("Planned payload changed before reclamation".into());
                }
                files.push(file);
            }
            (Some(before), None)
                if expected.is_some_and(|file| {
                    file.hash == before.object
                        && file.bytes == before.length
                        && ownership.original_identity(index) == Some(file.identity.as_str())
                }) =>
            {
                files.push(expected.cloned().ok_or("Missing frozen payload")?)
            }
            (None, None) if expected.is_none() => {}
            _ => return Err("Displaced payload lacks exact original ownership".into()),
        }
    }
    for name in storage::names(&journal, super::ENTRY_LIMIT * 3)? {
        if !known.contains(&name) {
            return Err("Unknown journal file or construction stage; reclamation refused".into());
        }
    }
    let intent_digest = restore.intent_digest.clone();
    let plan = Plan {
        version: 1,
        intent_digest: None,
        receipt,
        files,
    };
    plan.validate().map_err(storage::fail)?;
    if frozen.as_ref().is_some_and(|frozen| frozen != &plan) {
        return Err("Reclamation plan differs from retained tombstone".into());
    }
    let intent = MetadataPin::open(&journal, "intent.json")?;
    let workers = storage::existing_child(&store.namespace, "workers", true)?;
    let binding = lifetime.binding();
    let name = binding
        .source_phase
        .as_deref()
        .ok_or("Missing reclamation source phase")?;
    let phase = MetadataPin::open(&workers, name)?;
    store.verify_reclamation_phase(
        binding,
        plan.receipt.origin().map_err(storage::fail)?,
        plan.receipt.owners().map_err(storage::fail)?[0].resources_digest,
    )?;
    phase.verify(&workers, name)?;
    storage::existing_child(
        &store.journals,
        &plan
            .receipt
            .origin()
            .map_err(storage::fail)?
            .binding
            .journal_id,
        true,
    )?;
    Ok(Prepared {
        store,
        journal,
        intent,
        cancellation: None,
        phase,
        workers,
        plan,
        lifetime,
        workspace,
        config,
        settled_at_ms: settlement.settled_at_ms,
        intent_digest,
    })
}
fn unmanaged(store: &Store) -> Result<(), String> {
    if !cyber_core::worktrees::Repository::managed_locations_at(&store.root)
        .map_err(storage::fail)?
        .is_empty()
    {
        return Err("Snapshot cleanup checkout inventory changed".into());
    }
    Ok(())
}
impl Prepared {
    fn verify_routes(&self) -> Result<(), String> {
        let _reader_lease = &self.workspace;
        self.lifetime.verify()?;
        self.store
            .verify_worker_namespace(self.plan.receipt.origin().map_err(storage::fail)?)?;
        if !due(self.config.as_ref(), &self.store.root, self.settled_at_ms) {
            return Err("Snapshot retention no longer permits reclamation".into());
        }
        self.intent.verify(&self.journal, "intent.json")?;
        if let Some(pin) = &self.cancellation {
            pin.verify(&self.journal, super::preparation::FILE)?;
        }
        self.phase.verify(
            &self.workers,
            self.lifetime
                .binding()
                .source_phase
                .as_deref()
                .ok_or("Missing source phase route")?,
        )?;
        unmanaged(&self.store)
    }
    fn verify(&self) -> Result<(), String> {
        self.verify_routes()?;
        let (_, record) = self.store.journal(
            &self
                .plan
                .receipt
                .origin()
                .map_err(storage::fail)?
                .binding
                .journal_id,
        )?;
        let eligible = match &self.plan.receipt {
            cyber_server::runtime::snapshot_reclamation::Receipt::Preparation(receipt) => {
                cancelled::verify(&self.store, receipt, &record)?
            }
            _ => ordinary(&record),
        };
        if !eligible
            || self
                .store
                .worker_digest(Some(self.plan.receipt.origin().map_err(storage::fail)?))?
                .as_ref()
                != Some(&self.intent_digest)
        {
            return Err("Acknowledged reclamation intent changed".into());
        }
        let names = self
            .plan
            .files
            .iter()
            .map(|file| file.name.as_str())
            .chain(std::iter::once("intent.json"))
            .chain(self.cancellation.iter().map(|_| super::preparation::FILE))
            .collect::<BTreeSet<_>>();
        if storage::names(&self.journal, super::ENTRY_LIMIT * 3)?
            .iter()
            .any(|name| !names.contains(name.as_str()))
        {
            return Err("Unknown journal entry appeared before reclamation".into());
        }
        unmanaged(&self.store)
    }
    fn delete(self) -> Result<u64, String> {
        self.verify()?;
        for file in &self.plan.files {
            self.verify_file(file)?;
        }
        let directory = self
            .journal
            .try_clone()
            .map_err(storage::fail)?
            .into_std_file();
        let mut generation = ownership::generation(&directory)?;
        for file in &self.plan.files {
            self.verify_routes()?;
            if ownership::generation(&directory)? != generation {
                return Err("Reclamation journal directory changed during deletion".into());
            }
            if let Some(_pin) = self.verify_file(file)? {
                self.journal
                    .remove_file(&file.name)
                    .map_err(storage::fail)?;
                storage::sync(&self.journal)?;
                generation = ownership::generation(&directory)?;
            }
        }
        // Full inventory/receipt checks surround the bounded per-leaf route
        // checks, avoiding a full intent decode for every displaced image.
        self.verify()?;
        storage::sync(&self.journal)?;
        self.plan.total_bytes().map_err(storage::fail)
    }
    fn verify_file(&self, file: &Payload) -> Result<Option<std::fs::File>, String> {
        match self.journal.symlink_metadata(&file.name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage::fail(error)),
            _ => {}
        }
        let pin = storage::open(&self.journal, Path::new(&file.name))?;
        if storage::identity(&pin)? != file.identity
            || ownership::generation(&pin)? != file.generation
        {
            return Err("Reclamation payload identity or metadata changed".into());
        }
        Ok(Some(pin))
    }
}
