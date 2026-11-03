//! Model receipt recovery never admits an invocation draft to the manual editor.
#[cfg(any(target_os = "macos", target_os = "linux"))]
use super::recovery::RecoveryEntry;
use super::recovery::{RecoveryList, RecoveryQuery};
use super::*;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use cyber_core::binary::{
    editor::{DraftScope, Editor, Owner},
    storage::StorageRoot,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeQuery {
    pub session_id: String,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn scope_owner(turn: &TurnContext, root: PathBuf) -> Result<Owner, Error> {
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
        let scope = scope_owner(&turn, root.clone()).map_err(refused)?;
        let authority = state.runtime.capture_child_admission(&turn.session_id)?;
        let storage = state.runtime.binary_storage().map_err(refused)?;
        let listing = work(&state, move || {
            storage.list_model(&scope, query.after.as_deref(), query.limit.unwrap_or(64))
        })
        .await?;
        let mut documents = Vec::new();
        for entry in listing.documents {
            let decisions = state
                .runtime
                .file_decisions(&turn, "read", &[root.join(&entry.path)])
                .await;
            if decisions.first() == Some(&FileDecision::Allow) {
                documents.push(RecoveryEntry {
                    document_id: entry.document_id,
                    path: entry.path,
                    record_available: entry.record_available,
                });
            }
        }
        authority.verify(&state.runtime, &turn.session_id)?;
        let (fresh, fresh_root, _) = files::scope(&state, &parts, Some(&turn.session_id)).await?;
        if fresh != turn || fresh_root != root {
            return Err(ApiError::forbidden("Binary model recovery scope changed"));
        }
        // Recheck every disclosed name after bounded discovery, without prompts.
        let paths: Vec<_> = documents
            .iter()
            .map(|entry| root.join(&entry.path))
            .collect();
        let decisions = state.runtime.file_decisions(&turn, "read", &paths).await;
        documents = documents
            .into_iter()
            .zip(decisions)
            .filter_map(|(entry, decision)| (decision == FileDecision::Allow).then_some(entry))
            .collect();
        Ok(Json(Located {
            location: LocationInfo::of(FilePath::new(&turn.directory)),
            data: RecoveryList {
                documents,
                next_cursor: listing.next_cursor,
            },
        }))
    }
}

pub(super) async fn operation(
    State(state): State<AppState>,
    parts: Parts,
    Path((id, operation)): Path<(String, String)>,
    Query(query): Query<ScopeQuery>,
) -> Result<Json<Located<Option<binary::receipts::Operation>>>, ApiError> {
    inspect(state, parts, id, operation, query.session_id, false).await
}
pub(super) async fn reconcile(
    State(state): State<AppState>,
    parts: Parts,
    Path((id, operation)): Path<(String, String)>,
    Json(input): Json<saving::ReconcileRequest>,
) -> Result<Json<Located<Option<binary::receipts::Operation>>>, ApiError> {
    inspect(state, parts, id, operation, input.session_id, true).await
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct Admission {
    owner: Owner,
    read: ReadAuthority,
    storage: Arc<StorageRoot>,
    ancestor: crate::runtime::AdmissionAuthority,
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn admit(
    state: &AppState,
    parts: &Parts,
    session: &str,
    id: &str,
) -> Result<Admission, ApiError> {
    let (turn, root, _) = files::scope(state, parts, Some(session)).await?;
    let scope = scope_owner(&turn, root.clone()).map_err(refused)?;
    let storage = state.runtime.binary_storage().map_err(refused)?;
    let (owner, resource) = {
        let (storage, id) = (storage.clone(), id.to_owned());
        work(state, move || {
            let owner = storage.model_owner(&scope, &id)?;
            let resource = storage.resource(&owner, &id)?;
            Ok((owner, resource))
        })
        .await
        .map_err(|_| ApiError::not_found("NotFoundError", "No such model binary document"))?
    };
    let grant = permit_read(state, &turn, &resource).await?;
    let read = ReadAuthority {
        turn,
        root,
        grant,
        resource,
    };
    verify_read(state, parts, &read).await?;
    let ancestor = state.runtime.capture_child_admission(session)?;
    Ok(Admission {
        owner,
        read,
        storage,
        ancestor,
    })
}

async fn inspect(
    state: AppState,
    parts: Parts,
    id: String,
    operation: String,
    session: String,
    reconcile: bool,
) -> Result<Json<Located<Option<binary::receipts::Operation>>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, id, operation, session, reconcile);
        Err(editing::unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let admission = admit(&state, &parts, &session, &id).await?;
        let lifecycle = state.runtime.memory_mutation_lease().await?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let _cancel_on_disposal = cancel.clone().drop_guard();
        let runtime = state.runtime.clone();
        let (storage, owner, ancestor) = (
            admission.storage,
            admission.owner,
            admission.ancestor.clone(),
        );
        let resource = admission.read.resource.clone();
        let result = work(&state, move || {
            let _lifecycle = lifecycle;
            ancestor
                .verify(&runtime, &owner.session_id)
                .map_err(|_| Error::Changed)?;
            let mut editor = Editor::resume_for_resource(
                storage.open_existing(&id)?,
                &owner,
                &resource,
                || cancel.is_cancelled(),
            )?;
            editor.validate_read_resource()?;
            runtime
                .recover_binary_activity(editor.recovery_proof()?)
                .map_err(|_| Error::Changed)?;
            editor.lookup_operation(&operation)?;
            if reconcile
                && editor
                    .pending_operation()
                    .is_some_and(|pending| pending.id == operation)
            {
                editor.reconcile_cancellable(|| cancel.is_cancelled())?;
            }
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            runtime.settle_binary_notification(&mut editor, &operation)?;
            ancestor
                .verify(&runtime, &owner.session_id)
                .map_err(|_| Error::Changed)?;
            editor.validate_read_resource()?;
            // No callback dispatch or native replacement here. Existing unknown
            // evidence and notification-pending state remain explicit.
            editor.lookup_operation(&operation)
        })
        .await?;
        admission.ancestor.verify(&state.runtime, &session)?;
        verify_read(&state, &parts, &admission.read).await?;
        Ok(Json(Located {
            location: LocationInfo::of(FilePath::new(&admission.read.turn.directory)),
            data: result,
        }))
    }
}
