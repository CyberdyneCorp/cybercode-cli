//! Owner draft operations. File installation remains a separately reviewed action.
use super::*;
use crate::runtime::binary_documents::Content;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::runtime::binary_documents::EditAdmission;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use cyber_core::binary::draft::PatchBatch;
use cyber_core::binary::draft::Splice;
use cyber_core::binary::receipts::{Operation, SaveReview};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use cyber_core::binary::{
    draft::ImmutableBase,
    editor::{DraftScope, Editor, Owner},
};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use serde_json::Value;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::sync::atomic::Ordering;

#[derive(Serialize, JsonSchema)]
#[schemars(rename = "BinaryStatus")]
pub struct Status {
    #[serde(flatten)]
    pub metadata: binary::Metadata,
    pub read_only: bool,
    pub prepared: bool,
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub base_sha256: Option<String>,
    pub pending_operation: Option<Operation>,
    pub edit_supported: bool,
    pub edit_decision: FileDecision,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "BinaryPrepareRequest")]
pub struct PrepareRequest {
    pub session_id: String,
    pub revision: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "BinaryPatchRequest")]
pub struct PatchRequest {
    pub session_id: String,
    pub operation_id: String,
    pub revision: String,
    pub splices: Vec<Splice>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "BinaryHistoryAction")]
pub enum HistoryAction {
    Undo,
    Redo,
    Discard,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "BinaryHistoryRequest")]
pub struct HistoryRequest {
    pub session_id: String,
    pub operation_id: String,
    pub revision: String,
    pub action: HistoryAction,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "BinaryReviewRequest")]
pub struct ReviewRequest {
    pub session_id: String,
    pub revision: String,
    pub start: Option<usize>,
    pub limit: Option<usize>,
}
#[derive(Serialize, JsonSchema)]
#[schemars(rename = "BinaryEdited")]
pub struct Edited {
    pub operation: Operation,
    pub status: Status,
}

pub(super) fn unsupported() -> ApiError {
    ApiError::invalid("Binary editing is unavailable for this platform or storage configuration")
        .with_reason("binary_edit_unsupported")
}
pub(super) fn describe(
    entry: &Entry,
    content: &Content,
    supported: bool,
    decision: FileDecision,
) -> Status {
    let status = Status {
        metadata: content.metadata(),
        read_only: entry.read_only,
        prepared: false,
        dirty: false,
        can_undo: false,
        can_redo: false,
        base_sha256: None,
        pending_operation: None,
        edit_supported: supported && !entry.read_only && content.metadata().within_edit_size_limit,
        edit_decision: decision,
    };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let status = {
        let mut status = status;
        if let Content::Draft(editor) = content {
            status.prepared = true;
            status.dirty = editor.draft().can_undo();
            status.can_undo = editor.draft().can_undo();
            status.can_redo = editor.draft().can_redo();
            status.base_sha256 = Some(editor.draft().base().sha256().into());
            status.pending_operation = editor.pending_operation();
        }
        status
    };
    status
}
fn platform_supported(state: &AppState) -> bool {
    cfg!(any(target_os = "macos", target_os = "linux"))
        && state.runtime.binary_documents().storage().is_ok()
}

