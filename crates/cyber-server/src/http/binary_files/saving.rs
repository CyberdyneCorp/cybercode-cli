//! Reviewed native installation. Permission prompts finish before filesystem
//! ownership; the actual worker refreshes authority immediately before intent.
use super::editing::{Edited, ReviewRequest};
use super::*;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::runtime::binary_documents::{Content, EditAdmission};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use cyber_core::binary::editor::SavePreparation;
use cyber_core::binary::receipts::SaveReview;

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "BinarySaveRequest")]
pub struct SaveRequest {
    pub session_id: String,
    pub operation_id: String,
    pub review: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "BinaryReconcileRequest")]
pub struct ReconcileRequest {
    pub session_id: String,
}

pub(super) async fn save(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Json(input): Json<SaveRequest>,
) -> Result<Json<Located<Edited>>, ApiError> {
    install(state, parts, id, input, false).await
}
pub(super) async fn restore(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Json(input): Json<SaveRequest>,
) -> Result<Json<Located<Edited>>, ApiError> {
    install(state, parts, id, input, true).await
}

async fn install(
    state: AppState,
    parts: Parts,
    id: String,
    input: SaveRequest,
    restore: bool,
) -> Result<Json<Located<Edited>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, id, input, restore);
        Err(editing::unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (entry, location, authority) = owned(&state, &parts, &input.session_id, &id).await?;
        let cached = editing::staged_admission(&state, &authority, &entry).await?;
        let existing = {
            let (entry, operation_id, review) = (
                entry.clone(),
                input.operation_id.clone(),
                input.review.clone(),
            );
            work(&state, move || {
                let content = entry.document.lock().map_err(|_| Error::Io)?;
                content.validate_resource()?;
                let Content::Draft(editor) = &*content else {
                    return Err(Error::Invalid);
                };
                Ok(editor
                    .lookup_installation(&operation_id, &review, restore)?
                    .map(|operation| Edited {
                        operation,
                        status: editing::describe(&entry, &content, true, cached.grant),
                    }))
            })
            .await?
        };
        if let Some(existing) = existing {
            verify_read(&state, &parts, &authority).await?;
            return Ok(Json(Located {
                location,
                data: existing,
            }));
        }
        let review = {
            let entry = entry.clone();
            work(&state, move || {
                let content = entry.document.lock().map_err(|_| Error::Io)?;
                let Content::Draft(editor) = &*content else {
                    return Err(Error::Invalid);
                };
                if restore {
                    editor.restore_review(|| false)
                } else {
                    editor.review(0, 128, || false)
                }
            })
            .await?
        };
        if review.fingerprint != input.review {
            return Err(refused(Error::Changed));
        }
        let metadata = serde_json::json!({"binary_operation":if restore {"restore"} else {"save"},
            "requires_confirmation":true,"byte_review":review});
        let grant = editing::permit_edit(&state, &authority, &entry, metadata).await?;
        verify_read(&state, &parts, &authority).await?;
        let admission = EditAdmission {
            turn: authority.turn.clone(),
            grant,
            authority: state.runtime.capture_child_admission(&input.session_id)?,
        };
        let runtime = state.runtime.clone();
        let result = state
            .runtime
            .binary_file_work(
                authority.turn.clone(),
                id,
                entry.resource.clone(),
                (authority.grant, grant),
                move |work| {
                    admission
                        .authority
                        .verify(&runtime, &admission.turn.session_id)
                        .map_err(|_| Error::Changed)?;
                    let mut content = entry.document.lock().map_err(|_| Error::Io)?;
                    content.validate_resource()?;
                    let Content::Draft(editor) = &mut *content else {
                        return Err(Error::Invalid);
                    };
                    let prepared = if restore {
                        editor.prepare_restore(&input.operation_id, &input.review, || {
                            work.is_cancelled()
                        })?
                    } else {
                        editor.prepare_save(&input.operation_id, &input.review, || {
                            work.is_cancelled()
                        })?
                    };
                    let operation = match prepared {
                        SavePreparation::Existing(operation) => operation,
                        SavePreparation::Ready(prepared) => editor.commit_save_checked(
                            *prepared,
                            || work.is_cancelled(),
                            || {
                                admission
                                    .authority
                                    .verify(&runtime, &admission.turn.session_id)
                                    .map_err(|_| Error::Changed)?;
                                work.verify()
                            },
                        )?,
                    };
                    work.resume_notifications(editor);
                    Ok(Edited {
                        operation,
                        status: editing::describe(&entry, &content, true, grant),
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
pub(super) async fn restore_review(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Json(input): Json<ReviewRequest>,
) -> Result<Json<Located<SaveReview>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, id, input);
        Err(editing::unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (entry, location, authority) = owned(&state, &parts, &input.session_id, &id).await?;
        let result = work(&state, move || {
            let content = entry.document.lock().map_err(|_| Error::Io)?;
            content.validate_resource()?;
            let Content::Draft(editor) = &*content else {
                return Err(Error::Invalid);
            };
            if editor.draft().revision() != input.revision {
                return Err(Error::Changed);
            }
            editor.restore_review(|| false)
        })
        .await?;
        verify_read(&state, &parts, &authority).await?;
        Ok(Json(Located {
            location,
            data: result,
        }))
    }
}
pub(super) async fn reconcile(
    State(state): State<AppState>,
    parts: Parts,
    Path(id): Path<String>,
    Json(input): Json<ReconcileRequest>,
) -> Result<Json<Located<Edited>>, ApiError> {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (state, parts, id, input);
        Err(editing::unsupported())
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (entry, location, authority) = owned(&state, &parts, &input.session_id, &id).await?;
        let decision = state
            .runtime
            .binary_file_decision(&authority.turn, "edit", &entry.resource)
            .await;
        let result = work(&state, move || {
            let mut content = entry.document.lock().map_err(|_| Error::Io)?;
            content.validate_resource()?;
            let Content::Draft(editor) = &mut *content else {
                return Err(Error::Invalid);
            };
            let operation = editor.reconcile()?;
            Ok(Edited {
                operation,
                status: editing::describe(&entry, &content, true, decision),
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
