//! Owner review is read-only. Native acknowledgement requires the consuming owner transaction.
use super::{Runtime, RuntimeError, SnapshotRecoveryReview, SnapshotRecoverySelection, activity};
use rusqlite::OptionalExtension;

impl Runtime {
    /// Reconcile only acknowledgement metadata for a durably committed request.
    /// Unknown/incomplete requests cannot use this path to acquire an acknowledgement.
    pub async fn snapshot_recovery_acknowledge(
        &self,
        session: &str,
        request_id: &str,
    ) -> Result<super::SnapshotRecoveryResult, RuntimeError> {
        let mut result = self
            .snapshot_recovery_lookup(session, request_id)
            .await?
            .ok_or_else(|| RuntimeError::Invalid("No retained recovery request".into()))?;
        if result.state != super::SnapshotRecoveryState::Acknowledged {
            return Err(RuntimeError::Conflict(
                "Only an exact committed recovery receipt can reconcile journal acknowledgement"
                    .into(),
            ));
        }
        let receipt = result
            .receipt
            .clone()
            .ok_or_else(|| RuntimeError::Corrupt("Committed recovery lacks its receipt".into()))?;
        let origin = result.resolution.clone().ok_or_else(|| {
            RuntimeError::Corrupt("Committed recovery lacks its native origin".into())
        })?;
        let handle = self.inner.handle(session).await?;
        let state = handle.state.lock().await;
        let directory = state.info.directory.clone();
        let worker = self
            .snapshot_acknowledgement_worker(&handle, &state, receipt.clone(), origin.clone())
            .await?;
        drop(state);
        self.inner
            .options
            .snapshots
            .acknowledge_recovery(directory, worker)
            .await
            .map_err(RuntimeError::Conflict)?;
        handle.acknowledge_snapshot_uncertainty(&receipt.original.activity_id);
        handle.acknowledge_snapshot_uncertainty(&origin.activity_id);
        result.journal_acknowledged = Some(true);
        Ok(result)
    }
    /// A repeated request is always lookup-only, even after a partial outcome or
    /// later owner edits. Reconciliation is a separate operation.
    pub async fn snapshot_recovery_confirm(
        &self,
        session: &str,
        confirmation: &super::SnapshotRecoveryConfirmation,
    ) -> Result<super::SnapshotRecoveryResult, RuntimeError> {
        validate_request(confirmation)?;
        if let Some(existing) = self
            .snapshot_recovery_lookup(session, &confirmation.request_id)
            .await?
        {
            if existing.request != *confirmation {
                return Err(RuntimeError::Conflict(
                    "Recovery request ID was reused with different content".into(),
                ));
            }
            return Ok(existing);
        }
        let review = self
            .snapshot_recovery_review(
                session,
                &SnapshotRecoverySelection {
                    journal_id: confirmation.journal_id.clone(),
                    action: confirmation.action,
                },
            )
            .await?;
        if review.confirmation.as_deref() != Some(&confirmation.confirmation) {
            return Err(RuntimeError::Conflict(
                "Recovery confirmation is stale or unavailable".into(),
            ));
        }
        let original = review
            .origin
            .ok_or_else(|| RuntimeError::Conflict("Recovery lacks exact native origin".into()))?;
        let handle = self.inner.handle(session).await?;
        let state = handle.state.lock().await;
        let directory = state.info.directory.clone();
        let worker = self
            .snapshot_recovery_worker(
                &handle,
                &state,
                super::snapshot_recovery_types::RecoveryRequest {
                    confirmation: confirmation.clone(),
                    original,
                    authority: None,
                },
            )
            .await?;
        drop(state);
        let completed = self
            .inner
            .options
            .snapshots
            .resolve_recovery(directory, confirmation.clone(), worker)
            .await
            .map_err(|error| {
                RuntimeError::Conflict(format!(
                    "Recovery retains its request and native evidence: {error:?}"
                ))
            })?;
        let state = handle.state.lock().await;
        completed.commit_recovery(&state)?;
        drop(state);
        let mut result = self
            .snapshot_recovery_lookup(session, &confirmation.request_id)
            .await?
            .ok_or_else(|| {
                RuntimeError::Corrupt("Committed recovery request disappeared".into())
            })?;
        result.journal_acknowledged = Some(true);
        Ok(result)
    }

