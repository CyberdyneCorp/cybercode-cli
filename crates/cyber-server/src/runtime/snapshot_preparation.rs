//! Consuming proof for an exited snapshot worker that never published an origin.
use super::snapshot_recovery_authority::RecoveryAuthority;
use super::{Runtime, RuntimeError, SnapshotLifetime, SnapshotLifetimeBinding, activity};
use cyber_store::{NewEvent, StoreError};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, PoisonError, atomic::Ordering};

pub(super) const CANCELLED: &str = "snapshot.preparation.cancelled.1";
const PHASE: &str = "snapshot_preparation_cancelled";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotPreparationConfirmation {
    pub confirmation: String,
    pub request_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnapshotPreparation {
    pub activity_id: String,
    pub status: String,
    pub lifetime_available: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnapshotPreparationReview {
    pub activity_id: String,
    pub lifetime: SnapshotLifetimeBinding,
    pub native_sequence: i64,
    pub session_sequence: i64,
    pub confirmation: String,
    pub warning: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotPreparationReceipt {
    pub activity_id: String,
    pub request_id: String,
    pub confirmation: String,
    pub lifetime: SnapshotLifetimeBinding,
    pub native_sequence: i64,
    pub session_sequence: i64,
}

struct Target {
    sequence: i64,
    raw: String,
    record: activity::Record,
    binding: SnapshotLifetimeBinding,
}
fn invalid(message: &str) -> StoreError {
    StoreError::InvalidAppend(message.into())
}
fn preparation_error(error: StoreError) -> RuntimeError {
    match error {
        StoreError::InvalidAppend(message) => RuntimeError::Conflict(message),
        StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows) => {
            RuntimeError::Conflict("No retained unstarted snapshot preparation".into())
        }
        error => RuntimeError::Store(error),
    }
}
pub(super) fn validate_event(data: &serde_json::Value) -> Result<(), String> {
    let receipt: SnapshotPreparationReceipt =
        serde_json::from_value(data.clone()).map_err(|error| error.to_string())?;
    validate_receipt(&receipt).map_err(|error| error.to_string())
}
fn validate_receipt(receipt: &SnapshotPreparationReceipt) -> Result<(), StoreError> {
    if !(0..=1).contains(&receipt.native_sequence)
        || receipt.session_sequence < 0
        || receipt.lifetime.version != 1
        || receipt.lifetime.owner.activity_id != receipt.activity_id
        || receipt.request_id.is_empty()
        || receipt.request_id.len() > 256
        || receipt.request_id.contains('\0')
        || receipt.confirmation.len() != 64
        || !receipt
            .confirmation
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid("Invalid preparation cancellation receipt"));
    }
    Ok(())
}
fn target(db: &Connection, session: &str, activity: &str) -> Result<Target, StoreError> {
    let (sequence, kind, raw): (i64, String, String) = db.query_row(
        "SELECT seq,type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
        [activity],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let first: String = db.query_row(
        "SELECT data FROM event WHERE aggregate_id=?1 AND seq=0 AND type=?2",
        [activity, activity::CHANGED],
        |row| row.get(0),
    )?;
    let record: activity::Record =
        serde_json::from_str(&raw).map_err(|_| invalid("Invalid snapshot preparation"))?;
    let reserved: activity::Record =
        serde_json::from_str(&first).map_err(|_| invalid("Invalid snapshot reservation"))?;
    validate_unstarted(&reserved)?;
    validate_unstarted(&record)?;
    let binding = record
        .snapshot_lifetime
        .clone()
        .ok_or_else(|| invalid("Historical reservation has no exact lifetime proof"))?;
    if kind != activity::CHANGED
        || !(0..=1).contains(&sequence)
        || reserved.status != "pending"
        || record.session_id != session
        || reserved.session_id != session
        || (sequence == 0 && record.status != "pending")
        || (sequence == 1 && record.status != "unknown")
        || reserved.snapshot_lifetime != record.snapshot_lifetime
        || reserved.snapshot_request != record.snapshot_request
        || binding.version != 1
        || binding.owner.activity_id != activity
        || binding.owner.session_id != session
    {
        return Err(invalid(
            "Snapshot reservation differs from its unstarted owner",
        ));
    }
    let created: String = db.query_row(
        "SELECT id FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq LIMIT 1",
        [session, super::events::CREATED],
        |row| row.get(0),
    )?;
    if created != binding.owner.session_created_id {
        return Err(invalid("Snapshot preparation Session creation changed"));
    }
    Ok(Target {
        sequence,
        raw,
        record,
        binding,
    })
}
fn validate_unstarted(record: &activity::Record) -> Result<(), StoreError> {
    if !record.snapshot_worker
        || record.binary_document.is_some()
        || record.phase != "reserved"
        || record.snapshot_origin.is_some()
        || record.snapshot_receipt.is_some()
        || record.snapshot_restore_receipt.is_some()
        || record.snapshot_preparation_receipt.is_some()
    {
        return Err(invalid(
            "Only an exact unstarted snapshot preparation can be cancelled",
        ));
    }
    Ok(())
}
fn request_owner(created: &str, request: &str) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&("snapshot-preparation-request-v1", created, request))
        .expect("strings serialize");
    format!("op_snapshot_preparation_{:x}", Sha256::digest(bytes))
}

