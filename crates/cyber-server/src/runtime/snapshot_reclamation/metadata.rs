//! Receipt-bound metadata retirement history; shape validation grants no native I/O.
use super::{Completion, Plan as PayloadPlan, Receipt, invalid};
use cyber_store::{EventRegistry, StoreError, StoredEvent};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PLANNED: &str = "snapshot.reclamation.metadata.planned.1";
pub const COMPLETED: &str = "snapshot.reclamation.metadata.completed.1";
const FILE_LIMIT: usize = 10;
const FILE_BYTES: u64 = 8 * 1024 * 1024;
const ENCODED_LIMIT: usize = 16 * 1024;

struct EncodingLimit(usize);
impl std::io::Write for EncodingLimit {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|size| *size <= ENCODED_LIMIT)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "metadata encoding exceeds limit",
                )
            })?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn bounded_encoding(value: &impl Serialize) -> Result<(), String> {
    serde_json::to_writer(EncodingLimit(0), value)
        .map_err(|_| "Metadata retirement encoding exceeds limit".into())
}

/// The native adapter derives each route from the receipt and role, never from
/// a caller-provided pathname. These are unique regular metadata files only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Intent,
    Spare,
    Cancellation,
    Lifetime,
    SourcePhase,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub owner: String,
    pub role: Role,
    pub identity: String,
    pub generation: String,
    pub hash: String,
    pub bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub payload_plan_id: String,
    pub files: Vec<File>,
}
impl Plan {
    fn shape(&self) -> Result<(), StoreError> {
        if self.version != 1
            || self.files.is_empty()
            || self.files.len() > FILE_LIMIT
            || !self
                .payload_plan_id
                .strip_prefix("snapshot_reclaim_")
                .is_some_and(super::hash)
        {
            return Err(invalid("Invalid metadata retirement plan bounds"));
        }
        let mut roles = BTreeSet::new();
        let mut identities = BTreeSet::new();
        for file in &self.files {
            if file.owner.is_empty()
                || file.owner.len() > 128
                || !file
                    .owner
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || !native_identity(&file.identity)
                || !super::hash(&file.generation)
                || !super::hash(&file.hash)
                || file.bytes > FILE_BYTES
                || !roles.insert((&file.owner, file.role))
                || !identities.insert(&file.identity)
            {
                return Err(invalid(
                    "Invalid metadata retirement file or duplicate role",
                ));
            }
        }
        // Bound all strings before allocating their serialized representation.
        bounded_encoding(self).map_err(invalid)?;
        self.total_bytes().map(drop)
    }
    pub fn validate(&self, payload: &PayloadPlan) -> Result<(), StoreError> {
        self.shape()?;
        payload.validate()?;
        if self.payload_plan_id != PayloadPlan::id(&payload.receipt)? {
            return Err(invalid("Metadata retirement has a foreign payload plan"));
        }
        let owners = payload.receipt.owners()?;
        for owner in &owners {
            if owner
                .lifetime
                .checkout_resources_digest(&[])
                .map_err(invalid)?
                != owner.resources_digest
            {
                return Err(invalid(
                    "Metadata retirement needs an empty complete checkout inventory",
                ));
            }
            for role in [Role::Intent, Role::Lifetime, Role::SourcePhase] {
                if !self
                    .files
                    .iter()
                    .any(|file| file.owner == owner.origin.activity_id && file.role == role)
                {
                    return Err(invalid("Metadata retirement is missing a worker role"));
                }
            }
        }
        for file in &self.files {
            let owner = owners
                .iter()
                .find(|owner| owner.origin.activity_id == file.owner)
                .ok_or_else(|| invalid("Metadata retirement has a foreign worker"))?;
            if file.role == Role::Lifetime && file.identity != owner.lifetime.file_identity {
                return Err(invalid("Metadata retirement lifetime identity differs"));
            }
            if file.role == Role::Cancellation
                && !matches!(payload.receipt, Receipt::Preparation(_))
            {
                return Err(invalid("Unexpected cancellation metadata retirement role"));
            }
        }
        if matches!(payload.receipt, Receipt::Preparation(_))
            && !self
                .files
                .iter()
                .any(|file| file.role == Role::Cancellation)
        {
            return Err(invalid("Missing cancellation metadata retirement role"));
        }
        Ok(())
    }
    pub fn total_bytes(&self) -> Result<u64, StoreError> {
        self.files.iter().try_fold(0u64, |sum, file| {
            sum.checked_add(file.bytes)
                .ok_or_else(|| invalid("Metadata retirement size overflow"))
        })
    }
    /// Re-attest at the writer around a separately verified native callback.
    /// This method never proves inode ownership or performs filesystem effects.
    pub fn authorize(
        &self,
        db: &Connection,
        payload: &PayloadPlan,
    ) -> Result<BTreeSet<String>, StoreError> {
        self.validate(payload)?;
        if read_payload(db, &self.payload_plan_id)? != *payload {
            return Err(invalid("Metadata retirement payload inventory changed"));
        }
        payload_completed(db, &self.payload_plan_id, payload)?;
        let retained: Option<(String, String)> = db
            .query_row(
                "SELECT type,data FROM event WHERE aggregate_id=?1 AND seq=2",
                [&self.payload_plan_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((kind, raw)) = retained {
            let plan: Plan = serde_json::from_str(&raw).map_err(|e| invalid(e.to_string()))?;
            if kind != PLANNED || plan != *self {
                return Err(invalid(
                    "Metadata retirement differs from the retained plan",
                ));
            }
        }
        payload.authorize(db)
    }
}
fn native_identity(value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    let parts = value.split(':').collect::<Vec<_>>();
    value.len() <= 128
        && parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && part.parse::<u64>().is_ok()
        })
        && parts[3]
            .parse::<u32>()
            .is_ok_and(|nanos| nanos < 1_000_000_000)
}
fn read_payload(db: &Connection, id: &str) -> Result<PayloadPlan, StoreError> {
    let raw = event_at(db, id, 0, super::PLANNED)?;
    let payload: PayloadPlan = serde_json::from_str(&raw).map_err(|e| invalid(e.to_string()))?;
    payload.validate()?;
    if PayloadPlan::id(&payload.receipt)? != id {
        return Err(invalid("Foreign metadata retirement aggregate"));
    }
    Ok(payload)
}
fn event_at(db: &Connection, id: &str, sequence: i64, kind: &str) -> Result<String, StoreError> {
    db.query_row(
        "SELECT data FROM event WHERE aggregate_id=?1 AND seq=?2 AND type=?3",
        rusqlite::params![id, sequence, kind],
        |row| row.get(0),
    )
    .map_err(StoreError::from)
}
fn check_completion(raw: &str, id: &str, bytes: u64) -> Result<(), StoreError> {
    let completed: Completion = serde_json::from_str(raw).map_err(|e| invalid(e.to_string()))?;
    if completed.plan_id != id || completed.removed_bytes != bytes {
        return Err(invalid("Metadata or payload retirement completion differs"));
    }
    Ok(())
}
fn payload_completed(db: &Connection, id: &str, payload: &PayloadPlan) -> Result<(), StoreError> {
    check_completion(
        &event_at(db, id, 1, super::COMPLETED)?,
        id,
        payload.total_bytes()?,
    )
}
fn planned(db: &Connection, id: &str, payload: &PayloadPlan) -> Result<Plan, StoreError> {
    let plan: Plan =
        serde_json::from_str(&event_at(db, id, 2, PLANNED)?).map_err(|e| invalid(e.to_string()))?;
    plan.validate(payload)?;
    Ok(plan)
}

