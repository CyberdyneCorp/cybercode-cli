//! Exact-object checkpoint capture/install and restart recovery; activation stays gated.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
const PENDING: &str = ".checkpoint-pending";
const HISTORY: &str = ".checkpoint-history";
const INTENT_LIMIT: u64 = 16 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    id: native::FileIdentity,
    digest: String,
    length: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    id: String,
    state: native::FileIdentity,
    client: native::FileIdentity,
    lock: native::FileIdentity,
    journal: native::FileIdentity,
    intent: native::FileIdentity,
    before: Option<Proof>,
    after: Proof,
}
struct Journal {
    dir: Dir,
    intent: Intent,
    owner: native::RetainedChild,
}
#[derive(PartialEq, Eq)]
enum Phase {
    Original,
    Captured,
    Installed,
}
pub(in crate::memory::client_state) fn save(
    store: &mut MemoryClientStore,
    bytes: &[u8],
) -> Result<(), MemoryStorageError> {
    if bytes.len() > LIMIT {
        return Err(MemoryStorageError::TooLarge);
    }
    store.verify_binding()?;
    if child_directory(&store.dir, PENDING, false)?.is_some() {
        return Err(MemoryStorageError::RecoveryRequired);
    }
    if store.read()? != store.previous {
        return Err(MemoryStorageError::ReviewConflict);
    }
    if store.checkpoint() == Some(bytes) {
        let proof = snapshot(&store.dir, CHECKPOINT)?.ok_or(MemoryStorageError::ReviewConflict)?;
        if Some(proof.clone()) != proof_of(store.previous.as_ref())? {
            return Err(MemoryStorageError::ReviewConflict);
        }
        let source = retain(&store.dir, CHECKPOINT, &proof)?;
        native::sync_private(source.file())?;
        return store.sync();
    }
    let journal = Journal::prepare(store, bytes)?;
    let expected = journal.intent.after.clone();
    journal.commit(store)?;
    let restored = store.read()?;
    if proof_of(restored.as_ref())? != Some(expected) {
        return Err(MemoryStorageError::ReviewConflict);
    }
    store.previous = restored;
    Ok(())
}
pub(in crate::memory::client_state) fn recover(
    store: &MemoryClientStore,
) -> Result<(), MemoryStorageError> {
    if let Some(journal) = Journal::open(store)? {
        journal.commit(store)?;
    }
    Ok(())
}
fn proof_of(checkpoint: Option<&Checkpoint>) -> Result<Option<Proof>, MemoryStorageError> {
    checkpoint
        .map(|checkpoint| {
            let Some(ObjectIdentity::Windows(id)) = checkpoint.identity else {
                return Err(MemoryStorageError::ReviewConflict);
            };
            Ok(Proof {
                id,
                digest: digest(&checkpoint.bytes),
                length: checkpoint.bytes.len() as u64,
            })
        })
        .transpose()
}
fn descriptor(dir: &Dir) -> Result<File, MemoryStorageError> {
    Ok(dir.try_clone()?.into_std_file())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn bounded(file: &File, limit: u64) -> Result<Vec<u8>, MemoryStorageError> {
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(MemoryStorageError::TooLarge);
    }
    Ok(bytes)
}
fn snapshot(dir: &Dir, name: &str) -> Result<Option<Proof>, MemoryStorageError> {
    let parent = descriptor(dir)?;
    let observed = match native::open_private_file(&parent, name, native::Access::Read) {
        Ok(file) => file,
        Err(MemoryStorageError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let id = native::identity(&observed)?;
    drop(observed);
    let frozen = native::freeze_private_file(&parent, name, id)?;
    let bytes = bounded(&frozen, LIMIT as u64)?;
    Ok(Some(Proof {
        id,
        digest: digest(&bytes),
        length: bytes.len() as u64,
    }))
}
fn checked(file: &File, proof: &Proof) -> Result<(), MemoryStorageError> {
    if proof.length > LIMIT as u64 || native::identity(file)? != proof.id {
        return Err(MemoryStorageError::ReviewConflict);
    }
    let bytes = bounded(file, LIMIT as u64)?;
    if bytes.len() as u64 != proof.length || digest(&bytes) != proof.digest {
        return Err(MemoryStorageError::ReviewConflict);
    }
    Ok(())
}
fn retain(
    dir: &Dir,
    name: &str,
    proof: &Proof,
) -> Result<native::RetainedChild, MemoryStorageError> {
    let owner = native::retain_private_file(&descriptor(dir)?, name, proof.id)?;
    checked(owner.file(), proof)?;
    Ok(owner)
}
fn freeze(dir: &Dir, name: &str, proof: &Proof) -> Result<File, MemoryStorageError> {
    let file = native::freeze_private_file(&descriptor(dir)?, name, proof.id)?;
    checked(&file, proof)?;
    Ok(file)
}
fn create(dir: &Dir, name: &str, bytes: &[u8]) -> Result<native::FileIdentity, MemoryStorageError> {
    let mut file = native::create_private_file(&descriptor(dir)?, name)?;
    file.write_all(bytes)?;
    native::sync_private(&file)?;
    native::identity(&file)
}
fn move_file(
    from: &Dir,
    name: &str,
    to: &Dir,
    target: &str,
    proof: &Proof,
) -> Result<(), MemoryStorageError> {
    retain(from, name, proof)?.rename_to_durable(&descriptor(to)?, target)
}
impl Journal {
    fn prepare(store: &MemoryClientStore, bytes: &[u8]) -> Result<Self, MemoryStorageError> {
        store.verify_binding()?;
        let before = snapshot(&store.dir, CHECKPOINT)?;
        if store.read()? != store.previous || before != proof_of(store.previous.as_ref())? {
            return Err(MemoryStorageError::ReviewConflict);
        }
        let created = native::create_private_directory(&descriptor(&store.dir)?, PENDING)?;
        let journal = native::identity(&created)?;
        native::sync_private(&created)?;
        drop(created);
        let dir = child_directory(&store.dir, PENDING, false)?
            .ok_or(MemoryStorageError::RecoveryRequired)?;
        if native::identity(&descriptor(&dir)?)? != journal {
            return Err(MemoryStorageError::ReviewConflict);
        }
        let after = Proof {
            id: create(&dir, "after", bytes)?,
            digest: digest(bytes),
            length: bytes.len() as u64,
        };
        let mut intent_file = native::create_private_file(&descriptor(&dir)?, "intent.json")?;
        let intent = Intent {
            version: 1,
            id: crate::ids::new_id("mcs"),
            state: native::identity(&descriptor(&store.parent)?)?,
            client: native::identity(&descriptor(&store.dir)?)?,
            lock: native::identity(&store.lock)?,
            journal,
            intent: native::identity(&intent_file)?,
            before,
            after,
        };
        intent_file.write_all(
            &serde_json::to_vec(&intent).map_err(|_| MemoryStorageError::RecoveryRequired)?,
        )?;
        native::sync_private(&intent_file)?;
        drop(intent_file);
        let owner = native::retain_private_file(&descriptor(&dir)?, "intent.json", intent.intent)?;
        native::sync_private(&descriptor(&dir)?)?;
        store.sync()?;
        let prepared = Self { dir, intent, owner };
        prepared.verify(store)?;
        prepared.phase(store)?;
        Ok(prepared)
    }
    fn open(store: &MemoryClientStore) -> Result<Option<Self>, MemoryStorageError> {
        store.verify_binding()?;
        let Some(dir) = child_directory(&store.dir, PENDING, false)? else {
            return Ok(None);
        };
        let observed =
            native::open_private_file(&descriptor(&dir)?, "intent.json", native::Access::Read)?;
        let id = native::identity(&observed)?;
        drop(observed);
        let owner = native::retain_private_file(&descriptor(&dir)?, "intent.json", id)?;
        let intent: Intent = serde_json::from_slice(&bounded(owner.file(), INTENT_LIMIT)?)
            .map_err(|_| MemoryStorageError::RecoveryRequired)?;
        let result = Self { dir, intent, owner };
        result.verify(store)?;
        result.phase(store)?;
        Ok(Some(result))
    }
    fn verify(&self, store: &MemoryClientStore) -> Result<(), MemoryStorageError> {
        store.verify_binding()?;
        let objects = [
            (
                self.intent.state,
                native::identity(&descriptor(&store.parent)?)?,
            ),
            (
                self.intent.client,
                native::identity(&descriptor(&store.dir)?)?,
            ),
            (self.intent.lock, native::identity(&store.lock)?),
            (
                self.intent.journal,
                native::identity(&descriptor(&self.dir)?)?,
            ),
            (self.intent.intent, native::identity(self.owner.file())?),
        ];
        if self.intent.version != 1
            || self.intent.after.length > LIMIT as u64
            || objects
                .into_iter()
                .any(|(recorded, actual)| recorded != actual)
        {
            return Err(MemoryStorageError::ReviewConflict);
        }
        let current = child_directory(&store.dir, PENDING, false)?
            .ok_or(MemoryStorageError::ReviewConflict)?;
        if native::identity(&descriptor(&current)?)? != self.intent.journal {
            return Err(MemoryStorageError::ReviewConflict);
        }
        let observed = native::open_private_file(
            &descriptor(&self.dir)?,
            "intent.json",
            native::Access::Read,
        )?;
        if native::identity(&observed)? != self.intent.intent
            || bounded(&observed, INTENT_LIMIT)?
                != serde_json::to_vec(&self.intent)
                    .map_err(|_| MemoryStorageError::RecoveryRequired)?
        {
            return Err(MemoryStorageError::ReviewConflict);
        }
        // The generated identifier is exactly one backend-safe component, never a path.
        let suffix = self
            .intent
            .id
            .strip_prefix("mcs_")
            .ok_or(MemoryStorageError::RecoveryRequired)?;
        let parsed =
            ulid::Ulid::from_string(suffix).map_err(|_| MemoryStorageError::RecoveryRequired)?;
        if parsed.to_string() != suffix {
            return Err(MemoryStorageError::RecoveryRequired);
        }
        if self.completed()?.is_some() && self.phase(store)? != Phase::Installed {
            return Err(MemoryStorageError::RecoveryRequired);
        }
        Ok(())
    }
    fn completed(&self) -> Result<Option<Proof>, MemoryStorageError> {
        let marker = snapshot(&self.dir, "completed")?;
        if marker
            .as_ref()
            .is_some_and(|proof| proof.digest != digest(b"committed\n") || proof.length != 10)
        {
            return Err(MemoryStorageError::RecoveryRequired);
        }
        Ok(marker)
    }
    fn phase(&self, store: &MemoryClientStore) -> Result<Phase, MemoryStorageError> {
        let current = snapshot(&store.dir, CHECKPOINT)?;
        let before = snapshot(&self.dir, "before")?;
        let after = snapshot(&self.dir, "after")?;
        if current.as_ref() == Some(&self.intent.after)
            && after.is_none()
            && before == self.intent.before
        {
            return Ok(Phase::Installed);
        }
        if after.as_ref() != Some(&self.intent.after) {
            return Err(MemoryStorageError::ReviewConflict);
        }
        if current == self.intent.before && before.is_none() {
            return Ok(if current.is_some() {
                Phase::Original
            } else {
                Phase::Captured
            });
        }
        if current.is_none() && before == self.intent.before {
            return Ok(Phase::Captured);
        }
        Err(MemoryStorageError::ReviewConflict)
    }
    fn install(&self, store: &MemoryClientStore) -> Result<(), MemoryStorageError> {
        self.verify(store)?;
        if self.phase(store)? == Phase::Original {
            move_file(
                &store.dir,
                CHECKPOINT,
                &self.dir,
                "before",
                self.intent
                    .before
                    .as_ref()
                    .ok_or(MemoryStorageError::RecoveryRequired)?,
            )?;
        }
        self.verify(store)?;
        if self.phase(store)? == Phase::Captured {
            move_file(
                &self.dir,
                "after",
                &store.dir,
                CHECKPOINT,
                &self.intent.after,
            )?;
        }
        self.verify(store)?;
        if self.phase(store)? != Phase::Installed {
            return Err(MemoryStorageError::RecoveryRequired);
        }
        Ok(())
    }
    fn commit(self, store: &MemoryClientStore) -> Result<(), MemoryStorageError> {
        self.install(store)?;
        let installed = freeze(&store.dir, CHECKPOINT, &self.intent.after)?;
        let original = self
            .intent
            .before
            .as_ref()
            .map(|proof| freeze(&self.dir, "before", proof))
            .transpose()?;
        store.sync()?;
        match self.completed()? {
            Some(_) => {}
            None => {
                create(&self.dir, "completed", b"committed\n")?;
            }
        }
        native::sync_private(&descriptor(&self.dir)?)?;
        let marker_proof =
            snapshot(&self.dir, "completed")?.ok_or(MemoryStorageError::RecoveryRequired)?;
        let marker = freeze(&self.dir, "completed", &marker_proof)?;
        self.verify(store)?;
        self.phase(store)?;
        let history = child_directory(&store.dir, HISTORY, true)?
            .ok_or(MemoryStorageError::RecoveryRequired)?;
        let expected = self.intent.journal;
        let id = self.intent.id.clone();
        drop(marker);
        drop(original);
        drop(self.owner);
        drop(self.dir);
        native::retain_private_directory(&descriptor(&store.dir)?, PENDING, expected)?
            .rename_to_durable(&descriptor(&history)?, &id)?;
        native::sync_private(&descriptor(&history)?)?;
        store.sync()?;
        drop(installed);
        Ok(())
    }
}
#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;