struct Admission(Arc<super::Handle>);
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.snapshot_active.store(false, Ordering::SeqCst);
    }
}
struct Completion {
    runtime: Runtime,
    id: String,
    control: Arc<activity::Control>,
}
impl Drop for Completion {
    fn drop(&mut self) {
        self.runtime
            .inner
            .activities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
        self.control.done.cancel();
    }
}
struct Worker {
    runtime: Runtime,
    handle: Arc<super::Handle>,
    authority: RecoveryAuthority,
    sequence: i64,
    target: Target,
    control: Arc<activity::Control>,
    proof: Box<dyn SnapshotLifetime>,
    _lifecycle: tokio::sync::OwnedRwLockReadGuard<()>,
    _admission: Admission,
    _completion: Completion,
}
impl Worker {
    fn fingerprint(&self) -> Result<String, RuntimeError> {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&(
            "snapshot-preparation-review-v1",
            self.sequence,
            &self.authority,
            self.target.sequence,
            &self.target.raw,
        ))
        .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
    fn authorize(&self, db: &Connection) -> Result<(), StoreError> {
        if self.control.stop.is_cancelled() {
            return Err(invalid("Snapshot preparation cancellation was revoked"));
        }
        self.proof.verify().map_err(|error| invalid(&error))?;
        let owner = &self.target.binding.owner;
        let current = target(db, &owner.session_id, &owner.activity_id)?;
        let sequence: i64 = db.query_row(
            "SELECT MAX(seq) FROM event WHERE aggregate_id=?1",
            [&owner.session_id],
            |row| row.get(0),
        )?;
        if current.raw != self.target.raw
            || current.sequence != self.target.sequence
            || sequence != self.sequence
            || !self.authority.current(db)?
        {
            return Err(invalid("Snapshot preparation authority or owner changed"));
        }
        Ok(())
    }
    async fn confirm(
        self,
        confirmation: SnapshotPreparationConfirmation,
    ) -> Result<SnapshotPreparationReceipt, RuntimeError> {
        if self.fingerprint()? != confirmation.confirmation {
            return Err(RuntimeError::Conflict(
                "Snapshot preparation review changed".into(),
            ));
        }
        let cancelled = self.control.stop.clone().drop_guard();
        let joined = tokio::task::spawn_blocking(move || {
            let receipt = SnapshotPreparationReceipt {
                activity_id: self.target.binding.owner.activity_id.clone(),
                request_id: confirmation.request_id,
                confirmation: confirmation.confirmation,
                lifetime: self.target.binding.clone(),
                native_sequence: self.target.sequence,
                session_sequence: self.sequence,
            };
            let mut record = self.target.record.clone();
            record.status = "settled".into();
            record.phase = PHASE.into();
            record.admission_bindings = None;
            record.snapshot_preparation_receipt = Some(receipt.clone());
            let events = [
                vec![NewEvent::new(
                    activity::CHANGED,
                    serde_json::to_value(record).expect("record serializes"),
                )],
                vec![NewEvent::new(
                    CANCELLED,
                    serde_json::to_value(&receipt).expect("receipt serializes"),
                )],
            ];
            let owners = [
                (receipt.activity_id.clone(), receipt.native_sequence),
                (
                    request_owner(
                        &receipt.lifetime.owner.session_created_id,
                        &receipt.request_id,
                    ),
                    -1,
                ),
            ];
            let store = self.runtime.inner.store.clone();
            // Returning the worker keeps the real proof/resources through COMMIT
            // and its observed reply, rather than dropping them after the decision.
            let (_, (worker, receipt)) = store
                .append_pair_checked(owners, move |tx| {
                    self.authorize(tx)?;
                    Ok((events, (self, receipt)))
                })
                .map_err(preparation_error)?;
            worker
                .handle
                .acknowledge_snapshot_uncertainty(&receipt.activity_id);
            Ok::<_, RuntimeError>(receipt)
        });
        let result = joined.await.map_err(|_| {
            RuntimeError::Conflict(
                "Preparation cancellation interrupted; look up its request".into(),
            )
        })?;
        cancelled.disarm();
        result
    }
}

