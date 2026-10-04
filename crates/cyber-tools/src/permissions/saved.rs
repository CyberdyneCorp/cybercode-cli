//! Saved approvals per checkout (`permissions-modes` → Persisted approvals).

use std::path::Path;

use cyber_store::{Store, StoreError};
use serde::Serialize;

use super::engine::{Effect, Rule};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SavedApproval {
    pub id: i64,
    pub checkout_root: String,
    pub action: String,
    pub resource: String,
    pub created_at: i64,
    pub source_session: String,
}

pub fn save(
    store: &Store,
    root: &Path,
    action: &str,
    patterns: &[String],
    session: &str,
) -> Result<(), StoreError> {
    let (root, action, patterns, session) = (
        root.display().to_string(),
        action.to_string(),
        patterns.to_vec(),
        session.to_string(),
    );
    store.transaction(move |tx| {
        for pattern in &patterns {
            tx.execute(
                "INSERT INTO permission_saved (checkout_root, action, resource, created_at, source_session)
                 VALUES (?1, ?2, ?3, strftime('%s','now') * 1000, ?4)
                 ON CONFLICT (checkout_root, action, resource) DO NOTHING",
                rusqlite::params![root, action, pattern, session],
            )?;
        }
        Ok(())
    })
}

pub fn list(store: &Store, root: Option<&Path>) -> Result<Vec<SavedApproval>, StoreError> {
    let root = root.map(|r| r.display().to_string());
    store.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, checkout_root, action, resource, created_at, source_session FROM permission_saved
             WHERE ?1 IS NULL OR checkout_root = ?1 ORDER BY id",
        )?;
        let rows = stmt
            .query_map([root], |r| {
                Ok(SavedApproval {
                    id: r.get(0)?,
                    checkout_root: r.get(1)?,
                    action: r.get(2)?,
                    resource: r.get(3)?,
                    created_at: r.get(4)?,
                    source_session: r.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    })
}

pub fn revoke(store: &Store, id: i64) -> Result<bool, StoreError> {
    store.transaction(move |tx| {
        Ok(tx.execute("DELETE FROM permission_saved WHERE id = ?1", [id])? > 0)
    })
}

/// Saved approvals as `allow` rules for one checkout.
pub fn rules(store: &Store, root: &Path) -> Result<Vec<Rule>, StoreError> {
    Ok(list(store, Some(root))?
        .into_iter()
        .map(|a| Rule::new(&a.action, &a.resource, Effect::Allow, "saved"))
        .collect())
}
