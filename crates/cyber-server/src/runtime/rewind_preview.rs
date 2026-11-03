//! Non-mutating owner previews. Confirmation recomputes the complete preview under
//! Location ownership; client-supplied snapshot identities are never trusted.
use super::host::FileDiff;
use super::model::{CallStatus, RevertState, RevertTarget, SessionState};
use super::rewind::revert_event;
use super::{Inner, RevertPhase, Runtime, RuntimeError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_PREVIEW_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RewindSelection {
    pub message_id: String,
    pub target: RevertTarget,
    /// Exact Session Location selected by the owner, including for conversation-only rewind.
    pub expected_directory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RewindConfirmation {
    #[serde(flatten)]
    pub selection: RewindSelection,
    pub confirmation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RewindCodePreview {
    pub baseline: String,
    pub target: String,
    pub recorded: String,
    pub files: Vec<FileDiff>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RewindEffect {
    pub call_id: String,
    pub tool: String,
    pub status: CallStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RewindPreview {
    pub session_id: String,
    pub directory: String,
    pub sequence: i64,
    pub message_id: String,
    pub target: RevertTarget,
    pub active_tip: Option<String>,
    pub code: Option<RewindCodePreview>,
    /// Includes recovery calls retained outside the visible conversation branch.
    pub effects: Vec<RewindEffect>,
    pub warning: String,
    /// Fingerprint of the server-derived preview; not an authentication credential.
    pub confirmation: String,
}

fn stale() -> RuntimeError {
    RuntimeError::Conflict("Rewind preview changed; request and review a fresh preview".into())
}

fn validate(state: &SessionState, selection: &RewindSelection) -> Result<(), RuntimeError> {
    if state.revert.is_some() {
        return Err(RuntimeError::Invalid(
            "A revert is already staged; commit or clear it before a reviewed rewind".into(),
        ));
    }
    if state.pending(super::Delivery::Steer).next().is_some()
        || state.pending(super::Delivery::Queue).next().is_some()
    {
        return Err(RuntimeError::Busy(state.info.id.clone()));
    }
    if state.info.directory != selection.expected_directory {
        return Err(stale());
    }
    if !state
        .entries
        .iter()
        .any(|entry| matches!(entry, super::Entry::User { id, .. } if id == &selection.message_id))
    {
        return Err(RuntimeError::Invalid(
            "Rewind boundary is not an active user message".into(),
        ));
    }
    Ok(())
}

impl Inner {
    async fn build_rewind_preview(
        &self,
        state: &SessionState,
        selection: &RewindSelection,
    ) -> Result<RewindPreview, RuntimeError> {
        validate(state, selection)?;
        let code = if selection.target.code() {
            Some(
                self.rewind_code_preview(state, &selection.message_id)
                    .await?,
            )
        } else {
            None
        };
        let mut preview = RewindPreview {
            session_id: state.info.id.clone(),
            directory: state.info.directory.clone(),
            sequence: state.last_seq,
            message_id: selection.message_id.clone(),
            target: selection.target,
            active_tip: state.entries.last().map(|entry| entry.id().to_owned()),
            code,
            effects: state
                .calls
                .values()
                .map(|call| RewindEffect {
                    call_id: call.call_id.clone(),
                    tool: call.name.clone(),
                    status: call.status,
                })
                .collect(),
            warning: format!(
                "{} External effects and unresolved tool/native outcomes are not undone or acknowledged.",
                if selection.target == RevertTarget::Code {
                    "Only eligible local files are rewound; the conversation is unchanged."
                } else {
                    "Later messages leave the active path and remain on an inactive branch."
                }
            ),
            confirmation: String::new(),
        };
        let encoded = serde_json::to_vec(&preview)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
        if encoded.len().saturating_add(128) > MAX_PREVIEW_BYTES {
            return Err(RuntimeError::Invalid(
                "Rewind preview exceeds the 4 MiB limit".into(),
            ));
        }
        preview.confirmation = format!("{:x}", Sha256::digest(encoded));
        Ok(preview)
    }

    async fn rewind_code_preview(
        &self,
        state: &SessionState,
        message_id: &str,
    ) -> Result<RewindCodePreview, RuntimeError> {
        let unavailable = || {
            RuntimeError::Invalid(
                "Snapshots are unavailable for this Location; only the conversation can be rewound"
                    .into(),
            )
        };
        if !self.options.snapshots.reviewed_restore_available() {
            return Err(unavailable());
        }
        let recorded = state
            .revert
            .as_ref()
            .and_then(|r| r.applied.clone())
            .or_else(|| state.steps.iter().rev().find_map(|step| step.post.clone()))
            .ok_or_else(unavailable)?;
        // Hash the current tree again even on confirmation: metadata is not identity.
        let current = self
            .options
            .snapshots
            .track(&state.info.directory)
            .await
            .map_err(RuntimeError::Invalid)?
            .ok_or_else(unavailable)?;
        let target = state
            .steps
            .iter()
            .find(|step| step.user_message_id.as_deref() == Some(message_id))
            .and_then(|step| step.pre.clone())
            .ok_or_else(unavailable)?;
        let files = match self
            .options
            .snapshots
            .preview_restore(&state.info.directory, &target, &recorded, &current.tree)
            .await
        {
            Ok(files) => files,
            Err(super::RestoreError::Stale) => return Err(stale()),
            Err(super::RestoreError::Conflict(paths)) => {
                return Err(RuntimeError::RewindConflict(paths));
            }
            Err(super::RestoreError::Failed(message)) => {
                return Err(RuntimeError::Invalid(message));
            }
        };
        Ok(RewindCodePreview {
            baseline: current.tree,
            target,
            recorded,
            files,
            skipped: current.skipped,
        })
    }
}

impl Runtime {
    /// Build a review without restoring files, staging a revert, or changing the branch.
    pub async fn rewind_preview(
        &self,
        session_id: &str,
        selection: &RewindSelection,
    ) -> Result<RewindPreview, RuntimeError> {
        self.inner.ensure_idle(session_id)?;
        let handle = self.inner.handle(session_id).await?;
        self.inner
            .with_idle_location(&handle, async {
                self.ensure_no_background(session_id)?;
                let before = handle.state.lock().await.clone();
                let preview = self.inner.build_rewind_preview(&before, selection).await?;
                let current = handle.state.lock().await;
                self.inner.ensure_idle(session_id)?;
                if current.last_seq != before.last_seq || handle.is_compacting() {
                    return Err(stale());
                }
                Ok(preview)
            })
            .await
    }

    /// Revalidate before any restore. Hold the Session projection through the owned
    /// restore and event commit so a new prompt cannot change its branch mid-effect.
    pub async fn rewind_confirm(
        &self,
        session_id: &str,
        confirmation: &RewindConfirmation,
    ) -> Result<RevertState, RuntimeError> {
        if confirmation.confirmation.len() != 64 {
            return Err(stale());
        }
        self.inner.ensure_idle(session_id)?;
        let handle = self.inner.handle(session_id).await?;
        self.inner
            .with_idle_location(&handle, async {
                self.ensure_no_background(session_id)?;
                let before = handle.state.lock().await.clone();
                let preview = self
                    .inner
                    .build_rewind_preview(&before, &confirmation.selection)
                    .await?;
                if preview.confirmation != confirmation.confirmation {
                    return Err(stale());
                }
                let authority = self.capture_child_admission(session_id)?;
                let runtime = self.clone();
                let source = session_id.to_owned();
                let authorize = std::sync::Arc::new(move || {
                    authority
                        .verify(&runtime, &source)
                        .map_err(|error| error.to_string())
                });
                let mut state = handle.state.lock().await;
                self.inner.ensure_idle(session_id)?;
                self.ensure_no_background(session_id)?;
                validate(&state, &confirmation.selection)?;
                if state.last_seq != before.last_seq || handle.is_compacting() {
                    return Err(stale());
                }
                let (baseline, applied, diff) = if let Some(code) = &preview.code {
                    match self
                        .inner
                        .options
                        .snapshots
                        .restore_reviewed(
                            &state.info.directory,
                            &code.target,
                            &code.recorded,
                            &code.baseline,
                            authorize,
                        )
                        .await
                    {
                        Ok(_) => {}
                        Err(super::RestoreError::Stale) => return Err(stale()),
                        Err(super::RestoreError::Conflict(paths)) => {
                            return Err(RuntimeError::RewindConflict(paths));
                        }
                        Err(super::RestoreError::Failed(message)) => {
                            // The host may have partially restored files. Retain the
                            // native activity fence and existing backup evidence.
                            handle
                                .location_uncertain
                                .store(true, std::sync::atomic::Ordering::SeqCst);
                            return Err(RuntimeError::Invalid(message));
                        }
                    }
                    (
                        Some(code.baseline.clone()),
                        Some(code.target.clone()),
                        code.files.iter().map(|file| file.patch.as_str()).collect(),
                    )
                } else {
                    authorize().map_err(RuntimeError::Invalid)?;
                    (None, None, String::new())
                };
                let revert = RevertState {
                    message_id: preview.message_id,
                    target: preview.target,
                    baseline,
                    applied,
                    diff,
                };
                let committed = self.inner.commit_locked(
                    &mut state,
                    vec![
                        revert_event(session_id, &revert, RevertPhase::Stage),
                        revert_event(session_id, &revert, RevertPhase::Commit),
                    ],
                );
                if let Err(error) = committed {
                    if preview.code.is_some() {
                        handle
                            .location_uncertain
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    return Err(error);
                }
                Ok(revert)
            })
            .await
    }
}