impl Runtime {
    pub async fn snapshot_preparations(
        &self,
        session: &str,
    ) -> Result<Vec<SnapshotPreparation>, RuntimeError> {
        self.session_info(session).await?;
        let session = session.to_owned();
        self.inner.store.read(move |db| {
            let mut query = db.prepare("SELECT aggregate_id,data FROM event e WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1 AND json_extract(data,'$.snapshot_worker')=1 AND json_extract(data,'$.status')<>'settled' AND json_extract(data,'$.snapshot_origin') IS NULL AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq) ORDER BY aggregate_id LIMIT 4097")?;
            let rows = query.query_map([session], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
            let mut result = Vec::new();
            for row in rows {
                let (activity_id, data) = row?;
                let record: activity::Record = serde_json::from_str(&data).map_err(|_| invalid("Invalid snapshot preparation"))?;
                result.push(SnapshotPreparation { activity_id, status: record.status, lifetime_available: record.snapshot_lifetime.is_some() });
            }
            if result.len() > 4096 { return Err(invalid("Snapshot preparation limit exceeded")); }
            Ok(result)
        }).map_err(RuntimeError::from)
    }
    async fn preparation_worker(
        &self,
        session: &str,
        activity: &str,
    ) -> Result<Worker, RuntimeError> {
        let lifecycle = self.memory_mutation_lease().await?;
        let handle = self.inner.handle(session).await?;
        let state = handle.state.lock().await;
        self.inner.ensure_idle(session)?;
        self.ensure_no_background(session)?;
        state.ensure_worktree_ready()?;
        if self
            .inner
            .activities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .any(|control| control.source == session)
        {
            return Err(RuntimeError::Busy(session.into()));
        }
        let source = session.to_owned();
        let owner = activity.to_owned();
        let target = self
            .inner
            .store
            .read(move |db| {
                let tx = db.unchecked_transaction()?;
                let result = target(&tx, &source, &owner)?;
                tx.commit()?;
                Ok(result)
            })
            .map_err(preparation_error)?;
        let directory = std::fs::canonicalize(&state.info.directory)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        if target.binding.owner.directory != directory
            || target.binding.owner.worktree_id != state.info.worktree_id
        {
            return Err(RuntimeError::Conflict(
                "Snapshot preparation Location changed".into(),
            ));
        }
        let authority = RecoveryAuthority::capture(self, session)?;
        let sequence = state.last_seq;
        handle
            .snapshot_active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| RuntimeError::Busy(session.into()))?;
        let admission = Admission(handle.clone());
        let control = Arc::new(activity::Control {
            source: session.into(),
            binary_document: None,
            snapshot_worker: true,
            admission_sessions: super::admission_authority::binding_sessions(authority.bindings()),
            stop: self.inner.closed.child_token(),
            done: tokio_util::sync::CancellationToken::new(),
        });
        let id = cyber_core::ids::new_id("op_preparation");
        self.inner
            .activities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), control.clone());
        let completion = Completion {
            runtime: self.clone(),
            id,
            control: control.clone(),
        };
        drop(state);
        let proof = self
            .inner
            .options
            .snapshots
            .lock_worker(target.binding.clone())
            .await
            .map_err(RuntimeError::Conflict)?;
        if proof.binding() != &target.binding {
            return Err(RuntimeError::Conflict(
                "Foreign preparation lifetime proof".into(),
            ));
        }
        Ok(Worker {
            runtime: self.clone(),
            handle,
            authority,
            sequence,
            target,
            control,
            proof,
            _lifecycle: lifecycle,
            _admission: admission,
            _completion: completion,
        })
    }
    pub async fn snapshot_preparation_review(
        &self,
        session: &str,
        activity: &str,
    ) -> Result<SnapshotPreparationReview, RuntimeError> {
        let worker = self.preparation_worker(session, activity).await?;
        let store = self.inner.store.clone();
        tokio::task::spawn_blocking(move || {
            let worker = store.read(move |db| { let tx = db.unchecked_transaction()?; worker.authorize(&tx)?; tx.commit()?; Ok(worker) }).map_err(preparation_error)?;
            Ok(SnapshotPreparationReview { activity_id: worker.target.binding.owner.activity_id.clone(), lifetime: worker.target.binding.clone(),
                native_sequence: worker.target.sequence, session_sequence: worker.sequence, confirmation: worker.fingerprint()?,
                warning: "Cancels only this unstarted snapshot preparation. Files and transcript are preserved; checkout and process uncertainty remain independent.".into() })
        }).await.map_err(|_| RuntimeError::Conflict("Snapshot preparation review interrupted".into()))?
    }
    pub async fn snapshot_preparation_confirm(
        &self,
        session: &str,
        activity: &str,
        confirmation: &SnapshotPreparationConfirmation,
    ) -> Result<SnapshotPreparationReceipt, RuntimeError> {
        super::snapshot_recovery::validate_request_id(&confirmation.request_id)?;
        if confirmation.confirmation.len() != 64
            || !confirmation
                .confirmation
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(RuntimeError::Invalid(
                "Invalid preparation fingerprint".into(),
            ));
        }
        if let Some(receipt) = self
            .snapshot_preparation_lookup(session, activity, &confirmation.request_id)
            .await?
        {
            if receipt.confirmation != confirmation.confirmation {
                return Err(RuntimeError::Conflict(
                    "Preparation request content changed".into(),
                ));
            }
            return Ok(receipt);
        }
        self.preparation_worker(session, activity)
            .await?
            .confirm(confirmation.clone())
            .await
    }
    pub async fn snapshot_preparation_lookup(
        &self,
        session: &str,
        activity: &str,
        request: &str,
    ) -> Result<Option<SnapshotPreparationReceipt>, RuntimeError> {
        super::snapshot_recovery::validate_request_id(request)?;
        self.session_info(session).await?;
        let source = session.to_owned();
        let activity = activity.to_owned();
        let request = request.to_owned();
        let receipt = self.inner.store.read(move |db| {
            let tx = db.unchecked_transaction()?;
            let created: String = tx.query_row(
                "SELECT id FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq LIMIT 1",
                [&source, super::events::CREATED],
                |row| row.get(0),
            )?;
            let data: Option<(i64, String, String)> = tx
                .query_row(
                    "SELECT seq,type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
                    [request_owner(&created, &request)],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((sequence, kind, data)) = data else {
                return Ok(None);
            };
            let receipt: SnapshotPreparationReceipt =
                serde_json::from_str(&data).map_err(|_| invalid("Invalid preparation receipt"))?;
            validate_receipt(&receipt)?;
            if sequence != 0 || kind != CANCELLED
                || receipt.request_id != request
                || receipt.activity_id != activity
                || receipt.lifetime.owner.session_id != source
                || receipt.lifetime.owner.session_created_id != created
            {
                return Err(invalid("Preparation request belongs to another owner"));
            }
            let (seq, kind, data): (i64, String, String) = tx.query_row(
                "SELECT seq,type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
                [&activity],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let native: activity::Record =
                serde_json::from_str(&data).map_err(|_| invalid("Invalid preparation owner"))?;
            if seq != receipt.native_sequence + 1
                || kind != activity::CHANGED
                || !native.snapshot_worker
                || native.binary_document.is_some()
                || native.session_id != source
                || receipt.lifetime.owner.activity_id != activity
                || native.status != "settled"
                || native.phase != PHASE
                || native.snapshot_preparation_receipt.as_ref() != Some(&receipt)
                || native.snapshot_origin.is_some()
                || native.snapshot_lifetime.as_ref() != Some(&receipt.lifetime)
            {
                return Err(invalid("Preparation receipt lacks its exact native pair"));
            }
            tx.commit()?;
            Ok(Some(receipt))
        }).map_err(preparation_error)?;
        if let Some(receipt) = &receipt {
            self.inner
                .handle(session)
                .await?
                .acknowledge_snapshot_uncertainty(&receipt.activity_id);
        }
        Ok(receipt)
    }
}
