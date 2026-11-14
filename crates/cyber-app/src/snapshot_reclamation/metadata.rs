//! Serialize native metadata retirement against the receipt/reference writer.
use super::*;
use records::metadata::{self, Plan as MetadataPlan};

pub(super) fn batch(
    owner: &Reclamation,
    directory: Option<&str>,
    after: Option<&str>,
) -> Result<cyber_snapshot::reclamation::RetirementBatch, String> {
    let directory = directory.map(str::to_owned);
    let after = after.map(str::to_owned);
    owner.0.read(move |db| {
        let tx = db.unchecked_transaction()?;
        let mut query = tx.prepare("SELECT e.aggregate_id,json_extract(e.data,'$.receipt') FROM event e WHERE e.type=?1 AND e.seq=0 AND e.aggregate_id>coalesce(?2,'') AND EXISTS(SELECT 1 FROM event c WHERE c.aggregate_id=e.aggregate_id AND c.seq=1 AND c.type=?3) AND NOT EXISTS(SELECT 1 FROM event c WHERE c.aggregate_id=e.aggregate_id AND c.seq=3 AND c.type=?4) AND (?5 IS NULL OR coalesce(json_extract(e.data,'$.receipt.origin.binding.directory'),json_extract(e.data,'$.receipt.checkout.resolution_origin.binding.directory'))=?5) ORDER BY e.aggregate_id LIMIT 32")?;
        let mut rows = query.query(rusqlite::params![records::PLANNED, after, records::COMPLETED, metadata::COMPLETED, directory])?;
        let mut receipts = Vec::new();
        let mut last = None;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let raw = row.get_ref(1)?.as_str().map_err(|_| invalid("Invalid retirement receipt encoding"))?;
            if raw.len() > 64 * 1024 { return Err(invalid("Metadata retirement receipt exceeds discovery bound")); }
            let receipt: records::Receipt = serde_json::from_str(raw).map_err(invalid)?;
            if Plan::id(&receipt)? != id { return Err(invalid("Foreign retirement discovery owner")); }
            receipt.owners()?;
            last = Some(id);
            receipts.push(receipt);
        }
        let after = if receipts.len() == 32 { last } else { None };
        drop(rows); drop(query); tx.commit()?;
        Ok(cyber_snapshot::reclamation::RetirementBatch { receipts, after })
    }).map_err(|error| error.to_string())
}

pub(super) fn retire(
    owner: &Reclamation,
    payload: Plan,
    plan: MetadataPlan,
    delete: Delete,
) -> Result<u64, String> {
    plan.validate(&payload).map_err(|error| error.to_string())?;
    let existing = owner
        .record(&payload.receipt)?
        .ok_or("Missing completed payload reclamation")?;
    if existing.plan != payload || !existing.completed {
        return Err("Metadata retirement payload owner is incomplete or changed".into());
    }
    if let Some(retained) = existing.metadata {
        if retained != plan {
            return Err("Metadata retirement differs from retained request".into());
        }
        if existing.retired {
            return Ok(0);
        }
    } else {
        let (prepare, original) = (plan.clone(), payload.clone());
        owner
            .0
            .append_checked(&plan.payload_plan_id, Expected::Seq(1), move |db| {
                prepare.authorize(db, &original)?;
                Ok((
                    vec![NewEvent::new(
                        metadata::PLANNED,
                        serde_json::to_value(prepare).map_err(invalid)?,
                    )],
                    (),
                ))
            })
            .map_err(|error| error.to_string())?;
    }
    let (consume, original) = (plan.clone(), payload.clone());
    let removed = owner
        .0
        .transaction(move |db| {
            let record = records::lookup(db, &original.receipt)?
                .ok_or_else(|| invalid("Missing metadata retirement owner"))?;
            if record.plan != original
                || !record.completed
                || record.retired
                || record.metadata.as_ref() != Some(&consume)
            {
                return Err(invalid("Metadata retirement changed before deletion"));
            }
            consume.authorize(db, &original)?;
            // Native namespaces, leases and exact file pins belong to this bounded
            // callback. It may not perform source work or call back into Store.
            let removed = delete().map_err(invalid)?;
            if removed != consume.total_bytes()? {
                return Err(invalid("Metadata retirement byte accounting changed"));
            }
            Ok(removed)
        })
        .map_err(|error| error.to_string())?;
    let id = plan.payload_plan_id.clone();
    owner
        .0
        .append_checked(&id, Expected::Seq(2), move |db| {
            plan.authorize(db, &payload)?;
            let completion = Completion {
                plan_id: plan.payload_plan_id.clone(),
                removed_bytes: removed,
            };
            Ok((
                vec![NewEvent::new(
                    metadata::COMPLETED,
                    serde_json::to_value(completion).map_err(invalid)?,
                )],
                (),
            ))
        })
        .map_err(|error| error.to_string())?;
    Ok(removed)
}
