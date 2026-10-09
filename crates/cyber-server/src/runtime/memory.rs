//! Durable scope fencing and receipts; persisted admission never grants live write authority.
use super::{Bus, LiveEvent};
use cyber_core::memory::MemoryMutation;
use cyber_store::{EventRegistry, Expected, NewEvent, Store, StoreError, StoredEvent};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const ADMITTED: &str = "memory.mutation.admitted.1";
const UPDATED: &str = "memory.updated.1";

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

pub struct MemoryWrite<'a> {
    pub directory: &'a Path,
    pub project_id: &'a str,
    pub name: &'a str,
    pub deleted: bool,
    pub identity: Option<&'a str>,
    pub content: &'a str,
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
}

impl MemoryWriteOwner {
    pub fn finish(self, receipt: MemoryMutation) -> Result<MemoryChange, StoreError> {
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
            Expected::Seq(0),
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
    registry.projector(project);
}
fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    match event.kind.as_str() {
        ADMITTED => project_admission(tx, event),
        UPDATED => project_completion(tx, event),
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
    {
        return Err("Invalid memory admission identity".into());
    }
    cyber_core::memory::validate_name(&request.name).map_err(|e| e.to_string())?;
    cyber_core::memory::directory(Path::new("/"), &request.project_id)
        .map_err(|e| e.to_string())?;
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
    if event.seq != 1
        || row.1.is_some()
        || hash(&completed.previous_owner_key) != request.owner_hash
        || update.id != request.id
        || update.directory != request.directory
        || update.project_id != request.project_id
        || update.receipt.name != request.name
        || update.receipt.deleted != request.deleted
        || !cyber_core::ids::has_prefix(&update.receipt.id, "mem")
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
    #[test]
    fn committed_identity_replays_once_and_publishes_only_public_acknowledgement() {
        let store = store();
        let root = tempfile::tempdir().unwrap();
        let bus = Bus::new();
        let mut events = bus.subscribe();
        let owner = owned(admit(store.clone(), bus.clone(), write(root.path())).unwrap());
        assert!(events.try_recv().is_err());
        let result = owner.finish(receipt()).unwrap();
        assert!(
            matches!(events.try_recv().unwrap(), LiveEvent::MemoryUpdated { update, seq: 1 } if update == result)
        );
        assert!(
            matches!(admit(store.clone(), bus.clone(), write(root.path())).unwrap(), MemoryAdmission::Replay(update) if update == result)
        );
        assert!(events.try_recv().is_err());
        let mut changed = write(root.path());
        changed.content = "different fact";
        assert!(admit(store.clone(), bus, changed).is_err());
        let page = store.read_events(&result.id, -1, 10).unwrap();
        assert_eq!(page.events.len(), 2);
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
        let owner = owned(admit(store.clone(), bus, write(root.path())).unwrap());
        let update = MemoryChange {
            id: owner.request.id.clone(),
            directory: owner.request.directory.clone(),
            project_id: "global".into(),
            receipt: receipt(),
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
                .append(&owner.request.id, Expected::Seq(0), vec![event])
                .is_err()
        );
        let mut bad = receipt();
        bad.name = "other".into();
        assert!(owner.finish(bad).is_err());
        assert!(lookup(&store, &write(root.path())).is_err());
    }
}
