//! Billing is separate from conversation ownership and copied model context.
use super::{Runtime, RuntimeError};
use cyber_store::StoredEvent;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChildrenUsage {
    pub children_cost: f64,
    pub children_tokens: u64,
    pub children_unpriced_steps: u64,
    /// False when older purged child history cannot be reconstructed.
    pub children_usage_complete: bool,
    #[serde(default)]
    pub children_token_classes: cyber_llm::Usage,
    /// False when retained historical receipts lack source token-class evidence.
    #[serde(default)]
    pub children_token_classes_complete: bool,
}
impl Runtime {
    pub fn children_usage(&self, id: &str) -> Result<ChildrenUsage, RuntimeError> {
        let source = id.to_owned();
        self.inner.store.read(move|conn|Ok(conn.query_row(
            "SELECT children_cost,children_tokens,children_unpriced_steps,children_usage_complete,children_token_classes,children_token_classes_complete FROM session WHERE id=?1",
            [source],|row|read(row,0)).optional()?))?.ok_or_else(||RuntimeError::SessionNotFound(id.into()))
    }
}
pub(super) fn read(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<ChildrenUsage> {
    Ok(ChildrenUsage {
        children_cost: row.get(offset)?,
        children_tokens: row.get::<_, i64>(offset + 1)? as u64,
        children_unpriced_steps: row.get::<_, i64>(offset + 2)? as u64,
        children_usage_complete: row.get::<_, i64>(offset + 3)? != 0,
        children_token_classes: serde_json::from_str(&row.get::<_, String>(offset + 4)?).map_err(
            |error| {
                rusqlite::Error::FromSqlConversionFailure(
                    offset + 4,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            },
        )?,
        children_token_classes_complete: row.get::<_, i64>(offset + 5)? != 0,
    })
}
pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> rusqlite::Result<()> {
    if event.data["usage_accounted"] == true {
        return Ok(());
    }
    let usage = &event.data["usage"];
    let tokens = ["input", "output", "reasoning", "cache_read", "cache_write"]
        .iter()
        .map(|key| usage[*key].as_i64().unwrap_or(0))
        .fold(0i64, i64::saturating_add);
    let cost = event.data["cost"].as_f64();
    let unpriced = unpriced_increment(event, cost)?;
    let turns = event.data["reservation_turns_increment"]
        .as_i64()
        .unwrap_or_else(|| i64::from(event.kind == super::events::STEP_ENDED));
    let complete = ["input", "output", "reasoning", "cache_read", "cache_write"]
        .iter()
        .all(|field| usage[*field].as_i64().is_some_and(|value| value >= 0));
    let classes = serde_json::json!({
        "input":usage["input"].as_i64().unwrap_or(0),
        "output":usage["output"].as_i64().unwrap_or(0),
        "reasoning":usage["reasoning"].as_i64().unwrap_or(0),
        "cache_read":usage["cache_read"].as_i64().unwrap_or(0),
        "cache_write":usage["cache_write"].as_i64().unwrap_or(0),
    });
    let mut statement = tx.prepare(
        "WITH RECURSIVE ancestors(id) AS (
        SELECT parent_id FROM session WHERE id=?1 AND parent_id IS NOT NULL AND parent_id!=?1
        UNION SELECT s.parent_id FROM ancestors a JOIN session s ON s.id=a.id
        WHERE s.parent_id IS NOT NULL AND s.parent_id!=?1
    ) SELECT s.id FROM ancestors a JOIN session s ON s.id=a.id",
    )?;
    let ancestors = statement
        .query_map([&event.aggregate_id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for parent in ancestors {
        let inserted = tx.execute(
            "INSERT INTO session_children_charge(parent_id,source_id,event_id,cost,tokens,unpriced,turns,token_classes)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(parent_id,event_id) DO NOTHING",
            params![
                parent,
                event.aggregate_id,
                event.id,
                cost.unwrap_or(0.0),
                tokens,
                unpriced,
                turns,
                complete.then(|| classes.to_string())
            ],
        )?;
        if inserted == 1 {
            let updated = tx.execute("UPDATE session SET children_cost=children_cost+?2,children_tokens=children_tokens+?3,
                children_unpriced_steps=children_unpriced_steps+?4,
                children_token_classes_complete=children_token_classes_complete AND ?10,
                children_token_classes=json_set(children_token_classes,
                    '$.input',json_extract(children_token_classes,'$.input')+?5,
                    '$.output',json_extract(children_token_classes,'$.output')+?6,
                    '$.reasoning',json_extract(children_token_classes,'$.reasoning')+?7,
                    '$.cache_read',json_extract(children_token_classes,'$.cache_read')+?8,
                    '$.cache_write',json_extract(children_token_classes,'$.cache_write')+?9)
                WHERE id=?1 AND children_unpriced_steps+?4>=0",
                params![parent,cost.unwrap_or(0.0),tokens,unpriced,
                    classes["input"].as_i64(),classes["output"].as_i64(),classes["reasoning"].as_i64(),classes["cache_read"].as_i64(),classes["cache_write"].as_i64(), i64::from(complete)])?;
            if updated != 1 {
                return Err(rusqlite::Error::InvalidParameterName(
                    "reservation unpriced adjustment would make ancestor total negative".into(),
                ));
            }
        }
    }
    Ok(())
}

fn unpriced_increment(event: &StoredEvent, cost: Option<f64>) -> rusqlite::Result<i64> {
    let Some(raw) = event.data.get("reservation_unpriced_increment") else {
        return Ok(i64::from(cost.is_none()));
    };
    let value = raw.as_i64().filter(|n| (-1..=1).contains(n));
    if event.data["reservation_id"].as_str().is_none() || value.is_none() {
        return Err(rusqlite::Error::InvalidParameterName(
            "reservation_unpriced_increment requires a linked receipt and -1, 0 or 1".into(),
        ));
    }
    Ok(value.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_core::paths::DatabaseLocation;
    use cyber_store::{EventRegistry, Store, StoreOptions};
    use serde_json::json;

    #[test]
    fn signed_unpriced_receipts_resolve_once_and_turns_use_increment() {
        let store = Store::open(StoreOptions::new(
            DatabaseLocation::Memory,
            EventRegistry::default(),
        ))
        .unwrap();
        store.transaction(|tx| {
            for (id,parent) in [("root",None),("child",Some("root"))] {
                tx.execute("INSERT INTO session(id,title,directory,agent,model,mode,created_at,updated_at,last_seq,parent_id) VALUES (?1,'x','/r','build','test/main','default',1,1,0,?2)",params![id,parent])?;
            }
            let mut event=StoredEvent{id:"partial".into(),aggregate_id:"child".into(),seq:1,kind:super::super::events::AUXILIARY_USAGE.into(),time_ms:1,causation_id:None,
                data:json!({"reservation_id":"request","reservation_unpriced_increment":1,"reservation_turns_increment":1,"reservation_actual":{"turns":1},"cost":null,"usage":{"input":1,"output":0,"reasoning":0,"cache_read":0,"cache_write":0}})};
            project(tx,&event)?;
            event.id="priced".into();
            event.data["reservation_unpriced_increment"]=json!(-1);
            event.data["reservation_turns_increment"]=json!(0);
            event.data["cost"]=json!(0.5);
            event.data["usage"]["input"]=json!(0);
            project(tx,&event)?;
            project(tx,&event)?;
            let (unknown,turns):(i64,i64)=tx.query_row("SELECT children_unpriced_steps,(SELECT SUM(turns) FROM session_children_charge WHERE parent_id='root') FROM session WHERE id='root'",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
            assert_eq!((unknown,turns),(0,1));
            let adjustments:i64=tx.query_row("SELECT SUM(unpriced) FROM session_children_charge",[],|r|r.get(0))?;
            assert_eq!(adjustments,0);
            Ok(())
        }).unwrap();
        assert!(store.transaction(|tx| {
            let event=StoredEvent{id:"invalid-negative".into(),aggregate_id:"child".into(),seq:3,kind:super::super::events::AUXILIARY_USAGE.into(),time_ms:3,causation_id:None,
                data:json!({"reservation_id":"request","reservation_unpriced_increment":-1,"cost":0,"usage":{"input":0,"output":0,"reasoning":0,"cache_read":0,"cache_write":0}})};
            project(tx,&event)?;
            Ok(())
        }).is_err());
        assert_eq!(store.read(|tx|Ok(tx.query_row("SELECT COUNT(*) FROM session_children_charge WHERE event_id='invalid-negative'",[],|r|r.get::<_,i64>(0))?)).unwrap(),0);
    }
}