pub(super) fn history(
    db: &Connection,
    id: &str,
    payload: &PayloadPlan,
    sequence: i64,
    kind: &str,
) -> Result<(bool, Option<Plan>, bool), StoreError> {
    match (sequence, kind) {
        (0, super::PLANNED) => Ok((false, None, false)),
        (1, super::COMPLETED) => {
            payload_completed(db, id, payload)?;
            Ok((true, None, false))
        }
        (2, PLANNED) | (3, COMPLETED) => {
            payload_completed(db, id, payload)?;
            let plan = planned(db, id, payload)?;
            if sequence == 3 {
                check_completion(&event_at(db, id, 3, COMPLETED)?, id, plan.total_bytes()?)?;
            }
            Ok((true, Some(plan), sequence == 3))
        }
        _ => Err(invalid(
            "Invalid reclamation or metadata retirement history",
        )),
    }
}

pub(super) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(PLANNED, |value| {
            // Reject oversized vectors/encoding before cloning or deserializing
            // the event's JSON into owned metadata fields.
            if value
                .get("files")
                .and_then(serde_json::Value::as_array)
                .is_none_or(|files| files.is_empty() || files.len() > FILE_LIMIT)
            {
                return Err("Invalid metadata retirement file bounds".into());
            }
            bounded_encoding(value)?;
            serde_json::from_value::<Plan>(value.clone())
                .map_err(|e| e.to_string())?
                .shape()
                .map_err(|e| e.to_string())
        })
        .expect("metadata retirement plan event");
    registry
        .register_with(COMPLETED, |value| {
            serde_json::from_value::<Completion>(value.clone())
                .map(drop)
                .map_err(|e| e.to_string())
        })
        .expect("metadata retirement completion event");
}
pub(super) fn project(db: &Connection, event: &StoredEvent) -> Result<(), String> {
    let payload = read_payload(db, &event.aggregate_id).map_err(|e| e.to_string())?;
    payload_completed(db, &event.aggregate_id, &payload).map_err(|e| e.to_string())?;
    let plan = match event.kind.as_str() {
        PLANNED if event.seq == 2 => {
            serde_json::from_value::<Plan>(event.data.clone()).map_err(|e| e.to_string())?
        }
        COMPLETED if event.seq == 3 => {
            let plan = planned(db, &event.aggregate_id, &payload).map_err(|e| e.to_string())?;
            check_completion(
                &event.data.to_string(),
                &event.aggregate_id,
                plan.total_bytes().map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            plan
        }
        _ => return Err("Invalid metadata retirement event sequence".into()),
    };
    plan.validate(&payload).map_err(|e| e.to_string())?;
    let current: i64 = db
        .query_row(
            "SELECT seq FROM event_sequence WHERE aggregate_id=?1",
            [&event.aggregate_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or(-1);
    if current < event.seq {
        plan.authorize(db, &payload).map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl crate::runtime::Runtime {
    /// An observed retirement answers metadata acknowledgement without reopening
    /// removed files. A pending plan freezes them against competing rewrites.
    pub(crate) fn snapshot_metadata_retired(
        &self,
        receipt: &Receipt,
    ) -> Result<bool, crate::runtime::RuntimeError> {
        let retained = receipt.clone();
        let record = self.inner.store.read(move |db| {
            let tx = db.unchecked_transaction()?;
            retained.verify(&tx)?;
            let record = super::lookup(&tx, &retained)?;
            tx.commit()?;
            Ok(record)
        })?;
        let Some(record) = record else {
            return Ok(false);
        };
        if record.retired {
            let session = &receipt.origin()?.session_id;
            let handle = self
                .inner
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(session)
                .cloned();
            if let Some(handle) = handle {
                for owner in receipt.owners()? {
                    handle.acknowledge_snapshot_uncertainty(&owner.origin.activity_id);
                }
            }
            return Ok(true);
        }
        if record.metadata.is_some() {
            return Err(crate::runtime::RuntimeError::Conflict("Snapshot metadata retirement is pending; resume maintenance before acknowledgement".into()));
        }
        Ok(false)
    }
}
