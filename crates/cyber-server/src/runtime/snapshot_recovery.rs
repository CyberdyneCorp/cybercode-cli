//! Owner review is read-only. Native acknowledgement requires the consuming owner transaction.
use super::{Runtime, RuntimeError, SnapshotRecoveryReview, SnapshotRecoverySelection, activity};
use rusqlite::OptionalExtension;

impl Runtime {
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
        // Use an independently owned typed scope to exercise acknowledged state.
        let mut settled = activity::Scope::reserve_snapshot(&runtime.inner, session.clone())
            .await
            .unwrap();
        let bound = origin(&runtime, &session, settled.id());
        settled.launch_snapshot(bound.clone()).unwrap();
        settled.settle().unwrap();
        drop(settled);
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
