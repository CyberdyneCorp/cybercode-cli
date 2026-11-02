//! Owner-index discovery and explicit recovery never replay a source mutation.
use super::*;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use cyber_core::binary::editor::{DraftScope, Editor, Owner};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryQuery {
    pub session_id: String,
    pub after: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "BinaryRecoverRequest")]
pub struct RecoverRequest {
    pub session_id: String,
    #[serde(default)]
    pub read_only: bool,
}
#[derive(Serialize, JsonSchema)]
#[schemars(rename = "BinaryRecoveryEntry")]
pub struct RecoveryEntry {
    pub document_id: String,
    pub path: String,
    pub record_available: bool,
}
#[derive(Serialize, JsonSchema)]
#[schemars(rename = "BinaryRecoveryList")]
pub struct RecoveryList {
    pub documents: Vec<RecoveryEntry>,
    pub next_cursor: Option<String>,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn owner(turn: &TurnContext, root: PathBuf) -> Result<Owner, Error> {
    Ok(Owner {
        session_id: turn.session_id.clone(),
        directory: std::fs::canonicalize(&turn.directory).map_err(|_| Error::File)?,
        root,
        scope: DraftScope::Manual,
    })
}

pub(super) async fn list(
    State(state): State<AppState>,
    parts: Parts,
    Query(query): Query<RecoveryQuery>,
) -> Result<Json<Located<RecoveryList>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, query);
        Err(editing::unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (turn, root, _) = files::scope(&state, &parts, Some(&query.session_id)).await?;
        let owner = owner(&turn, root.clone()).map_err(refused)?;
        let storage = state
            .runtime
            .binary_documents()
            .storage()
            .map_err(refused)?;
        let listing = work(&state, move || {
            storage.list(&owner, query.after.as_deref(), query.limit.unwrap_or(64))
        })
        .await?;
        let mut documents = Vec::new();
        for entry in listing.documents {
            // Discovery discloses names only under current ordinary read rules.
            let decisions = state
                .runtime
                .file_decisions(&turn, "read", &[root.join(&entry.path)])
                .await;
            if decisions.first() != Some(&FileDecision::Deny) {
                documents.push(RecoveryEntry {
                    document_id: entry.document_id,
                    path: entry.path,
                    record_available: entry.record_available,
                });
            }
        }
        let (fresh, fresh_root, _) = files::scope(&state, &parts, Some(&query.session_id)).await?;
        if fresh != turn || fresh_root != root {
            return Err(ApiError::forbidden("Binary recovery scope changed"));
        }
        Ok(Json(Located {
            location: LocationInfo::of(FilePath::new(&turn.directory)),
            data: RecoveryList {
                documents,
                next_cursor: listing.next_cursor,
            },
        }))
    }
}

pub(super) async fn recover(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Json(input): Json<RecoverRequest>,
) -> Result<Json<Located<editing::Status>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, id, input);
        Err(editing::unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (turn, root, _) = files::scope(&state, &parts, Some(&input.session_id)).await?;
        let owner = owner(&turn, root.clone()).map_err(refused)?;
        let storage = state
            .runtime
            .binary_documents()
            .storage()
            .map_err(refused)?;
        let resource = {
            let (storage, owner, id) = (storage.clone(), owner.clone(), id.clone());
            work(&state, move || storage.resource(&owner, &id))
                .await
                .map_err(|_| ApiError::not_found("NotFoundError", "No such binary document"))?
        };
        let grant = permit_read(&state, &turn, &resource).await?;
        let authority = ReadAuthority {
            turn: turn.clone(),
            root,
            grant,
            resource,
        };
        verify_read(&state, &parts, &authority).await?;
        let lifecycle = state.runtime.memory_mutation_lease().await?;
        let registry = state.runtime.clone();
        let result = work(&state, move || {
            let _lifecycle = lifecycle;
            let editor = Editor::resume(storage.open_existing(&id)?, &owner)?;
            editor.validate_read_resource()?;
            registry
                .recover_binary_activity(editor.recovery_proof()?)
                .map_err(|_| Error::Changed)?;
            registry
                .binary_documents()
                .insert_recovered(id, owner, editor, input.read_only)
        })
        .await?;
        verify_read(&state, &parts, &authority).await?;
        let decision = state
            .runtime
            .binary_file_decision(&turn, "edit", &result.resource)
            .await;
        let status = work(&state, move || {
            let content = result.document.lock().map_err(|_| Error::Io)?;
            Ok(editing::describe(&result, &content, true, decision))
        })
        .await?;
        Ok(Json(Located {
            location: LocationInfo::of(FilePath::new(&turn.directory)),
            data: status,
        }))
    }
}

pub(super) async fn operation(
    State(state): State<AppState>,
    parts: Parts,
    Path((id, operation)): Path<(String, String)>,
    Query(query): Query<DocumentQuery>,
) -> Result<Json<Located<Option<binary::receipts::Operation>>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, id, operation, query);
        Err(editing::unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (entry, location, authority) = owned(&state, &parts, &query.session_id, &id).await?;
        let result = work(&state, move || {
            let content = entry.document.lock().map_err(|_| Error::Io)?;
            content.validate_resource()?;
            let crate::runtime::binary_documents::Content::Draft(editor) = &*content else {
                return Err(Error::Invalid);
            };
            editor.lookup_operation(&operation)
        })
        .await?;
        verify_read(&state, &parts, &authority).await?;
        Ok(Json(Located {
            location,
            data: result,
        }))
    }
}
