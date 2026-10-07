//! Location at a durable cursor, independent of the Session's latest binding.
use super::{ApiError, AppState};
use crate::runtime::{LiveEvent, RuntimeError};
use rusqlite::OptionalExtension;
use serde_json::Value;
use std::collections::HashMap;

pub(super) fn changed(kind: &str, data: &Value) -> Option<String> {
    match kind {
        "session.created.1" => data["info"]["directory"].as_str(),
        "session.worktree.rebound.1" => data["to"]["path"].as_str(),
        _ => None,
    }
    .map(str::to_owned)
}

pub(super) fn previous(event: &LiveEvent) -> Option<&str> {
    match event {
        LiveEvent::Durable { kind, data, .. } if kind == "session.worktree.rebound.1" => {
            data["from"]["path"].as_str()
        }
        _ => None,
    }
}

pub(super) async fn at(state: &AppState, id: &str, seq: i64) -> Result<Option<String>, ApiError> {
    let store = state.store.clone();
    let id = id.to_owned();
    let record: Option<(String, String)> = tokio::task::spawn_blocking(move || {
        store.read(move |db| {
            Ok(db
                .query_row(
                    "SELECT type, data FROM event WHERE aggregate_id=?1 AND seq<=?2
             AND type IN ('session.created.1', 'session.worktree.rebound.1')
             ORDER BY seq DESC LIMIT 1",
                    rusqlite::params![id, seq.max(0)],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?)
        })
    })
    .await
    .map_err(ApiError::unknown)?
    .map_err(ApiError::unknown)?;
    record
        .map(|(kind, data)| {
            let data = serde_json::from_str(&data).map_err(ApiError::unknown)?;
            changed(&kind, &data)
                .ok_or_else(|| ApiError::unknown("Invalid persisted Location binding"))
        })
        .transpose()
}

pub(super) struct Cached {
    directory: String,
    seq: Option<i64>,
}

pub(super) async fn live(
    state: &AppState,
    cache: &mut HashMap<String, Cached>,
    id: &str,
    event: &LiveEvent,
) -> Result<String, ApiError> {
    if let LiveEvent::Durable {
        seq, kind, data, ..
    } = event
    {
        let directory = match changed(kind, data) {
            Some(directory) => canonical(directory),
            None => match cache.get(id) {
                Some(cached) if cached.seq == Some(seq - 1) => cached.directory.clone(),
                _ => at(state, id, *seq)
                    .await?
                    .map(canonical)
                    .unwrap_or_default(),
            },
        };
        if !directory.is_empty()
            && cache
                .get(id)
                .and_then(|cached| cached.seq)
                .is_none_or(|last| *seq >= last)
        {
            cache.insert(
                id.into(),
                Cached {
                    directory: directory.clone(),
                    seq: Some(*seq),
                },
            );
        }
        return Ok(directory);
    }
    if let Some(cached) = cache.get(id) {
        return Ok(cached.directory.clone());
    }
    let directory = match state.runtime.state(id).await {
        Ok(session) => session.info.directory,
        Err(RuntimeError::SessionNotFound(_)) => {
            // Queued notifications can outlive the Session and its persisted history.
            return Ok(String::new());
        }
        Err(error) => return Err(error.into()),
    };
    let directory = canonical(directory);
    cache.insert(
        id.into(),
        Cached {
            directory: directory.clone(),
            seq: None,
        },
    );
    Ok(directory)
}

fn canonical(directory: String) -> String {
    std::fs::canonicalize(&directory).map_or(directory, |path| path.display().to_string())
}
