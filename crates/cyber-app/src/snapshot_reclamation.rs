//! Compose native cleanup with the same serialized writer that owns references.
mod metadata;
use cyber_server::runtime::{
    SnapshotOrigin,
    snapshot_reclamation::{self as records, Completion, Plan, Record},
};
use cyber_snapshot::reclamation::{Collect, Delete, Policy, Settlement};
use cyber_store::{Expected, NewEvent, Store, StoreError};
use std::sync::Arc;

pub(crate) struct Reclamation(pub Arc<Store>);
/// Bind native maintenance to this owned Store. Embedders must use the same
/// Store as their runtime; this grants no HTTP or source-file capability.
pub fn policy(store: Arc<Store>) -> Arc<dyn Policy> {
    Arc::new(Reclamation(store))
}
fn invalid(error: impl std::fmt::Display) -> StoreError {
    StoreError::InvalidAppend(error.to_string())
}

impl Policy for Reclamation {
    fn receipt(&self, origin: &SnapshotOrigin) -> Result<Option<Settlement>, String> {
        let origin = origin.clone();
        self.0
            .read(move |db| {
                let tx = db.unchecked_transaction()?;
                let Some(receipt) = records::settlement_receipt(&tx, &origin)? else {
                    return Ok(None);
                };
                let settled_at_ms = tx.query_row(
                    "SELECT time FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
                    [&origin.activity_id],
                    |row| row.get(0),
                )?;
                tx.commit()?;
                Ok(Some(Settlement {
                    receipt,
                    settled_at_ms,
                }))
            })
            .map_err(|error| error.to_string())
    }
    fn record(&self, receipt: &records::Receipt) -> Result<Option<Record>, String> {
        let receipt = receipt.clone();
        self.0
            .read(move |db| {
                let tx = db.unchecked_transaction()?;
                let record = records::lookup(&tx, &receipt)?;
                tx.commit()?;
                Ok(record)
            })
            .map_err(|error| error.to_string())
    }
    fn reclaim(&self, plan: Plan, delete: Delete) -> Result<u64, String> {
        plan.validate().map_err(|error| error.to_string())?;
        let id = Plan::id(&plan.receipt).map_err(|error| error.to_string())?;
        let existing = self.record(&plan.receipt)?;
        if let Some(existing) = existing {
            if existing.plan != plan {
                return Err("Reclamation plan differs from retained request".into());
            }
            if existing.completed {
                return Ok(0);
            }
        } else {
            let prepare = plan.clone();
            self.0
                .append_checked(&id, Expected::Seq(-1), move |db| {
                    prepare.authorize(db)?;
                    let event = NewEvent::new(
                        records::PLANNED,
                        serde_json::to_value(&prepare).map_err(invalid)?,
                    );
                    Ok((vec![event], ()))
                })
                .map_err(|error| error.to_string())?;
        }
        let consume = plan.clone();
        let removed = self
            .0
            .transaction(move |db| {
                let record = records::lookup(db, &consume.receipt)?
                    .ok_or_else(|| invalid("Missing committed reclamation plan"))?;
                if record.completed || record.plan != consume {
                    return Err(invalid("Reclamation request changed before deletion"));
                }
                consume.authorize(db)?;
                // No network, child processes, callbacks into Store or source-file
                // work. Native locks/pins remain owned by this bounded closure.
                delete().map_err(invalid)
            })
            .map_err(|error| error.to_string())?;
        let completion = Completion {
            plan_id: id.clone(),
            removed_bytes: removed,
        };
        self.0
            .append_checked(&id, Expected::Seq(0), move |db| {
                plan.authorize(db)?;
                if completion.removed_bytes != plan.total_bytes()? {
                    return Err(invalid("Reclamation byte accounting changed"));
                }
                Ok((
                    vec![NewEvent::new(
                        records::COMPLETED,
                        serde_json::to_value(completion).map_err(invalid)?,
                    )],
                    (),
                ))
            })
            .map_err(|error| error.to_string())?;
        Ok(removed)
    }
    fn retire(
        &self,
        payload: Plan,
        plan: records::metadata::Plan,
        delete: Delete,
    ) -> Result<u64, String> {
        metadata::retire(self, payload, plan, delete)
    }
    fn collect(&self, collect: Collect) -> Result<(), String> {
        self.0
            .transaction(move |db| collect(&records::references(db)?).map_err(invalid))
            .map_err(|error| error.to_string())
    }
    fn retirement_batch(
        &self,
        directory: Option<&str>,
        after: Option<&str>,
    ) -> Result<cyber_snapshot::reclamation::RetirementBatch, String> {
        metadata::batch(self, directory, after)
    }
    fn refused(&self, directory: &str, error: &str) {
        cyber_core::log::warn(
            "snapshot",
            "Snapshot reclamation refused; retained evidence is protected",
            serde_json::json!({"directory":directory,"reason":error}),
        );
    }
}
