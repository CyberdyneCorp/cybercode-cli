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

pub(super) async fn edit(
    data: PathBuf,
    config: std::sync::Arc<cyber_tools::ConfigFn>,
    runtime: cyber_server::runtime::Runtime,
    edit: cyber_server::http::MemoryEdit,
) -> Result<cyber_server::runtime::MemoryChange, ApiError> {
    let lease = runtime
        .memory_mutation_lease()
        .await
        .map_err(ApiError::from)?;
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        mutate(&data, config.as_ref(), &runtime, edit)
    })
    .await
    .map_err(ApiError::unknown)?
}

fn mutation_settings(config: &cyber_tools::ConfigFn, directory: &Path) -> Result<(), ApiError> {
    let (resolved, _) =
        config(directory).map_err(|_| ApiError::invalid("Memory configuration is unavailable"))?;
    let settings =
        cyber_core::memory::MemorySettings::from_config(&resolved, &cyber_core::env::ProcessEnv)
            .map_err(|error| ApiError::invalid(error.to_string()))?;
    if !settings.enabled {
        return Err(ApiError::forbidden("Memory is disabled"));
    }
    if !settings.generate {
        return Err(ApiError::forbidden("Memory is read-only"));
    }
    if !cfg!(unix) {
        return Err(ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "ServiceUnavailableError",
            "Memory mutations require native storage privacy and durability",
        ));
    }
    Ok(())
}

fn validate_edit(edit: &cyber_server::http::MemoryEdit) -> Result<(), ApiError> {
    cyber_core::memory::validate_name(&edit.name)
        .map_err(|error| ApiError::invalid(error.to_string()))?;
    if let Some(content) = &edit.content {
        if content.len() > 1_048_576 {
            return Err(ApiError::invalid("Memory file exceeds 1 MiB"));
        }
        let document = MemoryDocument::for_write(content)
            .map_err(|error| ApiError::invalid(error.to_string()))?;
        if document.metadata.name != edit.name {
            return Err(ApiError::invalid("Memory name does not match frontmatter"));
        }
    }
    Ok(())
}

fn mutate(
    data: &Path,
    config: &cyber_tools::ConfigFn,
    runtime: &cyber_server::runtime::Runtime,
    edit: cyber_server::http::MemoryEdit,
) -> Result<cyber_server::runtime::MemoryChange, ApiError> {
    use cyber_server::runtime::{MemoryAdmission, MemoryWrite};
    validate_edit(&edit)?;
    let project = project(&edit.directory, edit.scope);
    let identity = edit
        .identity
        .as_ref()
        .map(|identity| format!("http:{}", identity.key));
    let fingerprint = edit
        .identity
        .as_ref()
        .map(|identity| identity.digest.as_str())
        .unwrap_or_else(|| edit.content.as_deref().unwrap_or(""));
    let write = MemoryWrite {
        directory: &edit.directory,
        project_id: &project,
        name: &edit.name,
        deleted: edit.content.is_none(),
        identity: identity.as_deref(),
        content: fingerprint,
        http_hash: edit
            .identity
            .as_ref()
            .map(|identity| identity.digest.as_str()),
    };
    if let Some(change) = runtime
        .memory_write_receipt(&write)
        .map_err(admission_error)?
    {
        return Ok(change);
    }
    mutation_settings(config, &edit.directory)?;
    let store = match MemoryStore::existing(data, &project).map_err(storage_error)? {
        Some(store) => store,
        None if edit.content.is_some() => {
            MemoryStore::open(data, &project).map_err(storage_error)?
        }
        None => return Err(storage_error(MemoryStorageError::NotFound)),
    };
    let mut scope = store.claim().map_err(mutation_storage_error)?;
    scope.list().map_err(storage_error)?;
    if edit.content.is_none() {
        scope.read(&edit.name).map_err(storage_error)?;
    }
    mutation_settings(config, &edit.directory)?;
    let owner = match runtime.admit_memory_write(write).map_err(admission_error)? {
        MemoryAdmission::Replay(change) => return Ok(change),
        MemoryAdmission::Owned(owner) => *owner,
    };
    let prepared = match edit.content {
        Some(content) => scope.prepare_write(&content),
        None => scope.prepare_delete(&edit.name),
    }
    .map_err(storage_error)?;
    let mut change = None;
    prepared
        .commit_with_acknowledgement(|receipt| {
            change = Some(owner.finish(receipt.clone()).map_err(|error| {
                cyber_core::log::error(
                    "memory",
                    &error.to_string(),
                    serde_json::json!({"mutation_id":receipt.id}),
                );
                MemoryStorageError::Unsafe("memory acknowledgement requires reviewed recovery")
            })?);
            Ok(())
        })
        .map_err(storage_error)?;
    change.ok_or_else(|| ApiError::unknown("Memory change acknowledgement is missing"))
}

fn admission_error(error: cyber_store::StoreError) -> ApiError {
    match error {
        cyber_store::StoreError::Projector { reason, .. } => ApiError::conflict(reason),
        error => ApiError::unknown(error),
    }
}

fn mutation_storage_error(error: MemoryStorageError) -> ApiError {
    match error {
        MemoryStorageError::Busy => ApiError::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "ServiceUnavailableError",
            "Memory storage is busy; retry with the same Idempotency-Key",
        ),
        error => storage_error(error),
    }
}
