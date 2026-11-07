//! Immutable ordinary-restore settlement evidence, independent of later owner files.
use super::{Runtime, RuntimeError, SnapshotOrigin, SnapshotRestoreReceipt, snapshot_work};
use cyber_store::{NewEvent, StoreError};
use rusqlite::Connection;

const MAX_EVENTS: i64 = 4096;

impl SnapshotRestoreReceipt {
    /// Bind native disposition metadata to an already observed durable receipt.
    /// This digest alone grants no claim acquisition, acknowledgement or replay authority.
    pub fn checkout_acknowledgement_digest(&self) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        if self.session_before < 0
            || self.session_after < self.session_before
            || self.session_after - self.session_before > MAX_EVENTS
            || [&self.intent_digest, &self.events_digest]
                .iter()
                .any(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err("Invalid restore receipt acknowledgement frame".into());
        }
        let checkout = self
            .checkout
            .as_ref()
            .ok_or("Legacy restore receipt lacks checkout proof")?;
        checkout.validate(&self.origin)?;
        let bytes = serde_json::to_vec(&("snapshot-restore-checkout-ack-v1", self))
            .map_err(|error| error.to_string())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

impl super::SnapshotRestoreCheckout {
    pub(super) fn validate(&self, origin: &SnapshotOrigin) -> Result<(), String> {
        // The lifetime's Location namespace and the origin's journal collection
        // are different directories. Native guards verify each route separately.
        let owner = &self.lifetime.owner;
        if self.version != 1
            || self.lifetime.version != 2
            || self.lifetime.source_phase.is_none()
            || origin.version != 1
            || owner.activity_id != origin.activity_id
            || owner.session_id != origin.session_id
            || owner.session_created_id != origin.session_created_id
            || owner.worktree_id != origin.worktree_id
            || owner.directory != origin.binding.directory
            || self.lifetime.root_identity != origin.binding.root_identity
            || self.resources_digest.len() != 64
            || !self
                .resources_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("Restore receipt checkout ownership differs".into());
        }
        self.lifetime.checkout_owner().map(drop)
    }
}

impl Runtime {
    /// Lookup only; legacy settlements without an immutable receipt return None.
    pub async fn snapshot_restore_lookup(
        &self,
        session: &str,
        activity: &str,
    ) -> Result<Option<SnapshotRestoreReceipt>, RuntimeError> {
        use rusqlite::OptionalExtension;
        let source = session.to_owned();
        let owner = activity.to_owned();
        self.inner.store.read(move |db| {
            let tx = db.unchecked_transaction()?;
            let record = tx.query_row(
                "SELECT data FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq DESC LIMIT 1",
                [&owner, super::activity::CHANGED], |row| row.get::<_, String>(0),
            ).optional()?;
            let receipt = record.map(|data| serde_json::from_str::<super::activity::Record>(&data)
                .map_err(|error| StoreError::InvalidAppend(error.to_string())))
                .transpose()?.filter(|record| record.session_id == source)
                .and_then(|record| record.snapshot_restore_receipt);
            if let Some(receipt) = &receipt {
                if receipt.origin.session_id != source || receipt.origin.activity_id != owner {
                    return Err(StoreError::InvalidAppend("Foreign restore receipt owner".into()));
                }
                verify(&tx, receipt)?;
            }
            tx.commit()?;
            Ok(receipt)
        }).map_err(RuntimeError::from)
    }

    /// Acknowledge an exact already committed ordinary restore; never replay it.
    pub async fn snapshot_restore_acknowledge(
        &self,
        session: &str,
        activity: &str,
    ) -> Result<SnapshotRestoreReceipt, RuntimeError> {
        let receipt = self
            .snapshot_restore_lookup(session, activity)
            .await?
            .ok_or_else(|| {
                RuntimeError::Conflict("No immutable committed restore receipt".into())
            })?;
        let handle = self.inner.handle(session).await?;
        let state = handle.state.lock().await;
        let directory = state.info.directory.clone();
        let worker = self
            .snapshot_acknowledgement_worker_for(
                &handle,
                &state,
                super::snapshot_acknowledgement::Acknowledgement::Restore(receipt.clone()),
                receipt.origin.clone(),
            )
            .await?;
        drop(state);
        self.inner
            .options
            .snapshots
            .acknowledge_recovery(directory, worker)
            .await
            .map_err(RuntimeError::Conflict)?;
        handle.acknowledge_snapshot_uncertainty(&receipt.origin.activity_id);
        Ok(receipt)
    }
}

