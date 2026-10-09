//! Reviewed evidence grants fresh ownership; persisted nonces never do.
pub(super) mod requests;
use super::*;
use cyber_core::memory::{MemoryRecoveryReview, MemoryScope, MemoryStorageError};
pub use requests::{MemoryRecoveryIdentity, MemoryRecoveryRequestStatus};
pub(crate) use requests::{http_identity, receipt, status};

pub(super) const REVIEWED: &str = "memory.mutation.reviewed.1";

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecoveryAdmission {
    pub id: String,
    pub directory: PathBuf,
    pub project_id: String,
    pub journal: MemoryJournalIdentity,
    pub sequence: i64,
    pub fingerprint: String,
    pub completed: Option<MemoryChange>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reviewed {
    review: MemoryRecoveryAdmission,
    previous_owner_hash: String,
    next_owner_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    http_request: Option<requests::RequestIdentity>,
}

pub(super) fn register(registry: &mut EventRegistry) {
    requests::register(registry);
    registry
        .register(REVIEWED)
        .expect("valid memory recovery event");
}

fn snapshot(
    request: &Request,
    result: Option<String>,
    sequence: i64,
) -> Result<MemoryRecoveryAdmission, StoreError> {
    let journal = request
        .journal
        .clone()
        .ok_or_else(|| refusal("Unbound memory admission requires legacy review"))?;
    let fingerprint = hash(
        &serde_json::to_string(&(request, &result, sequence))
            .map_err(|_| refusal("Invalid memory review evidence"))?,
    );
    let completed: Option<MemoryChange> = result
        .map(|data| serde_json::from_str(&data))
        .transpose()
        .map_err(|_| refusal("Invalid persisted memory receipt"))?;
    if completed.as_ref().is_some_and(|change| {
        change.id != request.id
            || change.directory != request.directory
            || change.project_id != request.project_id
            || change.receipt != journal.receipt
    }) {
        return Err(refusal("Memory receipt does not match journal evidence"));
    }
    Ok(MemoryRecoveryAdmission {
        id: request.id.clone(),
        directory: request.directory.clone(),
        project_id: request.project_id.clone(),
        journal,
        sequence,
        fingerprint,
        completed,
    })
}

pub(crate) fn review(
    store: &Store,
    directory: &Path,
    project: &str,
    journal: &MemoryJournalIdentity,
) -> Result<MemoryRecoveryAdmission, StoreError> {
    let directory = std::fs::canonicalize(directory).map_err(StoreError::Io)?;
    let project = project.to_owned();
    let journal = journal.clone();
    store.read(move |db| {
        let mut statement = db.prepare("SELECT m.data,m.result,e.seq FROM memory_mutation m JOIN event_sequence e ON e.aggregate_id=m.id WHERE m.project_id=?1 AND (m.result IS NULL OR json_extract(m.data,'$.journal.receipt.id')=?2) LIMIT 2")?;
        let mut rows = statement.query(params![project, journal.receipt.id])?;
        let row = rows.next()?.ok_or_else(|| refusal("No matching memory admission evidence"))?;
        let request: Request = serde_json::from_str(&row.get::<_, String>(0)?).map_err(|_| refusal("Invalid persisted memory admission"))?;
        let result = row.get(1)?;
        let sequence = row.get(2)?;
        if rows.next()?.is_some() || request.directory != directory || request.project_id != project || request.journal.as_ref() != Some(&journal) {
            return Err(refusal("Memory admission and journal do not match"));
        }
        snapshot(&request, result, sequence)
    })
}

#[cfg(all(test, unix))]
fn claim(
    store: Arc<Store>,
    bus: Bus,
    review: &MemoryRecoveryAdmission,
) -> Result<MemoryAdmission, StoreError> {
    claim_with_identity(store, bus, review, None)
}

fn claim_with_identity(
    store: Arc<Store>,
    bus: Bus,
    review: &MemoryRecoveryAdmission,
    identity: Option<&MemoryRecoveryIdentity>,
) -> Result<MemoryAdmission, StoreError> {
    let http_request = identity
        .map(requests::RequestIdentity::from_http)
        .transpose()?;
    let expected = review.clone();
    let key = cyber_core::ids::new_id("mwo");
    let next_owner_hash = hash(&key);
    let (events, (mut request, completed)) =
        store.append_checked(&review.id, Expected::Seq(review.sequence), move |tx| {
            let (data, result): (String, Option<String>) = tx.query_row(
                "SELECT data,result FROM memory_mutation WHERE id=?1",
                [&expected.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let request: Request = serde_json::from_str(&data)
                .map_err(|_| refusal("Invalid persisted memory admission"))?;
            let current = snapshot(&request, result, expected.sequence)?;
            if current.fingerprint != expected.fingerprint
                || current.journal != expected.journal
                || current.directory != expected.directory
                || current.project_id != expected.project_id
            {
                return Err(refusal("Memory database review is stale"));
            }
            if let Some(completed) = current.completed {
                let events = match http_request {
                    Some(identity) => vec![requests::linked_event(expected, identity)],
                    None => Vec::new(),
                };
                return Ok((events, (request, Some(completed))));
            }
            let event = NewEvent::new(
                REVIEWED,
                serde_json::to_value(Reviewed {
                    review: expected,
                    previous_owner_hash: request.owner_hash.clone(),
                    next_owner_hash,
                    http_request,
                })
                .expect("memory review serializes"),
            );
            Ok((vec![event], (request, None)))
        })?;
    if let Some(completed) = completed {
        return Ok(MemoryAdmission::Replay(completed));
    }
    let expected_seq = events[0].seq;
    request.owner_hash = hash(&key);
    request.review_seq = Some(expected_seq);
    Ok(MemoryAdmission::Owned(Box::new(MemoryWriteOwner {
        store,
        bus,
        request,
        key,
        expected_seq,
    })))
}

pub(crate) fn recover(
    store: Arc<Store>,
    bus: Bus,
    scope: &mut MemoryScope<'_>,
    storage: &MemoryRecoveryReview,
    admission: &MemoryRecoveryAdmission,
) -> Result<MemoryChange, StoreError> {
    recover_with_identity(store, bus, scope, storage, admission, None)
}

pub(crate) fn recover_with_identity(
    store: Arc<Store>,
    bus: Bus,
    scope: &mut MemoryScope<'_>,
    storage: &MemoryRecoveryReview,
    admission: &MemoryRecoveryAdmission,
    identity: Option<&MemoryRecoveryIdentity>,
) -> Result<MemoryChange, StoreError> {
    if scope.path().file_name().and_then(|name| name.to_str())
        != Some(admission.project_id.as_str())
    {
        return Err(refusal("Memory recovery scope does not match admission"));
    }
    if let Some(identity) = identity
        && let Some(change) = receipt(&store, identity)?
    {
        if change.id != admission.id
            || change.directory != admission.directory
            || change.project_id != admission.project_id
            || change.receipt != admission.journal.receipt
        {
            return Err(refusal("Recovery request does not match admitted mutation"));
        }
        return Ok(change);
    }
    let current = scope
        .inspect_recovery()
        .map_err(|_| refusal("Memory storage review unavailable"))?
        .ok_or_else(|| refusal("Memory storage review is stale"))?;
    if current.fingerprint != storage.fingerprint
        || current.journal != storage.journal
        || current.journal != admission.journal
    {
        return Err(refusal("Memory storage review is stale or mismatched"));
    }
    let ownership = claim_with_identity(store, bus, admission, identity)?;
    let mut change = None;
    scope
        .recover_reviewed_with_acknowledgement(&current.fingerprint, |receipt| {
            let completed = match ownership {
                MemoryAdmission::Owned(owner) => owner
                    .finish(receipt.clone())
                    .map_err(|_| MemoryStorageError::RecoveryRequired)?,
                MemoryAdmission::Replay(completed) => completed,
            };
            change = Some(completed);
            Ok(())
        })
        .map_err(|_| refusal("Memory reconciliation retained recovery evidence"))?;
    change.ok_or_else(|| refusal("Memory reconciliation did not acknowledge completion"))
}

pub(super) fn project_review(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    let reviewed: Reviewed =
        serde_json::from_value(event.data.clone()).map_err(|e| e.to_string())?;
    let (data, result): (String, Option<String>) = tx
        .query_row(
            "SELECT data,result FROM memory_mutation WHERE id=?1",
            [&event.aggregate_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let mut request: Request = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    let current = snapshot(&request, result, event.seq - 1).map_err(|e| e.to_string())?;
    if current.completed.is_some()
        || event.seq != request.review_seq.unwrap_or(1) + 1
        || reviewed.review.id != event.aggregate_id
        || current.fingerprint != reviewed.review.fingerprint
        || current.journal != reviewed.review.journal
        || current.directory != reviewed.review.directory
        || current.project_id != reviewed.review.project_id
        || reviewed.review.sequence != event.seq - 1
        || reviewed.review.completed.is_some()
        || request.owner_hash != reviewed.previous_owner_hash
        || !valid_hash(&reviewed.next_owner_hash)
        || request.owner_hash == reviewed.next_owner_hash
    {
        return Err("Memory recovery review does not match pending evidence".into());
    }
    if let Some(identity) = &reviewed.http_request {
        requests::project_identity(tx, &request.id, identity).map_err(|e| e.to_string())?;
    }
    request.owner_hash = reviewed.next_owner_hash;
    request.review_seq = Some(event.seq);
    tx.execute(
        "UPDATE memory_mutation SET data=?2,owner_hash=?3 WHERE id=?1",
        params![
            request.id,
            serde_json::to_string(&request).map_err(|e| e.to_string())?,
            request.owner_hash
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use cyber_core::memory::MemoryStore;
    struct Fixture {
        root: tempfile::TempDir,
        db: Arc<Store>,
        memory: MemoryStore,
        bus: Bus,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let db = Arc::new(
                Store::open(cyber_store::StoreOptions::new(
                    cyber_core::paths::DatabaseLocation::Memory,
                    super::super::super::Runtime::registry(),
                ))
                .unwrap(),
            );
            let memory = MemoryStore::open(root.path(), "global").unwrap();
            Self {
                root,
                db,
                memory,
                bus: Bus::new(),
            }
        }
        fn write(&self) -> MemoryWrite<'_> {
            MemoryWrite {
                directory: self.root.path(),
                project_id: "global",
                name: "policy",
                deleted: false,
                identity: Some("reviewed:write"),
                content: "fact",
                http_hash: None,
            }
        }
        fn prepare(&self) -> MemoryRecoveryReview {
            let mut scope = self.memory.claim().unwrap();
            let MemoryAdmission::Owned(mut owner) =
                admit(self.db.clone(), self.bus.clone(), self.write()).unwrap()
            else {
                panic!("owner")
            };
            let prepared = scope
                .prepare_write("---\nname: policy\ndescription: Policy\ntype: user\n---\nFact\n")
                .unwrap();
            owner
                .bind_journal(prepared.journal_identity().unwrap())
                .unwrap();
            drop(prepared);
            drop(owner);
            scope.inspect_recovery().unwrap().unwrap()
        }
        fn review(&self, storage: &MemoryRecoveryReview) -> MemoryRecoveryAdmission {
            review(&self.db, self.root.path(), "global", &storage.journal).unwrap()
        }
    }

    #[test]
    fn fresh_review_reconciles_unknown_once_and_publishes_after_acknowledgement() {
        let f = Fixture::new();
        let storage = f.prepare();
        let admission = f.review(&storage);
        let mut live = f.bus.subscribe();
        let mut scope = f.memory.claim().unwrap();
        let change = recover(
            f.db.clone(),
            f.bus.clone(),
            &mut scope,
            &storage,
            &admission,
        )
        .unwrap();
        assert_eq!(change.receipt, storage.receipt);
        assert_eq!(scope.read("policy").unwrap().body, "Fact");
        assert!(matches!(
            live.try_recv().unwrap(),
            LiveEvent::MemoryUpdated { seq: 3, .. }
        ));
        assert!(live.try_recv().is_err());
        assert_eq!(lookup(&f.db, &f.write()).unwrap(), Some(change));
        assert!(
            recover(
                f.db.clone(),
                f.bus.clone(),
                &mut scope,
                &storage,
                &admission
            )
            .is_err()
        );
    }

    #[test]
    fn keyed_runtime_replay_returns_receipt_without_a_journal_or_duplicate_event() {
        let f = Fixture::new();
        let storage = f.prepare();
        let admission = f.review(&storage);
        let identity = MemoryRecoveryIdentity {
            key: "runtime-replay".into(),
            digest: "b".repeat(64),
        };
        let mut live = f.bus.subscribe();
        let mut scope = f.memory.claim().unwrap();
        let change = recover_with_identity(
            f.db.clone(),
            f.bus.clone(),
            &mut scope,
            &storage,
            &admission,
            Some(&identity),
        )
        .unwrap();
        live.try_recv().unwrap();
        assert!(scope.inspect_recovery().unwrap().is_none());
        assert_eq!(
            recover_with_identity(
                f.db.clone(),
                f.bus.clone(),
                &mut scope,
                &storage,
                &admission,
                Some(&identity)
            )
            .unwrap(),
            change
        );
        let mut foreign = admission.clone();
        foreign.directory = PathBuf::from("/");
        assert!(
            recover_with_identity(
                f.db.clone(),
                f.bus.clone(),
                &mut scope,
                &storage,
                &foreign,
                Some(&identity)
            )
            .is_err()
        );
        assert!(live.try_recv().is_err());
        assert_eq!(
            f.db.read_events(&change.id, -1, 20).unwrap().events.len(),
            4
        );
    }

    #[test]
    fn stale_file_and_database_reviews_refuse_before_effects() {
        for stale_file in [false, true] {
            let f = Fixture::new();
            let storage = f.prepare();
            let admission = f.review(&storage);
            if stale_file {
                use std::os::unix::fs::PermissionsExt;
                let path = f.memory.path().join("policy.md");
                std::fs::write(&path, "User edit").unwrap();
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
            } else {
                drop(claim(f.db.clone(), f.bus.clone(), &admission).unwrap());
            }
            let mut scope = f.memory.claim().unwrap();
            assert!(
                recover(
                    f.db.clone(),
                    f.bus.clone(),
                    &mut scope,
                    &storage,
                    &admission
                )
                .is_err()
            );
            assert!(
                f.memory
                    .path()
                    .join(".memory-transaction/note.after")
                    .exists()
            );
            assert!(!f.memory.path().join("MEMORY.md").exists());
            if stale_file {
                assert_eq!(
                    std::fs::read_to_string(f.memory.path().join("policy.md")).unwrap(),
                    "User edit"
                );
            }
        }
    }

    #[test]
    fn disposed_recovery_owner_requires_new_review_and_fresh_nonce() {
        let f = Fixture::new();
        let storage = f.prepare();
        let admission = f.review(&storage);
        let MemoryAdmission::Owned(first) = claim(f.db.clone(), f.bus.clone(), &admission).unwrap()
        else {
            panic!("owner")
        };
        let stale_key = first.key.clone();
        drop(first);
        assert!(claim(f.db.clone(), f.bus.clone(), &admission).is_err());
        let fresh = f.review(&storage);
        assert_ne!(fresh.fingerprint, admission.fingerprint);
        let MemoryAdmission::Owned(second) = claim(f.db.clone(), f.bus.clone(), &fresh).unwrap()
        else {
            panic!("owner")
        };
        assert_ne!(second.key, stale_key);
        let forged = NewEvent::new(
            UPDATED,
            serde_json::to_value(Completion {
                update: MemoryChange {
                    id: second.request.id.clone(),
                    directory: second.request.directory.clone(),
                    project_id: "global".into(),
                    receipt: storage.receipt.clone(),
                },
                previous_owner_key: stale_key,
            })
            .unwrap(),
        );
        assert!(
            f.db.append(
                &second.request.id,
                Expected::Seq(second.expected_seq),
                vec![forged]
            )
            .is_err()
        );
        drop(second);
        let mut scope = f.memory.claim().unwrap();
        recover(
            f.db.clone(),
            f.bus.clone(),
            &mut scope,
            &storage,
            &f.review(&storage),
        )
        .unwrap();
    }

    #[test]
    fn completed_acknowledgement_then_failed_archive_replays_without_notification() {
        let f = Fixture::new();
        let storage = f.prepare();
        let admission = f.review(&storage);
        use std::os::unix::fs::PermissionsExt;
        let history = f.memory.path().join(".memory-history");
        std::fs::create_dir(&history).unwrap();
        std::fs::set_permissions(&history, std::fs::Permissions::from_mode(0o700)).unwrap();
        let collision = history.join(&storage.receipt.id);
        std::fs::create_dir(&collision).unwrap();
        let mut live = f.bus.subscribe();
        let mut scope = f.memory.claim().unwrap();
        assert!(
            recover(
                f.db.clone(),
                f.bus.clone(),
                &mut scope,
                &storage,
                &admission
            )
            .is_err()
        );
        assert!(matches!(
            scope.read("policy"),
            Err(MemoryStorageError::RecoveryRequired)
        ));
        assert!(live.try_recv().is_ok());
        let completed_storage = scope.inspect_recovery().unwrap().unwrap();
        let completed_admission = f.review(&completed_storage);
        assert!(completed_admission.completed.is_some());
        std::fs::remove_dir(collision).unwrap();
        let change = recover(
            f.db.clone(),
            f.bus.clone(),
            &mut scope,
            &completed_storage,
            &completed_admission,
        )
        .unwrap();
        assert_eq!(change, completed_admission.completed.unwrap());
        assert!(live.try_recv().is_err());
        assert_eq!(scope.read("policy").unwrap().body, "Fact");
        assert_eq!(
            f.db.read_events(&change.id, -1, 20).unwrap().events.len(),
            4
        );
    }

    #[test]
    fn copied_journal_cannot_reconcile_another_project_scope() {
        let f = Fixture::new();
        let storage = f.prepare();
        let admission = f.review(&storage);
        let other = MemoryStore::open(f.root.path(), "prj_other").unwrap();
        let target = other.path().join(".memory-transaction");
        std::fs::create_dir(&target).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        for entry in std::fs::read_dir(f.memory.path().join(".memory-transaction")).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
        let mut scope = other.claim().unwrap();
        let copied = scope.inspect_recovery().unwrap().unwrap();
        assert_eq!(copied.journal, storage.journal);
        assert!(recover(f.db.clone(), f.bus.clone(), &mut scope, &copied, &admission).is_err());
        assert!(!other.path().join("policy.md").exists());
        assert_eq!(f.review(&storage).fingerprint, admission.fingerprint);
    }

    #[test]
    fn foreign_journal_and_legacy_unbound_admission_refuse() {
        let f = Fixture::new();
        let storage = f.prepare();
        let mut foreign = storage.journal.clone();
        foreign.intent_fingerprint = "f".repeat(64);
        assert!(review(&f.db, f.root.path(), "global", &foreign).is_err());
        assert!(review(&f.db, std::path::Path::new("/"), "global", &storage.journal).is_err());
        let legacy = Fixture::new();
        drop(admit(legacy.db.clone(), legacy.bus.clone(), legacy.write()).unwrap());
        assert!(review(&legacy.db, legacy.root.path(), "global", &storage.journal).is_err());
        assert!(!legacy.memory.path().join("policy.md").exists());
    }
}
