//! Durable scope fencing and receipts; persisted admission never grants live write authority.
pub(super) mod recovery;
mod request_status;
use super::{Bus, LiveEvent};
use cyber_core::memory::{MemoryJournalIdentity, MemoryMutation};
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreError, StoredEvent};
pub use recovery::{MemoryRecoveryAdmission, MemoryRecoveryIdentity, MemoryRecoveryRequestStatus};
pub(super) use recovery::{recover, review};
pub use request_status::MemoryRequestStatus;
pub(super) use request_status::status as request_status;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const ADMITTED: &str = "memory.mutation.admitted.1";
const UPDATED: &str = "memory.updated.1";
const BOUND: &str = "memory.mutation.journal_bound.1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: String,
    directory: PathBuf,
    project_id: String,
    name: String,
    deleted: bool,
    request_hash: String,
    owner_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    http_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    journal: Option<MemoryJournalIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    review_seq: Option<i64>,
}

/// Public acknowledged change, without note content or execution capabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryChange {
    pub id: String,
    pub directory: PathBuf,
    pub project_id: String,
    pub receipt: MemoryMutation,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    update: MemoryChange,
    previous_owner_key: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    journal: MemoryJournalIdentity,
    previous_owner_key: String,
    next_owner_hash: String,
}

pub struct MemoryWrite<'a> {
    pub directory: &'a Path,
    pub project_id: &'a str,
    pub name: &'a str,
    pub deleted: bool,
    pub identity: Option<&'a str>,
    pub content: &'a str,
    pub http_hash: Option<&'a str>,
}

pub enum MemoryAdmission {
    Owned(Box<MemoryWriteOwner>),
    Replay(MemoryChange),
}

/// Dropping this owner leaves admission unresolved; no replay or rollback is inferred.
pub struct MemoryWriteOwner {
    store: Arc<Store>,
    bus: Bus,
    request: Request,
    key: String,
    expected_seq: i64,
}

impl MemoryWriteOwner {
    /// Persist correlation before storage effects; disposal still grants no recovery authority.
    pub fn bind_journal(&mut self, journal: MemoryJournalIdentity) -> Result<(), StoreError> {
        if let Some(previous) = &self.request.journal {
            return if previous == &journal {
                Ok(())
            } else {
                Err(refusal("Memory journal identity changed"))
            };
        }
        let next_key = cyber_core::ids::new_id("mwo");
        let next_owner_hash = hash(&next_key);
        self.store.append(
            &self.request.id,
            Expected::Seq(0),
            vec![NewEvent::new(
                BOUND,
                serde_json::to_value(Binding {
                    journal: journal.clone(),
                    previous_owner_key: self.key.clone(),
                    next_owner_hash: next_owner_hash.clone(),
                })
                .expect("memory binding serializes"),
            )],
        )?;
        self.request.journal = Some(journal);
        self.request.owner_hash = next_owner_hash;
        self.key = next_key;
        self.expected_seq = 1;
        Ok(())
    }

    pub fn finish(self, receipt: MemoryMutation) -> Result<MemoryChange, StoreError> {
        if self
            .request
            .journal
            .as_ref()
            .map(|journal| &journal.receipt)
            != Some(&receipt)
        {
            return Err(refusal(
                "Memory completion requires its bound journal receipt",
            ));
        }
        let update = MemoryChange {
            id: self.request.id.clone(),
            directory: self.request.directory.clone(),
            project_id: self.request.project_id.clone(),
            receipt,
        };
        let completion = Completion {
            update: update.clone(),
            previous_owner_key: self.key,
        };
        let events = self.store.append(
            &update.id,
            Expected::Seq(self.expected_seq),
            vec![NewEvent::new(
                UPDATED,
                serde_json::to_value(completion).expect("memory receipt serializes"),
            )],
        )?;
        self.bus.publish(LiveEvent::MemoryUpdated {
            update: update.clone(),
            seq: events[0].seq,
        });
        Ok(update)
    }
}

