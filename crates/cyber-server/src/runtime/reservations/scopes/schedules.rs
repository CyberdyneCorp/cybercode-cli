//! An occurrence is immutable dispatch context, independent of the source Session's
//! later foreground work. Children read their captured occurrence, never an ancestor's
//! newly active occurrence.
use super::{Snapshot, budget, invalid};
use cyber_core::budget::Budget;
use cyber_store::{StoreError, StoredEvent};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

fn available(tx: &Connection) -> Result<bool, StoreError> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schedule_occurrence')",
        [], |row| row.get(0),
    )?)
}

fn load(tx: &Connection, id: &str) -> Result<Option<(Option<Budget>, i64)>, StoreError> {
    let row = tx
        .query_row(
            "SELECT spec,created_ms FROM schedule_occurrence WHERE id=?1",
            [id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    row.map(|(raw, created)| {
        let spec: Value = serde_json::from_str(&raw).map_err(|error| invalid(id, error))?;
        if !spec.is_object() {
            return Err(invalid(id, "schedule occurrence spec must be an object"));
        }
        let limits = spec
            .get("budget")
            .filter(|v| !v.is_null())
            .map(|value| budget(&value.to_string(), id))
            .transpose()?;
        Ok((limits, created))
    })
    .transpose()
}

fn snapshot(id: &str, limits: Budget, created: i64) -> Snapshot {
    Snapshot {
        id: format!("schedule_occurrence:{id}"),
        kind: "schedule".into(),
        owner: id.into(),
        budget: limits,
        evidence: json!({"version":1,"activation_ms":created}),
    }
}

pub(super) fn current(tx: &Connection, session: &str) -> Result<Option<Snapshot>, StoreError> {
    if !available(tx)? {
        return Ok(None);
    }
    let occurrence=tx.query_row("SELECT COALESCE(inherited_schedule_occurrence_id,active_schedule_occurrence_id) FROM session WHERE id=?1",[session],|row|row.get::<_,Option<String>>(0)).optional()?.flatten();
    let Some(id) = occurrence else {
        return Ok(None);
    };
    match load(tx, &id)? {
        Some((limits, created)) => Ok(limits.map(|limits| snapshot(&id, limits, created))),
        None => {
            let scope = format!("schedule_occurrence:{id}");
            let captured:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM budget_scope_binding WHERE session_id=?1 AND scope_id=?2)",params![session,scope],|row|row.get(0))?;
            if captured {
                Ok(None)
            } else {
                Err(invalid(&scope, "schedule occurrence evidence is missing"))
            }
        }
    }
}

/// Present `unattended`, including null, is the creation authority. Its saved Run
/// context can predate the parent's current foreground/scheduled occurrence.
pub(super) fn created(
    tx: &Connection,
    event: &StoredEvent,
) -> Result<Option<Snapshot>, StoreError> {
    let Some(binding) = event.data.get("unattended") else {
        return current(tx, &event.aggregate_id);
    };
    if binding.is_null() || !available(tx)? {
        return Ok(None);
    }
    let id = binding["occurrence_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            invalid(
                &event.aggregate_id,
                "invalid saved schedule occurrence binding",
            )
        })?;
    let (limits, created) = load(tx, id)?.ok_or_else(|| {
        invalid(
            &format!("schedule_occurrence:{id}"),
            "saved schedule occurrence evidence is missing",
        )
    })?;
    Ok(limits.map(|limits| snapshot(id, limits, created)))
}

