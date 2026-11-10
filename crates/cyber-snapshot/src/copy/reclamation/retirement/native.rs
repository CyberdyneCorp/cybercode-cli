//! Native guards and fixed semantic routes; no source image is opened or installed.
use super::*;
use cyber_core::worktrees::CheckoutRetirementGuard;
use cyber_server::runtime::{SnapshotLifetimeBinding, SnapshotOrigin};
use metadata::{File as Entry, Plan as MetadataPlan, Role};
use std::os::unix::fs::FileExt;

struct Owner {
    origin: SnapshotOrigin,
    binding: SnapshotLifetimeBinding,
    resources_digest: String,
    journal: Option<Dir>,
    journal_generation: Option<String>,
    _lifetime: Option<super::super::super::lifetime::Lifetime>,
}
struct Leaf {
    entry: Entry,
    owner: usize,
    name: String,
    pin: Option<MetadataPin>,
}
pub(super) struct Prepared {
    pub plan: MetadataPlan,
    pub payload: records::Plan,
    store: Store,
    workers: Dir,
    workers_generation: String,
    owners: Vec<Owner>,
    checkouts: CheckoutRetirementGuard,
    leaves: Vec<Leaf>,
    _workspace: tokio::sync::OwnedMutexGuard<()>,
    config: Arc<crate::ConfigFn>,
    settled: i64,
}

