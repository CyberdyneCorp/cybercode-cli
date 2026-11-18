//! File-copy cancellation-record construction stages: a `tmp_*` file beside a
//! journal's `preparation.json` that `storage::atomic` flushed but never renamed.
//! A stage counts only when its bytes are the cancellation receipt of the exact
//! origin that owns the journal; any other journal file is not construction
//! evidence and stays untouched. The namespace is found from the origin's
//! recorded Location and identities, so it works after the Location was removed.
use super::preparation::{FILE, LIMIT};
use super::storage::{self, Result};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use cyber_core::construction_discard::{self as native, StageEvidence};
use cyber_server::runtime::{
    ConstructionFence, ConstructionFenceError, ConstructionStageKind, NativeConstructionStage,
    SnapshotOrigin, SnapshotPreparationReceipt,
};
use std::fs::File;
use std::path::{Path, PathBuf};

const STAGE_PREFIX: &str = "tmp_";

struct Namespace {
    dir: Dir,
    journal: Dir,
    path: PathBuf,
}

fn child(parent: &Dir, name: &str, private: bool) -> Result<Option<Dir>> {
    match parent.symlink_metadata(name) {
        Ok(_) => storage::existing_child(parent, name, private).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(storage::fail(error)),
    }
}

/// The origin's namespace and journal, with their recorded identities.
fn namespace(data: &Path, origin: &SnapshotOrigin) -> Result<Option<Namespace>> {
    let binding = &origin.binding;
    if !binding.journal_id.starts_with("restore_")
        || !native::valid_component(&binding.journal_id)
        || !origin.activity_id.starts_with("op_")
        || !native::valid_component(&origin.activity_id)
    {
        return Err("Invalid snapshot journal binding".into());
    }
    if !data.exists() {
        return Ok(None);
    }
    let data = data.canonicalize().map_err(storage::fail)?;
    let root = Dir::open_ambient_dir(&data, cap_std::ambient_authority()).map_err(storage::fail)?;
    let Some(snapshot) = child(&root, "snapshot", false)? else {
        return Ok(None);
    };
    let Some(global) = child(&snapshot, "global", true)? else {
        return Ok(None);
    };
    let key = storage::digest(
        format!("{}\0{}", binding.directory.display(), binding.root_identity).as_bytes(),
    );
    let Some(dir) = child(&global, &key, true)? else {
        return Ok(None);
    };
    let Some(journals) = child(&dir, "journals", true)? else {
        return Ok(None);
    };
    if storage::dir_identity(&journals)? != binding.namespace_identity {
        return Err("Snapshot journal namespace changed".into());
    }
    let journal = match journals.open_dir_nofollow(&binding.journal_id) {
        Ok(journal) => journal,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(storage::fail(error)),
    };
    if storage::dir_identity(&journal)? != binding.journal_identity {
        return Err("Snapshot journal identity changed".into());
    }
    Ok(Some(Namespace {
        dir,
        journal,
        path: data.join("snapshot").join("global").join(key),
    }))
}

/// Whether `name` is a complete cancellation receipt stage of exactly `origin`.
fn is_stage(journal: &Dir, name: &str, origin: &SnapshotOrigin) -> bool {
    if !name.starts_with(STAGE_PREFIX) || !native::valid_component(name) {
        return false;
    }
    let Ok(bytes) = storage::read(journal, name, LIMIT) else {
        return false;
    };
    serde_json::from_slice::<SnapshotPreparationReceipt>(&bytes).is_ok_and(|receipt| {
        receipt.activity_id == origin.activity_id && receipt.origin.as_ref() == Some(origin)
    })
}

pub(crate) fn stages(data: &Path, origin: &SnapshotOrigin) -> Result<Vec<String>> {
    let Some(namespace) = namespace(data, origin)? else {
        return Ok(vec![]);
    };
    let mut found: Vec<String> = storage::names(&namespace.journal, super::ENTRY_LIMIT * 3)?
        .into_iter()
        .filter(|name| is_stage(&namespace.journal, name, origin))
        .collect();
    found.sort();
    Ok(found)
}