pub(super) fn admit(
    store: Arc<Store>,
    bus: Bus,
    write: MemoryWrite<'_>,
) -> Result<MemoryAdmission, StoreError> {
    let MemoryWrite {
        directory,
        project_id,
        name,
        deleted,
        identity,
        content,
        http_hash,
    } = write;
    let key = cyber_core::ids::new_id("mwo");
    let id = identity
        .map(|identity| format!("mwr_{:x}", Sha256::digest(identity.as_bytes())))
        .unwrap_or_else(|| cyber_core::ids::new_id("mwr"));
    let directory = std::fs::canonicalize(directory).map_err(StoreError::Io)?;
    let request_hash = request_hash(&directory, project_id, name, deleted, content);
    let request = Request {
        id: id.clone(),
        directory,
        project_id: project_id.into(),
        name: name.into(),
        deleted,
        request_hash,
        owner_hash: hash(&key),
        http_hash: http_hash.map(str::to_owned),
        journal: None,
        review_seq: None,
    };
    let expected = request.clone();
    let (_, replay) = store.append_checked(&id, Expected::Any, move |tx| {
        let previous: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT request_hash,result FROM memory_mutation WHERE id=?1",
                [&expected.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((digest, result)) = previous {
            if digest != expected.request_hash {
                return Err(refusal(
                    "Memory request identity was used with different content",
                ));
            }
            let result =
                result.ok_or_else(|| refusal("Memory mutation requires reviewed recovery"))?;
            let update = serde_json::from_str(&result)
                .map_err(|_| refusal("Invalid persisted memory receipt"))?;
            return Ok((Vec::new(), Some(update)));
        }
        Ok((
            vec![NewEvent::new(
                ADMITTED,
                serde_json::to_value(expected).expect("memory admission serializes"),
            )],
            None,
        ))
    })?;
    Ok(match replay {
        Some(update) => MemoryAdmission::Replay(update),
        None => MemoryAdmission::Owned(Box::new(MemoryWriteOwner {
            store,
            bus,
            request,
            key,
            expected_seq: 0,
        })),
    })
}

pub(super) fn lookup(
    store: &Store,
    write: &MemoryWrite<'_>,
) -> Result<Option<MemoryChange>, StoreError> {
    let Some(identity) = write.identity else {
        return Ok(None);
    };
    let id = format!("mwr_{:x}", Sha256::digest(identity.as_bytes()));
    let directory = std::fs::canonicalize(write.directory).map_err(StoreError::Io)?;
    let digest = request_hash(
        &directory,
        write.project_id,
        write.name,
        write.deleted,
        write.content,
    );
    store.read(move |db| {
        let row: Option<(String, Option<String>)> = db
            .query_row(
                "SELECT request_hash,result FROM memory_mutation WHERE id=?1",
                [&id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match row {
            None => Ok(None),
            Some((previous, _)) if previous != digest => Err(refusal(
                "Memory request identity was used with different content",
            )),
            Some((_, None)) => Err(refusal("Memory mutation requires reviewed recovery")),
            Some((_, Some(result))) => serde_json::from_str(&result)
                .map(Some)
                .map_err(|_| refusal("Invalid persisted memory receipt")),
        }
    })
}

pub(super) fn http_identity(
    store: &Store,
    key: &str,
    digest: &str,
) -> Result<Option<bool>, StoreError> {
    let id = format!("mwr_{:x}", Sha256::digest(format!("http:{key}").as_bytes()));
    let recovery_digest = digest.to_owned();
    let digest = digest.to_owned();
    let result = store.read(move |db| {
        let row: Option<(String, Option<String>)> = db
            .query_row(
                "SELECT data,result FROM memory_mutation WHERE id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((data, result)) = row else {
            return Ok(None);
        };
        let request: Request = serde_json::from_str(&data)
            .map_err(|_| refusal("Invalid persisted memory admission"))?;
        if request.http_hash.as_deref() != Some(&digest) {
            return Err(refusal("Idempotency-Key was used with a different request"));
        }
        Ok(Some(result.is_some()))
    })?;
    match result {
        Some(_) => Ok(result),
        None => recovery::http_identity(store, key, &recovery_digest),
    }
}

fn hash(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}
fn request_hash(
    directory: &Path,
    project: &str,
    name: &str,
    deleted: bool,
    content: &str,
) -> String {
    let mut digest = Sha256::new();
    for field in [
        directory.as_os_str().as_encoded_bytes(),
        project.as_bytes(),
        name.as_bytes(),
        if deleted { b"delete" } else { b"write" },
        content.as_bytes(),
    ] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    format!("sha256:{:x}", digest.finalize())
}
fn refusal(reason: &str) -> StoreError {
    StoreError::Projector {
        kind: ADMITTED.into(),
        reason: reason.into(),
    }
}
fn valid_hash(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub(super) fn register(registry: &mut EventRegistry) {
    registry.register(ADMITTED).expect("valid memory event");
    registry.register(UPDATED).expect("valid memory event");
    registry.register(BOUND).expect("valid memory event");
    recovery::register(registry);
    registry.projector(project);
}
fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    match event.kind.as_str() {
        ADMITTED => project_admission(tx, event),
        UPDATED => project_completion(tx, event),
        BOUND => project_binding(tx, event),
        recovery::REVIEWED => recovery::project_review(tx, event),
        recovery::requests::LINKED => recovery::requests::project_linked(tx, event),
        _ => Ok(()),
    }
}
fn project_admission(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    let request: Request = serde_json::from_value(event.data.clone()).map_err(|e| e.to_string())?;
    if event.seq != 0
        || request.id != event.aggregate_id
        || !request.id.starts_with("mwr_")
        || !request.directory.is_absolute()
        || !valid_hash(&request.request_hash)
        || !valid_hash(&request.owner_hash)
        || request.journal.is_some()
        || request.review_seq.is_some()
    {
        return Err("Invalid memory admission identity".into());
    }
    if request
        .http_hash
        .as_deref()
        .is_some_and(|digest| !valid_hash(&format!("sha256:{digest}")))
    {
        return Err("Invalid memory HTTP request digest".into());
    }
    cyber_core::memory::validate_name(&request.name).map_err(|e| e.to_string())?;
    cyber_core::memory::directory(Path::new("/"), &request.project_id)
        .map_err(|e| e.to_string())?;
    if request.http_hash.is_some() {
        let key_hash = request.id.replacen("mwr_", "mrr_", 1);
        let recovery: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_recovery_request WHERE key_hash=?1)",
                [key_hash],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if recovery {
            return Err("Idempotency-Key was used with a different request".into());
        }
    }
    let pending: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM memory_mutation WHERE project_id=?1 AND result IS NULL)",
            [&request.project_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if pending {
        return Err("Memory mutation requires reviewed recovery".into());
    }
    tx.execute("INSERT INTO memory_mutation(id,project_id,request_hash,owner_hash,data) VALUES(?1,?2,?3,?4,?5)", params![request.id,request.project_id,request.request_hash,request.owner_hash,serde_json::to_string(&request).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
    Ok(())
}
fn project_completion(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    let completed: Completion =
        serde_json::from_value(event.data.clone()).map_err(|e| e.to_string())?;
    let row: (String, Option<String>) = tx
        .query_row(
            "SELECT data,result FROM memory_mutation WHERE id=?1",
            [&event.aggregate_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let request: Request = serde_json::from_str(&row.0).map_err(|e| e.to_string())?;
    let update = completed.update;
    let expected_seq = request
        .review_seq
        .map(|seq| seq + 1)
        .unwrap_or(if request.journal.is_some() { 2 } else { 1 });
    if event.seq != expected_seq
        || row.1.is_some()
        || hash(&completed.previous_owner_key) != request.owner_hash
        || update.id != request.id
        || update.directory != request.directory
        || update.project_id != request.project_id
        || update.receipt.name != request.name
        || update.receipt.deleted != request.deleted
        || !cyber_core::ids::has_prefix(&update.receipt.id, "mem")
        || request
            .journal
            .as_ref()
            .is_some_and(|journal| journal.receipt != update.receipt)
    {
        return Err(
            "Memory completion does not hold the admitted owner and receipt identity".into(),
        );
    }
    tx.execute(
        "UPDATE memory_mutation SET result=?2 WHERE id=?1",
        params![
            update.id,
            serde_json::to_string(&update).map_err(|e| e.to_string())?
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn project_binding(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    let binding: Binding = serde_json::from_value(event.data.clone()).map_err(|e| e.to_string())?;
    let (data, result): (String, Option<String>) = tx
        .query_row(
            "SELECT data,result FROM memory_mutation WHERE id=?1",
            [&event.aggregate_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    let mut request: Request = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    let receipt = &binding.journal.receipt;
    if event.seq != 1
        || result.is_some()
        || request.journal.is_some()
        || hash(&binding.previous_owner_key) != request.owner_hash
        || !valid_hash(&binding.next_owner_hash)
        || binding.next_owner_hash == request.owner_hash
        || receipt.name != request.name
        || receipt.deleted != request.deleted
        || !cyber_core::ids::has_prefix(&receipt.id, "mem")
        || !valid_hash(&format!("sha256:{}", binding.journal.intent_fingerprint))
    {
        return Err("Memory journal binding does not hold the admitted owner and identity".into());
    }
    request.journal = Some(binding.journal);
    request.owner_hash = binding.next_owner_hash;
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

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> Arc<Store> {
        Arc::new(
            Store::open(cyber_store::StoreOptions::new(
                cyber_core::paths::DatabaseLocation::Memory,
                super::super::Runtime::registry(),
            ))
            .unwrap(),
        )
    }
    fn write(root: &Path) -> MemoryWrite<'_> {
        MemoryWrite {
            directory: root,
            project_id: "global",
            name: "policy",
            deleted: false,
            identity: Some("http:first"),
            content: "durable fact",
            http_hash: None,
        }
    }
    fn owned(admission: MemoryAdmission) -> MemoryWriteOwner {
        match admission {
            MemoryAdmission::Owned(owner) => *owner,
            _ => panic!("expected owner"),
        }
    }
    fn receipt() -> MemoryMutation {
        MemoryMutation {
            id: cyber_core::ids::new_id("mem"),
            name: "policy".into(),
            deleted: false,
        }
    }
    fn journal() -> MemoryJournalIdentity {
        MemoryJournalIdentity {
            receipt: receipt(),
            intent_fingerprint: "a".repeat(64),
        }
    }
    #[test]
    fn http_request_status_tracks_admission_binding_and_completion_without_effects() {
        let store = store();
        let root = tempfile::tempdir().unwrap();
        let bus = Bus::new();
        let mut events = bus.subscribe();
        let digest = "b".repeat(64);
        let mut request = write(root.path());
        request.content = &digest;
        request.http_hash = Some(&digest);
        assert_eq!(request_status(&store, "first").unwrap(), None);
        assert!(request_status(&store, "bad key").is_err());
        let mut owner = owned(admit(store.clone(), bus, request).unwrap());
        let admitted = request_status(&store, "first").unwrap().unwrap();
        assert_eq!(admitted.request_fingerprint, digest);
        assert_eq!(admitted.name, "policy");
        assert!(admitted.journal.is_none());
        assert!(admitted.completed.is_none());
        let journal = journal();
        owner.bind_journal(journal.clone()).unwrap();
        let bound = request_status(&store, "first").unwrap().unwrap();
        assert_eq!(bound.journal, Some(journal.clone()));
        assert!(bound.completed.is_none());
        assert!(events.try_recv().is_err());
        let change = owner.finish(journal.receipt).unwrap();
        events.try_recv().unwrap();
        let status = request_status(&store, "first").unwrap().unwrap();
        assert_eq!(status.completed, Some(change.clone()));
        assert_eq!(
            request_status(&store, "first").unwrap(),
            Some(status.clone())
        );
        assert_public_request_status(status);
        assert!(events.try_recv().is_err());
        assert_eq!(
            store.read_events(&change.id, -1, 10).unwrap().events.len(),
            3
        );
    }

    fn assert_public_request_status(status: MemoryRequestStatus) {
        let public = serde_json::to_value(status).unwrap();
        assert!(public.get("owner_hash").is_none());
        assert!(public.get("request_hash").is_none());
    }

    #[test]
    fn http_request_status_refuses_corrupt_ledger_and_receipt_evidence() {
        for column in ["project_id", "request_hash", "owner_hash", "data", "result"] {
            let store = store();
            let root = tempfile::tempdir().unwrap();
            let digest = "b".repeat(64);
            let mut request = write(root.path());
            request.content = &digest;
            request.http_hash = Some(&digest);
            let mut owner = owned(admit(store.clone(), Bus::new(), request).unwrap());
            let journal = journal();
            owner.bind_journal(journal.clone()).unwrap();
            let change = owner.finish(journal.receipt).unwrap();
            store
                .transaction(move |tx| {
                    tx.execute(
                        &format!("UPDATE memory_mutation SET {column}='corrupt' WHERE id=?1"),
                        [&change.id],
                    )?;
                    Ok(())
                })
                .unwrap();
            assert!(request_status(&store, "first").is_err(), "{column}");
        }
        let store = store();
        let root = tempfile::tempdir().unwrap();
        drop(owned(
            admit(store.clone(), Bus::new(), write(root.path())).unwrap(),
        ));
        assert!(request_status(&store, "first").is_err());
    }

    #[test]
    fn http_request_status_refuses_well_formed_foreign_receipts_and_journals() {
        for pointer in [
            "/id",
            "/directory",
            "/project_id",
            "/receipt/name",
            "/receipt/id",
            "/receipt/deleted",
            "/journal/receipt/name",
            "/http_hash",
        ] {
            let store = store();
            let root = tempfile::tempdir().unwrap();
            let digest = "b".repeat(64);
            let mut request = write(root.path());
            request.content = &digest;
            request.http_hash = Some(&digest);
            let mut owner = owned(admit(store.clone(), Bus::new(), request).unwrap());
            let journal = journal();
            owner.bind_journal(journal.clone()).unwrap();
            let change = owner.finish(journal.receipt).unwrap();
            store
                .transaction(move |tx| {
                    let column = if pointer.starts_with("/journal/") || pointer == "/http_hash" {
                        "data"
                    } else {
                        "result"
                    };
                    let data: String = tx.query_row(
                        &format!("SELECT {column} FROM memory_mutation WHERE id=?1"),
                        [&change.id],
                        |row| row.get(0),
                    )?;
                    let mut data: serde_json::Value = serde_json::from_str(&data).unwrap();
                    *data.pointer_mut(pointer).unwrap() = if pointer == "/receipt/deleted" {
                        serde_json::json!(true)
                    } else {
                        serde_json::json!("foreign")
                    };
                    tx.execute(
                        &format!("UPDATE memory_mutation SET {column}=?1 WHERE id=?2"),
                        params![data.to_string(), change.id],
                    )?;
                    Ok(())
                })
                .unwrap();
            assert!(request_status(&store, "first").is_err(), "{pointer}");
        }
    }
    #[test]
    fn committed_identity_replays_once_and_publishes_only_public_acknowledgement() {
        let store = store();
        let root = tempfile::tempdir().unwrap();
        let bus = Bus::new();
        let mut events = bus.subscribe();
        let mut owner = owned(admit(store.clone(), bus.clone(), write(root.path())).unwrap());
        assert!(events.try_recv().is_err());
        let journal = journal();
        owner.bind_journal(journal.clone()).unwrap();
        assert!(events.try_recv().is_err());
        let result = owner.finish(journal.receipt).unwrap();
        assert!(
            matches!(events.try_recv().unwrap(), LiveEvent::MemoryUpdated { update, seq: 2 } if update == result)
        );
        assert!(
            matches!(admit(store.clone(), bus.clone(), write(root.path())).unwrap(), MemoryAdmission::Replay(update) if update == result)
        );
        assert!(events.try_recv().is_err());
        let mut changed = write(root.path());
        changed.content = "different fact";
        assert!(admit(store.clone(), bus, changed).is_err());
        let page = store.read_events(&result.id, -1, 10).unwrap();
        assert_eq!(page.events.len(), 3);
        assert!(
            !serde_json::to_string(&page.events)
                .unwrap()
                .contains("durable fact")
        );
        assert_eq!(lookup(&store, &write(root.path())).unwrap(), Some(result));
    }
    #[test]
    fn disposed_owner_and_another_identity_cannot_reexecute_or_borrow_scope_authority() {
        let store = store();
        let root = tempfile::tempdir().unwrap();
        let bus = Bus::new();
        drop(owned(
            admit(store.clone(), bus.clone(), write(root.path())).unwrap(),
        ));
        assert!(lookup(&store, &write(root.path())).is_err());
        assert!(admit(store.clone(), bus.clone(), write(root.path())).is_err());
        let mut different = write(root.path());
        different.identity = Some("http:second");
        assert!(admit(store.clone(), bus.clone(), different).is_err());
        let mut other = write(root.path());
        other.project_id = "prj_other";
        other.identity = Some("http:other");
        assert!(admit(store, bus, other).is_ok());
    }
    #[test]
    fn completion_requires_unpublished_capability_and_pinned_receipt() {
        let store = store();
        let root = tempfile::tempdir().unwrap();
        let bus = Bus::new();
        let mut owner = owned(admit(store.clone(), bus, write(root.path())).unwrap());
        let journal = journal();
        owner.bind_journal(journal.clone()).unwrap();
        let update = MemoryChange {
            id: owner.request.id.clone(),
            directory: owner.request.directory.clone(),
            project_id: "global".into(),
            receipt: journal.receipt,
        };
        let event = NewEvent::new(
            UPDATED,
            serde_json::to_value(Completion {
                update,
                previous_owner_key: "fabricated".into(),
            })
            .unwrap(),
        );
        assert!(
            store
                .append(&owner.request.id, Expected::Seq(1), vec![event])
                .is_err()
        );
        let mut bad = receipt();
        bad.name = "other".into();
        assert!(owner.finish(bad).is_err());
        assert!(lookup(&store, &write(root.path())).is_err());
    }
    #[test]
    fn unbound_and_mismatched_journals_cannot_complete_or_publish() {
        for bound in [false, true] {
            let store = store();
            let root = tempfile::tempdir().unwrap();
            let bus = Bus::new();
            let mut live = bus.subscribe();
            let mut owner = owned(admit(store.clone(), bus, write(root.path())).unwrap());
            if bound {
                owner.bind_journal(journal()).unwrap();
                assert!(owner.bind_journal(journal()).is_err());
            }
            assert!(owner.finish(receipt()).is_err());
            assert!(lookup(&store, &write(root.path())).is_err());
            assert!(live.try_recv().is_err());
        }
    }

    #[test]
    fn binding_requires_live_capability_and_survives_owner_disposal() {
        let store = store();
        let root = tempfile::tempdir().unwrap();
        let bus = Bus::new();
        let mut owner = owned(admit(store.clone(), bus.clone(), write(root.path())).unwrap());
        let id = owner.request.id.clone();
        let journal = journal();
        let forged = NewEvent::new(
            BOUND,
            serde_json::to_value(Binding {
                journal: journal.clone(),
                previous_owner_key: "fabricated".into(),
                next_owner_hash: hash("next-owner"),
            })
            .unwrap(),
        );
        assert!(store.append(&id, Expected::Seq(0), vec![forged]).is_err());
        let old_key = owner.key.clone();
        owner.bind_journal(journal.clone()).unwrap();
        owner.bind_journal(journal.clone()).unwrap();
        let leaked = store.read_events(&id, -1, 10).unwrap().events[1]
            .data
            .clone();
        assert_eq!(leaked["previous_owner_key"], old_key);
        assert!(!serde_json::to_string(&leaked).unwrap().contains(&owner.key));
        let stale = NewEvent::new(
            UPDATED,
            serde_json::to_value(Completion {
                update: MemoryChange {
                    id: id.clone(),
                    directory: owner.request.directory.clone(),
                    project_id: "global".into(),
                    receipt: journal.receipt.clone(),
                },
                previous_owner_key: old_key,
            })
            .unwrap(),
        );
        assert!(store.append(&id, Expected::Seq(1), vec![stale]).is_err());
        drop(owner);
        let persisted: Request = store
            .read(move |db| {
                let data: String = db.query_row(
                    "SELECT data FROM memory_mutation WHERE id=?1",
                    [&id],
                    |row| row.get(0),
                )?;
                Ok(serde_json::from_str(&data).unwrap())
            })
            .unwrap();
        assert_eq!(persisted.journal, Some(journal));
        assert!(admit(store, bus, write(root.path())).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn disposed_bound_owner_preserves_correlated_uninstalled_journal() {
        let store = store();
        let root = tempfile::tempdir().unwrap();
        let memory = cyber_core::memory::MemoryStore::open(root.path(), "global").unwrap();
        let mut scope = memory.claim().unwrap();
        let mut owner = owned(admit(store.clone(), Bus::new(), write(root.path())).unwrap());
        let id = owner.request.id.clone();
        let prepared = scope
            .prepare_write(
                "---\nname: policy\ndescription: Durable policy\ntype: user\n---\nFact\n",
            )
            .unwrap();
        let journal = prepared.journal_identity().unwrap();
        owner.bind_journal(journal.clone()).unwrap();
        drop(prepared);
        drop(owner);
        assert!(!memory.path().join("policy.md").exists());
        assert!(!memory.path().join("MEMORY.md").exists());
        let review = scope.inspect_recovery().unwrap().unwrap();
        assert_eq!(review.journal, journal);
        let events = store.read_events(&id, -1, 10).unwrap();
        assert_eq!(
            events.events[1].data["journal"],
            serde_json::to_value(journal).unwrap()
        );
        assert!(lookup(&store, &write(root.path())).is_err());
        assert!(admit(store, Bus::new(), write(root.path())).is_err());
    }

    #[test]
    fn historical_unbound_completion_remains_projectable() {
        let store = store();
        let root = tempfile::tempdir().unwrap();
        let bus = Bus::new();
        let owner = owned(admit(store.clone(), bus, write(root.path())).unwrap());
        let update = MemoryChange {
            id: owner.request.id.clone(),
            directory: owner.request.directory.clone(),
            project_id: "global".into(),
            receipt: receipt(),
        };
        let historical = NewEvent::new(
            UPDATED,
            serde_json::to_value(Completion {
                update: update.clone(),
                previous_owner_key: owner.key,
            })
            .unwrap(),
        );
        store
            .append(&update.id, Expected::Seq(0), vec![historical])
            .unwrap();
        assert_eq!(lookup(&store, &write(root.path())).unwrap(), Some(update));
    }

    #[test]
    fn http_identity_retains_pending_and_cross_endpoint_key_conflicts() {
        let store = store();
        let root = tempfile::tempdir().unwrap();
        let bus = Bus::new();
        let digest = "a".repeat(64);
        let mut request = write(root.path());
        request.identity = Some("http:request");
        request.http_hash = Some(&digest);
        let mut owner = owned(admit(store.clone(), bus, request).unwrap());
        assert_eq!(
            http_identity(&store, "request", &digest).unwrap(),
            Some(false)
        );
        assert!(http_identity(&store, "request", &"b".repeat(64)).is_err());
        let journal = journal();
        owner.bind_journal(journal.clone()).unwrap();
        owner.finish(journal.receipt).unwrap();
        assert_eq!(
            http_identity(&store, "request", &digest).unwrap(),
            Some(true)
        );
        assert!(http_identity(&store, "request", &"b".repeat(64)).is_err());
    }
}
