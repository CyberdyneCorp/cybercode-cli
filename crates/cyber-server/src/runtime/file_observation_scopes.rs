//! Durable Session provenance for observation owners. Projectors only check
//! journal/Session state; they never consult a changing filesystem during replay.
//! Alias acceptance additionally needs live pinned native proof and remains
//! disabled until that owner proof is connected to dispatch and user renewal.
use super::file_observations::FileWatch;
use super::{Runtime, RuntimeError, file_observations};
use cyber_core::hooks::file_observations::{self as records, SessionScope};
use cyber_store::{Expected, NewEvent, Store, StoredEvent};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

fn current(db: &Connection, aggregate: &str) -> Result<Option<(i64, SessionScope)>, String> {
    let row: Option<(i64, String, String)> = db
        .query_row(
            "SELECT seq,type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
            [aggregate],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    row.map(|(seq, kind, data)| {
        let scope: SessionScope = serde_json::from_str(&data).map_err(|error| error.to_string())?;
        scope.validate()?;
        if kind != records::SCOPE || scope.aggregate() != aggregate {
            return Err("invalid file observation Session scope history".into());
        }
        Ok((seq, scope))
    })
    .transpose()
}

pub(super) fn project(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    let scope: SessionScope =
        serde_json::from_value(event.data.clone()).map_err(|error| error.to_string())?;
    scope.validate()?;
    if scope.aggregate() != event.aggregate_id {
        return Err("file observation scope aggregate differs from its Session".into());
    }
    let valid: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM session WHERE id=?1 AND directory=?2 AND archived=0)",
            params![scope.session_id, scope.session_directory],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !valid {
        return Err("file observation scope differs from its live Session".into());
    }
    file_observations::verify_scope_generation(tx, &scope)
}

/// Record the current owned Session scope once per generation. At this
/// increment only exact directory admission is supported; a scope record
/// alone never authorizes an alias or filesystem access.
pub(super) async fn admit(
    watch: &FileWatch,
    runtime: Runtime,
    store: &Store,
    session_id: &str,
) -> Result<(), RuntimeError> {
    let _open = runtime.inner.open().await?;
    let handle = runtime.inner.handle(session_id).await?;
    let info = handle.state.lock().await.info.clone();
    if info.archived || info.directory != watch.state().directory || !watch.current()? {
        return Err(RuntimeError::Invalid(
            "file observation Session scope is unavailable or stale".into(),
        ));
    }
    let scope = SessionScope {
        session_id: info.id,
        session_directory: info.directory,
        directory: watch.state().directory.clone(),
        root_identity: watch.state().root_identity.clone(),
        generation: watch.state().generation.clone(),
    };
    let aggregate = scope.aggregate();
    let lookup = aggregate.clone();
    let previous =
        store.read(move |db| current(db, &lookup).map_err(cyber_store::StoreError::Unavailable))?;
    if previous
        .as_ref()
        .is_some_and(|(_, previous)| previous == &scope)
    {
        return Ok(());
    }
    runtime.inner.store.append(
        &aggregate,
        Expected::Seq(previous.map_or(-1, |(seq, _)| seq)),
        vec![NewEvent::new(
            records::SCOPE,
            serde_json::to_value(scope)
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?,
        )],
    )?;
    Ok(())
}
