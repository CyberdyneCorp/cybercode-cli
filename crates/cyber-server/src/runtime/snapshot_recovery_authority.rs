//! Snapshot-only authority may preserve a closed boundary, never reopen or inherit it.
use super::{Runtime, RuntimeError, admission_authority};
use cyber_store::StoreError;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecoveryAuthority {
    bindings: Vec<admission_authority::Binding>,
    solve_sequences: Vec<i64>,
    closed_scopes: Vec<Option<String>>,
}

impl RecoveryAuthority {
    pub fn capture(runtime: &Runtime, source: &str) -> Result<Self, RuntimeError> {
        let source = source.to_owned();
        runtime
            .inner
            .store
            .read(move |db| {
                let tx = db.unchecked_transaction()?;
                let bindings =
                    admission_authority::snapshot_chain(&tx, &source)?.ok_or_else(|| {
                        StoreError::InvalidAppend("Recovery ancestry is unavailable".into())
                    })?;
                let solve_sequences = solve_sequences(&tx, &bindings)?;
                if bindings.len() > 4096 {
                    return Err(StoreError::InvalidAppend(
                        "Recovery ancestry exceeds limit".into(),
                    ));
                }
                let closed_scopes = closed_scopes(&tx, &bindings)?;
                tx.commit()?;
                Ok(Self {
                    bindings,
                    solve_sequences,
                    closed_scopes,
                })
            })
            .map_err(Into::into)
    }
    pub fn bindings(&self) -> &[admission_authority::Binding] {
        &self.bindings
    }
    pub fn current(&self, db: &Connection) -> Result<bool, StoreError> {
        let Some(source) = self.bindings.first() else {
            return Ok(false);
        };
        Ok(
            admission_authority::snapshot_chain(db, &source.id)?.as_ref() == Some(&self.bindings)
                && solve_sequences(db, &self.bindings)? == self.solve_sequences
                && closed_scopes(db, &self.bindings)? == self.closed_scopes,
        )
    }
    pub fn verify(&self, runtime: &Runtime) -> Result<(), RuntimeError> {
        let captured = self.clone();
        let current = runtime.inner.store.read(move |db| {
            let tx = db.unchecked_transaction()?;
            let current = captured.current(&tx)?;
            tx.commit()?;
            Ok(current)
        })?;
        if !current {
            return Err(RuntimeError::Conflict(
                "Snapshot recovery authority changed".into(),
            ));
        }
        Ok(())
    }
}
fn closed_scopes(
    db: &Connection,
    bindings: &[admission_authority::Binding],
) -> Result<Vec<Option<String>>, StoreError> {
    bindings
        .iter()
        .map(|binding| super::subtree_reopen::active_scope(db, &binding.id))
        .collect()
}
fn solve_sequences(
    db: &Connection,
    bindings: &[admission_authority::Binding],
) -> Result<Vec<i64>, StoreError> {
    bindings
        .iter()
        .map(|binding| {
            Ok(db.query_row(
                "SELECT COALESCE(MAX(seq),-1) FROM event WHERE aggregate_id=?1",
                [format!("solve_fence_{}", binding.id)],
                |row| row.get(0),
            )?)
        })
        .collect()
}
