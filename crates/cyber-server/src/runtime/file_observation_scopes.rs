//! Durable Session provenance for observation owners. Projectors only check
//! journal/Session state; they never consult a changing filesystem during replay.
//! Alias acceptance additionally requires retained native proof before each
//! handler and user renewal; these records cannot recreate that authority.
use super::file_observations::FileWatch;
use super::{Runtime, RuntimeError, file_observations};
use cyber_core::hooks::file_observations::{self as records, Checkpoint, SessionScope};
use cyber_store::{Expected, NewEvent, Store, StoredEvent};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

pub(super) fn project_input(tx: &Transaction<'_>, event: &StoredEvent) -> Result<(), String> {
    let Some(scope) = event
        .data
        .get("observation_scope")
        .filter(|scope| !scope.is_null())
    else {
        return Ok(());
    };
    let scope: records::InputScope =
        serde_json::from_value(scope.clone()).map_err(|error| error.to_string())?;
    scope.validate()?;
    let valid: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM session WHERE id=?1 AND directory=?2)",
            params![event.aggregate_id, scope.session_directory],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !valid || event.data["source"] != "user" {
        return Err("observation acknowledgement scope differs from its owned user input".into());
    }
    Ok(())
}

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

/// Deterministic provenance check only. Live native membership must be checked
/// separately before alias effects; replay cannot recover that capability.
pub(super) fn matches(
    db: &Connection,
    state: &Checkpoint,
    session: &str,
    directory: &str,
) -> Result<bool, String> {
    if state.directory == directory {
        return Ok(true);
    }
    if !state.requires_native_lease {
        return Ok(false);
    }
    let expected = SessionScope {
        session_id: session.into(),
        session_directory: directory.into(),
        directory: state.directory.clone(),
        root_identity: state.root_identity.clone(),
        generation: state.generation.clone(),
    };
    Ok(current(db, &expected.aggregate())?.is_some_and(|(_, scope)| scope == expected))
}

/// Record the current owned Session scope once per generation. Native
/// alias admission requires the FileWatch's retained native root; a scope
/// record alone never authorizes an alias or filesystem access.
pub(super) async fn admit(
    watch: &FileWatch,
    runtime: Runtime,
    store: &Store,
    session_id: &str,
) -> Result<(), RuntimeError> {
    let _open = runtime.inner.open().await?;
    let handle = runtime.inner.handle(session_id).await?;
    let info = handle.state.lock().await.info.clone();
    if info.archived || !watch.current()? {
        return Err(RuntimeError::Invalid(
            "file observation Session scope is unavailable or stale".into(),
        ));
    }
    watch.validate_native_scope(&info.directory)?;
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
    let changed = previous.as_ref().is_none_or(|(_, previous)| {
        previous.root_identity != scope.root_identity
            || previous.directory != scope.directory
            || previous.session_directory != scope.session_directory
    });
    runtime.inner.store.append(
        &aggregate,
        Expected::Seq(previous.map_or(-1, |(seq, _)| seq)),
        vec![NewEvent::new(
            records::SCOPE,
            serde_json::to_value(&scope)
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?,
        )],
    )?;
    if changed {
        handle
            .envelope_epoch
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        *handle
            .envelope
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
    Ok(())
}
