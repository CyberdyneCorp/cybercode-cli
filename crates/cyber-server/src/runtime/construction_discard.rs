//! Owner-only discard of construction evidence (`snapshots-checkpoints` → Owner-only
//! discard of construction evidence). A construction stage is a private file a
//! constructor flushed but never published: a checkout claim stage
//! (`preparing_*`), a checkout acknowledgement stage (`ack_preparing_*`) or a
//! file-copy cancellation-record stage (`tmp_*` beside `preparation.json`).
//!
//! Review is read-only and returns a fingerprint over the stage's native identity,
//! digest, fences, Location state and the Session revision. Confirmation re-fences
//! the owner activity, records a durable intent in the Session's history, moves the
//! stage with a no-replace rename into a private `discarded/<key>/` slot of its
//! namespace and unlinks it there. Each request ID has one receipt; a retry resumes
//! the frozen plan and a lookup never acts.
use super::{Runtime, RuntimeError, SessionInfo, activity};
use cyber_core::construction_discard::{self as native, Place, StageEvidence};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

mod confirm;
pub use confirm::spawn_retry;

/// Durable intent and completion records of one discard request.
pub const DISCARDED: &str = "session.snapshot.construction_discarded.1";
/// The only actor this revision admits.
pub const LOCAL_OWNER: &str = "local_owner";
const LISTING_LIMIT: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConstructionStageKind {
    CheckoutClaim,
    CheckoutAcknowledgement,
    CancellationRecord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConstructionLocation {
    Present,
    Removed,
}

/// One listed stage. Listing grants no discard authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConstructionStageEntry {
    pub name: String,
    pub kind: ConstructionStageKind,
    /// The constructing activity; absent when the name has no derivable owner.
    pub activity_id: Option<String>,
    pub location: ConstructionLocation,
}