pub(crate) fn stage(
    data: &Path,
    origin: &SnapshotOrigin,
    name: &str,
) -> Result<Option<NativeConstructionStage>> {
    let Some(namespace) = namespace(data, origin)? else {
        return Ok(None);
    };
    if !is_stage(&namespace.journal, name, origin) {
        return Ok(None);
    }
    let (_, evidence) = native::observe(&namespace.journal, name).map_err(storage::fail)?;
    Ok(Some(native_stage(&namespace, origin, name, evidence)))
}

fn native_stage(
    namespace: &Namespace,
    origin: &SnapshotOrigin,
    name: &str,
    evidence: StageEvidence,
) -> NativeConstructionStage {
    NativeConstructionStage {
        name: name.into(),
        kind: ConstructionStageKind::CancellationRecord,
        activity_id: Some(origin.activity_id.clone()),
        worktree_id: origin.worktree_id.clone(),
        journal_id: Some(origin.binding.journal_id.clone()),
        evidence,
        source: namespace
            .path
            .join("journals")
            .join(&origin.binding.journal_id),
        namespace: namespace.path.clone(),
    }
}

/// Held namespace lock, owner lifetime lock and stage handle.
pub(crate) struct Fence {
    namespace: Dir,
    journal: Dir,
    _lock: File,
    _worker: Option<File>,
    file: File,
    name: String,
    evidence: StageEvidence,
}

impl ConstructionFence for Fence {
    fn quarantine(&mut self, key: &str) -> std::result::Result<(), String> {
        native::quarantine(
            &self.journal,
            &self.name,
            &self.namespace,
            key,
            &self.file,
            &self.evidence,
        )
        .map_err(storage::fail)
    }
}

fn refused(error: impl std::fmt::Display) -> ConstructionFenceError {
    ConstructionFenceError::Refused(error.to_string())
}

/// The owner's lifetime lock, held exclusively; a held lock is a live worker.
fn lock_worker(
    dir: &Dir,
    activity: &str,
) -> std::result::Result<Option<File>, ConstructionFenceError> {
    let Some(workers) = child(dir, "workers", true).map_err(refused)? else {
        return Ok(None);
    };
    let file = match storage::open(&workers, Path::new(activity)) {
        Ok(file) => file,
        Err(_) if workers.symlink_metadata(activity).is_err() => return Ok(None),
        Err(error) => return Err(refused(error)),
    };
    match native::try_lock(&file) {
        Ok(true) => Ok(Some(file)),
        Ok(false) => Err(ConstructionFenceError::Busy(
            "Snapshot worker is still active".into(),
        )),
        Err(error) => Err(refused(error)),
    }
}

pub(crate) fn fence(
    data: &Path,
    origin: &SnapshotOrigin,
    stage: &NativeConstructionStage,
) -> std::result::Result<Fence, ConstructionFenceError> {
    let namespace = namespace(data, origin)
        .map_err(refused)?
        .ok_or_else(|| refused("The journal of this cancellation record is gone"))?;
    let expected = native_stage(&namespace, origin, &stage.name, stage.evidence.clone());
    if expected != *stage {
        return Err(refused("Cancellation record stage moved since review"));
    }
    let lock = storage::lock(&namespace.dir, false).map_err(|error| {
        if error == "Snapshot storage is busy" {
            ConstructionFenceError::Busy(error)
        } else {
            refused(error)
        }
    })?;
    if let Some(identity) = &origin.binding.lock_identity
        && storage::identity(&lock).map_err(refused)? != *identity
    {
        return Err(refused("Snapshot namespace lock was replaced"));
    }
    let worker = lock_worker(&namespace.dir, &origin.activity_id)?;
    if !is_stage(&namespace.journal, &stage.name, origin) {
        return Err(refused(
            "Not a cancellation-record stage of this activity; it stays untouched",
        ));
    }
    if namespace.journal.symlink_metadata(FILE).is_err() {
        return Err(refused(
            "The cancellation of this activity is not reconciled yet; run recovery-preparation-request --acknowledge before discarding this stage",
        ));
    }
    let (file, observed) = native::observe(&namespace.journal, &stage.name).map_err(refused)?;
    if observed != stage.evidence {
        return Err(refused("Construction stage changed since review"));
    }
    Ok(Fence {
        namespace: namespace.dir,
        journal: namespace.journal,
        _lock: lock,
        _worker: worker,
        file,
        name: stage.name.clone(),
        evidence: stage.evidence.clone(),
    })
}

