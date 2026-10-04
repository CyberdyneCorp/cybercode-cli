//! Read-only SQL for `cyber db query` (`storage-events` → Database command).

use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::Value;

use crate::error::StoreError;

#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

/// Run `sql` on a read-only connection with `query_only=ON`. Writes fail with
/// `attempt to write a readonly database`.
pub fn query_readonly(path: &Path, sql: &str) -> Result<QueryResult, StoreError> {
    if !path.exists() {
        return Err(StoreError::Unavailable(format!(
            "no database at {}",
            path.display()
        )));
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute_batch("PRAGMA query_only = ON;")?;
    let mut stmt = conn.prepare(sql)?;
    let columns: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    let width = columns.len();
    let rows = stmt
        .query_map([], |row| {
            (0..width).map(|i| row.get_ref(i).map(to_json)).collect()
        })?
        .collect::<Result<_, _>>()?;
    Ok(QueryResult { columns, rows })
}

fn to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => Value::from(i),
        ValueRef::Real(f) => Value::from(f),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::String(format!("<blob {} bytes>", b.len())),
    }
}
