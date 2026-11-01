//! Restore is a separately reviewed native operation. It does not use Session
//! history or silently replace an owner's unsaved draft.
use super::*;
use crate::binary::draft::{ChangeRange, encode_hex};

impl Editor {
    pub fn restore_review(&self, cancelled: impl Fn() -> bool) -> Result<SaveReview, Error> {
        self.available()?;
        let current = self
            .draft
            .review(&self.draft.revision(), 0, 128, &cancelled)?;
        if current.range_count != 0 {
            return Err(Error::Changed);
        }
        let last = self.record.last_save.as_ref().ok_or(Error::Invalid)?;
        let preimage = last.preimage.open(&self.store)?;
        let changed = preimage.sha256() != self.draft.base().sha256();
        let original_length = self.draft.length();
        let proposed_length = Offset(preimage.length());
        let ranges = if changed {
            vec![ChangeRange {
                original_offset: Offset(0),
                original_length,
                draft_offset: Offset(0),
                draft_length: proposed_length,
                before_sha256: current.base_sha256.clone(),
                after_sha256: preimage.sha256().into(),
                before_hex: encode_hex(&self.draft.read(0, original_length.0.min(256) as usize)?),
                after_hex: encode_hex(&preimage.read(0, preimage.length().min(256) as usize)?),
                preview_truncated: original_length.0 > 256 || preimage.length() > 256,
            }]
        } else {
            vec![]
        };
        let difference = signature(&(
            "restore-diff-v1",
            &current.revision,
            &current.base_sha256,
            preimage.sha256(),
            &last.operation,
        ))?;
        let effects = format!(
            "Restore Last Save replaces the current acknowledged content with its retained preimage. {EFFECTS}"
        );
        let fingerprint = signature(&(
            "restore-review-v1",
            &difference,
            &self.record.target,
            &effects,
        ))?;
        let meta = &self.record.target.metadata;
        Ok(SaveReview {
            draft: Review {
                revision: current.revision,
                base_sha256: current.base_sha256,
                proposed_sha256: preimage.sha256().into(),
                base_length: original_length,
                proposed_length,
                original_bytes: Offset(if changed { original_length.0 } else { 0 }),
                proposed_bytes: Offset(if changed { preimage.length() } else { 0 }),
                range_count: ranges.len(),
                ranges,
                next_range: None,
                fingerprint: difference,
            },
            permissions: meta.mode,
            owner_uid: meta.uid,
            owner_gid: meta.gid,
            metadata_sha256: meta.evidence.clone(),
            effects,
            fingerprint,
        })
    }
    pub fn prepare_restore(
        &mut self,
        id: &str,
        review: &str,
        cancelled: impl Fn() -> bool,
    ) -> Result<SavePreparation, Error> {
        let payload = signature(&("restore-v1", review))?;
        if let Some(prior) = self.prior(id, &payload)? {
            return Ok(SavePreparation::Existing(prior));
        }
        self.available()?;
        let current = self.restore_review(&cancelled)?;
        if current.fingerprint != review {
            return Err(Error::Changed);
        }
        let target = self.record.target.clone();
        let (_parent, source) = match target.validate() {
            Ok(source) => source,
            Err(Error::Changed | Error::File) => {
                return self
                    .conflict(id, "restore", payload, "source_binding_changed")
                    .map(SavePreparation::Existing);
            }
            Err(error) => return Err(error),
        };
        let last = self.record.last_save.as_ref().ok_or(Error::Invalid)?;
        if native::content(&source, &cancelled)? != last.installed_sha256 {
            return self
                .conflict(id, "restore", payload, "source_content_changed")
                .map(SavePreparation::Existing);
        }
        let base = last.preimage.open(&self.store)?;
        let after_draft = Draft::new(base)?;
        let (stage_name, mut stage) = self.store.stage(after_draft.length().0)?;
        if after_draft.write_to(&mut stage, &cancelled)? != current.draft.proposed_sha256 {
            return Err(Error::Changed);
        }
        native::copy_metadata(&source, &stage)?;
        native::sync(&stage)?;
        self.store.sync()?;
        let after = Target {
            root: target.root.clone(),
            relative: target.relative.clone(),
            parents: target.parents.clone(),
            file: native::id(&stage)?,
            length: after_draft.length(),
            metadata: native::metadata(&stage)?,
            observed: String::new(),
        };
        let operation = Operation {
            id: id.into(),
            kind: "restore".into(),
            payload_sha256: payload,
            outcome: Outcome::Unknown,
            revision: after_draft.revision(),
            file_effect: FileEffect::Unknown,
            reason: None,
            notification_pending: false,
        };
        let intent = Intent {
            operation,
            prior: target,
            after,
            new_base: BaseRef::of(after_draft.base()),
            after_draft: after_draft.snapshot(),
            stage: stage_name,
        };
        Ok(SavePreparation::Ready(Box::new(PreparedSave {
            intent,
            after: after_draft,
            expected_revision: self.draft.revision(),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::{
        draft::{PatchBatch, Splice},
        storage::Store,
    };
    fn saved() -> (tempfile::TempDir, Editor, Owner) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("data")).unwrap();
        std::fs::create_dir(root.path().join("project")).unwrap();
        let owner = Owner {
            session_id: "ses_restore".into(),
            directory: root.path().join("project").canonicalize().unwrap(),
        };
        std::fs::write(owner.directory.join("bytes"), [0, 255, 128, 27]).unwrap();
        let mut doc = Document::open(&owner.directory, "bytes").unwrap();
        let mut editor = Editor::create(
            Store::open(&root.path().join("data"), "bin_restore").unwrap(),
            owner.clone(),
            &mut doc,
            || false,
        )
        .unwrap();
        editor
            .patch(
                "patch_one",
                &PatchBatch {
                    revision: editor.draft().revision(),
                    splices: vec![Splice {
                        offset: Offset(1),
                        delete_length: Offset(2),
                        expected_hex: "ff80".into(),
                        insert_hex: "61626364".into(),
                    }],
                },
            )
            .unwrap();
        let review = editor.review(0, 128, || false).unwrap();
        let SavePreparation::Ready(ready) = editor
            .prepare_save("save_one", &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("save preparation")
        };
        assert_eq!(
            editor.commit_save(*ready).unwrap().outcome,
            Outcome::Acknowledged
        );
        (root, editor, owner)
    }
    #[test]
    fn retained_binary_preimage_restores_after_restart_with_a_fresh_review_and_stable_receipt() {
        let (root, editor, owner) = saved();
        assert_eq!(
            std::fs::read(owner.directory.join("bytes")).unwrap(),
            [0, b'a', b'b', b'c', b'd', 27]
        );
        drop(editor);
        let mut editor = Editor::resume(
            Store::open(&root.path().join("data"), "bin_restore").unwrap(),
            &owner,
        )
        .unwrap();
        let review = editor.restore_review(|| false).unwrap();
        assert_eq!(review.draft.base_length.0, 6);
        assert_eq!(review.draft.proposed_length.0, 4);
        let SavePreparation::Ready(ready) = editor
            .prepare_restore("restore_one", &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("restore preparation")
        };
        let restored = editor.commit_save(*ready).unwrap();
        assert_eq!(restored.outcome, Outcome::Acknowledged);
        assert_eq!(restored.kind, "restore");
        assert_eq!(
            std::fs::read(owner.directory.join("bytes")).unwrap(),
            [0, 255, 128, 27]
        );
        let identity = editor.target().file.clone();
        let SavePreparation::Existing(receipt) = editor
            .prepare_restore("restore_one", &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("retry repeated restore")
        };
        assert_eq!(receipt.revision, restored.revision);
        assert_eq!(editor.target().file, identity);
    }
    #[test]
    fn restore_never_discards_a_dirty_draft_or_overwrites_external_bytes() {
        let (_root, mut editor, owner) = saved();
        let review = editor.restore_review(|| false).unwrap();
        std::fs::write(owner.directory.join("bytes"), b"external").unwrap();
        let SavePreparation::Existing(op) = editor
            .prepare_restore("restore_conflict", &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("external bytes accepted")
        };
        assert_eq!(op.outcome, Outcome::Conflict);
        assert_eq!(
            std::fs::read(owner.directory.join("bytes")).unwrap(),
            b"external"
        );
        editor
            .patch(
                "patch_dirty",
                &PatchBatch {
                    revision: editor.draft().revision(),
                    splices: vec![Splice {
                        offset: Offset(0),
                        delete_length: Offset(1),
                        expected_hex: "00".into(),
                        insert_hex: "ff".into(),
                    }],
                },
            )
            .unwrap();
        assert!(editor.restore_review(|| false).is_err());
        assert_eq!(editor.draft().read(0, 1).unwrap(), [255]);
    }
}
