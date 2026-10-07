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
}
impl Runtime {
    pub fn children_usage(&self, id: &str) -> Result<ChildrenUsage, RuntimeError> {
        let source = id.to_owned();
        self.inner.store.read(move|conn|Ok(conn.query_row(
            "SELECT children_cost,children_tokens,children_unpriced_steps,children_usage_complete FROM session WHERE id=?1",
            [source],|row|read(row,0)).optional()?))?.ok_or_else(||RuntimeError::SessionNotFound(id.into()))
    }
}
pub(super) fn read(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<ChildrenUsage> {
    Ok(ChildrenUsage {
        children_cost: row.get(offset)?,
        children_tokens: row.get::<_, i64>(offset + 1)? as u64,
        children_unpriced_steps: row.get::<_, i64>(offset + 2)? as u64,
        children_usage_complete: row.get::<_, i64>(offset + 3)? != 0,
    })
}
pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> rusqlite::Result<()> {
    let usage = &event.data["usage"];
    let tokens = ["input", "output", "reasoning", "cache_read", "cache_write"]
        .iter()
        .map(|key| usage[*key].as_i64().unwrap_or(0))
        .fold(0i64, i64::saturating_add);
    let cost = event.data["cost"].as_f64();
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
            "INSERT INTO session_children_charge(parent_id,source_id,event_id,cost,tokens,unpriced)
            VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(parent_id,event_id) DO NOTHING",
            params![
                parent,
                event.aggregate_id,
                event.id,
                cost.unwrap_or(0.0),
                tokens,
                i64::from(cost.is_none())
            ],
        )?;
        if inserted == 1 {
            tx.execute("UPDATE session SET children_cost=children_cost+?2,children_tokens=children_tokens+?3,
                children_unpriced_steps=children_unpriced_steps+?4 WHERE id=?1",
                params![parent,cost.unwrap_or(0.0),tokens,i64::from(cost.is_none())])?;
        }
    }
    Ok(())
}
