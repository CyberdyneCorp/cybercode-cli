//! Durable request correlation resolves receipts without authorizing execution.
use super::*;

pub(in crate::runtime::memory) const LINKED: &str = "memory.recovery.request_linked.1";

pub struct MemoryRecoveryIdentity {
    pub key: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryRecoveryRequestStatus {
    pub id: String,
    pub mutation_id: String,
    pub directory: PathBuf,
    pub project_id: String,
    pub journal: MemoryJournalIdentity,
    pub completed: Option<MemoryChange>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestIdentity {
    key_hash: String,
    request_hash: String,
}
impl RequestIdentity {
    pub(super) fn from_http(identity: &MemoryRecoveryIdentity) -> Result<Self, StoreError> {
        if !(1..=128).contains(&identity.key.len())
            || !identity.key.bytes().all(|b| (0x21..=0x7e).contains(&b))
            || !valid_hash(&format!("sha256:{}", identity.digest))
        {
            return Err(refusal("Invalid memory recovery request identity"));
        }
        Ok(Self {
            key_hash: key_hash(&identity.key),
            request_hash: identity.digest.clone(),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Linked {
    review: MemoryRecoveryAdmission,
    identity: RequestIdentity,
}

pub(super) fn register(registry: &mut EventRegistry) {
    registry
        .register(LINKED)
        .expect("valid recovery receipt event");
}

fn key_hash(key: &str) -> String {
    format!("mrr_{:x}", Sha256::digest(format!("http:{key}").as_bytes()))
}

pub(super) fn linked_event(review: MemoryRecoveryAdmission, identity: RequestIdentity) -> NewEvent {
    NewEvent::new(
        LINKED,
        serde_json::to_value(Linked { review, identity }).expect("memory request serializes"),
    )
}

pub(super) fn project_identity(
    tx: &Transaction<'_>,
    mutation: &str,
    identity: &RequestIdentity,
) -> Result<(), StoreError> {
    let suffix = identity
        .key_hash
        .strip_prefix("mrr_")
        .ok_or_else(|| refusal("Invalid recovery key identity"))?;
    if !valid_hash(&format!("sha256:{suffix}"))
        || !valid_hash(&format!("sha256:{}", identity.request_hash))
    {
        return Err(refusal("Invalid recovery key identity"));
    }
    let existing: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM memory_mutation WHERE id=?1)",
        [format!("mwr_{suffix}")],
        |row| row.get(0),
    )?;
    if existing {
        return Err(refusal("Idempotency-Key was used with a different request"));
    }
    tx.execute(
        "INSERT INTO memory_recovery_request(key_hash,request_hash,mutation_id) VALUES(?1,?2,?3)",
        params![identity.key_hash, identity.request_hash, mutation],
    )?;
    Ok(())
}

pub(in crate::runtime::memory) fn project_linked(
    tx: &Transaction<'_>,
    event: &StoredEvent,
) -> Result<(), String> {
    let linked: Linked = serde_json::from_value(event.data.clone()).map_err(|e| e.to_string())?;
    let (data, result): (String, Option<String>) = tx
        .query_row(
            "SELECT data,result FROM memory_mutation WHERE id=?1",
            [&event.aggregate_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let request: Request = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    let current = snapshot(&request, result, event.seq - 1).map_err(|e| e.to_string())?;
    if current.completed.is_none()
        || current.id != linked.review.id
        || current.fingerprint != linked.review.fingerprint
        || current.journal != linked.review.journal
    {
        return Err("Recovery receipt link requires exact acknowledged review".into());
    }
    project_identity(tx, &event.aggregate_id, &linked.identity).map_err(|e| e.to_string())
}

fn read_status(
    db: &rusqlite::Connection,
    key: &str,
) -> Result<Option<(String, MemoryRecoveryRequestStatus)>, StoreError> {
    let row: Option<(String, String, String, Option<String>)> = db.query_row(
        "SELECT r.request_hash,r.mutation_id,m.data,m.result FROM memory_recovery_request r JOIN memory_mutation m ON m.id=r.mutation_id WHERE r.key_hash=?1", [key],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional()?;
    let Some((digest, mutation_id, data, result)) = row else {
        return Ok(None);
    };
    let request: Request = serde_json::from_str(&data)
        .map_err(|_| refusal("Invalid memory recovery request evidence"))?;
    let reviewed = snapshot(&request, result, 0)?;
    if mutation_id != reviewed.id {
        return Err(refusal("Invalid recovery mutation identity"));
    }
    Ok(Some((
        digest,
        MemoryRecoveryRequestStatus {
            id: key.into(),
            mutation_id,
            directory: reviewed.directory,
            project_id: reviewed.project_id,
            journal: reviewed.journal,
            completed: reviewed.completed,
        },
    )))
}

pub(crate) fn status(
    store: &Store,
    key: &str,
) -> Result<Option<MemoryRecoveryRequestStatus>, StoreError> {
    let key = key_hash(key);
    store.read(move |db| Ok(read_status(db, &key)?.map(|(_, status)| status)))
}

pub(crate) fn http_identity(
    store: &Store,
    key: &str,
    digest: &str,
) -> Result<Option<bool>, StoreError> {
    let key = key_hash(key);
    let digest = digest.to_owned();
    store.read(move |db| match read_status(db, &key)? {
        None => Ok(None),
        Some((previous, _)) if previous != digest => {
            Err(refusal("Idempotency-Key was used with a different request"))
        }
        Some((_, status)) => Ok(Some(status.completed.is_some())),
    })
}

pub(crate) fn receipt(
    store: &Store,
    identity: &MemoryRecoveryIdentity,
) -> Result<Option<MemoryChange>, StoreError> {
    RequestIdentity::from_http(identity)?;
    match http_identity(store, &identity.key, &identity.digest)? {
        None => Ok(None),
        Some(false) => Err(refusal(
            "Memory recovery request remains unresolved; review again",
        )),
        Some(true) => status(store, &identity.key)?
            .and_then(|status| status.completed)
            .map(Some)
            .ok_or_else(|| refusal("Memory recovery receipt unavailable")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: tempfile::TempDir,
        store: Arc<Store>,
        bus: Bus,
        owner: Option<MemoryWriteOwner>,
        journal: MemoryJournalIdentity,
    }
    impl Fixture {
        fn new(identity: &str, digest: Option<&str>) -> Self {
            Self::with_disk(identity, digest, false)
        }
        fn with_disk(identity: &str, digest: Option<&str>, disk: bool) -> Self {
            let root = tempfile::tempdir().unwrap();
            let store = Arc::new(
                Store::open(cyber_store::StoreOptions::new(
                    if disk {
                        cyber_core::paths::DatabaseLocation::File(root.path().join("events.db"))
                    } else {
                        cyber_core::paths::DatabaseLocation::Memory
                    },
                    crate::runtime::Runtime::registry(),
                ))
                .unwrap(),
            );
            let bus = Bus::new();
            let MemoryAdmission::Owned(mut owner) = admit(
                store.clone(),
                bus.clone(),
                MemoryWrite {
                    directory: root.path(),
                    project_id: "global",
                    name: "policy",
                    deleted: false,
                    identity: Some(identity),
                    content: "fact",
                    http_hash: digest,
                },
            )
            .unwrap() else {
                panic!("owner")
            };
            let journal = MemoryJournalIdentity {
                receipt: MemoryMutation {
                    id: cyber_core::ids::new_id("mem"),
                    name: "policy".into(),
                    deleted: false,
                },
                intent_fingerprint: "a".repeat(64),
            };
            owner.bind_journal(journal.clone()).unwrap();
            Self {
                root,
                store,
                bus,
                owner: Some(*owner),
                journal,
            }
        }
        fn review(&self) -> MemoryRecoveryAdmission {
            review(&self.store, self.root.path(), "global", &self.journal).unwrap()
        }
    }
    fn identity(key: &str) -> MemoryRecoveryIdentity {
        MemoryRecoveryIdentity {
            key: key.into(),
            digest: "b".repeat(64),
        }
    }

    #[test]
    fn durable_receipt_resolves_only_after_acknowledgement_and_rejects_digest_reuse() {
        let mut f = Fixture::new("tool:original", None);
        drop(f.owner.take());
        let key = identity("receipt-key");
        let MemoryAdmission::Owned(owner) =
            claim_with_identity(f.store.clone(), f.bus.clone(), &f.review(), Some(&key)).unwrap()
        else {
            panic!("owner")
        };
        assert!(
            status(&f.store, &key.key)
                .unwrap()
                .unwrap()
                .completed
                .is_none()
        );
        assert!(receipt(&f.store, &key).is_err());
        let change = owner.finish(f.journal.receipt.clone()).unwrap();
        assert_eq!(receipt(&f.store, &key).unwrap(), Some(change));
        assert_eq!(
            super::super::super::http_identity(&f.store, &key.key, &key.digest).unwrap(),
            Some(true)
        );
        let changed = MemoryRecoveryIdentity {
            key: key.key.clone(),
            digest: "c".repeat(64),
        };
        assert!(receipt(&f.store, &changed).is_err());
        assert!(
            admit(
                f.store.clone(),
                f.bus.clone(),
                MemoryWrite {
                    directory: f.root.path(),
                    project_id: "global",
                    name: "policy",
                    deleted: false,
                    identity: Some("http:receipt-key"),
                    content: "different",
                    http_hash: Some(&"c".repeat(64))
                }
            )
            .is_err()
        );
    }

    #[test]
    fn receipt_and_unknown_fencing_survive_database_reopen() {
        for complete in [false, true] {
            let mut f = Fixture::with_disk("tool:original", None, true);
            drop(f.owner.take());
            let key = identity("persistent-review");
            let MemoryAdmission::Owned(owner) =
                claim_with_identity(f.store.clone(), f.bus.clone(), &f.review(), Some(&key))
                    .unwrap()
            else {
                panic!("owner")
            };
            if complete {
                owner.finish(f.journal.receipt.clone()).unwrap();
            } else {
                drop(owner);
            }
            let Fixture {
                root,
                store,
                bus,
                owner,
                journal: _,
            } = f;
            drop(owner);
            drop(store);
            drop(bus);
            let reopened = Store::open(cyber_store::StoreOptions::new(
                cyber_core::paths::DatabaseLocation::File(root.path().join("events.db")),
                crate::runtime::Runtime::registry(),
            ))
            .unwrap();
            assert_eq!(
                status(&reopened, &key.key)
                    .unwrap()
                    .unwrap()
                    .completed
                    .is_some(),
                complete
            );
            assert_eq!(receipt(&reopened, &key).is_ok(), complete);
            if !complete {
                assert!(
                    admit(
                        Arc::new(reopened),
                        Bus::new(),
                        MemoryWrite {
                            directory: root.path(),
                            project_id: "global",
                            name: "policy",
                            deleted: false,
                            identity: Some("tool:original"),
                            content: "fact",
                            http_hash: None
                        }
                    )
                    .is_err()
                );
            }
        }
    }

    #[test]
    fn normal_http_key_collision_rolls_back_takeover_and_keeps_original_owner() {
        let mut f = Fixture::new("http:collision", Some(&"b".repeat(64)));
        let review = f.review();
        assert!(
            claim_with_identity(
                f.store.clone(),
                f.bus.clone(),
                &review,
                Some(&identity("collision"))
            )
            .is_err()
        );
        assert_eq!(f.store.aggregate_seq(&review.id).unwrap(), Some(1));
        assert!(status(&f.store, "collision").unwrap().is_none());
        f.owner
            .take()
            .unwrap()
            .finish(f.journal.receipt.clone())
            .unwrap();
    }

    #[test]
    fn acknowledged_archive_review_links_new_request_without_another_completion() {
        let mut f = Fixture::new("tool:original", None);
        let mut live = f.bus.subscribe();
        let change = f
            .owner
            .take()
            .unwrap()
            .finish(f.journal.receipt.clone())
            .unwrap();
        live.try_recv().unwrap();
        let key = identity("archive-key");
        assert!(
            matches!(claim_with_identity(f.store.clone(), f.bus.clone(), &f.review(), Some(&key)).unwrap(), MemoryAdmission::Replay(replay) if replay == change)
        );
        assert_eq!(receipt(&f.store, &key).unwrap(), Some(change.clone()));
        assert_eq!(
            f.store
                .read_events(&change.id, -1, 20)
                .unwrap()
                .events
                .len(),
            4
        );
        assert!(live.try_recv().is_err());
    }

    #[test]
    fn lost_owner_keeps_unresolved_key_until_a_later_explicit_review_resolves_it() {
        let mut f = Fixture::new("tool:original", None);
        drop(f.owner.take());
        let first = identity("first-review");
        drop(
            claim_with_identity(f.store.clone(), f.bus.clone(), &f.review(), Some(&first)).unwrap(),
        );
        assert!(receipt(&f.store, &first).is_err());
        assert!(
            status(&f.store, &first.key)
                .unwrap()
                .unwrap()
                .completed
                .is_none()
        );
        let second = identity("second-review");
        let MemoryAdmission::Owned(owner) =
            claim_with_identity(f.store.clone(), f.bus.clone(), &f.review(), Some(&second))
                .unwrap()
        else {
            panic!("owner")
        };
        let change = owner.finish(f.journal.receipt.clone()).unwrap();
        assert_eq!(receipt(&f.store, &first).unwrap(), Some(change.clone()));
        assert_eq!(receipt(&f.store, &second).unwrap(), Some(change));
    }
}
