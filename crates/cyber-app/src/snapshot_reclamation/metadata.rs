//! Serialize native metadata retirement against the receipt/reference writer.
use super::*;
use records::metadata::{self, Plan as MetadataPlan};

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
