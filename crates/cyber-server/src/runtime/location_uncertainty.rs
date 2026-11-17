//! Opaque native outcomes and exact snapshot owners have separate recovery authority.
use super::{Handle, RuntimeError, activity};
use cyber_store::Store;
use std::collections::HashSet;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;

impl Handle {
    pub(super) fn has_location_uncertainty(&self) -> bool {
        let snapshots = self
            .snapshot_uncertain
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.location_uncertain.load(Ordering::SeqCst) || !snapshots.is_empty()
    }

    pub(super) fn mark_snapshot_uncertain(&self, activity: &str) {
        self.snapshot_uncertain
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(activity.to_owned());
    }

    /// Call only at the writer after observing this exact owner's durable
    /// acknowledgement. Opaque uncertainty and other owners are unaffected.
    pub(super) fn acknowledge_snapshot_uncertainty(&self, activity: &str) {
        self.snapshot_uncertain
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(activity);
    }
}

pub(super) fn load(store: &Store, session: &str) -> Result<(bool, HashSet<String>), RuntimeError> {
    let session = session.to_owned();
    let rows = store.read(move |db| {
        let mut query = db.prepare("SELECT e.aggregate_id,e.data FROM event e WHERE e.type='native.activity.changed.1' AND json_extract(e.data,'$.session_id')=?1 AND json_extract(e.data,'$.status')<>'settled' AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq) LIMIT 4097")?;
        Ok(query.query_map([session], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?)
    })?;
    if rows.len() > 4096 {
        return Err(RuntimeError::Conflict(
            "Native Location uncertainty limit exceeded; retained evidence requires inspection"
                .into(),
        ));
    }
    let mut opaque = false;
    let mut snapshots = HashSet::new();
    for (id, data) in rows {
        let record: activity::Record = serde_json::from_str(&data)
            .map_err(|error| RuntimeError::Corrupt(error.to_string()))?;
        if !matches!(record.status.as_str(), "pending" | "unknown") {
            return Err(RuntimeError::Corrupt(
                "Invalid native activity status".into(),
            ));
        }
        if record.snapshot_worker && record.binary_document.is_none() {
            snapshots.insert(id);
        } else if record.binary_document.is_none() || record.snapshot_worker {
            // Document-specific workers retain their separate document recovery
            // contract. Ambiguous mixed kinds never gain snapshot authority.
            opaque = true;
        }
    }
    Ok((opaque, snapshots))
}