impl Prepared {
    pub fn new(
        store: Store,
        workspace: tokio::sync::OwnedMutexGuard<()>,
        config: Arc<crate::ConfigFn>,
        settled: i64,
        record: records::Record,
    ) -> Result<Self, String> {
        let frozen = record.metadata;
        if let Some(plan) = &frozen {
            plan.validate(&record.plan).map_err(storage::fail)?;
        }
        let workers = storage::existing_child(&store.namespace, "workers", true)?;
        let mut owners = Vec::new();
        for evidence in record.plan.receipt.owners().map_err(storage::fail)? {
            validate_names(evidence.origin, evidence.lifetime)?;
            let journal = existing_journal(&store, evidence.origin, frozen.is_some())?;
            let lifetime = match workers.symlink_metadata(&evidence.lifetime.owner.activity_id) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && frozen.is_some() => {
                    None
                }
                _ => Some(store.lock_lifetime(evidence.lifetime)?),
            };
            let journal_generation = journal.as_ref().map(directory_generation).transpose()?;
            owners.push(Owner {
                origin: evidence.origin.clone(),
                binding: evidence.lifetime.clone(),
                resources_digest: evidence.resources_digest.to_owned(),
                journal,
                journal_generation,
                _lifetime: lifetime,
            });
        }
        let checkouts = match &frozen {
            Some(plan) => CheckoutRetirementGuard::resume_committed(plan.checkouts.clone())
                .map_err(storage::fail)?,
            None => {
                let mut claims = Vec::new();
                for owner in &owners {
                    let lifetime = owner
                        ._lifetime
                        .as_ref()
                        .ok_or("Missing original metadata lifetime")?;
                    let proof = super::super::checkouts::Checkouts::acquire(
                        &store,
                        lifetime,
                        &owner.binding,
                        &owner.resources_digest,
                        &super::super::checkouts::receipt_digest(&record.plan.receipt)?,
                    )?;
                    claims.extend(proof.into_claims());
                }
                CheckoutRetirementGuard::prepare(claims).map_err(storage::fail)?
            }
        };
        let entries = match &frozen {
            Some(plan) => plan.files.clone(),
            None => capture(&store, &workers, &owners, &record.plan.receipt)?,
        };
        let plan = MetadataPlan {
            version: if checkouts.plans().is_empty() { 1 } else { 2 },
            payload_plan_id: records::Plan::id(&record.plan.receipt).map_err(storage::fail)?,
            files: entries,
            checkouts: checkouts.plans(),
        };
        plan.validate(&record.plan).map_err(storage::fail)?;
        if frozen.as_ref().is_some_and(|frozen| *frozen != plan) {
            return Err("Metadata retirement plan changed".into());
        }
        let mut leaves = Vec::new();
        for entry in &plan.files {
            let owner = owners
                .iter()
                .position(|owner| owner.origin.activity_id == entry.owner)
                .ok_or("Foreign metadata worker")?;
            let name = name(entry.role, &owners[owner])?.to_owned();
            let dir = leaf_directory(&owners[owner], &workers, entry.role);
            let pin = pin_entry(dir, &name, entry, frozen.is_some())?;
            leaves.push(Leaf {
                entry: entry.clone(),
                owner,
                name,
                pin,
            });
        }
        // Freeze native identities/generations first, then re-attest original
        // typed intent/phase context. verify() rejects any subsequent mutation.
        if frozen.is_none() {
            super::verify_initial(&store, &record.plan)?;
        }
        leaves.sort_by_key(|leaf| priority(leaf.entry.role));
        let prepared = Self {
            workers_generation: directory_generation(&workers)?,
            store,
            workers,
            owners,
            checkouts,
            leaves,
            plan,
            payload: record.plan,
            _workspace: workspace,
            config,
            settled,
        };
        prepared.verify()?;
        Ok(prepared)
    }
    fn verify(&self) -> Result<(), String> {
        self.store.verify_root()?;
        if !due(self.config.as_ref(), &self.store.root, self.settled) {
            return Err("Snapshot retention no longer permits metadata retirement".into());
        }
        self.verify_checkouts()?;
        let fresh = Store::existing(
            &self.store.data_path,
            &self.store.root.to_string_lossy(),
            self.store.options.clone(),
        )?
        .ok_or("Metadata namespace disappeared")?;
        let fresh_workers = storage::existing_child(&fresh.namespace, "workers", true)?;
        for owner in &self.owners {
            self.verify_owner(owner, &fresh, &fresh_workers)?;
        }
        if directory_generation(&self.workers)? != self.workers_generation {
            return Err("Metadata worker directory changed".into());
        }
        for leaf in &self.leaves {
            let dir = leaf_directory(&self.owners[leaf.owner], &self.workers, leaf.entry.role);
            match (&leaf.pin, dir) {
                (Some(pin), Some(dir)) => pin.verify(dir, &leaf.name)?,
                (None, Some(dir)) => absent(dir, &leaf.name)?,
                (None, None) => {}
                _ => return Err("Metadata journal disappeared during retirement".into()),
            }
        }
        self.inventory()
    }
    fn verify_checkouts(&self) -> Result<(), String> {
        let current = cyber_core::worktrees::Repository::managed_locations_at(&self.store.root)
            .map_err(storage::fail)?;
        for owner in &self.owners {
            let plans = self
                .plan
                .checkouts
                .iter()
                .filter(|plan| plan.binding.owner_id == owner.origin.activity_id)
                .collect::<Vec<_>>();
            if current.len() != plans.len()
                || !current.iter().zip(plans).all(|((_, managed), plan)| {
                    managed.common_dir == plan.binding.common_dir
                        && managed.id == plan.binding.worktree_id
                        && managed.name == plan.binding.name
                })
            {
                return Err("Metadata retirement checkout registration changed".into());
            }
        }
        self.checkouts.verify().map_err(storage::fail)
    }
    fn verify_owner(
        &self,
        owner: &Owner,
        fresh: &Store,
        fresh_workers: &Dir,
    ) -> Result<(), String> {
        self.store.verify_lock_route(
            owner
                .origin
                .binding
                .lock_identity
                .as_deref()
                .ok_or("Legacy metadata lock binding")?,
        )?;
        if owner.origin.binding.directory != self.store.root
            || owner.origin.binding.root_identity != self.store.root_id
            || owner.binding.root_identity != self.store.root_id
            || owner.binding.namespace_identity != storage::dir_identity(&fresh.namespace)?
            || owner.binding.namespace_identity != storage::dir_identity(&self.store.namespace)?
            || owner.binding.directory_identity != storage::dir_identity(fresh_workers)?
            || owner.binding.directory_identity != storage::dir_identity(&self.workers)?
            || owner.origin.binding.namespace_identity != storage::dir_identity(&fresh.journals)?
            || owner.origin.binding.namespace_identity
                != storage::dir_identity(&self.store.journals)?
        {
            return Err("Metadata retirement namespace route changed".into());
        }
        match &owner.journal {
            Some(journal) => {
                let route = storage::existing_child(
                    &fresh.journals,
                    &owner.origin.binding.journal_id,
                    true,
                )?;
                if storage::dir_identity(&route)? != owner.origin.binding.journal_identity
                    || storage::dir_identity(journal)? != owner.origin.binding.journal_identity
                    || Some(directory_generation(journal)?) != owner.journal_generation
                {
                    return Err("Metadata retirement journal route or generation changed".into());
                }
            }
            None => absent(&fresh.journals, &owner.origin.binding.journal_id)?,
        }
        Ok(())
    }
    fn inventory(&self) -> Result<(), String> {
        for (index, owner) in self.owners.iter().enumerate() {
            if let Some(journal) = &owner.journal {
                let known = self
                    .leaves
                    .iter()
                    .filter(|leaf| leaf.owner == index && journal_role(leaf.entry.role))
                    .map(|leaf| leaf.name.as_str())
                    .collect::<BTreeSet<_>>();
                if storage::names(journal, super::super::super::ENTRY_LIMIT * 3)?
                    .iter()
                    .any(|name| !known.contains(name.as_str()))
                {
                    return Err("Unknown journal entry prevents metadata retirement".into());
                }
            }
        }
        Ok(())
    }
    pub fn delete(mut self) -> Result<u64, String> {
        self.verify()?;
        loop {
            self.verify()?;
            if !self.checkouts.remove_next().map_err(storage::fail)? {
                break;
            }
        }
        for index in 0..self.leaves.len() {
            self.verify()?;
            let leaf = &self.leaves[index];
            if leaf.pin.is_some() {
                let dir = leaf_directory(&self.owners[leaf.owner], &self.workers, leaf.entry.role)
                    .ok_or("Missing metadata directory")?;
                dir.remove_file(&leaf.name).map_err(storage::fail)?;
                storage::sync(dir)?;
                let generation = directory_generation(dir)?;
                if journal_role(leaf.entry.role) {
                    self.owners[leaf.owner].journal_generation = Some(generation);
                } else {
                    self.workers_generation = generation;
                }
                self.leaves[index].pin = None;
            }
        }
        for index in 0..self.owners.len() {
            self.verify()?;
            if self.owners[index].journal.is_some() {
                self.store
                    .journals
                    .remove_dir(&self.owners[index].origin.binding.journal_id)
                    .map_err(storage::fail)?;
                storage::sync(&self.store.journals)?;
                self.owners[index].journal = None;
                self.owners[index].journal_generation = None;
            }
        }
        // Resumption can observe already missing names before any unlink here;
        // synchronize their exact parent directories before durable completion.
        storage::sync(&self.workers)?;
        storage::sync(&self.store.journals)?;
        self.verify()?;
        self.plan.total_bytes().map_err(storage::fail)
    }
}

