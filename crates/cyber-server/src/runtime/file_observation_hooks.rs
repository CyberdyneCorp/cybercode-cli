//! Private observation provenance on hook admission. It is kept in the start
//! event for writer validation, not in public receipts or telemetry.
use super::RuntimeError;
use super::hooks::HookExecutionRecord;
use cyber_core::hooks::file_observations as records;
use cyber_core::hooks::{HookDefinition, HookEvent};
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) const FIELD: &str = "file_observation_binding";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    observation_id: String,
    generation: String,
    file_path: String,
}

pub(super) fn prepare(
    event: &HookEvent,
    definition: &HookDefinition,
    record: &mut HookExecutionRecord,
) -> Result<Option<Value>, RuntimeError> {
    if event.is_synthetic() || event.event() != "FileChanged" {
        return Ok(None);
    }
    let field = |name: &str| {
        event.as_json()[name]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| RuntimeError::Invalid(format!("FileChanged lacks core {name}")))
    };
    let binding = Binding {
        observation_id: field("observation_id")?,
        generation: field("watch_generation")?,
        file_path: field("file_path")?,
    };
    if !records::relative_name(&binding.file_path) {
        return Err(RuntimeError::Invalid("invalid observed file path".into()));
    }
    let scope = serde_json::to_vec(&(
        &record.session_id,
        &binding.file_path,
        definition.scope,
        &definition.source,
        &definition.pointer,
        &definition.digest,
    ))
    .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
    record.id =
        records::receipt_id(&binding.observation_id, &scope).map_err(RuntimeError::Invalid)?;
    serde_json::to_value(binding)
        .map(Some)
        .map_err(|error| RuntimeError::Invalid(error.to_string()))
}

pub(super) fn project(
    tx: &Transaction<'_>,
    data: &Value,
    record: &HookExecutionRecord,
) -> Result<(), String> {
    let binding = data.get(FIELD);
    if record.synthetic || record.event != "FileChanged" {
        if binding.is_some() {
            return Err("observation binding belongs only to real FileChanged admission".into());
        }
        return Ok(());
    }
    let binding: Binding = serde_json::from_value(
        binding
            .ok_or("FileChanged admission lacks provenance")?
            .clone(),
    )
    .map_err(|error| error.to_string())?;
    let archived: bool = tx
        .query_row(
            "SELECT archived FROM session WHERE id=?1",
            [&record.session_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if archived {
        return Err("archived Session cannot admit file observations".into());
    }
    super::file_observations::verify_hook_source(
        tx,
        &record.directory,
        &record.session_id,
        &binding.observation_id,
        &binding.generation,
        &binding.file_path,
    )
}
