//! Explicit read-only review followed by fingerprint-bound storage recovery.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryRecoveryReview {
    pub receipt: MemoryMutation,
    pub proposed_note: Option<MemoryDocument>,
    pub completed: bool,
    pub fingerprint: String,
    pub journal: MemoryJournalIdentity,
}

impl<'store> MemoryScope<'store> {
    /// Inspect validated recovery evidence without normalizing links or changing files.
    pub fn inspect_recovery(&mut self) -> Result<Option<MemoryRecoveryReview>, MemoryStorageError> {
        let Some(pending) = self.recovery_prepared()? else {
            return Ok(None);
        };
        let fingerprint = pending.review_fingerprint()?;
        pending.verify_catalog()?;
        pending.verify_desired()?;
        pending.verify_slots()?;
        let marker = optional_bytes(&pending.dir, "completed", NOTE_LIMIT)?;
        if marker
            .as_deref()
            .is_some_and(|bytes| bytes != b"committed\n")
        {
            return Err(MemoryStorageError::RecoveryRequired);
        }
        Ok(Some(MemoryRecoveryReview {
            receipt: MemoryMutation {
                id: pending.intent.id.clone(),
                name: pending.intent.name.clone(),
                deleted: pending.intent.after_note.is_none(),
            },
            proposed_note: pending.desired_note()?,
            completed: marker.is_some(),
            fingerprint,
            journal: pending.journal_identity()?,
        }))
    }

    /// A review fingerprint grants no database ownership or durable publication authority.
    pub fn recover_reviewed(
        &mut self,
        fingerprint: &str,
    ) -> Result<MemoryMutation, MemoryStorageError> {
        self.recover_reviewed_with_acknowledgement(fingerprint, |_| Ok(()))
    }

    pub fn recover_reviewed_with_acknowledgement(
        &mut self,
        fingerprint: &str,
        acknowledge: impl FnOnce(&MemoryMutation) -> Result<(), MemoryStorageError>,
    ) -> Result<MemoryMutation, MemoryStorageError> {
        let pending = self
            .recovery_prepared()?
            .ok_or(MemoryStorageError::ReviewConflict)?;
        // Compare before commit's normalization, installation or acknowledgement effects.
        if pending.review_fingerprint()? != fingerprint {
            return Err(MemoryStorageError::ReviewConflict);
        }
        pending.commit_with_acknowledgement(acknowledge)
    }