/// Bytes of quarantined stages that still await their unlink; they count toward
/// the snapshot cap until removed.
pub(super) fn pending_bytes(namespace: &Dir) -> Result<u64> {
    native::pending(namespace, super::ENTRY_LIMIT)
        .map_err(storage::fail)?
        .iter()
        .try_fold(0u64, |total, entry| total.checked_add(entry.size))
        .ok_or_else(|| "Snapshot storage overflow".into())
}

#[cfg(test)]
mod tests {
    use super::super::{Options, Store, lifetime::Lifetime};
    use super::*;
    use cyber_server::runtime::{SnapshotBinding, SnapshotLifetime, SnapshotLifetimeRequest};

    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn options() -> Options {
        Options {
            max_file: 65536,
            max_total: 4 * 1024 * 1024,
            ignore: vec![],
            private_roots: vec![],
        }
    }

    /// A cancelled preparation whose cancellation record was cut after its stage was
    /// flushed and before it was renamed: the exact state `storage::atomic` leaves.
    fn setup() -> (
        tempfile::TempDir,
        Store,
        Lifetime,
        SnapshotPreparationReceipt,
        String,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        let store =
            Store::open(&temp.path().join("data"), root.to_str().unwrap(), options()).unwrap();
        let lifetime = store
            .reserve_lifetime(SnapshotLifetimeRequest {
                activity_id: "op_discard".into(),
                session_id: "ses_fixture".into(),
                session_created_id: "evt_fixture".into(),
                directory: store.root.clone(),
                worktree_id: None,
            })
            .unwrap();
        let dir = storage::private_child(&store.journals, "restore_discard").unwrap();
        let origin = SnapshotOrigin {
            version: 1,
            activity_id: "op_discard".into(),
            session_id: "ses_fixture".into(),
            session_created_id: "evt_fixture".into(),
            worktree_id: None,
            binding: SnapshotBinding {
                directory: store.root.clone(),
                root_identity: store.root_id.clone(),
                namespace_identity: storage::dir_identity(&store.journals).unwrap(),
                journal_id: "restore_discard".into(),
                journal_identity: storage::dir_identity(&dir).unwrap(),
                lock_identity: Some(store.lock_identity().unwrap()),
            },
        };
        let receipt = SnapshotPreparationReceipt {
            activity_id: origin.activity_id.clone(),
            request_id: "cancel-discard".into(),
            confirmation: "a".repeat(64),
            lifetime: lifetime.binding().clone(),
            native_sequence: 1,
            session_sequence: 8,
            origin: Some(origin),
            source_admitted: Some(false),
            checkout_resources_digest: None,
        };
        let stage = format!("tmp_{}", ulid::Ulid::new());
        storage::create(&dir, &stage, &serde_json::to_vec(&receipt).unwrap()).unwrap();
        (temp, store, lifetime, receipt, stage)
    }

    fn data(temp: &tempfile::TempDir) -> PathBuf {
        temp.path().join("data")
    }

    fn busy(result: std::result::Result<Fence, ConstructionFenceError>) -> bool {
        matches!(result, Err(ConstructionFenceError::Busy(_)))
    }

    fn refused(result: std::result::Result<Fence, ConstructionFenceError>, text: &str) {
        match result {
            Err(ConstructionFenceError::Refused(message)) => {
                assert!(message.contains(text), "{message}")
            }
            Err(other) => panic!("{other:?}"),
            Ok(_) => panic!("fence admitted"),
        }
    }

    #[test]
    fn only_an_exact_cancellation_receipt_stage_is_construction_evidence() {
        let (temp, store, _lifetime, receipt, stage) = setup();
        let origin = receipt.origin.clone().unwrap();
        let journal = store.journals.open_dir_nofollow("restore_discard").unwrap();
        // Another journal stage and a receipt of a foreign origin are not evidence.
        storage::create(&journal, "tmp_other", b"intent bytes").unwrap();
        let mut foreign = receipt.clone();
        foreign.origin.as_mut().unwrap().activity_id = "op_foreign".into();
        storage::create(
            &journal,
            "tmp_foreign",
            &serde_json::to_vec(&foreign).unwrap(),
        )
        .unwrap();
        assert_eq!(stages(&data(&temp), &origin).unwrap(), vec![stage.clone()]);
        assert!(stage_of(&temp, &origin, "tmp_other").is_none());
        let reviewed = stage_of(&temp, &origin, &stage).unwrap();
        assert_eq!(reviewed.kind, ConstructionStageKind::CancellationRecord);
        assert_eq!(reviewed.activity_id.as_deref(), Some("op_discard"));
        assert_eq!(reviewed.journal_id.as_deref(), Some("restore_discard"));
        assert!(reviewed.source.ends_with("journals/restore_discard"));
    }