    /// Durable request lookup never opens a native namespace or starts inference.
    pub async fn snapshot_recovery_lookup(
        &self,
        session: &str,
        request_id: &str,
    ) -> Result<Option<super::SnapshotRecoveryResult>, RuntimeError> {
        validate_request_id(request_id)?;
        self.session_info(session).await?;
        let created = self
            .inner
            .store
            .read_events(session, -1, 1)?
            .events
            .first()
            .ok_or_else(|| RuntimeError::Conflict("Recovery Session creation is missing".into()))?
            .id
            .clone();
        let id = request_owner(&created, request_id);
        let owner = id.clone();
        let data = self.inner.store.read(move |db| {
            Ok(db
                .query_row(
                    "SELECT type,data FROM event WHERE aggregate_id=?1 ORDER BY seq DESC LIMIT 1",
                    [owner],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?)
        })?;
        let Some((kind, data)) = data else {
            return Ok(None);
        };
        let record: activity::Record = serde_json::from_str(&data)
            .map_err(|error| RuntimeError::Corrupt(error.to_string()))?;
        let request = record.snapshot_request.ok_or_else(|| {
            RuntimeError::Conflict("Request ID belongs to another native operation".into())
        })?;
        if kind != activity::CHANGED
            || !record.snapshot_worker
            || record.binary_document.is_some()
            || record.session_id != session
            || request.original.session_id != session
            || request.original.session_created_id != created
            || request.confirmation.request_id != request_id
            || record
                .snapshot_origin
                .as_ref()
                .is_some_and(|origin| origin.activity_id != id)
        {
            return Err(RuntimeError::Conflict(
                "Recovery request owner differs from the current Session".into(),
            ));
        }
        let state = match (record.status.as_str(), &record.snapshot_receipt) {
            ("pending", None) => super::SnapshotRecoveryState::Pending,
            ("settled", None) if record.phase == "snapshot_preparation_cancelled" => {
                let cancelled = record
                    .snapshot_preparation_receipt
                    .as_ref()
                    .ok_or_else(|| {
                        RuntimeError::Corrupt("Missing preparation cancellation receipt".into())
                    })?;
                if self
                    .snapshot_preparation_lookup(session, &id, &cancelled.request_id)
                    .await?
                    .as_ref()
                    != Some(cancelled)
                {
                    return Err(RuntimeError::Corrupt(
                        "Preparation cancellation lacks its durable pair".into(),
                    ));
                }
                if cancelled.origin.is_some() {
                    super::SnapshotRecoveryState::CancelledBeforeEffects
                } else {
                    super::SnapshotRecoveryState::CancelledBeforeOrigin
                }
            }
            ("unknown", None) | ("settled", None) => super::SnapshotRecoveryState::Unknown,
            ("settled", Some(receipt)) if receipt.request_id == request_id => {
                super::SnapshotRecoveryState::Acknowledged
            }
            ("settled", Some(_)) => super::SnapshotRecoveryState::ResolvedByAnotherRequest,
            _ => {
                return Err(RuntimeError::Corrupt(
                    "Invalid recovery request settlement".into(),
                ));
            }
        };
        Ok(Some(super::SnapshotRecoveryResult {
            request: request.confirmation,
            original: request.original,
            resolution: record.snapshot_origin,
            state,
            receipt: record.snapshot_receipt,
            journal_acknowledged: None,
        }))
    }
    pub async fn snapshot_recovery_review(
        &self,
        session: &str,
        selection: &SnapshotRecoverySelection,
    ) -> Result<SnapshotRecoveryReview, RuntimeError> {
        let before = self.session_info(session).await?;
        let mut review = self
            .inner
            .options
            .snapshots
            .recovery_review(&before.directory, &selection.journal_id, selection.action)
            .await
            .map_err(RuntimeError::Invalid)?;
        if review.journal_id != selection.journal_id || review.action != selection.action {
            return Err(RuntimeError::Corrupt(
                "Snapshot host returned a different recovery selection".into(),
            ));
        }
        if let Some(origin) = &review.origin {
            let directory = std::fs::canonicalize(&before.directory)
                .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
            if origin.binding.directory != directory {
                return Err(RuntimeError::Conflict(
                    "Recovery Location differs from the current Session".into(),
                ));
            }
            let created = self
                .inner
                .store
                .read_events(session, -1, 1)?
                .events
                .first()
                .map(|event| event.id.clone());
            if origin.session_id != session
                || created.as_deref() != Some(&origin.session_created_id)
                || origin.worktree_id != before.worktree_id
            {
                return Err(RuntimeError::Conflict(
                    "Recovery journal belongs to a different Session/checkout".into(),
                ));
            }
            self.validate_recovery_activity(&mut review)?;
        }
        let after = self.session_info(session).await?;
        if after.directory != before.directory || after.worktree_id != before.worktree_id {
            return Err(RuntimeError::Conflict(
                "Session Location changed during recovery review".into(),
            ));
        }
        Ok(review)
    }

    fn validate_recovery_activity(
        &self,
        review: &mut SnapshotRecoveryReview,
    ) -> Result<(), RuntimeError> {
        let origin = review.origin.as_ref().expect("caller checked origin");
        if origin
            .binding
            .lock_identity
            .as_deref()
            .is_none_or(str::is_empty)
        {
            review.confirmation = None;
            review
                .conflicts
                .push("Legacy snapshot origin has no exact native lock proof".into());
            return Ok(());
        }
        let id = origin.activity_id.clone();
        let rows = self.inner.store.read(move |db| {
            let latest = db.query_row("SELECT data FROM event WHERE aggregate_id=?1 AND type='native.activity.changed.1' ORDER BY seq DESC LIMIT 1", [&id], |row| row.get::<_,String>(0)).optional()?;
            let launched = db.query_row("SELECT data FROM event WHERE aggregate_id=?1 AND seq=1 AND type='native.activity.changed.1'", [&id], |row| row.get::<_,String>(0)).optional()?;
            Ok((latest,launched))
        })?;
        let Some((latest, launched)) = rows.0.zip(rows.1) else {
            review.confirmation = None;
            review
                .conflicts
                .push("Exact native origin evidence is unavailable".into());
            return Ok(());
        };
        for data in [&latest, &launched] {
            let record: activity::Record = serde_json::from_str(data)
                .map_err(|error| RuntimeError::Corrupt(error.to_string()))?;
            if !record.snapshot_worker
                || record.binary_document.is_some()
                || record.session_id != origin.session_id
                || record.snapshot_origin.as_ref() != Some(origin)
            {
                return Err(RuntimeError::Conflict(
                    "Journal cannot acknowledge this native activity".into(),
                ));
            }
        }
        let record: activity::Record = serde_json::from_str(&latest)
            .map_err(|error| RuntimeError::Corrupt(error.to_string()))?;
        if !matches!(record.status.as_str(), "pending" | "unknown" | "settled") {
            return Err(RuntimeError::Corrupt(
                "Invalid snapshot native activity status".into(),
            ));
        }
        let active = self
            .inner
            .activities
            .lock()
            .map_err(|_| RuntimeError::Conflict("Activity registry unavailable".into()))?
            .contains_key(&origin.activity_id);
        if active || record.status == "settled" {
            review.confirmation = None;
            review.conflicts.push(
                if active {
                    "Original snapshot worker is still active"
                } else {
                    "Native activity is already acknowledged; reconcile its existing receipt"
                }
                .into(),
            );
        }
        Ok(())
    }
}

pub(super) fn validate_request_id(id: &str) -> Result<(), RuntimeError> {
    if id.is_empty() || id.len() > 256 || id.contains('\0') {
        return Err(RuntimeError::Invalid(
            "Recovery request ID must contain 1–256 bytes without NUL".into(),
        ));
    }
    Ok(())
}
pub(super) fn validate_request(
    request: &super::SnapshotRecoveryConfirmation,
) -> Result<(), RuntimeError> {
    validate_request_id(&request.request_id)?;
    if request.confirmation.len() != 64
        || !request
            .confirmation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !request.journal_id.starts_with("restore_")
        || request.journal_id.len() > 256
        || !request
            .journal_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(RuntimeError::Invalid(
            "Invalid recovery journal or fingerprint".into(),
        ));
    }
    Ok(())
}
pub(super) fn request_owner(created: &str, request: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"snapshot-recovery-request-v1");
    digest.update((created.len() as u64).to_be_bytes());
    digest.update(created.as_bytes());
    digest.update((request.len() as u64).to_be_bytes());
    digest.update(request.as_bytes());
    format!("op_snapshot_recovery_{:x}", digest.finalize())
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use crate::runtime::{SnapshotBinding, SnapshotOrigin, SnapshotRecoveryAction};

    fn origin(runtime: &Runtime, session: &str, activity: &str) -> SnapshotOrigin {
        SnapshotOrigin {
            version: 1,
            session_id: session.into(),
            session_created_id: runtime
                .inner
                .store
                .read_events(session, -1, 1)
                .unwrap()
                .events[0]
                .id
                .clone(),
            activity_id: activity.into(),
            worktree_id: None,
            binding: SnapshotBinding {
                directory: std::env::temp_dir(),
                root_identity: "instrumented".into(),
                namespace_identity: "instrumented".into(),
                journal_id: "restore_fixture".into(),
                journal_identity: "instrumented".into(),
                lock_identity: Some("instrumented_lock".into()),
            },
        }
    }
    fn review(origin: SnapshotOrigin) -> SnapshotRecoveryReview {
        SnapshotRecoveryReview {
            journal_id: "restore_fixture".into(),
            action: SnapshotRecoveryAction::Keep,
            origin: Some(origin),
            filesystem_complete: false,
            acknowledged: false,
            confirmation: Some("a".repeat(64)),
            paths: vec![],
            exclusions: None,
            recorded_created_parents: None,
            conflicts: vec![],
        }
    }
    #[tokio::test]
    async fn exact_origin_review_distinguishes_live_unknown_and_settled_without_writing() {
        let (_temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let mut scope = activity::Scope::reserve_snapshot(&runtime.inner, session.clone())
            .await
            .unwrap();
        let bound = origin(&runtime, &session, scope.id());
        scope.launch_snapshot(bound.clone()).unwrap();
        let sequence = runtime.inner.store.aggregate_seq(scope.id()).unwrap();
        let mut live = review(bound.clone());
        runtime.validate_recovery_activity(&mut live).unwrap();
        assert!(live.confirmation.is_none());
        assert!(live.conflicts[0].contains("still active"));
        assert_eq!(
            runtime.inner.store.aggregate_seq(scope.id()).unwrap(),
            sequence
        );
        drop(scope);
        let sequence = runtime
            .inner
            .store
            .aggregate_seq(&bound.activity_id)
            .unwrap();
        let mut unknown = review(bound.clone());
        runtime.validate_recovery_activity(&mut unknown).unwrap();
        assert!(unknown.confirmation.is_some());
        assert!(unknown.conflicts.is_empty());
        assert_eq!(
            runtime
                .inner
                .store
                .aggregate_seq(&bound.activity_id)
                .unwrap(),
            sequence
        );
        // Model an independently committed acknowledgement of this exact owner;
        // ordinary admission must not create another scope through its fence.
        let mut acknowledged_record = runtime
            .inner
            .store
            .read_events(&bound.activity_id, -1, 10)
            .unwrap()
            .events
            .last()
            .unwrap()
            .data
            .clone();
        acknowledged_record["status"] = "settled".into();
        runtime
            .inner
            .store
            .append(
                &bound.activity_id,
                cyber_store::Expected::Seq(sequence.unwrap()),
                vec![cyber_store::NewEvent::new(
                    activity::CHANGED,
                    acknowledged_record,
                )],
            )
            .unwrap();
        let mut acknowledged = review(bound);
        runtime
            .validate_recovery_activity(&mut acknowledged)
            .unwrap();
        assert!(acknowledged.confirmation.is_none());
        assert!(acknowledged.conflicts[0].contains("already acknowledged"));
        runtime.shutdown().await;
    }
    #[tokio::test]
    async fn journal_cannot_relabel_a_generic_activity_or_replace_a_typed_origin() {
        let (_temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let handle = runtime.inner.handle(&session).await.unwrap();
        let mut scope = activity::Scope::reserve(&runtime.inner, &handle)
            .await
            .unwrap();
        scope.launch().unwrap();
        let before = runtime.inner.store.aggregate_seq(scope.id()).unwrap();
        assert!(
            runtime
                .validate_recovery_activity(&mut review(origin(&runtime, &session, scope.id())))
                .is_err()
        );
        assert_eq!(
            runtime.inner.store.aggregate_seq(scope.id()).unwrap(),
            before
        );
        scope.settle().unwrap();
        drop(scope);
        let mut typed = activity::Scope::reserve_snapshot(&runtime.inner, session.clone())
            .await
            .unwrap();
        let mut bound = origin(&runtime, &session, typed.id());
        typed.launch_snapshot(bound.clone()).unwrap();
        bound.binding.journal_identity = "foreign".into();
        assert!(
            runtime
                .validate_recovery_activity(&mut review(bound))
                .is_err()
        );
        drop(typed);
        runtime.shutdown().await;
    }
    #[tokio::test]
    async fn legacy_origin_without_exact_lock_proof_stays_inspectable_and_fenced() {
        let (_temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let mut scope = activity::Scope::reserve_snapshot(&runtime.inner, session.clone())
            .await
            .unwrap();
        let mut bound = origin(&runtime, &session, scope.id());
        bound.binding.lock_identity = None;
        scope.launch_snapshot(bound.clone()).unwrap();
        drop(scope);
        let sequence = runtime
            .inner
            .store
            .aggregate_seq(&bound.activity_id)
            .unwrap();
        let mut legacy = review(bound.clone());
        runtime.validate_recovery_activity(&mut legacy).unwrap();
        assert!(legacy.confirmation.is_none());
        assert!(
            legacy
                .conflicts
                .iter()
                .any(|conflict| conflict.contains("exact native lock proof"))
        );
        assert_eq!(
            runtime
                .inner
                .store
                .aggregate_seq(&bound.activity_id)
                .unwrap(),
            sequence
        );
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn missing_native_evidence_disables_confirmation_without_creating_activity() {
        let (_temp, runtime, session, _, _) = super::super::binary_documents_tests::fixture().await;
        let mut missing = review(origin(&runtime, &session, "op_missing"));
        runtime.validate_recovery_activity(&mut missing).unwrap();
        assert!(missing.confirmation.is_none());
        assert!(!missing.conflicts.is_empty());
        assert_eq!(
            runtime.inner.store.aggregate_seq("op_missing").unwrap(),
            None
        );
        runtime.shutdown().await;
    }
}