    fn recovery_prepared(
        &mut self,
    ) -> Result<Option<PreparedMemory<'_, 'store>>, MemoryStorageError> {
        mutation_platform()?;
        verify_private_directory(&self.store.dir)?;
        let dir = match self.store.dir.open_dir_nofollow(TRANSACTION) {
            Ok(dir) => dir,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        verify_private_directory(&dir)?;
        verify_history(&self.store.dir)?;
        let json = optional_bytes(&dir, "intent.json", INTENT_LIMIT)?
            .ok_or(MemoryStorageError::RecoveryRequired)?;
        let intent: Intent =
            serde_json::from_slice(&json).map_err(|_| MemoryStorageError::RecoveryRequired)?;
        validate_intent(&intent)?;
        Ok(Some(PreparedMemory {
            scope: self,
            dir,
            intent,
        }))
    }
}

impl PreparedMemory<'_, '_> {
    fn review_fingerprint(&self) -> Result<String, MemoryStorageError> {
        let mut files = BTreeMap::new();
        for (name, limit) in [
            ("intent.json", INTENT_LIMIT),
            ("note.before", NOTE_LIMIT),
            ("note.after", NOTE_LIMIT),
            ("index.before", INDEX_LIMIT),
            ("index.after", INDEX_LIMIT),
            ("completed", NOTE_LIMIT),
        ] {
            files.insert(name, reviewed_file(&self.dir, name, limit)?);
        }
        let snapshot = serde_json::to_vec(&(
            "memory-recovery-review-v1",
            &self.scope.store.path,
            directory_identity(&self.scope.store.dir)?,
            directory_identity(&self.dir)?,
            files,
            reviewed_file(
                &self.scope.store.dir,
                &format!("{}.md", self.intent.name),
                NOTE_LIMIT,
            )?,
            reviewed_file(&self.scope.store.dir, "MEMORY.md", INDEX_LIMIT)?,
            self.scope.fingerprints(&self.intent.name)?,
        ))
        .map_err(|_| MemoryStorageError::RecoveryRequired)?;
        Ok(hash(&snapshot))
    }
}

#[derive(Serialize)]
pub(super) struct ReviewedFile {
    pub(super) digest: String,
    identity: Option<(u64, u64)>,
}

pub(super) fn reviewed_file(
    dir: &Dir,
    name: &str,
    limit: u64,
) -> Result<Option<ReviewedFile>, MemoryStorageError> {
    let Some(bytes) = optional_bytes(dir, name, limit)? else {
        return Ok(None);
    };
    let identity = file_identity(&dir.symlink_metadata(name)?);
    Ok(Some(ReviewedFile {
        digest: hash(&bytes),
        identity,
    }))
}

pub(super) fn directory_identity(dir: &Dir) -> Result<Option<(u64, u64)>, MemoryStorageError> {
    Ok(file_identity(&dir.dir_metadata()?))
}

fn file_identity(metadata: &cap_std::fs::Metadata) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        Some((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::tests::note;
    use super::*;

    #[test]
    fn review_is_read_only_and_recovers_a_partial_install() {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        scope.write(&note("Before")).unwrap();
        let mut pending = scope.prepare_write(&note("After")).unwrap();
        let journal = pending.journal_identity().unwrap();
        pending.apply_note().unwrap();
        drop(pending);
        let review = scope.inspect_recovery().unwrap().unwrap();
        assert_eq!(review.journal, journal);
        assert_eq!(review.proposed_note.unwrap().body, "After");
        assert!(!review.completed);
        assert!(store.path().join(TRANSACTION).join("index.after").exists());
        let receipt = scope.recover_reviewed(&review.fingerprint).unwrap();
        assert_eq!(receipt, review.receipt);
        assert_eq!(scope.read("rule").unwrap().body, "After");
        assert!(scope.inspect_recovery().unwrap().is_none());
        assert!(matches!(
            scope.recover_reviewed(&review.fingerprint),
            Err(MemoryStorageError::ReviewConflict)
        ));
    }

    #[test]
    fn stale_review_preserves_external_target_and_staged_evidence() {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        scope.write(&note("Before")).unwrap();
        drop(scope.prepare_write(&note("After")).unwrap());
        let review = scope.inspect_recovery().unwrap().unwrap();
        std::fs::write(store.path().join("rule.md"), note("External edit")).unwrap();
        let result = scope.recover_reviewed_with_acknowledgement(&review.fingerprint, |_| {
            panic!("stale review acknowledged")
        });
        assert!(matches!(result, Err(MemoryStorageError::ReviewConflict)));
        assert!(
            std::fs::read_to_string(store.path().join("rule.md"))
                .unwrap()
                .contains("External edit")
        );
        assert!(store.path().join(TRANSACTION).join("note.after").exists());
        assert!(!store.path().join(TRANSACTION).join("note.before").exists());
    }

    #[test]
    fn stale_index_and_catalog_reviews_refuse_before_note_installation() {
        for target in ["MEMORY.md", "other.md"] {
            let data = tempfile::tempdir().unwrap();
            let store = MemoryStore::open(data.path(), "global").unwrap();
            let mut scope = store.claim().unwrap();
            scope.write(&note("Before")).unwrap();
            drop(scope.prepare_write(&note("After")).unwrap());
            let review = scope.inspect_recovery().unwrap().unwrap();
            let text = if target == "other.md" {
                note("New note").replace("name: rule", "name: other")
            } else {
                "User index\n".into()
            };
            create_file(&store.dir, "new-file", text.as_bytes()).unwrap();
            store.dir.rename("new-file", &store.dir, target).unwrap();
            assert!(matches!(
                scope.recover_reviewed(&review.fingerprint),
                Err(MemoryStorageError::ReviewConflict)
            ));
            assert!(
                std::fs::read_to_string(store.path().join("rule.md"))
                    .unwrap()
                    .contains("Before")
            );
            assert!(!store.path().join(TRANSACTION).join("note.before").exists());
        }
    }

    #[test]
    fn review_refuses_aliases_and_corrupt_evidence_without_normalization() {
        for alias in [false, true] {
            let data = tempfile::tempdir().unwrap();
            let store = MemoryStore::open(data.path(), "global").unwrap();
            let mut scope = store.claim().unwrap();
            let pending = scope.prepare_write(&note("After")).unwrap();
            if alias {
                pending
                    .dir
                    .hard_link("note.after", &pending.scope.store.dir, "rule.md")
                    .unwrap();
            } else {
                pending.dir.remove_file("index.after").unwrap();
                create_file(&pending.dir, "index.after", b"Corrupted index\n").unwrap();
            }
            drop(pending);
            assert!(scope.inspect_recovery().is_err());
            assert!(store.path().join(TRANSACTION).join("note.after").exists());
            assert!(!store.path().join(TRANSACTION).join("completed").exists());
            if alias {
                assert_eq!(store.dir.symlink_metadata("rule.md").unwrap().nlink(), 2);
            }
        }
    }

    #[test]
    fn review_cannot_be_reused_for_another_scope_or_replaced_inode() {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let other_data = tempfile::tempdir().unwrap();
        let other = MemoryStore::open(other_data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        drop(scope.prepare_write(&note("After")).unwrap());
        let review = scope.inspect_recovery().unwrap().unwrap();
        let mut other_scope = other.claim().unwrap();
        drop(other_scope.prepare_write(&note("After")).unwrap());
        assert!(matches!(
            other_scope.recover_reviewed(&review.fingerprint),
            Err(MemoryStorageError::ReviewConflict)
        ));
        let dir = store.dir.open_dir_nofollow(TRANSACTION).unwrap();
        let bytes = optional_bytes(&dir, "note.after", NOTE_LIMIT)
            .unwrap()
            .unwrap();
        create_file(&dir, "replacement", &bytes).unwrap();
        dir.rename("replacement", &dir, "note.after").unwrap();
        assert!(matches!(
            scope.recover_reviewed(&review.fingerprint),
            Err(MemoryStorageError::ReviewConflict)
        ));
        assert!(!store.path().join("rule.md").exists());
    }

    #[test]
    fn acknowledgement_failure_requires_a_new_completed_review() {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        drop(scope.prepare_write(&note("After")).unwrap());
        let review = scope.inspect_recovery().unwrap().unwrap();
        assert!(
            scope
                .recover_reviewed_with_acknowledgement(&review.fingerprint, |_| Err(
                    MemoryStorageError::RecoveryRequired
                ))
                .is_err()
        );
        assert!(matches!(
            scope.read("rule"),
            Err(MemoryStorageError::RecoveryRequired)
        ));
        assert!(matches!(
            scope.recover_reviewed(&review.fingerprint),
            Err(MemoryStorageError::ReviewConflict)
        ));
        let completed = scope.inspect_recovery().unwrap().unwrap();
        assert!(completed.completed);
        assert_eq!(completed.receipt, review.receipt);
        scope.recover_reviewed(&completed.fingerprint).unwrap();
        assert_eq!(scope.read("rule").unwrap().body, "After");
    }
}
