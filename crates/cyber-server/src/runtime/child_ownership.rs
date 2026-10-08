//! Durable exclusive result ownership; releasing an owner is not native acknowledgement.
use super::{AdmissionAuthority, RuntimeError, admission_authority::Binding};
use cyber_store::{Expected, NewEvent, Store, StoredEvent};
use rusqlite::{OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(super) const CHANGED: &str = "child.execution.changed.1";
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Record {
    pub source_id: String,
    pub parent_id: String,
    pub child_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission_bindings: Option<Vec<Binding>>,
}
pub(super) struct Lease {
    id: String,
    store: Arc<Store>,
    record: Record,
}
impl Lease {
    pub fn operation_id(&self) -> &str {
        &self.id
    }
    pub fn claim(
        store: Arc<Store>,
        parent: &str,
        child: &str,
        authority: &AdmissionAuthority,
    ) -> Result<Self, RuntimeError> {
        let id = cyber_core::ids::new_id("op");
        let record = Record {
            source_id: authority.bindings[0].id.clone(),
            parent_id: parent.into(),
            child_id: child.into(),
            status: "held".into(),
            admission_bindings: Some(authority.bindings.clone()),
        };
        store.append(&id, Expected::Seq(-1), vec![change(&record)])?;
        Ok(Self { id, store, record })
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.record.status = "released".into();
        self.record.admission_bindings = None;
        if let Err(error) = self
            .store
            .append(&self.id, Expected::Any, vec![change(&self.record)])
        {
            cyber_core::log::error(
                "child-ownership",
                &error.to_string(),
                serde_json::json!({"operation_id":self.id}),
            );
        }
    }
}
fn change(record: &Record) -> NewEvent {
    NewEvent::new(
        CHANGED,
        serde_json::to_value(record).expect("ownership serializes"),
    )
}
pub(super) fn held(store: &Store) -> Result<Vec<(String, Record)>, RuntimeError> {
    let rows = store.read(|db| {
        let mut query = db.prepare("SELECT aggregate_id,data FROM event e WHERE type='child.execution.changed.1' AND json_extract(data,'$.status')='held' AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq)")?;
        Ok(query.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?)
    })?;
    rows.into_iter()
        .map(|(id, data)| {
            serde_json::from_str(&data)
                .map(|record| (id, record))
                .map_err(|error| RuntimeError::Corrupt(error.to_string()))
        })
        .collect()
}
pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    let record: Record =
        serde_json::from_value(event.data.clone()).map_err(|error| error.to_string())?;
    let old: Option<String> = tx
        .query_row(
            "SELECT data FROM event WHERE aggregate_id=?1 AND seq<?2 ORDER BY seq DESC LIMIT 1",
            rusqlite::params![event.aggregate_id, event.seq],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match record.status.as_str() {
        "held" => {
            if record.source_id != record.parent_id && record.source_id != record.child_id {
                return Err("Child ownership has a different execution source".into());
            }
            let parent: Option<Option<String>> = tx
                .query_row(
                    "SELECT parent_id FROM session WHERE id=?1",
                    [&record.child_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?;
            if parent.is_some_and(|parent| parent.as_deref() != Some(record.parent_id.as_str())) {
                return Err("Child execution target has a different parent".into());
            }
            if !super::admission_authority::existing_target_open(tx, &record.child_id)
                .map_err(|error| error.to_string())?
            {
                return Err(super::admission_authority::CLOSED.into());
            }
            if old.is_some() {
                return Err("Child ownership cannot be reacquired with the same operation".into());
            }
            let bindings = record
                .admission_bindings
                .as_ref()
                .ok_or("Missing child ownership authority")?;
            if bindings.first().map(|binding| binding.id.as_str())
                != Some(record.source_id.as_str())
                || !super::admission_authority::bindings_current(tx, bindings)
                    .map_err(|error| error.to_string())?
            {
                return Err(super::admission_authority::STALE.into());
            }
            let busy = tx.query_row("SELECT EXISTS(SELECT 1 FROM event e WHERE type='child.execution.changed.1' AND aggregate_id<>?1 AND json_extract(data,'$.child_id')=?2 AND json_extract(data,'$.status')='held' AND NOT EXISTS(SELECT 1 FROM event n WHERE n.aggregate_id=e.aggregate_id AND n.seq>e.seq))", rusqlite::params![event.aggregate_id,record.child_id], |row| row.get::<_, bool>(0)).map_err(|error| error.to_string())?;
            if busy {
                return Err("Subagent execution ownership is held or requires recovery".into());
            }
        }
        "released" => {
            let old: Record =
                serde_json::from_str(&old.ok_or("Missing child ownership reservation")?)
                    .map_err(|error| error.to_string())?;
            if old.status != "held"
                || old.source_id != record.source_id
                || old.parent_id != record.parent_id
                || old.child_id != record.child_id
                || record.admission_bindings.is_some()
            {
                return Err("Child ownership release differs from its reservation".into());
            }
        }
        _ => return Err("Invalid child execution ownership status".into()),
    }
    Ok(())
}