    fn stage_of(
        temp: &tempfile::TempDir,
        origin: &SnapshotOrigin,
        name: &str,
    ) -> Option<NativeConstructionStage> {
        stage(&data(temp), origin, name).unwrap()
    }

    #[test]
    fn unreconciled_live_or_busy_owners_refuse_and_keep_the_stage() {
        let (temp, store, lifetime, receipt, stage) = setup();
        let origin = receipt.origin.clone().unwrap();
        let reviewed = stage_of(&temp, &origin, &stage).unwrap();
        // The live worker's lifetime lock and the held namespace lock are live holders.
        assert!(busy(fence(&data(&temp), &origin, &reviewed)));
        drop(store);
        assert!(busy(fence(&data(&temp), &origin, &reviewed)));
        drop(lifetime);
        // The published cancellation record is still missing: reconcile it first.
        refused(fence(&data(&temp), &origin, &reviewed), "not reconciled");
        let journal = data(&temp).join("snapshot/global");
        assert!(walk(&journal).iter().any(|path| path.ends_with(&stage)));
    }

    fn walk(root: &Path) -> Vec<PathBuf> {
        let mut found = vec![];
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.extend(walk(&path));
            } else {
                found.push(path);
            }
        }
        found
    }

    #[test]
    fn reconciled_stage_is_quarantined_counted_and_unlinked_even_after_location_removal() {
        let (temp, store, lifetime, receipt, stage) = setup();
        let origin = receipt.origin.clone().unwrap();
        store.acknowledge_preparation(&receipt).unwrap();
        let before = store.usage().unwrap();
        drop((store, lifetime));
        // The Location directory is removed: the stage is still found by its origin.
        std::fs::remove_dir_all(temp.path().join("work")).unwrap();
        let reviewed = stage_of(&temp, &origin, &stage).unwrap();
        let mut changed = reviewed.clone();
        changed.evidence.size += 1;
        refused(
            fence(&data(&temp), &origin, &changed),
            "changed since review",
        );
        let mut fenced = fence(&data(&temp), &origin, &reviewed).unwrap();
        fenced.quarantine(KEY).unwrap();
        drop(fenced);
        assert!(stages(&data(&temp), &origin).unwrap().is_empty());
        let namespace =
            Dir::open_ambient_dir(&reviewed.namespace, cap_std::ambient_authority()).unwrap();
        assert_eq!(pending_bytes(&namespace).unwrap(), reviewed.evidence.size);
        assert_eq!(
            native::unlink(&namespace, KEY, &stage, &reviewed.evidence).unwrap(),
            native::Place::Gone
        );
        assert_eq!(pending_bytes(&namespace).unwrap(), 0);
        assert!(before > 0);
    }

    #[test]
    fn quarantined_bytes_count_toward_the_snapshot_cap() {
        let (temp, store, lifetime, receipt, stage) = setup();
        let origin = receipt.origin.clone().unwrap();
        store.acknowledge_preparation(&receipt).unwrap();
        drop((store, lifetime));
        let root = temp.path().join("work");
        let fresh = || Store::open(&data(&temp), root.to_str().unwrap(), options()).unwrap();
        let before = fresh().usage().unwrap();
        let reviewed = stage_of(&temp, &origin, &stage).unwrap();
        let mut fenced = fence(&data(&temp), &origin, &reviewed).unwrap();
        fenced.quarantine(KEY).unwrap();
        drop(fenced);
        // The stage moved from the journal into the quarantine: still counted.
        assert_eq!(fresh().usage().unwrap(), before);
        let namespace =
            Dir::open_ambient_dir(&reviewed.namespace, cap_std::ambient_authority()).unwrap();
        native::unlink(&namespace, KEY, &stage, &reviewed.evidence).unwrap();
        assert_eq!(fresh().usage().unwrap(), before - reviewed.evidence.size);
    }
}