pub(super) async fn status(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Query(query): Query<DocumentQuery>,
) -> Result<Json<Located<Status>>, ApiError> {
    let (entry, location, authority) = owned(&state, &parts, &query.session_id, &id).await?;
    let decision = state
        .runtime
        .binary_file_decision(&authority.turn, "edit", &entry.resource)
        .await;
    let supported = platform_supported(&state);
    let result = work(&state, move || {
        let content = entry.document.lock().map_err(|_| Error::Io)?;
        content.validate_resource()?;
        Ok(describe(&entry, &content, supported, decision))
    })
    .await?;
    verify_read(&state, &parts, &authority).await?;
    Ok(Json(Located {
        location,
        data: result,
    }))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn permit_edit(
    state: &AppState,
    authority: &ReadAuthority,
    entry: &Entry,
    metadata: Value,
) -> Result<FileDecision, ApiError> {
    if entry.read_only {
        return Err(ApiError::forbidden("This binary document is read-only")
            .with_reason("binary_read_only"));
    }
    let decision = state
        .runtime
        .binary_file_decision(&authority.turn, "edit", &entry.resource)
        .await;
    if decision == FileDecision::Deny {
        return Err(files::denied(decision, "Permission denied: edit"));
    }
    state
        .runtime
        .authorize_binary_edit(&authority.turn, &entry.resource, metadata)
        .await
        .map_err(|message| files::denied(decision, &message))?;
    Ok(decision)
}

pub(super) async fn prepare(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Json(input): Json<PrepareRequest>,
) -> Result<Json<Located<Status>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, id, input);
        Err(unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (entry, location, authority) = owned(&state, &parts, &input.session_id, &id).await?;
        let storage = state
            .runtime
            .binary_documents()
            .storage()
            .map_err(|_| unsupported())?;
        let root = entry.root.clone();
        work(&state, move || storage.admit_project(&root))
            .await
            .map_err(|_| unsupported())?;
        let grant = permit_edit(
            &state,
            &authority,
            &entry,
            serde_json::json!({"binary_operation": "prepare"}),
        )
        .await?;
        verify_read(&state, &parts, &authority).await?;
        let admission = EditAdmission {
            turn: authority.turn.clone(),
            grant,
            authority: state.runtime.capture_child_admission(&input.session_id)?,
        };
        let storage = state
            .runtime
            .binary_documents()
            .storage()
            .map_err(|_| unsupported())?;
        let turn = authority.turn.clone();
        let resource = entry.resource.clone();
        let result = state
            .runtime
            .binary_file_work(
                turn.clone(),
                id.clone(),
                resource,
                (authority.grant, grant),
                move |cancel| {
                    cancel.verify()?;
                    let mut content = entry.document.lock().map_err(|_| Error::Io)?;
                    if content.metadata().revision != input.revision {
                        return Err(Error::Changed);
                    }
                    content.validate_resource()?;
                    if let Content::Live(document) = &mut *content {
                        let owner = Owner {
                            session_id: turn.session_id,
                            directory: std::fs::canonicalize(&turn.directory)
                                .map_err(|_| Error::File)?,
                            root: entry.root.clone(),
                            scope: DraftScope::Manual,
                        };
                        let editor = Editor::create(storage.open(&id)?, owner, document, || {
                            cancel.is_cancelled()
                        })?;
                        *content = Content::Draft(Box::new(editor));
                        entry.editing.store(true, Ordering::Release);
                    }
                    *entry.edit_admission.lock().map_err(|_| Error::Io)? = Some(admission);
                    Ok(describe(&entry, &content, true, grant))
                },
            )
            .await?
            .map_err(refused)?;
        verify_read(&state, &parts, &authority).await?;
        Ok(Json(Located {
            location,
            data: result,
        }))
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn staged_admission(
    state: &AppState,
    authority: &ReadAuthority,
    entry: &Entry,
) -> Result<EditAdmission, ApiError> {
    if entry.read_only {
        return Err(ApiError::forbidden("This binary document is read-only")
            .with_reason("binary_read_only"));
    }
    let cached = entry
        .edit_admission
        .lock()
        .map_err(|_| refused(Error::Io))?
        .clone()
        .ok_or_else(|| {
            ApiError::forbidden("Prepare this draft under current edit authority")
                .with_reason("binary_edit_admission_required")
        })?;
    if cached.turn != authority.turn
        || cached
            .authority
            .verify(&state.runtime, &authority.turn.session_id)
            .is_err()
    {
        return Err(
            ApiError::forbidden("Edit scope changed; prepare this draft again")
                .with_reason("binary_authority_changed"),
        );
    }
    let decision = state
        .runtime
        .binary_file_decision(&authority.turn, "edit", &entry.resource)
        .await;
    if decision == FileDecision::Deny
        || (decision == FileDecision::Ask && cached.grant != FileDecision::Ask)
    {
        return Err(files::denied(decision, "Binary edit authority changed"));
    }
    Ok(cached)
}

pub(super) async fn patch(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Json(input): Json<PatchRequest>,
) -> Result<Json<Located<Edited>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, id, input);
        Err(unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (entry, location, authority) = owned(&state, &parts, &input.session_id, &id).await?;
        let admission = staged_admission(&state, &authority, &entry).await?;
        let grant = admission.grant;
        let runtime = state.runtime.clone();
        let result = state
            .runtime
            .binary_file_work(
                authority.turn.clone(),
                id,
                entry.resource.clone(),
                (authority.grant, grant),
                move |cancel| {
                    if cancel.is_cancelled()
                        || admission
                            .authority
                            .verify(&runtime, &admission.turn.session_id)
                            .is_err()
                    {
                        return Err(Error::Cancelled);
                    }
                    let mut content = entry.document.lock().map_err(|_| Error::Io)?;
                    content.validate_resource()?;
                    let Content::Draft(editor) = &mut *content else {
                        return Err(Error::Invalid);
                    };
                    cancel.verify()?;
                    let operation = editor.patch(
                        &input.operation_id,
                        &PatchBatch {
                            revision: input.revision,
                            splices: input.splices,
                        },
                    )?;
                    Ok(Edited {
                        operation,
                        status: describe(&entry, &content, true, grant),
                    })
                },
            )
            .await?
            .map_err(refused)?;
        verify_read(&state, &parts, &authority).await?;
        Ok(Json(Located {
            location,
            data: result,
        }))
    }
}