pub(super) fn latest(tx: &Connection, captured: &Snapshot) -> Result<Option<Snapshot>, StoreError> {
    if !available(tx)? {
        return Ok(None);
    }
    Ok(load(tx, &captured.owner)?.and_then(|(limits, created)| {
        limits.map(|limits| snapshot(&captured.owner, limits, created))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_core::budget::Enforcement;
    use cyber_core::paths::DatabaseLocation;
    use cyber_store::{EventRegistry, Store, StoreOptions};

    #[test]
    fn occurrence_scopes_follow_saved_source_context_and_outlive_foreground_replacement() {
        let store = Store::open(StoreOptions::new(
            DatabaseLocation::Memory,
            EventRegistry::default(),
        ))
        .unwrap();
        store.transaction(|tx| {
            // Exercise only the collector's schema contract, independently of the
            // scheduler migration and its other admission/lifecycle columns.
            tx.execute_batch("DROP TABLE IF EXISTS schedule_occurrence; CREATE TABLE schedule_occurrence(id TEXT PRIMARY KEY,spec TEXT NOT NULL,created_ms INTEGER NOT NULL);")?;
            let columns=tx.prepare("PRAGMA table_info(session)")?.query_map([],|row|row.get::<_,String>(1))?.collect::<Result<Vec<_>,_>>()?;
            for column in ["inherited_schedule_occurrence_id","active_schedule_occurrence_id"] {
                if !columns.iter().any(|name|name==column) {tx.execute(&format!("ALTER TABLE session ADD COLUMN {column} TEXT"),[])?;}
            }
            let limits=Budget{max_tokens:Some(10),max_turns:Some(1),enforcement:Enforcement::Reserved,..Default::default()};
            for (id,tokens) in [("old",10),("new",20)] {
                tx.execute("INSERT INTO schedule_occurrence(id,spec,created_ms) VALUES (?1,?2,1)",params![id,json!({"budget":Budget{max_tokens:Some(tokens),..limits.clone()}}).to_string()])?;
            }
            for (id,parent,inherited,active) in [("root",None,None,Some("old")),("child",Some("root"),Some("old"),None),("delayed",Some("root"),Some("old"),None),("foreground",Some("root"),None,None)] {
                tx.execute("INSERT INTO session(id,title,directory,agent,model,mode,created_at,updated_at,last_seq,parent_id,inherited_schedule_occurrence_id,active_schedule_occurrence_id) VALUES (?1,'x','/r','build','test/main','default',1,1,0,?2,?3,?4)",params![id,parent,inherited,active])?;
            }
            assert_eq!(super::super::collect(tx,"root",None,2)?.0[0].max_tokens,Some(10));
            let birth=|id:&str,unattended:Value| StoredEvent{id:format!("born-{id}"),aggregate_id:id.into(),seq:0,kind:"session.created.1".into(),time_ms:2,causation_id:None,data:json!({"info":{"parent_id":"root"},"unattended":unattended})};
            super::super::bind_created(tx,&birth("child",json!({"occurrence_id":"old","mode":"default","budget":limits})))?;
            tx.execute("UPDATE session SET active_schedule_occurrence_id='new' WHERE id='root'",[])?;
            assert_eq!(super::super::collect(tx,"root",None,3)?.0[0].id,"schedule_occurrence:new");
            assert_eq!(super::super::collect(tx,"child",None,3)?.0[0].id,"schedule_occurrence:old");
            // Delayed workflow context is older than the parent's newly active
            // occurrence; explicit null means foreground context with no occurrence.
            super::super::bind_created(tx,&birth("delayed",json!({"occurrence_id":"old","mode":"default"})))?;
            super::super::bind_created(tx,&birth("foreground",Value::Null))?;
            assert_eq!(super::super::collect(tx,"delayed",None,3)?.0.len(),1);
            assert_eq!(super::super::collect(tx,"delayed",None,3)?.0[0].id,"schedule_occurrence:old");
            assert!(super::super::collect(tx,"foreground",None,3)?.0.is_empty());
            tx.execute("UPDATE session SET active_schedule_occurrence_id=NULL WHERE id='root'",[])?;
            assert!(super::super::collect(tx,"root",None,4)?.0.is_empty());
            tx.execute("DELETE FROM schedule_occurrence WHERE id='old'",[])?;
            assert!(super::super::collect(tx,"child",None,5)?.0[0].uncertain);
            assert!(super::super::collect(tx,"delayed",None,5)?.0[0].uncertain);
            Ok(())
        }).unwrap();
    }
}
