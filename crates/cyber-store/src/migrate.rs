//! Schema migrations (`storage-events` → Idempotent migrations).

use std::collections::HashSet;

use rusqlite::{Connection, TransactionBehavior, params};

use crate::error::StoreError;

pub(crate) struct Migration {
    pub id: &'static str,
    pub sql: &'static str,
}

/// Every known migration, in ID order.
pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        id: "20261003000000_event_store",
        sql: include_str!("../migrations/20261003000000_event_store.sql"),
    },
    Migration {
        id: "20261004000000_sessions",
        sql: include_str!("../migrations/20261004000000_sessions.sql"),
    },
    Migration {
        id: "20261005000000_permissions_todos",
        sql: include_str!("../migrations/20261005000000_permissions_todos.sql"),
    },
    Migration {
        id: "20261006000000_idempotency",
        sql: include_str!("../migrations/20261006000000_idempotency.sql"),
    },
    Migration {
        id: "20261007000000_jobs",
        sql: include_str!("../migrations/20261007000000_jobs.sql"),
    },
    Migration {
        id: "20261007010000_subagent_names",
        sql: include_str!("../migrations/20261007010000_subagent_names.sql"),
    },
];

pub(crate) enum Outcome {
    Current,
    /// The database was written by a newer version; open read-only.
    TooNew(Vec<String>),
}

pub(crate) fn migrate(
    conn: &mut Connection,
    migrations: &[Migration],
) -> Result<Outcome, StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS migration (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL) STRICT",
    )?;
    let applied = applied_ids(conn)?;
    let known: HashSet<&str> = migrations.iter().map(|m| m.id).collect();
    let mut unknown: Vec<String> = applied
        .iter()
        .filter(|id| !known.contains(id.as_str()))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Ok(Outcome::TooNew(unknown));
    }
    let pending: Vec<&Migration> = migrations
        .iter()
        .filter(|m| !applied.contains(m.id))
        .collect();
    if applied.is_empty() && is_empty_schema(conn)? {
        apply(conn, &pending)?;
    } else {
        for migration in pending {
            apply(conn, &[migration])?;
        }
    }
    Ok(Outcome::Current)
}

/// Apply migrations and their journal rows in one transaction.
fn apply(conn: &mut Connection, migrations: &[&Migration]) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    for migration in migrations {
        tx.execute_batch(migration.sql)?;
        tx.execute(
            "INSERT INTO migration (id, time_completed) VALUES (?1, ?2)",
            params![migration.id, now],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn applied_ids(conn: &Connection) -> Result<HashSet<String>, StoreError> {
    let mut stmt = conn.prepare("SELECT id FROM migration")?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

fn is_empty_schema(conn: &Connection) -> Result<bool, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != 'migration'",
        [],
        |r| r.get(0),
    )?;
    Ok(count == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_ids_are_sorted_and_unique() {
        let ids: Vec<&str> = MIGRATIONS.iter().map(|m| m.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn migrating_twice_is_a_no_op() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, MIGRATIONS).unwrap();
        migrate(&mut conn, MIGRATIONS).unwrap();
        let n: i64 = conn
            .query_row("SELECT count(*) FROM migration", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, MIGRATIONS.len() as i64);
    }
}
