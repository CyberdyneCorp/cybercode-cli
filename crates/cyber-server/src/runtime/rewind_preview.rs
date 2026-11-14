//! Non-mutating owner previews. Confirmation recomputes the complete preview under
//! Location ownership; client-supplied snapshot identities are never trusted.
use super::host::FileDiff;
use super::model::{CallStatus, RevertState, RevertTarget, SessionState};
use super::rewind::revert_event;
use super::{Inner, RevertPhase, Runtime, RuntimeError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_PREVIEW_BYTES: usize = 4 * 1024 * 1024;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RewindAction {
    #[default]
    Commit,
    Stage,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RewindSelection {
    #[serde(default)]
    pub action: RewindAction,
    pub message_id: String,
    pub target: RevertTarget,
    /// Exact Session Location selected by the owner, including for conversation-only rewind.
    pub expected_directory: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
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
    pub action: RewindAction,
    pub staged: Option<RevertState>,
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
    validate_stage(state, selection)?;
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

fn validate_stage(state: &SessionState, selection: &RewindSelection) -> Result<(), RuntimeError> {
    match (&state.revert, selection.action) {
        (Some(_), RewindAction::Commit) => Err(RuntimeError::Invalid(
            "A revert is already staged; commit or clear it before a reviewed rewind".into(),
        )),
        (None, RewindAction::Clear) => Err(RuntimeError::Invalid("No revert is staged".into())),
        (Some(staged), action)
            if staged.target != selection.target
                || (action == RewindAction::Clear && staged.message_id != selection.message_id) =>
        {
            Err(stale())
        }
        _ => Ok(()),
    }
}

fn disclosure(action: RewindAction, target: RevertTarget) -> String {
    let local = match action {
        RewindAction::Stage => {
            "Files are restored when selected; the conversation boundary is staged until the next prompt, shell command or compaction. Clear reverses the stage while preserving later owner edits."
        }
        RewindAction::Clear => {
            "Restore the pre-stage baseline where selected and clear the staged boundary; the active conversation is unchanged."
        }
        RewindAction::Commit if target == RevertTarget::Code => {
            "Only eligible local files are rewound; the conversation is unchanged."
        }
        RewindAction::Commit => {
            "Later messages leave the active path and remain on an inactive branch."
        }
    };
    format!(
        "{local} External effects and unresolved tool/native outcomes are not undone or acknowledged."
    )
}

fn staged_paths(preview: &RewindPreview) -> Result<Option<Vec<String>>, RuntimeError> {
    if preview.action != RewindAction::Stage || !preview.target.code() {
        return Ok(None);
    }
    let mut paths: std::collections::BTreeSet<String> = preview
        .staged
        .as_ref()
        .and_then(|r| r.paths.clone())
        .unwrap_or_default()
        .into_iter()
        .collect();
    if let Some(code) = &preview.code {
        paths.extend(code.files.iter().map(|file| file.file.clone()));
    }
    if paths.len() > 4096 {
        return Err(RuntimeError::Invalid(
            "Reviewed undo exceeds 4096 changed paths".into(),
        ));
    }
    Ok(Some(paths.into_iter().collect()))
}

impl Inner {
    async fn build_rewind_preview(
        &self,
        state: &SessionState,
        selection: &RewindSelection,
    ) -> Result<RewindPreview, RuntimeError> {
        validate(state, selection)?;
        let code = if selection.target.code() {
            Some(self.rewind_code_preview(state, selection).await?)
        } else {
            None
        };
        let mut preview = RewindPreview {
            action: selection.action,
            staged: state.revert.clone(),
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
            warning: disclosure(selection.action, selection.target),
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
        selection: &RewindSelection,
    ) -> Result<RewindCodePreview, RuntimeError> {
        let unavailable = || {
            RuntimeError::Invalid(self.options.snapshots.unavailable_reason(&state.info.directory).unwrap_or_else(||
                "Snapshots are unavailable for this Location; only the conversation can be rewound".into()))
        };
        if !self.options.snapshots.reviewed_restore_available()
            || (selection.action != RewindAction::Commit
                && !self.options.snapshots.reviewed_staging_available())
        {
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
        let target = if selection.action == RewindAction::Clear {
            state
                .revert
                .as_ref()
                .and_then(|r| r.baseline.clone())
                .ok_or_else(unavailable)?
        } else {
            state
                .steps
                .iter()
                .find(|step| step.user_message_id.as_deref() == Some(&selection.message_id))
                .and_then(|step| step.pre.clone())
                .ok_or_else(unavailable)?
        };
        let planned = if selection.action == RewindAction::Clear {
            let paths = state
                .revert
                .as_ref()
                .and_then(|r| r.paths.clone())
                .ok_or_else(|| RuntimeError::Invalid(
                    "Legacy file stage lacks reviewed path ownership; use its original clear/commit flow".into(),
                ))?;
            self.options
                .snapshots
                .preview_restore_paths(
                    &state.info.directory,
                    &target,
                    &recorded,
                    &current.tree,
                    paths,
                )
                .await
        } else {
            self.options
                .snapshots
                .preview_restore(&state.info.directory, &target, &recorded, &current.tree)
                .await
        };
        let files = match planned {
            Ok(files) => files,
            Err(super::RestoreError::Stale) => return Err(stale()),
            Err(super::RestoreError::Conflict(paths)) => {
                return Err(RuntimeError::RewindConflict(paths));
            }
            Err(super::RestoreError::Failed(message)) => {
                return Err(RuntimeError::Invalid(message));
            }
        };
        let mut skipped = current.skipped;
        skipped.extend(
            self.options
                .snapshots
                .exclusions(
                    &state.info.directory,
                    vec![target.clone(), recorded.clone()],
                )
                .await
                .map_err(RuntimeError::Invalid)?,
        );
        skipped.sort();
        skipped.dedup();
        Ok(RewindCodePreview {
            baseline: current.tree,
            target,
            recorded,
            files,
            skipped,
        })
    }
}

impl Runtime {
    /// Read-only owner inspection; never settles evidence, restores files or starts work.
    pub async fn snapshot_recovery(
        &self,
        session_id: &str,
    ) -> Result<Vec<super::SnapshotRecovery>, RuntimeError> {
        use rusqlite::OptionalExtension;
        let id = session_id.to_owned();
        // Inspection must remain available while restore holds the live projection.
        let directory = self
            .inner
            .store
            .read(move |db| {
                Ok(db
                    .query_row("SELECT directory FROM session WHERE id=?1", [id], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?)
            })?
            .ok_or_else(|| RuntimeError::SessionNotFound(session_id.into()))?;
        self.inner
            .options
            .snapshots
            .recovery(&directory)
            .await
            .map_err(RuntimeError::Invalid)
    }
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

    /// Revalidate before restore. Owned workers fence Session revisions and retain
    /// their resource claims through filesystem work and the consuming event commit.
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
        let directory = handle.state.lock().await.info.directory.clone();
        let owned = confirmation.selection.target != RevertTarget::Conversation
            && self
                .inner
                .options
                .snapshots
                .owned_restore_available(&directory);
        if owned {
            self.confirm_rewind(&handle, confirmation, true).await
        } else {
            self.inner
                .with_idle_location(&handle, self.confirm_rewind(&handle, confirmation, false))
                .await
        }
    }

    async fn restore_reviewed_code<'a>(
        &self,
        handle: &'a std::sync::Arc<super::Handle>,
        state: tokio::sync::MutexGuard<'a, SessionState>,
        preview: &RewindPreview,
        authorize: std::sync::Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
        owned: bool,
    ) -> Result<
        (
            tokio::sync::MutexGuard<'a, SessionState>,
            Option<super::SnapshotCompletion>,
        ),
        RuntimeError,
    > {
        let code = preview.code.as_ref().ok_or_else(stale)?;
        let paths = if preview.action == RewindAction::Clear {
            Some(
                state
                    .revert
                    .as_ref()
                    .and_then(|r| r.paths.clone())
                    .ok_or_else(stale)?,
            )
        } else {
            None
        };
        if owned {
            let request = super::SnapshotRestoreRequest {
                directory: state.info.directory.clone(),
                target: code.target.clone(),
                recorded: code.recorded.clone(),
                expected: code.baseline.clone(),
                paths,
            };
            // Native checkout admission resolves policy through the runtime and
            // may read this Session. Keep an immutable revision, not its mutex,
            // across that host call; the worker fences stale revisions.
            let captured = state.clone();
            drop(state);
            let worker = self.snapshot_worker(handle, &captured).await?;
            let completed = self
                .inner
                .options
                .snapshots
                .restore_owned(request, worker)
                .await
                .map_err(restore_failure)?;
            return Ok((handle.state.lock().await, Some(completed)));
        }
        let restored = if let Some(paths) = paths {
            self.inner
                .options
                .snapshots
                .restore_reviewed_paths(
                    &state.info.directory,
                    &code.target,
                    &code.recorded,
                    &code.baseline,
                    authorize,
                    paths,
                )
                .await
        } else {
            self.inner
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
        };
        if matches!(restored, Err(super::RestoreError::Failed(_))) {
            handle
                .location_uncertain
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        restored.map_err(restore_failure)?;
        Ok((state, None))
    }

    async fn confirm_rewind(
        &self,
        handle: &std::sync::Arc<super::Handle>,
        confirmation: &RewindConfirmation,
        owned: bool,
    ) -> Result<RevertState, RuntimeError> {
        let session_id = handle.state.lock().await.info.id.clone();
        let session_id = session_id.as_str();
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
        let paths = staged_paths(&preview)?;
        let staged_diff = if preview.action == RewindAction::Stage {
            if let Some(code) = &preview.code {
                let baseline = state
                    .revert
                    .as_ref()
                    .and_then(|r| r.baseline.as_deref())
                    .unwrap_or(&code.baseline);
                Some(
                    self.inner
                        .preview_diff(&state.info.directory, baseline, &code.target)
                        .await,
                )
            } else {
                None
            }
        } else {
            None
        };
        let mut completion = None;
        let (baseline, applied, diff) = if let Some(code) = &preview.code {
            let (restored_state, completed) = self
                .restore_reviewed_code(handle, state, &preview, authorize, owned)
                .await?;
            state = restored_state;
            completion = completed;
            let applied = code.target.clone();
            (
                state
                    .revert
                    .as_ref()
                    .and_then(|r| r.baseline.clone())
                    .or_else(|| Some(code.baseline.clone())),
                Some(applied),
                staged_diff
                    .unwrap_or_else(|| code.files.iter().map(|file| file.patch.as_str()).collect()),
            )
        } else {
            authorize().map_err(RuntimeError::Invalid)?;
            (None, None, String::new())
        };
        let revert = RevertState {
            paths,
            message_id: preview.message_id,
            target: preview.target,
            baseline,
            applied,
            diff,
        };
        let events = match preview.action {
            RewindAction::Commit => vec![
                revert_event(session_id, &revert, RevertPhase::Stage),
                revert_event(session_id, &revert, RevertPhase::Commit),
            ],
            RewindAction::Stage => {
                vec![revert_event(session_id, &revert, RevertPhase::Stage)]
            }
            RewindAction::Clear => {
                vec![revert_event(session_id, &revert, RevertPhase::Clear)]
            }
        };
        let owned_commit = completion.is_some();
        let committed = if let Some(completed) = completion {
            completed.commit(&mut state, events)
        } else {
            self.inner.commit_locked(&mut state, events).map(drop)
        };
        if let Err(error) = committed {
            if preview.code.is_some() && !owned_commit {
                handle
                    .location_uncertain
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            return Err(error);
        }
        Ok(revert)
    }
}

fn restore_failure(error: super::RestoreError) -> RuntimeError {
    match error {
        super::RestoreError::Stale => stale(),
        super::RestoreError::Conflict(paths) => RuntimeError::RewindConflict(paths),
        super::RestoreError::Failed(message) => RuntimeError::Invalid(message),
    }
}
