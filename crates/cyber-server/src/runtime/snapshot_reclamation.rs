//! Durable private-payload cleanup plans. These never settle native activities.
use super::{SnapshotOrigin, SnapshotRestoreReceipt, activity};
use cyber_store::{EventRegistry, StoreError, StoredEvent};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const PLANNED: &str = "snapshot.reclamation.planned.1";
pub const COMPLETED: &str = "snapshot.reclamation.completed.1";
const LIMIT: usize = 4096;
const BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub name: String,
    pub identity: String,
    pub generation: String,
    pub hash: String,
    pub bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub receipt: SnapshotRestoreReceipt,
    pub files: Vec<File>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub plan_id: String,
    pub removed_bytes: u64,
}
#[derive(Debug, Clone)]
pub struct Record {
    pub plan: Plan,
    pub completed: bool,
}
fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::InvalidAppend(message.into())
}
fn hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
impl Plan {
    pub fn id(receipt: &SnapshotRestoreReceipt) -> Result<String, StoreError> {
        let frame = serde_json::to_vec(&("snapshot-reclamation-v1", receipt))
            .map_err(|e| invalid(e.to_string()))?;
        Ok(format!("snapshot_reclaim_{:x}", Sha256::digest(frame)))
    }
    pub fn validate(&self) -> Result<(), StoreError> {
        let checkout = self
            .receipt
            .checkout
            .as_ref()
            .ok_or_else(|| invalid("Reclamation requires retained checkout evidence"))?;
        self.receipt
            .checkout_acknowledgement_digest()
            .map_err(invalid)?;
        let empty = checkout
            .lifetime
            .checkout_resources_digest(&[])
            .map_err(invalid)?;
        if self.version != 1
            || self.receipt.origin.worktree_id.is_some()
            || checkout.resources_digest != empty
            || self.files.len() > LIMIT
            || serde_json::to_vec(self)
                .map_err(|e| invalid(e.to_string()))?
                .len()
                > BYTES
        {
            return Err(invalid(
                "Reclamation plan exceeds supported ownership or bounds",
            ));
        }
        let mut names = BTreeSet::new();
        for file in &self.files {
            let index = file
                .name
                .strip_prefix("stage_")
                .and_then(|index| index.parse::<usize>().ok());
            if index.is_none_or(|index| index >= LIMIT || file.name != format!("stage_{index}"))
                || !names.insert(&file.name)
                || file.identity.is_empty()
                || file.identity.len() > 128
                || !hash(&file.generation)
                || !hash(&file.hash)
            {
                return Err(invalid("Invalid private reclamation payload"));
            }
        }
        self.total_bytes()?;
        Ok(())
    }
    pub fn total_bytes(&self) -> Result<u64, StoreError> {
        self.files.iter().try_fold(0u64, |sum, file| {
            sum.checked_add(file.bytes)
                .ok_or_else(|| invalid("Reclamation size overflow"))
        })
    }
    /// Must run at the Store writer immediately around private deletion. A caller
    /// must separately retain native namespace, reader, lifetime and lease proof.
    pub fn authorize(&self, db: &Connection) -> Result<BTreeSet<String>, StoreError> {
        self.validate()?;
        super::snapshot_restore_receipt::verify(db, &self.receipt)?;
        let journal = &self.receipt.origin.binding.journal_id;
        let directory = self
            .receipt
            .origin
            .binding
            .directory
            .to_str()
            .ok_or_else(|| invalid("Non-UTF8 reclamation Location"))?;
        let protected: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM event e LEFT JOIN session s ON s.id=json_extract(e.data,'$.session_id') WHERE e.type='native.activity.changed.1' AND coalesce(json_extract(e.data,'$.status'),'unknown')<>'settled' AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq) AND (s.id IS NULL OR (coalesce(json_extract(e.data,'$.status'),'unknown')='unknown' AND json_extract(e.data,'$.snapshot_lifetime') IS NULL) OR s.directory=?2 OR json_extract(e.data,'$.session_id')=?3 OR json_extract(e.data,'$.snapshot_lifetime.owner.directory')=?2 OR EXISTS(SELECT 1 FROM json_tree(e.data) j WHERE j.key='journal_id' AND j.value=?1)))",
            rusqlite::params![journal, directory, self.receipt.origin.session_id], |row| row.get(0),
        )?;
        if protected {
            return Err(invalid(
                "An unsettled native owner or incomplete Location view protects this journal",
            ));
        }
        references(db)
    }
}