/// A reviewed stage, frozen into the discard receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConstructionStage {
    pub name: String,
    pub kind: ConstructionStageKind,
    pub activity_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal_id: Option<String>,
    pub evidence: StageEvidence,
    /// Operations that refuse while this stage exists.
    pub fences: Vec<String>,
    pub location: ConstructionLocation,
    /// The private directory that holds the stage.
    pub source: PathBuf,
    /// The namespace whose `discarded/` quarantine receives it.
    pub namespace: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConstructionDiscardSelection {
    /// The exact stage name, from the construction listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// An activity with exactly one stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConstructionDiscardReview {
    pub session_id: String,
    pub session_sequence: i64,
    pub stage: ConstructionStage,
    pub confirmation: String,
    pub warning: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConstructionDiscardConfirmation {
    /// The reviewed stage name; one stage per confirmation.
    pub stage: String,
    pub confirmation: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConstructionDiscardStatus {
    /// Recorded before any filesystem change; the plan is frozen.
    Intent,
    /// Moved into the quarantine, unlink not yet done (`discard pending`).
    Pending,
    /// The stage is gone.
    Completed,
    /// A different file holds the name; nothing was deleted.
    Changed,
}

/// The audit receipt of one request, as last recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConstructionDiscardReceipt {
    pub request_id: String,
    pub session_id: String,
    pub confirmation: String,
    pub stage: ConstructionStage,
    pub actor: String,
    pub recorded_ms: i64,
    pub status: ConstructionDiscardStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ConstructionDiscardReceipt {
    /// Still owes its unlink: listed and reported as `discard pending`.
    pub fn pending(&self) -> bool {
        matches!(
            self.status,
            ConstructionDiscardStatus::Intent | ConstructionDiscardStatus::Pending
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConstructionInventory {
    pub stages: Vec<ConstructionStageEntry>,
    /// Requests whose stage is not unlinked yet (`discard pending`).
    pub pending: Vec<ConstructionDiscardReceipt>,
}

/// A native stage as a host observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeConstructionStage {
    pub name: String,
    pub kind: ConstructionStageKind,
    pub activity_id: Option<String>,
    pub worktree_id: Option<String>,
    pub journal_id: Option<String>,
    pub evidence: StageEvidence,
    pub source: PathBuf,
    pub namespace: PathBuf,
}

/// Why a host refused to fence a stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstructionFenceError {
    /// A live holder (constructor or worker lifetime) still owns it.
    Busy(String),
    Refused(String),
}

/// Exclusive native ownership of one stage, held from fencing through its move.
pub trait ConstructionFence: Send {
    /// Move the stage into `discarded/<key>/` of its namespace without replacing anything.
    fn quarantine(&mut self, key: &str) -> Result<(), String>;
}

pub(super) fn validate_event(data: &serde_json::Value) -> Result<(), String> {
    let receipt: ConstructionDiscardReceipt =
        serde_json::from_value(data.clone()).map_err(|error| error.to_string())?;
    if receipt.actor != LOCAL_OWNER
        || receipt.request_id.is_empty()
        || receipt.request_id.len() > 256
        || !native::valid_component(&receipt.stage.name)
    {
        return Err("Invalid construction discard receipt".into());
    }
    Ok(())
}

pub(super) fn quarantine_key(session: &str, request: &str) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&("construction-discard-v1", session, request))
        .expect("strings serialize");
    format!("{:x}", Sha256::digest(bytes))
}

fn location_state(info: &SessionInfo) -> ConstructionLocation {
    match std::fs::symlink_metadata(&info.directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ConstructionLocation::Removed,
        _ => ConstructionLocation::Present,
    }
}

fn kind_of(name: &str) -> Option<ConstructionStageKind> {
    if name.starts_with("ack_preparing_") {
        Some(ConstructionStageKind::CheckoutAcknowledgement)
    } else if name.starts_with("preparing_") {
        Some(ConstructionStageKind::CheckoutClaim)
    } else if name.starts_with("tmp_") {
        Some(ConstructionStageKind::CancellationRecord)
    } else {
        None
    }
}

fn fences(stage: &NativeConstructionStage, activity: &str) -> Vec<String> {
    match stage.kind {
        ConstructionStageKind::CheckoutClaim => vec![
            "managed worktree removal".into(),
            "checkout metadata retirement".into(),
            format!("new checkout claims by {activity}"),
        ],
        ConstructionStageKind::CheckoutAcknowledgement => vec![
            format!("managed worktree removal until {activity}'s acknowledgement is verified"),
            "checkout metadata retirement".into(),
        ],
        ConstructionStageKind::CancellationRecord => vec![format!(
            "reclamation of journal {}",
            stage.journal_id.as_deref().unwrap_or("unknown")
        )],
    }
}

fn conflict(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Conflict(message.into())
}

/// The latest record of every snapshot activity of one Session.
pub(super) fn session_activities(
    db: &rusqlite::Connection,
    session: &str,
) -> Result<BTreeMap<String, activity::Record>, cyber_store::StoreError> {
    let mut query = db.prepare(
        "SELECT aggregate_id,data FROM event e WHERE type=?2 AND json_extract(data,'$.session_id')=?1 \
         AND json_extract(data,'$.snapshot_worker')=1 \
         AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq) \
         ORDER BY aggregate_id LIMIT 4097",
    )?;
    let rows = query.query_map([session, activity::CHANGED], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut found = BTreeMap::new();
    for row in rows {
        let (id, data) = row?;
        let record: activity::Record = serde_json::from_str(&data).map_err(|_| {
            cyber_store::StoreError::InvalidAppend("Invalid snapshot activity".into())
        })?;
        found.insert(id, record);
    }
    if found.len() > LISTING_LIMIT {
        return Err(cyber_store::StoreError::InvalidAppend(
            "Snapshot activity limit exceeded".into(),
        ));
    }
    Ok(found)
}

/// The origin of an activity whose cancellation recorded a journal binding.
fn cancellation_origin(record: &activity::Record) -> Option<&super::SnapshotOrigin> {
    record
        .snapshot_preparation_receipt
        .as_ref()
        .and_then(|receipt| receipt.origin.as_ref())
}

impl Runtime {
    pub(super) async fn construction_activities(
        &self,
        session: &str,
    ) -> Result<BTreeMap<String, activity::Record>, RuntimeError> {
        let session = session.to_owned();
        Ok(self
            .inner
            .store
            .read(move |db| session_activities(db, &session))?)
    }

    /// Read-only inventory of this Session's construction stages and of its
    /// discards that still owe an unlink.
    pub async fn construction_inventory(
        &self,
        session: &str,
    ) -> Result<ConstructionInventory, RuntimeError> {
        let info = self.session_info(session).await?;
        let location = location_state(&info);
        let mut stages: Vec<ConstructionStageEntry> = self
            .inner
            .tools
            .snapshot_checkout_constructions(&info)
            .map_err(conflict)?
            .into_iter()
            .map(|stage| ConstructionStageEntry {
                kind: match stage.kind {
                    cyber_core::worktrees::SnapshotConstructionKind::CheckoutClaim => {
                        ConstructionStageKind::CheckoutClaim
                    }
                    cyber_core::worktrees::SnapshotConstructionKind::CheckoutAcknowledgement => {
                        ConstructionStageKind::CheckoutAcknowledgement
                    }
                },
                name: stage.name,
                activity_id: stage.owner_id,
                location,
            })
            .collect();
        for (activity, record) in self.construction_activities(session).await? {
            let Some(origin) = cancellation_origin(&record) else {
                continue;
            };
            for name in self
                .inner
                .options
                .snapshots
                .cancellation_stages(origin.clone())
                .await
                .map_err(conflict)?
            {
                stages.push(ConstructionStageEntry {
                    name,
                    kind: ConstructionStageKind::CancellationRecord,
                    activity_id: Some(activity.clone()),
                    location,
                });
            }
        }
        let pending = self
            .construction_receipts(Some(session))
            .await?
            .into_iter()
            .filter(ConstructionDiscardReceipt::pending)
            .collect();
        Ok(ConstructionInventory { stages, pending })
    }

    /// Resolve the selection to exactly one stage name.
    async fn select_stage(
        &self,
        session: &str,
        selection: &ConstructionDiscardSelection,
    ) -> Result<String, RuntimeError> {
        match (&selection.stage, &selection.activity_id) {
            (Some(stage), None) => Ok(stage.clone()),
            (None, Some(activity)) => {
                let names: Vec<String> = self
                    .construction_inventory(session)
                    .await?
                    .stages
                    .into_iter()
                    .filter(|stage| stage.activity_id.as_deref() == Some(activity))
                    .map(|stage| stage.name)
                    .collect();
                match names.as_slice() {
                    [name] => Ok(name.clone()),
                    [] => Err(conflict(format!(
                        "Activity {activity} has no construction stage"
                    ))),
                    _ => Err(conflict(format!(
                        "Activity {activity} has {} construction stages; review one stage per confirmation: {}",
                        names.len(),
                        names.join(", ")
                    ))),
                }
            }
            _ => Err(RuntimeError::Invalid(
                "Name exactly one construction stage or one activity".into(),
            )),
        }
    }

    /// Observe one stage natively, without opening it for writing or locking it.
    pub(super) async fn native_stage(
        &self,
        info: &SessionInfo,
        activities: &BTreeMap<String, activity::Record>,
        name: &str,
    ) -> Result<NativeConstructionStage, RuntimeError> {
        if !native::valid_component(name) {
            return Err(RuntimeError::Invalid(
                "Invalid construction stage name".into(),
            ));
        }
        let missing = || conflict(format!("No construction stage {name} for this Session"));
        match kind_of(name).ok_or_else(missing)? {
            ConstructionStageKind::CancellationRecord => {
                for origin in activities.values().filter_map(cancellation_origin) {
                    if let Some(stage) = self
                        .inner
                        .options
                        .snapshots
                        .cancellation_stage(origin.clone(), name.to_owned())
                        .await
                        .map_err(conflict)?
                    {
                        return Ok(stage);
                    }
                }
                Err(missing())
            }
            _ => self
                .inner
                .tools
                .snapshot_construction_stage(info, name)
                .map_err(conflict)?
                .ok_or_else(missing),
        }
    }

    /// The reviewed stage, with its owner proven to be a snapshot activity of this Session.
    pub(super) async fn reviewed_stage(
        &self,
        info: &SessionInfo,
        name: &str,
    ) -> Result<(ConstructionStage, BTreeMap<String, activity::Record>), RuntimeError> {
        let activities = self.construction_activities(&info.id).await?;
        let native = self.native_stage(info, &activities, name).await?;
        let activity = native.activity_id.clone().ok_or_else(|| {
            conflict(format!(
                "Construction stage {name} has no derivable owner; it stays listed and fenced"
            ))
        })?;
        if !activities.contains_key(&activity) {
            return Err(conflict(format!(
                "Construction stage {name} names {activity}, which is not a snapshot activity of this Session"
            )));
        }
        let location = location_state(info);
        if location == ConstructionLocation::Removed && info.worktree_id.is_some() {
            return Err(conflict(
                "The managed checkout of this removed Location cannot be verified; its evidence stays fenced",
            ));
        }
        let stage = ConstructionStage {
            fences: fences(&native, &activity),
            name: native.name,
            kind: native.kind,
            activity_id: activity,
            session_id: info.id.clone(),
            worktree_id: native.worktree_id,
            journal_id: native.journal_id,
            evidence: native.evidence,
            location,
            source: native.source,
            namespace: native.namespace,
        };
        Ok((stage, activities))
    }

    pub(super) fn discard_fingerprint(
        session: &str,
        sequence: i64,
        stage: &ConstructionStage,
    ) -> Result<String, RuntimeError> {
        use sha2::{Digest, Sha256};
        let bytes =
            serde_json::to_vec(&("construction-discard-review-v1", session, sequence, stage))
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    /// Read-only review of one stage with its confirmation fingerprint.
    pub async fn construction_discard_review(
        &self,
        session: &str,
        selection: &ConstructionDiscardSelection,
    ) -> Result<ConstructionDiscardReview, RuntimeError> {
        let name = self.select_stage(session, selection).await?;
        let handle = self.inner.handle(session).await?;
        let (info, sequence) = {
            let state = handle.state.lock().await;
            (state.info.clone(), state.last_seq)
        };
        let (stage, _) = self.reviewed_stage(&info, &name).await?;
        let confirmation = Self::discard_fingerprint(session, sequence, &stage)?;
        Ok(ConstructionDiscardReview {
            session_id: session.into(),
            session_sequence: sequence,
            stage,
            confirmation,
            warning:
                "Discards only this unpublished construction stage after fencing its activity. \
                      Restore journals, payload images, published claims, acknowledgements and \
                      cancellation records are never touched; the managed worktree is not removed."
                    .into(),
        })
    }

    /// Every recorded receipt, latest record per request; one Session or all.
    pub(super) async fn construction_receipts(
        &self,
        session: Option<&str>,
    ) -> Result<Vec<ConstructionDiscardReceipt>, RuntimeError> {
        let session = session.map(str::to_owned);
        Ok(self.inner.store.read(move |db| {
            let mut query = db.prepare(
                "SELECT data FROM event e WHERE type=?1 AND (?2 IS NULL OR aggregate_id=?2) \
                 AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.type=e.type \
                 AND json_extract(n.data,'$.request_id')=json_extract(e.data,'$.request_id') AND n.seq>e.seq) \
                 ORDER BY aggregate_id,seq LIMIT 4097",
            )?;
            let rows = query.query_map(rusqlite::params![DISCARDED, session], |row| {
                row.get::<_, String>(0)
            })?;
            let mut found = vec![];
            for row in rows {
                let receipt: ConstructionDiscardReceipt = serde_json::from_str(&row?)
                    .map_err(|_| cyber_store::StoreError::InvalidAppend("Invalid discard receipt".into()))?;
                found.push(receipt);
            }
            Ok(found)
        })?)
    }

    /// The receipt of one request as last recorded, without acting.
    pub async fn construction_discard_lookup(
        &self,
        session: &str,
        request: &str,
    ) -> Result<Option<ConstructionDiscardReceipt>, RuntimeError> {
        super::snapshot_recovery::validate_request_id(request)?;
        self.session_info(session).await?;
        self.discard_receipt(session, request)
    }

    /// Store-only lookup; safe while the Session lock is held.
    pub(super) fn discard_receipt(
        &self,
        session: &str,
        request: &str,
    ) -> Result<Option<ConstructionDiscardReceipt>, RuntimeError> {
        let (session, request) = (session.to_owned(), request.to_owned());
        Ok(self.inner.store.read(move |db| {
            use rusqlite::OptionalExtension;
            let data: Option<String> = db
                .query_row(
                    "SELECT data FROM event WHERE aggregate_id=?1 AND type=?2 \
                     AND json_extract(data,'$.request_id')=?3 ORDER BY seq DESC LIMIT 1",
                    rusqlite::params![session, DISCARDED, request],
                    |row| row.get(0),
                )
                .optional()?;
            data.map(|data| {
                serde_json::from_str(&data).map_err(|_| {
                    cyber_store::StoreError::InvalidAppend("Invalid discard receipt".into())
                })
            })
            .transpose()
        })?)
    }
}

/// Where the frozen stage is now. Paths come from the receipt; identities are
/// compared before anything is reported as the reviewed stage.
pub(super) fn locate(stage: &ConstructionStage, key: &str) -> Result<Place, String> {
    native::locate_at(
        &stage.source,
        &stage.namespace,
        &stage.name,
        key,
        &stage.evidence,
    )
    .map_err(|error| error.to_string())
}

/// Unlink the frozen stage from its quarantine slot when it still has the reviewed identity.
pub(super) fn unlink(stage: &ConstructionStage, key: &str) -> Result<Place, String> {
    native::unlink_at(&stage.namespace, key, &stage.name, &stage.evidence)
        .map_err(|error| error.to_string())
}
