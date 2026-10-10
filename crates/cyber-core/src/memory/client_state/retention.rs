//! Persisted bounded cleanup authority; absence resumes, replacements never do.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
const CLEANUP: &str = ".checkpoint-cleanup.json";
const KEEP: usize = 2;
const BATCH: usize = 8;
const SCAN_LIMIT: usize = 4096;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    name: String,
    id: native::FileIdentity,
    after: native::FileIdentity,
    files: BTreeMap<String, Proof>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    version: u32,
    id: native::FileIdentity,
    state: native::FileIdentity,
    client: native::FileIdentity,
    lock: native::FileIdentity,
    history: native::FileIdentity,
    targets: Vec<Target>,
}
struct Cleanup {
    plan: Plan,
    owner: native::RetainedChild,
}
pub(super) fn recover(store: &MemoryClientStore) -> Result<(), MemoryStorageError> {
    if let Some(cleanup) = Cleanup::open(store)? {
        cleanup.run(store)?;
    }
    Ok(())
}
pub(super) fn prune(store: &MemoryClientStore) -> Result<(), MemoryStorageError> {
    store.verify_binding()?;
    let Some(history) = child_directory(&store.dir, HISTORY, false)? else {
        return Ok(());
    };
    loop {
        let names = candidates(&history)?;
        if names.len() <= KEEP {
            return Ok(());
        }
        let mut targets = Vec::new();
        for name in names {
            targets.push(capture(store, &history, &name)?);
        }
        let current = snapshot(&store.dir, CHECKPOINT)?.map(|proof| proof.id);
        let expired = select(targets, current)?;
        Cleanup::prepare(store, &history, expired)?.run(store)?;
    }
}
fn canonical(name: &str) -> bool {
    name.strip_prefix("mcs_")
        .and_then(|suffix| {
            ulid::Ulid::from_string(suffix)
                .ok()
                .map(|id| id.to_string() == suffix)
        })
        .unwrap_or(false)
}
fn candidates(history: &Dir) -> Result<Vec<String>, MemoryStorageError> {
    let mut names = Vec::new();
    for (index, entry) in history.entries()?.enumerate() {
        if index >= SCAN_LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        if let Ok(name) = entry?.file_name().into_string()
            && canonical(&name)
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}
fn select(
    targets: Vec<Target>,
    current: Option<native::FileIdentity>,
) -> Result<Vec<Target>, MemoryStorageError> {
    let protected = targets
        .iter()
        .filter(|target| Some(target.after) == current)
        .count();
    if protected > 1 {
        return Err(MemoryStorageError::ReviewConflict);
    }
    let others = targets.len() - protected;
    let expired = others.saturating_sub(KEEP - protected);
    Ok(targets
        .into_iter()
        .filter(|target| Some(target.after) != current)
        .take(expired.min(BATCH))
        .collect())
}
fn keys(dir: &Dir, files: &BTreeMap<String, Proof>) -> Result<(), MemoryStorageError> {
    for (index, entry) in dir.entries()?.enumerate() {
        if index >= 3 {
            return Err(MemoryStorageError::ReviewConflict);
        }
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| MemoryStorageError::ReviewConflict)?;
        if !files.contains_key(&name) {
            return Err(MemoryStorageError::ReviewConflict);
        }
    }
    Ok(())
}
fn capture(
    store: &MemoryClientStore,
    history: &Dir,
    name: &str,
) -> Result<Target, MemoryStorageError> {
    let dir = child_directory(history, name, false)?.ok_or(MemoryStorageError::ReviewConflict)?;
    let id = native::identity(&descriptor(&dir)?)?;
    let observed =
        native::open_private_file(&descriptor(&dir)?, "intent.json", native::Access::Read)?;
    let intent_id = native::identity(&observed)?;
    drop(observed);
    let frozen = native::freeze_private_file(&descriptor(&dir)?, "intent.json", intent_id)?;
    let bytes = bounded(&frozen, INTENT_LIMIT)?;
    let intent: Intent =
        serde_json::from_slice(&bytes).map_err(|_| MemoryStorageError::RecoveryRequired)?;
    archive_context(store, name, id, intent_id, &intent)?;
    if serde_json::to_vec(&intent).map_err(|_| MemoryStorageError::RecoveryRequired)? != bytes {
        return Err(MemoryStorageError::ReviewConflict);
    }
    let marker = snapshot(&dir, "completed")?.ok_or(MemoryStorageError::RecoveryRequired)?;
    if marker.digest != digest(b"committed\n") || marker.length != 10 {
        return Err(MemoryStorageError::RecoveryRequired);
    }
    if snapshot(&dir, "before")? != intent.before || snapshot(&dir, "after")?.is_some() {
        return Err(MemoryStorageError::ReviewConflict);
    }
    let mut files = BTreeMap::from([
        (
            "intent.json".into(),
            Proof {
                id: intent_id,
                digest: digest(&bytes),
                length: bytes.len() as u64,
            },
        ),
        ("completed".into(), marker),
    ]);
    if let Some(before) = intent.before {
        files.insert("before".into(), before);
    }
    keys(&dir, &files)?;
    Ok(Target {
        name: name.into(),
        id,
        after: intent.after.id,
        files,
    })
}
fn archive_context(
    store: &MemoryClientStore,
    name: &str,
    id: native::FileIdentity,
    intent_id: native::FileIdentity,
    intent: &Intent,
) -> Result<(), MemoryStorageError> {
    let objects = [
        (intent.state, native::identity(&descriptor(&store.parent)?)?),
        (intent.client, native::identity(&descriptor(&store.dir)?)?),
        (intent.lock, native::identity(&store.lock)?),
        (intent.journal, id),
        (intent.intent, intent_id),
    ];
    if intent.version != 1
        || intent.id != name
        || objects.into_iter().any(|(want, actual)| want != actual)
    {
        return Err(MemoryStorageError::ReviewConflict);
    }
    Ok(())
}
impl Cleanup {
    fn prepare(
        store: &MemoryClientStore,
        history: &Dir,
        targets: Vec<Target>,
    ) -> Result<Self, MemoryStorageError> {
        store.verify_binding()?;
        let mut file = native::create_private_file(&descriptor(&store.dir)?, CLEANUP)?;
        let plan = Plan {
            version: 1,
            id: native::identity(&file)?,
            state: native::identity(&descriptor(&store.parent)?)?,
            client: native::identity(&descriptor(&store.dir)?)?,
            lock: native::identity(&store.lock)?,
            history: native::identity(&descriptor(history)?)?,
            targets,
        };
        let bytes = encoded(&plan)?;
        file.write_all(&bytes)?;
        native::sync_private(&file)?;
        drop(file);
        let owner = native::retain_private_file(&descriptor(&store.dir)?, CLEANUP, plan.id)?;
        store.sync()?;
        let result = Self { plan, owner };
        result.verify(store, history)?;
        result.preflight(history)?;
        Ok(result)
    }
    fn open(store: &MemoryClientStore) -> Result<Option<Self>, MemoryStorageError> {
        let observed = match native::open_private_file(
            &descriptor(&store.dir)?,
            CLEANUP,
            native::Access::Read,
        ) {
            Ok(file) => file,
            Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let id = native::identity(&observed)?;
        drop(observed);
        let owner = native::retain_private_file(&descriptor(&store.dir)?, CLEANUP, id)?;
        let plan = serde_json::from_slice(&bounded(owner.file(), INTENT_LIMIT)?)
            .map_err(|_| MemoryStorageError::RecoveryRequired)?;
        let result = Self { plan, owner };
        let history = child_directory(&store.dir, HISTORY, false)?
            .ok_or(MemoryStorageError::ReviewConflict)?;
        result.verify(store, &history)?;
        result.preflight(&history)?;
        Ok(Some(result))
    }
    fn verify(&self, store: &MemoryClientStore, history: &Dir) -> Result<(), MemoryStorageError> {
        store.verify_binding()?;
        shape(&self.plan)?;
        let current = snapshot(&store.dir, CHECKPOINT)?.map(|proof| proof.id);
        if self
            .plan
            .targets
            .iter()
            .any(|target| Some(target.after) == current)
        {
            return Err(MemoryStorageError::ReviewConflict);
        }
        let objects = [
            (
                self.plan.state,
                native::identity(&descriptor(&store.parent)?)?,
            ),
            (
                self.plan.client,
                native::identity(&descriptor(&store.dir)?)?,
            ),
            (self.plan.lock, native::identity(&store.lock)?),
            (self.plan.history, native::identity(&descriptor(history)?)?),
            (self.plan.id, native::identity(self.owner.file())?),
        ];
        if objects.into_iter().any(|(want, actual)| want != actual) {
            return Err(MemoryStorageError::ReviewConflict);
        }
        let file =
            native::open_private_file(&descriptor(&store.dir)?, CLEANUP, native::Access::Read)?;
        if native::identity(&file)? != self.plan.id
            || bounded(&file, INTENT_LIMIT)? != encoded(&self.plan)?
        {
            return Err(MemoryStorageError::ReviewConflict);
        }
        Ok(())
    }
    fn preflight(&self, history: &Dir) -> Result<(), MemoryStorageError> {
        for target in &self.plan.targets {
            if let Some(dir) = child_directory(history, &target.name, false)? {
                if native::identity(&descriptor(&dir)?)? != target.id {
                    return Err(MemoryStorageError::ReviewConflict);
                }
                remaining(&dir, target)?;
            }
        }
        Ok(())
    }
    fn run(self, store: &MemoryClientStore) -> Result<(), MemoryStorageError> {
        let history = child_directory(&store.dir, HISTORY, false)?
            .ok_or(MemoryStorageError::ReviewConflict)?;
        self.verify(store, &history)?;
        self.preflight(&history)?;
        for target in &self.plan.targets {
            self.verify(store, &history)?;
            remove_target(&history, target)?;
        }
        native::sync_private(&descriptor(&history)?)?;
        store.sync()?;
        let bytes = encoded(&self.plan)?;
        let proof = Proof {
            id: self.plan.id,
            digest: digest(&bytes),
            length: bytes.len() as u64,
        };
        drop(self.owner);
        let source = native::dispose_private_file(&descriptor(&store.dir)?, CLEANUP, proof.id)?;
        checked(source.file(), &proof)?;
        source.remove_durable()?;
        store.sync()
    }
}
fn encoded(plan: &Plan) -> Result<Vec<u8>, MemoryStorageError> {
    let bytes = serde_json::to_vec(plan).map_err(|_| MemoryStorageError::RecoveryRequired)?;
    if bytes.len() as u64 > INTENT_LIMIT {
        return Err(MemoryStorageError::TooLarge);
    }
    Ok(bytes)
}
fn shape(plan: &Plan) -> Result<(), MemoryStorageError> {
    if plan.version != 1 || plan.targets.is_empty() || plan.targets.len() > BATCH {
        return Err(MemoryStorageError::RecoveryRequired);
    }
    let mut names = BTreeSet::new();
    for target in &plan.targets {
        if !canonical(&target.name) || !names.insert(&target.name) {
            return Err(MemoryStorageError::RecoveryRequired);
        }
        file_shape(&target.files)?;
    }
    Ok(())
}
fn file_shape(files: &BTreeMap<String, Proof>) -> Result<(), MemoryStorageError> {
    if !files.contains_key("intent.json") || !files.contains_key("completed") || files.len() > 3 {
        return Err(MemoryStorageError::RecoveryRequired);
    }
    let marker = &files["completed"];
    if marker.length != 10
        || marker.digest != digest(b"committed\n")
        || files["intent.json"].length > INTENT_LIMIT
    {
        return Err(MemoryStorageError::RecoveryRequired);
    }
    for (name, proof) in files {
        if !matches!(name.as_str(), "intent.json" | "completed" | "before")
            || proof.length > LIMIT as u64
            || proof.digest.len() != 64
            || !proof
                .digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(MemoryStorageError::RecoveryRequired);
        }
    }
    Ok(())
}
fn remaining(dir: &Dir, target: &Target) -> Result<(), MemoryStorageError> {
    keys(dir, &target.files)?;
    remaining_proofs(dir, target)
}
fn remaining_proofs(dir: &Dir, target: &Target) -> Result<(), MemoryStorageError> {
    for (name, proof) in &target.files {
        if let Some(actual) = snapshot(dir, name)?
            && actual != *proof
        {
            return Err(MemoryStorageError::ReviewConflict);
        }
    }
    Ok(())
}
fn remove_target(history: &Dir, target: &Target) -> Result<(), MemoryStorageError> {
    let directory =
        match native::dispose_private_directory(&descriptor(history)?, &target.name, target.id) {
            Ok(source) => source,
            Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
    for name in directory.child_names(3)? {
        if !target.files.contains_key(&name) {
            return Err(MemoryStorageError::ReviewConflict);
        }
    }
    {
        let dir = Dir::from_std_file(directory.file().try_clone()?);
        remaining_proofs(&dir, target)?;
    }
    for (name, proof) in &target.files {
        let source = match native::dispose_private_file(directory.file(), name, proof.id) {
            Ok(source) => source,
            Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => return Err(error),
        };
        checked(source.file(), proof)?;
        source.remove_durable()?;
    }
    directory.remove_durable()
}
#[cfg(test)]
#[path = "retention_tests.rs"]
mod tests;