/// Complete bounded durable roots. Failure must never be treated as an empty set.
pub fn references(db: &Connection) -> Result<BTreeSet<String>, StoreError> {
    let mut query =
        db.prepare("SELECT DISTINCT root FROM snapshot_reference ORDER BY root LIMIT 100001")?;
    let roots = query
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<BTreeSet<_>, _>>()?;
    if roots.len() > 100000 {
        return Err(invalid(
            "Snapshot reference limit exceeded; reclamation refused",
        ));
    }
    Ok(roots)
}

pub fn restore_receipt(
    db: &Connection,
    origin: &SnapshotOrigin,
) -> Result<Option<SnapshotRestoreReceipt>, StoreError> {
    let raw: Option<String> = db
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq DESC LIMIT 1",
            [&origin.activity_id, activity::CHANGED],
            |row| row.get(0),
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let record: activity::Record =
        serde_json::from_str(&raw).map_err(|e| invalid(e.to_string()))?;
    let Some(receipt) = record.snapshot_restore_receipt else {
        return Ok(None);
    };
    if receipt.origin != *origin {
        return Err(invalid("Foreign reclamation receipt"));
    }
    super::snapshot_restore_receipt::verify(db, &receipt)?;
    Ok(Some(receipt))
}

pub fn lookup(
    db: &Connection,
    receipt: &SnapshotRestoreReceipt,
) -> Result<Option<Record>, StoreError> {
    let id = Plan::id(receipt)?;
    let first: Option<(String, String)> = db
        .query_row(
            "SELECT type,data FROM event WHERE aggregate_id=?1 AND seq=0",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((kind, raw)) = first else {
        return Ok(None);
    };
    let plan: Plan = serde_json::from_str(&raw).map_err(|e| invalid(e.to_string()))?;
    plan.validate()?;
    if kind != PLANNED || plan.receipt != *receipt {
        return Err(invalid("Reclamation request owner differs"));
    }
    let (seq, kind, raw): (i64, String, String) = db.query_row(
        "SELECT seq,type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
        [&id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let completed = match seq {
        0 if kind == PLANNED => false,
        1 if kind == COMPLETED => {
            let completion: Completion =
                serde_json::from_str(&raw).map_err(|e| invalid(e.to_string()))?;
            if completion.plan_id != id || completion.removed_bytes != plan.total_bytes()? {
                return Err(invalid("Reclamation completion differs"));
            }
            true
        }
        _ => return Err(invalid("Invalid reclamation history")),
    };
    Ok(Some(Record { plan, completed }))
}

pub(super) fn register(registry: &mut EventRegistry) {
    registry
        .register_with(PLANNED, |value| {
            serde_json::from_value::<Plan>(value.clone())
                .map_err(|e| e.to_string())?
                .validate()
                .map_err(|e| e.to_string())
        })
        .expect("reclamation plan event");
    registry
        .register_with(COMPLETED, |value| {
            serde_json::from_value::<Completion>(value.clone())
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .expect("reclamation completion event");
}
/// Validate append ownership while allowing historical projection rebuilds after
/// later fences or Session deletion. Replaying records never grants file effects.
pub fn project(db: &Connection, event: &StoredEvent) -> Result<(), String> {
    let current = if matches!(event.kind.as_str(), PLANNED | COMPLETED) {
        db.query_row(
            "SELECT seq FROM event_sequence WHERE aggregate_id=?1",
            [&event.aggregate_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .unwrap_or(-1)
    } else {
        return Ok(());
    };
    let appending = current < event.seq;
    match event.kind.as_str() {
        PLANNED => {
            let plan: Plan =
                serde_json::from_value(event.data.clone()).map_err(|e| e.to_string())?;
            if event.seq != 0
                || Plan::id(&plan.receipt).map_err(|e| e.to_string())? != event.aggregate_id
            {
                return Err("Foreign reclamation plan owner".into());
            }
            plan.validate().map_err(|e| e.to_string())?;
            if appending {
                plan.authorize(db).map_err(|e| e.to_string())?;
            }
        }
        COMPLETED => {
            let completion: Completion =
                serde_json::from_value(event.data.clone()).map_err(|e| e.to_string())?;
            let raw: String = db
                .query_row(
                    "SELECT data FROM event WHERE aggregate_id=?1 AND seq=0 AND type=?2",
                    [&event.aggregate_id, PLANNED],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            let plan: Plan = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            if event.seq != 1
                || completion.plan_id != event.aggregate_id
                || completion.removed_bytes != plan.total_bytes().map_err(|e| e.to_string())?
            {
                return Err("Foreign reclamation completion".into());
            }
            plan.validate().map_err(|e| e.to_string())?;
            if appending {
                plan.authorize(db).map_err(|e| e.to_string())?;
            }
        }
        _ => {}
    }
    Ok(())
}