pub(super) fn capture(
    origin: Option<&SnapshotOrigin>,
    digest: &str,
    sequence: i64,
    events: &[NewEvent],
    lifetime: Option<&super::SnapshotLifetimeBinding>,
    resources: &[cyber_core::worktrees::CheckoutBinding],
) -> Result<SnapshotRestoreReceipt, RuntimeError> {
    let origin = origin.ok_or_else(|| {
        RuntimeError::Conflict("Immutable snapshot receipt lacks its native origin".into())
    })?;
    let count = i64::try_from(events.len())
        .map_err(|_| RuntimeError::Invalid("Snapshot event batch is too large".into()))?;
    if digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        || sequence < 0
        || count > MAX_EVENTS
    {
        return Err(RuntimeError::Conflict(
            "Invalid immutable snapshot receipt".into(),
        ));
    }
    let checkout = lifetime
        .map(|lifetime| {
            let checkout = super::SnapshotRestoreCheckout {
                version: 1,
                lifetime: lifetime.clone(),
                resources_digest: lifetime.checkout_resources_digest(resources)?,
            };
            checkout.validate(origin)?;
            Ok::<_, String>(Box::new(checkout))
        })
        .transpose()
        .map_err(RuntimeError::Conflict)?;
    Ok(SnapshotRestoreReceipt {
        origin: origin.clone(),
        intent_digest: digest.to_owned(),
        session_before: sequence,
        session_after: sequence
            .checked_add(count)
            .ok_or_else(|| RuntimeError::Invalid("Snapshot Session sequence overflow".into()))?,
        events_digest: snapshot_work::restore_events_digest(events)
            .map_err(RuntimeError::Invalid)?,
        checkout,
    })
}

/// Caller must hold one database read transaction across the complete proof.
pub(super) fn verify(db: &Connection, receipt: &SnapshotRestoreReceipt) -> Result<(), StoreError> {
    let (_, native) = super::snapshot_recovery_record::attest(db, &receipt.origin)?;
    let invalid = || StoreError::InvalidAppend("Ordinary restore settlement proof differs".into());
    if let Some(checkout) = &receipt.checkout {
        checkout.validate(&receipt.origin).map_err(|_| invalid())?;
        if native.snapshot_lifetime.as_ref() != Some(&checkout.lifetime) {
            return Err(invalid());
        }
    }
    if native.status != "settled"
        || native.phase != "snapshot_restore_committed"
        || native.snapshot_restore_receipt.as_ref() != Some(receipt)
        || native.snapshot_request.is_some()
        || native.snapshot_receipt.is_some()
        || receipt.session_before < 0
        || receipt.session_after < receipt.session_before
        || receipt.session_after - receipt.session_before > MAX_EVENTS
    {
        return Err(invalid());
    }
    let created: String = db.query_row(
        "SELECT id FROM event WHERE aggregate_id=?1 AND type=?2 ORDER BY seq LIMIT 1",
        [&receipt.origin.session_id, super::events::CREATED],
        |row| row.get(0),
    )?;
    if created != receipt.origin.session_created_id {
        return Err(invalid());
    }
    let anchor: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM event WHERE aggregate_id=?1 AND seq=?2)",
        rusqlite::params![receipt.origin.session_id, receipt.session_before],
        |row| row.get(0),
    )?;
    if !anchor {
        return Err(invalid());
    }
    let mut query = db.prepare(
        "SELECT seq,type,data,causation_id FROM event WHERE aggregate_id=?1 AND seq>?2 AND seq<=?3 ORDER BY seq",
    )?;
    let rows = query.query_map(
        rusqlite::params![
            receipt.origin.session_id,
            receipt.session_before,
            receipt.session_after
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    )?;
    let mut events = Vec::new();
    for row in rows {
        let (seq, kind, data, causation_id) = row?;
        if seq != receipt.session_before + events.len() as i64 + 1 {
            return Err(invalid());
        }
        events.push(NewEvent {
            kind,
            data: serde_json::from_str(&data).map_err(|_| invalid())?,
            causation_id,
        });
    }
    if receipt.session_after - receipt.session_before != events.len() as i64
        || snapshot_work::restore_events_digest(&events).map_err(|_| invalid())?
            != receipt.events_digest
    {
        return Err(invalid());
    }
    Ok(())
}