fn journal_role(role: Role) -> bool {
    matches!(role, Role::Intent | Role::Spare | Role::Cancellation)
}
fn priority(role: Role) -> u8 {
    match role {
        Role::Intent => 0,
        Role::Spare => 1,
        Role::Cancellation => 2,
        Role::SourcePhase => 3,
        Role::Lifetime => 4,
    }
}
fn name(role: Role, owner: &Owner) -> Result<&str, String> {
    match role {
        Role::Intent => Ok("intent.json"),
        Role::Spare => Ok("intent.next"),
        Role::Cancellation => Ok("preparation.json"),
        Role::Lifetime => Ok(&owner.binding.owner.activity_id),
        Role::SourcePhase => owner
            .binding
            .source_phase
            .as_deref()
            .ok_or("Missing metadata source phase".into()),
    }
}
fn leaf_directory<'a>(owner: &'a Owner, workers: &'a Dir, role: Role) -> Option<&'a Dir> {
    if journal_role(role) {
        owner.journal.as_ref()
    } else {
        Some(workers)
    }
}
fn directory_generation(dir: &Dir) -> Result<String, String> {
    ownership::generation(&dir.try_clone().map_err(storage::fail)?.into_std_file())
}
fn absent(dir: &Dir, name: &str) -> Result<(), String> {
    match dir.symlink_metadata(name) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(storage::fail(error)),
        _ => Err("Retired metadata route was replaced".into()),
    }
}
fn validate_names(
    origin: &SnapshotOrigin,
    binding: &SnapshotLifetimeBinding,
) -> Result<(), String> {
    let component = |name: &str, limit| {
        !name.is_empty()
            && name.len() <= limit
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    };
    if !origin.binding.journal_id.starts_with("restore_")
        || !component(&origin.binding.journal_id, 256)
        || !component(&origin.activity_id, 128)
        || binding.owner.activity_id != origin.activity_id
        || binding
            .source_phase
            .as_deref()
            .and_then(|name| name.strip_prefix("phase_"))
            .is_none_or(|id| id.len() != 26 || id.parse::<ulid::Ulid>().is_err())
    {
        return Err("Invalid metadata retirement component".into());
    }
    Ok(())
}
fn existing_journal(
    store: &Store,
    origin: &SnapshotOrigin,
    resuming: bool,
) -> Result<Option<Dir>, String> {
    match store.journals.symlink_metadata(&origin.binding.journal_id) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && resuming => Ok(None),
        _ => {
            let dir = storage::existing_child(&store.journals, &origin.binding.journal_id, true)?;
            if storage::dir_identity(&dir)? != origin.binding.journal_identity {
                return Err("Metadata journal identity changed".into());
            }
            Ok(Some(dir))
        }
    }
}
fn content_hash(file: &std::fs::File) -> Result<String, String> {
    let length = file.metadata().map_err(storage::fail)?.len();
    if length > super::super::super::RECORD_LIMIT {
        return Err("Metadata file exceeds retirement bound".into());
    }
    let mut hash = blake3::Hasher::new();
    let mut offset = 0;
    let mut bytes = [0; 8192];
    while offset < length {
        let limit = (length - offset).min(bytes.len() as u64) as usize;
        let read = file
            .read_at(&mut bytes[..limit], offset)
            .map_err(storage::fail)?;
        if read == 0 {
            return Err("Metadata file shortened while hashing".into());
        }
        hash.update(&bytes[..read]);
        offset += read as u64;
    }
    Ok(hash.finalize().to_hex().to_string())
}
fn pin_entry(
    dir: Option<&Dir>,
    name: &str,
    entry: &Entry,
    resuming: bool,
) -> Result<Option<MetadataPin>, String> {
    let Some(dir) = dir else {
        return if resuming {
            Ok(None)
        } else {
            Err("Missing metadata journal".into())
        };
    };
    if let Err(error) = dir.symlink_metadata(name) {
        if error.kind() == std::io::ErrorKind::NotFound && resuming {
            return Ok(None);
        }
        return Err(storage::fail(error));
    }
    let pin = MetadataPin::open(dir, name)?;
    if pin.identity != entry.identity
        || pin.generation != entry.generation
        || pin.file.metadata().map_err(storage::fail)?.len() != entry.bytes
        || content_hash(&pin.file)? != entry.hash
    {
        return Err("Metadata retirement file identity, generation or contents changed".into());
    }
    pin.verify(dir, name)?;
    Ok(Some(pin))
}
fn capture(
    store: &Store,
    workers: &Dir,
    owners: &[Owner],
    receipt: &Receipt,
) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    for owner in owners {
        let (_, journal) = store.journal(&owner.origin.binding.journal_id)?;
        let mut roles = vec![Role::Intent, Role::SourcePhase, Role::Lifetime];
        if journal.updates.is_some() {
            roles.push(Role::Spare);
        }
        if matches!(receipt, Receipt::Preparation(_)) {
            roles.push(Role::Cancellation);
        }
        for role in roles {
            let name = name(role, owner)?;
            let dir = leaf_directory(owner, workers, role).ok_or("Missing retirement journal")?;
            let pin = MetadataPin::open(dir, name)?;
            let entry = Entry {
                owner: owner.origin.activity_id.clone(),
                role,
                identity: pin.identity.clone(),
                generation: pin.generation.clone(),
                hash: content_hash(&pin.file)?,
                bytes: pin.file.metadata().map_err(storage::fail)?.len(),
            };
            pin.verify(dir, name)?;
            entries.push(entry);
        }
    }
    Ok(entries)
}