pub(super) async fn history(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Json(input): Json<HistoryRequest>,
) -> Result<Json<Located<Edited>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, id, input);
        Err(unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (entry, location, authority) = owned(&state, &parts, &input.session_id, &id).await?;
        let admission = staged_admission(&state, &authority, &entry).await?;
        let grant = admission.grant;
        let runtime = state.runtime.clone();
        let result = state
            .runtime
            .binary_file_work(
                authority.turn.clone(),
                id,
                entry.resource.clone(),
                (authority.grant, grant),
                move |cancel| {
                    if cancel.is_cancelled()
                        || admission
                            .authority
                            .verify(&runtime, &admission.turn.session_id)
                            .is_err()
                    {
                        return Err(Error::Cancelled);
                    }
                    let mut content = entry.document.lock().map_err(|_| Error::Io)?;
                    content.validate_resource()?;
                    let Content::Draft(editor) = &mut *content else {
                        return Err(Error::Invalid);
                    };
                    cancel.verify()?;
                    let operation = match input.action {
                        HistoryAction::Undo => {
                            editor.history(&input.operation_id, &input.revision, false)?
                        }
                        HistoryAction::Redo => {
                            editor.history(&input.operation_id, &input.revision, true)?
                        }
                        HistoryAction::Discard => {
                            editor.discard(&input.operation_id, &input.revision)?
                        }
                    };
                    Ok(Edited {
                        operation,
                        status: describe(&entry, &content, true, grant),
                    })
                },
            )
            .await?
            .map_err(refused)?;
        verify_read(&state, &parts, &authority).await?;
        Ok(Json(Located {
            location,
            data: result,
        }))
    }
}

pub(super) async fn review(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Json(input): Json<ReviewRequest>,
) -> Result<Json<Located<SaveReview>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, id, input);
        Err(unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (entry, location, authority) = owned(&state, &parts, &input.session_id, &id).await?;
        let cancellation = tokio_util::sync::CancellationToken::new();
        let _cancel_on_drop = cancellation.clone().drop_guard();
        let result = work(&state, move || {
            let content = entry.document.lock().map_err(|_| Error::Io)?;
            content.validate_resource()?;
            let Content::Draft(editor) = &*content else {
                return Err(Error::Invalid);
            };
            if editor.draft().revision() != input.revision {
                return Err(Error::Changed);
            }
            editor.review(input.start.unwrap_or(0), input.limit.unwrap_or(128), || {
                cancellation.is_cancelled()
            })
        })
        .await?;
        verify_read(&state, &parts, &authority).await?;
        Ok(Json(Located {
            location,
            data: result,
        }))
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    #[test]
    fn binary_wire_requests_roundtrip_on_supported_and_unsupported_platforms() {
        fn check<T: serde::de::DeserializeOwned + Serialize>(value: serde_json::Value) {
            let request: T = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), value);
        }
        check::<PrepareRequest>(serde_json::json!({"session_id":"ses_one","revision":"r"}));
        check::<PatchRequest>(
            serde_json::json!({"session_id":"ses_one","operation_id":"patch_one","revision":"r","splices":[{"offset":"9007199254740993","delete_length":"1","expected_hex":"00","insert_hex":"ff"}]}),
        );
        check::<HistoryRequest>(
            serde_json::json!({"session_id":"ses_one","operation_id":"undo_one","revision":"r","action":"undo"}),
        );
        check::<ReviewRequest>(
            serde_json::json!({"session_id":"ses_one","revision":"r","start":0,"limit":128}),
        );
        check::<super::super::recovery::RecoverRequest>(
            serde_json::json!({"session_id":"ses_one","read_only":true}),
        );
    }
}
