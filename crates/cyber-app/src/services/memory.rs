//! Bounded explicit-user review using the same directory-bound memory store as tools and CLI.
use cyber_core::memory::{MemoryCatalog, MemoryDocument, MemoryStorageError, MemoryStore};
use cyber_server::http::{ApiError, MemoryScope};
use std::path::{Path, PathBuf};

fn project(directory: &Path, scope: MemoryScope) -> String {
    match scope {
        MemoryScope::Project => cyber_core::project::identify(directory).id,
        MemoryScope::Global => "global".into(),
    }
}

pub(super) async fn list(
    data: PathBuf,
    directory: PathBuf,
    scope: MemoryScope,
) -> Result<MemoryCatalog, ApiError> {
    tokio::task::spawn_blocking(move || {
        let project = project(&directory, scope);
        let Some(store) = MemoryStore::existing(&data, &project).map_err(storage_error)? else {
            return Ok(MemoryCatalog::default());
        };
        let owner = store.claim().map_err(storage_error)?;
        owner.list().map_err(storage_error)
    })
    .await
    .map_err(ApiError::unknown)?
}

pub(super) async fn read(
    data: PathBuf,
    directory: PathBuf,
    scope: MemoryScope,
    name: String,
) -> Result<MemoryDocument, ApiError> {
    cyber_core::memory::validate_name(&name)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    tokio::task::spawn_blocking(move || {
        let project = project(&directory, scope);
        let store = MemoryStore::existing(&data, &project)
            .map_err(storage_error)?
            .ok_or_else(|| storage_error(MemoryStorageError::NotFound))?;
        let owner = store.claim().map_err(storage_error)?;
        owner.read(&name).map_err(storage_error)
    })
    .await
    .map_err(ApiError::unknown)?
}

fn storage_error(error: MemoryStorageError) -> ApiError {
    match error {
        MemoryStorageError::NotFound => {
            ApiError::not_found("MemoryNotFoundError", error.to_string())
        }
        MemoryStorageError::Busy
        | MemoryStorageError::RecoveryRequired
        | MemoryStorageError::Unsafe(_)
        | MemoryStorageError::Conflict
        | MemoryStorageError::ReviewConflict => ApiError::conflict(error.to_string()),
        MemoryStorageError::Format(_) | MemoryStorageError::TooLarge => {
            ApiError::invalid(error.to_string())
        }
        MemoryStorageError::Io(_) => ApiError::unknown(error),
    }
}
