//! Consuming proof for an exited snapshot worker with no admitted source effects.
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
    pub origin: Option<super::SnapshotOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_resources_digest: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<super::SnapshotOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_admitted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_resources_digest: Option<String>,
}

impl SnapshotPreparationReceipt {
    pub const MAX_ENCODED_BYTES: u64 = 64 * 1024;
    /// Issued by trusted adapters only after observing this exact committed pair.
    pub fn checkout_acknowledgement_digest(&self) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        validate_receipt(self).map_err(|error| error.to_string())?;
        if self.checkout_resources_digest.is_none() || self.source_admitted != Some(false) {
            return Err(
                "Preparation receipt has no positive checkout acknowledgement authority".into(),
            );
        }
        self.lifetime.checkout_owner()?;
        let bytes = serde_json::to_vec(&("snapshot-preparation-checkout-ack-v1", self))
            .map_err(|error| error.to_string())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
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
    if !(0..=2).contains(&receipt.native_sequence)
        || receipt
            .checkout_resources_digest
            .as_ref()
            .is_some_and(|digest| {
                receipt.lifetime.version != 2
                    || receipt.lifetime.source_phase.is_none()
                    || receipt.source_admitted != Some(false)
                    || digest.len() != 64
                    || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        || receipt.session_sequence < 0
        || !matches!(receipt.lifetime.version, 1 | 2)
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
    match &receipt.origin {
        None if receipt.native_sequence <= 1 && receipt.source_admitted != Some(true) => {}
        Some(origin)
            if receipt.source_admitted == Some(false)
                && receipt.lifetime.version == 2
                && receipt.lifetime.source_phase.is_some()
                && receipt.native_sequence >= 1
                && origin.version == 1
                && origin.activity_id == receipt.activity_id
                && origin.session_id == receipt.lifetime.owner.session_id
                && origin.session_created_id == receipt.lifetime.owner.session_created_id
                && origin.worktree_id == receipt.lifetime.owner.worktree_id
                && origin.binding.directory == receipt.lifetime.owner.directory
                && origin.binding.root_identity == receipt.lifetime.root_identity => {}
        _ => {
            return Err(invalid(
                "Cancellation receipt has no positive pre-effect proof",
            ));
        }
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
    validate_pre_effect(&record)?;
    let binding = record
        .snapshot_lifetime
        .clone()
        .ok_or_else(|| invalid("Historical reservation has no exact lifetime proof"))?;
    if kind != activity::CHANGED
        || !(0..=2).contains(&sequence)
        || reserved.status != "pending"
        || record.session_id != session
        || reserved.session_id != session
        || reserved.snapshot_lifetime != record.snapshot_lifetime
        || reserved.snapshot_request != record.snapshot_request
        || binding.version != 2
        || binding.owner.activity_id != activity
        || binding.owner.session_id != session
    {
        return Err(invalid(
            "Snapshot reservation differs from its unstarted owner",
        ));
    }
    match &record.snapshot_origin {
        None if (sequence == 0 && record.status == "pending"
            || sequence == 1 && record.status == "unknown")
            && record.phase == "reserved" => {}
        Some(origin)
            if reserved.snapshot_source_admitted == Some(false)
                && record.snapshot_source_admitted == Some(false)
                && (sequence == 1 && record.status == "pending"
                    || sequence == 2 && record.status == "unknown")
                && record.phase == "launching" =>
        {
            let data: String = db.query_row(
                "SELECT data FROM event WHERE aggregate_id=?1 AND seq=1 AND type=?2",
                [activity, activity::CHANGED],
                |row| row.get(0),
            )?;
            let launched: activity::Record =
                serde_json::from_str(&data).map_err(|_| invalid("Invalid preparation origin"))?;
            validate_pre_effect(&launched)?;
            if origin.version != 1
                || launched.snapshot_origin.as_ref() != Some(origin)
                || launched.status != "pending"
                || launched.phase != "launching"
                || launched.session_id != session
                || launched.snapshot_lifetime != record.snapshot_lifetime
                || launched.snapshot_request != record.snapshot_request
                || launched.snapshot_source_admitted != Some(false)
                || origin.activity_id != activity
                || origin.session_id != session
                || origin.session_created_id != binding.owner.session_created_id
                || origin.worktree_id != binding.owner.worktree_id
                || origin.binding.directory != binding.owner.directory
                || origin.binding.root_identity != binding.root_identity
            {
                return Err(invalid("Preparation origin differs from its reservation"));
            }
        }
        _ => return Err(invalid("Source effects may already have been admitted")),
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
    validate_pre_effect(record)?;
    if record.snapshot_origin.is_some() || record.phase != "reserved" {
        return Err(invalid("Reservation is not unstarted"));
    }
    Ok(())
}
fn validate_pre_effect(record: &activity::Record) -> Result<(), StoreError> {
    if !record.snapshot_worker
        || record.binary_document.is_some()
        || record.snapshot_source_admitted == Some(true)
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
fn acknowledged_target(
    db: &Connection,
    receipt: &SnapshotPreparationReceipt,
) -> Result<Target, StoreError> {
    validate_receipt(receipt)?;
    let owner = &receipt.lifetime.owner;
    let created: String = db.query_row(
        "SELECT id FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq LIMIT 1",
        [&owner.session_id, super::events::CREATED],
        |row| row.get(0),
    )?;
    let (request_seq, request_kind, data): (i64, String, String) = db.query_row(
        "SELECT seq,type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
        [request_owner(&created, &receipt.request_id)],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let request: SnapshotPreparationReceipt =
        serde_json::from_str(&data).map_err(|_| invalid("Invalid preparation request receipt"))?;
    let (sequence, kind, raw): (i64, String, String) = db.query_row(
        "SELECT seq,type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
        [&receipt.activity_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let record: activity::Record =
        serde_json::from_str(&raw).map_err(|_| invalid("Invalid preparation owner"))?;
    if created != owner.session_created_id
        || request_seq != 0
        || request_kind != CANCELLED
        || request != *receipt
        || sequence != receipt.native_sequence + 1
        || kind != activity::CHANGED
        || !record.snapshot_worker
        || record.binary_document.is_some()
        || record.session_id != owner.session_id
        || record.status != "settled"
        || record.phase != PHASE
        || record.snapshot_preparation_receipt.as_ref() != Some(receipt)
        || record.snapshot_origin != receipt.origin
        || record.snapshot_source_admitted != receipt.source_admitted
        || record.snapshot_lifetime.as_ref() != Some(&receipt.lifetime)
        || record.snapshot_receipt.is_some()
        || record.snapshot_restore_receipt.is_some()
    {
        return Err(invalid(
            "Preparation receipt lacks its exact durable native pair",
        ));
    }
    Ok(Target {
        sequence,
        raw,
        record,
        binding: receipt.lifetime.clone(),
    })
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
    resources: Option<Vec<cyber_core::worktrees::CheckoutBinding>>,
    checkouts: Option<Box<dyn super::SnapshotLocationGuard>>,
    proof: Box<dyn SnapshotLifetime>,
    _lifecycle: tokio::sync::OwnedRwLockReadGuard<()>,
    _admission: Admission,
    _completion: Completion,
}
impl Worker {
    fn resources_digest(&self) -> Result<Option<String>, RuntimeError> {
        self.resources
            .as_ref()
            .map(|resources| {
                self.target
                    .binding
                    .checkout_resources_digest(resources)
                    .map_err(RuntimeError::Conflict)
            })
            .transpose()
    }
    fn verify_resources(&self) -> Result<(), String> {
        if self.proof.checkout_bindings()? != self.resources {
            return Err("Snapshot preparation resource publication changed".into());
        }
        match (&self.resources, &self.checkouts) {
            (Some(resources), Some(checkouts)) => {
                checkouts.verify()?;
                if checkouts.bindings()? != *resources {
                    return Err("Retained snapshot preparation resources changed".into());
                }
            }
            (None, None) => {}
            _ => return Err("Snapshot preparation resource proof is incomplete".into()),
        }
        if let Some(expected) = self
            .target
            .record
            .snapshot_preparation_receipt
            .as_ref()
            .and_then(|receipt| receipt.checkout_resources_digest.as_ref())
        {
            let observed = self.resources_digest().map_err(|error| error.to_string())?;
            if observed.as_ref() != Some(expected) {
                return Err("Committed preparation resource binding differs".into());
            }
        }
        Ok(())
    }
    fn acknowledge_native(&mut self, receipt: &SnapshotPreparationReceipt) -> Result<(), String> {
        self.proof.acknowledge_preparation(receipt)?;
        if receipt.checkout_resources_digest.is_some() {
            self.verify_resources()?;
            self.checkouts
                .as_mut()
                .ok_or("Missing original checkout acknowledgement proof")?
                .acknowledge_preparation(receipt)?;
        }
        Ok(())
    }
    fn fingerprint(&self) -> Result<String, RuntimeError> {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&(
            "snapshot-preparation-review-v2",
            self.sequence,
            &self.authority,
            self.target.sequence,
            &self.target.raw,
            self.resources_digest()?,
        ))
        .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
    fn authorize(&self, db: &Connection) -> Result<(), StoreError> {
        if self.control.stop.is_cancelled() {
            return Err(invalid("Snapshot preparation cancellation was revoked"));
        }
        self.proof
            .verify_preparation(self.target.record.snapshot_origin.as_ref())
            .map_err(|error| invalid(&error))?;
        self.verify_resources().map_err(|error| invalid(&error))?;
        let owner = &self.target.binding.owner;
        let current = match self.target.record.snapshot_preparation_receipt.as_ref() {
            Some(receipt) => acknowledged_target(db, receipt)?,
            None => target(db, &owner.session_id, &owner.activity_id)?,
        };
        let sequence: i64 = db.query_row(
            "SELECT MAX(seq) FROM event WHERE aggregate_id=?1",
            [&owner.session_id],
            |row| row.get(0),
        )?;
        if current.raw != self.target.raw
            || current.sequence != self.target.sequence
            || sequence != self.sequence
            || !self.authority.current(db)?
            || self.control.stop.is_cancelled()
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
                origin: self.target.record.snapshot_origin.clone(),
                source_admitted: self.target.record.snapshot_source_admitted,
                checkout_resources_digest: self.resources_digest()?,
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
                    let mut worker = self;
                    if receipt.checkout_resources_digest.is_some() {
                        worker
                            .proof
                            .prepare_preparation_acknowledgement(&receipt)
                            .map_err(|error| invalid(&error))?;
                        worker
                            .checkouts
                            .as_mut()
                            .ok_or_else(|| invalid("Missing checkout receipt proof"))?
                            .prepare_preparation_acknowledgement(&receipt)
                            .map_err(|error| invalid(&error))?;
                    }
                    worker.authorize(tx)?;
                    Ok((events, (worker, receipt)))
                })
                .map_err(preparation_error)?;
            let mut worker = worker;
            worker
                .handle
                .acknowledge_snapshot_uncertainty(&receipt.activity_id);
            worker.acknowledge_native(&receipt).map_err(|error| RuntimeError::Conflict(format!("Preparation cancellation committed; native journal or checkout acknowledgement requires reconciliation: {error}")))?;
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
        self.snapshot_preparations_with_history(session, false)
            .await
    }
    pub async fn snapshot_preparations_with_history(
        &self,
        session: &str,
        include_cancelled: bool,
    ) -> Result<Vec<SnapshotPreparation>, RuntimeError> {
        self.session_info(session).await?;
        let created = self
            .inner
            .store
            .read_events(session, -1, 1)?
            .events
            .first()
            .ok_or_else(|| {
                RuntimeError::Conflict("Preparation Session creation is missing".into())
            })?
            .id
            .clone();
        let session = session.to_owned();
        self.inner.store.read(move |db| {
            let mut query = db.prepare("SELECT aggregate_id,data FROM event e WHERE type='native.activity.changed.1' AND json_extract(data,'$.session_id')=?1 AND json_extract(data,'$.snapshot_worker')=1 AND ((json_extract(data,'$.status')<>'settled' AND COALESCE(json_extract(data,'$.snapshot_source_admitted'),0)=0) OR (?2 AND json_extract(data,'$.status')='settled' AND json_extract(data,'$.phase')='snapshot_preparation_cancelled' AND json_extract(data,'$.snapshot_preparation_receipt.lifetime.owner.session_created_id')=?3)) AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq) ORDER BY aggregate_id LIMIT 4097")?;
            let rows = query.query_map(rusqlite::params![session, include_cancelled, created], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
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
        acknowledged: Option<SnapshotPreparationReceipt>,
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
                let result = match acknowledged.as_ref() {
                    Some(receipt) => acknowledged_target(&tx, receipt)?,
                    None => target(&tx, &source, &owner)?,
                };
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
        let info = state.info.clone();
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
        let resources = proof.checkout_bindings().map_err(RuntimeError::Conflict)?;
        let checkouts = resources
            .as_ref()
            .map(|resources| {
                target
                    .binding
                    .checkout_resources_digest(resources)
                    .map_err(RuntimeError::Conflict)?;
                self.inner
                    .tools
                    .reacquire_snapshot_location(&info, &target.binding, resources)
                    .map_err(RuntimeError::Conflict)
            })
            .transpose()?;
        let worker = Worker {
            runtime: self.clone(),
            handle,
            authority,
            sequence,
            target,
            control,
            resources,
            checkouts,
            proof,
            _lifecycle: lifecycle,
            _admission: admission,
            _completion: completion,
        };
        worker.verify_resources().map_err(RuntimeError::Conflict)?;
        Ok(worker)
    }
    pub async fn snapshot_preparation_review(
        &self,
        session: &str,
        activity: &str,
    ) -> Result<SnapshotPreparationReview, RuntimeError> {
        let worker = self.preparation_worker(session, activity, None).await?;
        let store = self.inner.store.clone();
        tokio::task::spawn_blocking(move || {
            let worker = store.read(move |db| { let tx = db.unchecked_transaction()?; worker.authorize(&tx)?; tx.commit()?; Ok(worker) }).map_err(preparation_error)?;
            Ok(SnapshotPreparationReview { activity_id: worker.target.binding.owner.activity_id.clone(), lifetime: worker.target.binding.clone(),
                native_sequence: worker.target.sequence, session_sequence: worker.sequence, confirmation: worker.fingerprint()?,
                checkout_resources_digest: worker.resources_digest()?,
                origin: worker.target.record.snapshot_origin.clone(),
                warning: "Cancels only this pre-effect snapshot preparation. Files and transcript are preserved; checkout and process uncertainty remain independent.".into() })
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
        self.preparation_worker(session, activity, None)
            .await?
            .confirm(confirmation.clone())
            .await
    }
    pub async fn snapshot_preparation_acknowledge(
        &self,
        session: &str,
        activity: &str,
        request: &str,
    ) -> Result<SnapshotPreparationReceipt, RuntimeError> {
        let receipt = self
            .snapshot_preparation_lookup(session, activity, request)
            .await?
            .ok_or_else(|| {
                RuntimeError::Conflict("No committed preparation cancellation receipt".into())
            })?;
        if receipt.origin.is_none() && receipt.checkout_resources_digest.is_none() {
            return Ok(receipt);
        }
        let worker = self
            .preparation_worker(session, activity, Some(receipt.clone()))
            .await?;
        let cancelled = worker.control.stop.clone().drop_guard();
        let store = self.inner.store.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut worker = store
                .read(move |db| {
                    let tx = db.unchecked_transaction()?;
                    worker.authorize(&tx)?;
                    tx.commit()?;
                    Ok(worker)
                })
                .map_err(preparation_error)?;
            worker
                .acknowledge_native(&receipt)
                .map_err(RuntimeError::Conflict)?;
            Ok(receipt)
        })
        .await
        .map_err(|_| {
            RuntimeError::Conflict(
                "Preparation journal acknowledgement interrupted; reconcile its retained receipt"
                    .into(),
            )
        })?;
        cancelled.disarm();
        result
    }
    /// Discover an exact committed cancellation; never infer safety from absence.
    pub async fn snapshot_preparation_receipt(
        &self,
        session: &str,
        activity: &str,
    ) -> Result<Option<SnapshotPreparationReceipt>, RuntimeError> {
        self.session_info(session).await?;
        let source = session.to_owned();
        let owner = activity.to_owned();
        let request = self
            .inner
            .store
            .read(move |db| {
                let record: Option<(String, String)> = db.query_row(
                "SELECT type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
                [owner], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
                let Some((kind, data)) = record else {
                    return Ok(None);
                };
                let record: activity::Record = serde_json::from_str(&data)
                    .map_err(|_| invalid("Invalid preparation record"))?;
                if kind != activity::CHANGED
                    || !record.snapshot_worker
                    || record.binary_document.is_some()
                    || record.session_id != source
                {
                    return Err(invalid("Preparation discovery belongs to another owner"));
                }
                Ok(record
                    .snapshot_preparation_receipt
                    .map(|receipt| receipt.request_id))
            })
            .map_err(preparation_error)?;
        match request {
            Some(request) => {
                self.snapshot_preparation_lookup(session, activity, &request)
                    .await
            }
            None => Ok(None),
        }
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
            acknowledged_target(&tx, &receipt)?;
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
