//! Immutable acknowledged source versions. Mappings contain offsets only: cancelled
//! or superseded draft commands never enter the shared saved-source namespace.
use super::*;
use crate::binary::{draft::Review, receipts::Version};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Evidence {
    version: u32,
    pub info: Version,
    pub operation: Operation,
    pub(super) owner: Owner,
    before: BaseRef,
    after: BaseRef,
    pub target: Target,
    revision: String,
    ranges: Vec<(u64, u64, u64, u64)>,
}
impl Evidence {
    pub fn prepared(
        editor: &Editor,
        operation: &Operation,
        review: &Review,
        target: Target,
        after: &Draft<Base>,
        ranges: Vec<(u64, u64, u64, u64)>,
    ) -> Result<Self, Error> {
        if ranges.len() > super::super::draft::MAX_HISTORY * super::super::draft::MAX_SPLICES {
            return Err(Error::Limit);
        }
        let document = editor.store.document_id();
        let version_id = format!(
            "ver_{}",
            signature(&(
                "binary-version-v1",
                document,
                &operation.id,
                &operation.payload_sha256
            ))?
        );
        Ok(Self {
            version: 1,
            info: Version {
                version_id,
                document_id: document.into(),
                operation_id: operation.id.clone(),
                kind: operation.kind.clone(),
                path: crate::vcs::slash_path(&target.relative),
                before_sha256: review.base_sha256.clone(),
                after_sha256: review.proposed_sha256.clone(),
                before_length: review.base_length,
                after_length: review.proposed_length,
                original_bytes: review.original_bytes,
                proposed_bytes: review.proposed_bytes,
                range_count: review.range_count,
                created_ms: chrono::Utc::now().timestamp_millis(),
            },
            operation: operation.clone(),
            owner: editor.record.owner.clone(),
            before: editor.record.base.clone(),
            after: BaseRef::of(after.base()),
            target,
            revision: review.revision.clone(),
            ranges,
        })
    }
    fn validate_common(&self, document: &str) -> Result<(), Error> {
        let operation = &self.operation;
        let expected = format!(
            "ver_{}",
            signature(&(
                "binary-version-v1",
                document,
                &operation.id,
                &operation.payload_sha256
            ))?
        );
        if self.version != 1
            || self.info.version_id != expected
            || self.info.document_id != document
            || operation.outcome != Outcome::Acknowledged
            || operation.file_effect != FileEffect::Changed
            || operation.id != self.info.operation_id
            || self.info.kind != operation.kind
            || self.target.root != self.owner.root
            || self.info.path != crate::vcs::slash_path(&self.target.relative)
            || self.target.relative.as_os_str().is_empty()
            || self
                .target
                .relative
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(Error::Changed);
        }
        if self.info.before_sha256 != self.before.sha256
            || self.info.after_sha256 != self.after.sha256
            || self.info.before_length != self.before.length
            || self.info.after_length != self.after.length
            || !self.before.name.starts_with("base_")
            || !self.after.name.starts_with("base_")
            || self.info.range_count > self.ranges.len()
            || self.ranges.len()
                > crate::binary::draft::MAX_HISTORY * crate::binary::draft::MAX_SPLICES
        {
            return Err(Error::Changed);
        }
        let (mut before, mut after) = (0, 0);
        for &(at, removed, to, inserted) in &self.ranges {
            if at < before || to < after || at - before != to - after {
                return Err(Error::Invalid);
            }
            before = at
                .checked_add(removed)
                .filter(|end| *end <= self.before.length.0)
                .ok_or(Error::Invalid)?;
            after = to
                .checked_add(inserted)
                .filter(|end| *end <= self.after.length.0)
                .ok_or(Error::Invalid)?;
        }
        if self.before.length.0 - before != self.after.length.0 - after {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    fn validate(&self, editor: &Editor, operation: &Operation) -> Result<(), Error> {
        self.validate_common(editor.store.document_id())?;
        if self.owner != editor.record.owner
            || self.operation.id != operation.id
            || self.operation.payload_sha256 != operation.payload_sha256
            || self.after.name != editor.record.base.name
        {
            return Err(Error::Changed);
        }
        Ok(())
    }
}
impl Editor {
    /// Publish only after native acknowledgement is durable. The current journal
    /// retains the evidence if publication fails; the next save cannot discard it.
    pub fn publish_last_version(&mut self) -> Result<Option<Version>, Error> {
        self.store.validate()?;
        let Some(last) = &self.record.last_save else {
            return Ok(None);
        };
        let Some(evidence) = &last.evidence else {
            return Ok(None);
        };
        let operation = self
            .record
            .operations
            .iter()
            .find(|operation| operation.id == last.operation)
            .ok_or(Error::Invalid)?;
        if operation.outcome != Outcome::Acknowledged
            || operation.file_effect != FileEffect::Changed
            || evidence.info.operation_id != operation.id
            || evidence.after.sha256 != last.installed_sha256
        {
            return Err(Error::Changed);
        }
        evidence.validate(self, operation)?;
        let name = format!("{}.json", evidence.info.version_id);
        match self.store.directory().symlink_metadata(&name) {
            Ok(_) => {
                let prior = self.store.read_record(&name)?;
                if prior != serde_json::to_vec(evidence).map_err(|_| Error::Invalid)? {
                    return Err(Error::Changed);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.store
                    .record(&name, encoded_size(evidence)?, |writer| {
                        serde_json::to_writer(writer, evidence).map_err(|_| Error::Io)
                    })?;
            }
            Err(_) => return Err(Error::Io),
        }
        Ok(Some(evidence.info.clone()))
    }
}

/// Read-only saved-source view. This type cannot mutate or access draft history.
pub struct VersionView {
    evidence: Evidence,
    before: Base,
    after: Base,
}
impl VersionView {
    pub fn info(&self) -> &Version {
        &self.evidence.info
    }
    pub fn page(
        &self,
        before: bool,
        offset: Offset,
        limit: usize,
    ) -> Result<super::super::Page, Error> {
        let base = if before { &self.before } else { &self.after };
        if limit == 0 || limit > super::super::MAX_PAGE || offset.0 > base.length() {
            return Err(Error::Limit);
        }
        let bytes = base.read(
            offset.0,
            (base.length() - offset.0).min(limit as u64) as usize,
        )?;
        Ok(super::super::Page {
            offset,
            eof: offset.0 + bytes.len() as u64 == base.length(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
        })
    }
    pub fn review(
        &self,
        start: usize,
        limit: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<Review, Error> {
        if limit == 0 || limit > 128 {
            return Err(Error::Limit);
        }
        let mut ranges = vec![];
        let (mut count, mut removed_total, mut inserted_total) = (0, 0, 0);
        for &(at, removed, to, inserted) in &self.evidence.ranges {
            let before = digest_range(&self.before, at, removed, &cancelled)?;
            let after = digest_range(&self.after, to, inserted, &cancelled)?;
            if removed == inserted && before == after {
                continue;
            }
            if count >= start && ranges.len() < limit {
                ranges.push(crate::binary::draft::ChangeRange {
                    original_offset: Offset(at),
                    original_length: Offset(removed),
                    draft_offset: Offset(to),
                    draft_length: Offset(inserted),
                    before_sha256: before,
                    after_sha256: after,
                    before_hex: crate::binary::draft::encode_hex(
                        &self.before.read(at, removed.min(256) as usize)?,
                    ),
                    after_hex: crate::binary::draft::encode_hex(
                        &self.after.read(to, inserted.min(256) as usize)?,
                    ),
                    preview_truncated: removed > 256 || inserted > 256,
                });
            }
            count += 1;
            removed_total += removed;
            inserted_total += inserted;
        }
        let info = self.info();
        if start > count
            || count != info.range_count
            || removed_total != info.original_bytes.0
            || inserted_total != info.proposed_bytes.0
        {
            return Err(Error::Changed);
        }
        Ok(Review {
            revision: self.evidence.revision.clone(),
            base_sha256: info.before_sha256.clone(),
            proposed_sha256: info.after_sha256.clone(),
            base_length: info.before_length,
            proposed_length: info.after_length,
            original_bytes: Offset(removed_total),
            proposed_bytes: Offset(inserted_total),
            range_count: count,
            next_range: (start + ranges.len() < count).then_some(start + ranges.len()),
            ranges,
            fingerprint: signature(&self.evidence)?,
        })
    }
}
fn digest_range(
    base: &Base,
    at: u64,
    length: u64,
    cancelled: &impl Fn() -> bool,
) -> Result<String, Error> {
    let mut hash = Sha256::new();
    let mut done = 0;
    while done < length {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        let size = (length - done).min(super::super::DEFAULT_PAGE as u64) as usize;
        hash.update(base.read(at + done, size)?);
        done += size as u64;
    }
    Ok(format!("{:x}", hash.finalize()))
}
impl crate::binary::storage::StorageRoot {
    /// Enumerate acknowledged versions for one authorized source and document.
    /// This reads immutable evidence only, never the private command journal.
    pub fn versions(
        &self,
        source: &Document,
        document: &str,
        after: Option<&str>,
        limit: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<crate::binary::receipts::Versions, Error> {
        use cap_fs_ext::DirExt;
        valid_document(document)?;
        if limit == 0 || limit > 64 || after.is_some_and(|id| !valid_version(id)) {
            return Err(Error::Limit);
        }
        source.validate()?;
        let root = self.directory()?;
        let dir = root.open_dir_nofollow(document).map_err(|_| Error::File)?;
        crate::binary::storage::private(
            root,
            &dir.try_clone().map_err(|_| Error::Io)?.into_std_file(),
            true,
        )?;
        let mut found = vec![];
        for (count, entry) in dir.entries().map_err(|_| Error::Io)?.enumerate() {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            if count >= 4096 {
                return Err(Error::Limit);
            }
            let name = entry.map_err(|_| Error::Io)?.file_name();
            let name = name.to_str().ok_or(Error::File)?;
            let Some(version) = name.strip_suffix(".json").filter(|id| valid_version(id)) else {
                continue;
            };
            if after.is_some_and(|after| version <= after) {
                continue;
            }
            let evidence = read_evidence(&dir, document, version)?;
            bind_source(&evidence, source)?;
            found.push(evidence.info);
            found.sort_by(|a, b| a.version_id.cmp(&b.version_id));
            found.truncate(limit + 1);
        }
        source.validate()?;
        self.directory()?;
        let next_cursor = (found.len() > limit).then(|| found[limit - 1].version_id.clone());
        found.truncate(limit);
        Ok(crate::binary::receipts::Versions {
            versions: found,
            next_cursor,
        })
    }
    /// Caller establishes fresh read authority for the live source resource first.
    /// Saved versions are shared source history; unsaved owner/model drafts are not.
    pub fn open_version(
        &self,
        source: &Document,
        document: &str,
        version: &str,
        cancelled: impl Fn() -> bool,
    ) -> Result<VersionView, Error> {
        use cap_fs_ext::DirExt;
        valid_document(document)?;
        if !valid_version(version) {
            return Err(Error::Invalid);
        }
        source.validate()?;
        let root = self.directory()?;
        let dir = root.open_dir_nofollow(document).map_err(|_| Error::File)?;
        crate::binary::storage::private(
            root,
            &dir.try_clone().map_err(|_| Error::Io)?.into_std_file(),
            true,
        )?;
        let evidence = read_evidence(&dir, document, version)?;
        bind_source(&evidence, source)?;
        let before = self.saved_base(
            document,
            &evidence.before.name,
            evidence.before.length.0,
            &evidence.before.sha256,
            &cancelled,
        )?;
        let after = self.saved_base(
            document,
            &evidence.after.name,
            evidence.after.length.0,
            &evidence.after.sha256,
            &cancelled,
        )?;
        source.validate()?;
        Ok(VersionView {
            evidence,
            before,
            after,
        })
    }
}

fn valid_document(document: &str) -> Result<(), Error> {
    if !document.starts_with("bin_")
        || document.len() > 100
        || !document
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn valid_version(version: &str) -> bool {
    version.len() == 68
        && version.starts_with("ver_")
        && version[4..].bytes().all(|b| b.is_ascii_hexdigit())
}
pub(super) fn read_evidence(
    dir: &cap_std::fs::Dir,
    document: &str,
    version: &str,
) -> Result<Evidence, Error> {
    let bytes =
        crate::binary::storage::small_record(dir, &format!("{version}.json"), 16 * 1024 * 1024)?;
    let evidence: Evidence = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    evidence.validate_common(document)?;
    if evidence.info.version_id != version {
        return Err(Error::Changed);
    }
    Ok(evidence)
}
fn bind_source(evidence: &Evidence, source: &Document) -> Result<(), Error> {
    if evidence.target.root != source.root()
        || evidence.target.root.join(&evidence.target.relative) != source.resource().path()
        || evidence.target.parents.first() != Some(&source.project_identity()?)
    {
        return Err(Error::File);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::draft::{PatchBatch, Splice, encode_hex};
    fn fixture() -> (tempfile::TempDir, Editor, Owner) {
        fixture_with(&[0, 255, 128, 27])
    }
    fn fixture_with(content: &[u8]) -> (tempfile::TempDir, Editor, Owner) {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("data")).unwrap();
        std::fs::create_dir(temp.path().join("project")).unwrap();
        let root = temp.path().join("project").canonicalize().unwrap();
        std::fs::write(root.join("bytes"), content).unwrap();
        let owner = Owner {
            session_id: "ses_versions".into(),
            directory: root.clone(),
            root,
            scope: DraftScope::Manual,
        };
        let mut source = Document::open(&owner.root, "bytes").unwrap();
        let editor = Editor::create(
            Store::open(&temp.path().join("data"), "bin_versions").unwrap(),
            owner.clone(),
            &mut source,
            || false,
        )
        .unwrap();
        (temp, editor, owner)
    }
    fn patch(editor: &mut Editor, id: &str, expected: &[u8], inserted: &[u8]) {
        editor
            .patch(
                id,
                &PatchBatch {
                    revision: editor.draft().revision(),
                    splices: vec![Splice {
                        offset: Offset(0),
                        delete_length: Offset(expected.len() as u64),
                        expected_hex: encode_hex(expected),
                        insert_hex: encode_hex(inserted),
                    }],
                },
            )
            .unwrap();
    }
    fn save(editor: &mut Editor, id: &str) {
        let review = editor.review(0, 128, || false).unwrap();
        let SavePreparation::Ready(prepared) = editor
            .prepare_save(id, &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("native save");
        };
        assert_eq!(
            editor.commit_save(*prepared).unwrap().outcome,
            Outcome::Acknowledged
        );
    }
    #[test]
    fn saved_version_listing_is_bounded_and_excludes_unsaved_history() {
        let (temp, mut editor, owner) = fixture();
        let storage =
            crate::binary::storage::StorageRoot::prepare(&temp.path().join("data")).unwrap();
        patch(&mut editor, "patch_one", &[0], &[1]);
        save(&mut editor, "saved_one");
        let first = editor.publish_last_version().unwrap().unwrap();
        patch(&mut editor, "patch_two", &[1], &[2]);
        save(&mut editor, "saved_two");
        let second = editor.publish_last_version().unwrap().unwrap();
        patch(&mut editor, "private_patch", &[2], b"private unsaved bytes");
        let source = Document::open(&owner.root, "bytes").unwrap();
        let page = storage
            .versions(&source, "bin_versions", None, 1, || false)
            .unwrap();
        assert_eq!(page.versions.len(), 1);
        let cursor = page.next_cursor.as_deref().unwrap();
        assert_eq!(cursor, page.versions[0].version_id);
        let next = storage
            .versions(&source, "bin_versions", Some(cursor), 1, || false)
            .unwrap();
        assert_eq!(next.versions.len(), 1);
        assert!(next.next_cursor.is_none());
        let mut actual = vec![
            page.versions[0].version_id.clone(),
            next.versions[0].version_id.clone(),
        ];
        let mut expected = vec![first.version_id, second.version_id];
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
        assert!(
            storage
                .versions(&source, "bin_versions", None, 65, || false)
                .is_err()
        );
        assert!(matches!(
            storage.versions(&source, "bin_versions", None, 1, || true),
            Err(Error::Cancelled)
        ));
        assert!(
            storage
                .versions(&source, "../bin_versions", None, 1, || false)
                .is_err()
        );
        std::fs::write(owner.root.join("other"), [0, 255]).unwrap();
        let other = Document::open(&owner.root, "other").unwrap();
        assert!(
            storage
                .versions(&other, "bin_versions", None, 1, || false)
                .is_err()
        );
    }
    #[test]
    fn saved_versions_retain_both_byte_images_without_transient_draft_commands() {
        let (temp, mut editor, owner) = fixture();
        let secret = b"NEVER_SHARE_TRANSIENT_DRAFT_COMMAND";
        patch(&mut editor, "temporary", &[0], secret);
        patch(&mut editor, "final", secret, &[1]);
        assert!(editor.publish_last_version().unwrap().is_none());
        save(&mut editor, "saved_one");
        let first = editor.publish_last_version().unwrap().unwrap();
        assert_eq!(first.original_bytes, Offset(1));
        assert_eq!(first.proposed_bytes, Offset(1));
        assert_eq!(first.range_count, 1);
        let first_file = format!("{}.json", first.version_id);
        let bytes = editor.store.read_record(&first_file).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(&encode_hex(secret)));
        let evidence: Evidence = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            evidence
                .before
                .open(&editor.store)
                .unwrap()
                .read(0, 4)
                .unwrap(),
            [0, 255, 128, 27]
        );
        assert_eq!(
            evidence
                .after
                .open(&editor.store)
                .unwrap()
                .read(0, 4)
                .unwrap(),
            [1, 255, 128, 27]
        );
        assert_eq!(
            editor.publish_last_version().unwrap().unwrap().version_id,
            first.version_id
        );
        patch(&mut editor, "next_patch", &[1], &[2]);
        save(&mut editor, "saved_two");
        let second = editor.publish_last_version().unwrap().unwrap();
        assert_ne!(first.version_id, second.version_id);
        assert_eq!(second.before_sha256, first.after_sha256);
        drop(editor);
        let mut resumed = Editor::resume(
            Store::open(&temp.path().join("data"), "bin_versions").unwrap(),
            &owner,
        )
        .unwrap();
        assert_eq!(resumed.store.read_record(&first_file).unwrap(), bytes);
        assert_eq!(
            resumed.publish_last_version().unwrap().unwrap().version_id,
            second.version_id
        );
        assert_eq!(
            std::fs::read(owner.root.join("bytes")).unwrap(),
            [2, 255, 128, 27]
        );
    }
    #[test]
    fn shared_saved_views_exclude_current_private_drafts_and_bind_every_byte_to_its_source() {
        let (temp, mut editor, owner) = fixture();
        patch(&mut editor, "patch_one", &[0], &[1]);
        save(&mut editor, "saved");
        let info = editor.publish_last_version().unwrap().unwrap();
        patch(
            &mut editor,
            "unsaved_private",
            &[1],
            b"CURRENT_PRIVATE_DRAFT",
        );
        let storage =
            crate::binary::storage::StorageRoot::prepare(&temp.path().join("data")).unwrap();
        let source = Document::open(&owner.root, "bytes").unwrap();
        let view = storage
            .open_version(&source, "bin_versions", &info.version_id, || false)
            .unwrap();
        assert_eq!(
            view.page(true, Offset(0), 4).unwrap().bytes,
            [0, 255, 128, 27]
        );
        assert_eq!(
            view.page(false, Offset(0), 4).unwrap().bytes,
            [1, 255, 128, 27]
        );
        let review = view.review(0, 1, || false).unwrap();
        assert_eq!(review.ranges[0].before_hex, "00");
        assert_eq!(review.ranges[0].after_hex, "01");
        assert_eq!(review.range_count, 1);
        assert!(matches!(view.review(0, 1, || true), Err(Error::Cancelled)));
        assert!(
            view.page(false, Offset(0), crate::binary::MAX_PAGE + 1)
                .is_err()
        );
        assert!(matches!(
            storage.open_version(&source, "bin_versions", &info.version_id, || true),
            Err(Error::Cancelled)
        ));
        std::fs::write(owner.root.join("other"), [0, 255, 128, 27]).unwrap();
        let other = Document::open(&owner.root, "other").unwrap();
        assert!(
            storage
                .open_version(&other, "bin_versions", &info.version_id, || false)
                .is_err()
        );
        assert!(
            storage
                .open_version(&source, "../bin_versions", &info.version_id, || false)
                .is_err()
        );
        assert_eq!(
            std::fs::read(owner.root.join("bytes")).unwrap(),
            [1, 255, 128, 27]
        );
        assert!(editor.draft().can_undo());
    }
    #[test]
    fn semantically_identical_byte_layouts_checkpoint_without_replacement_or_new_versions() {
        let (_temp, mut editor, owner) = fixture_with(b"aaaa");
        let original_id = editor.target().file.clone();
        patch(&mut editor, "remove_first", b"a", b"");
        editor
            .patch(
                "insert_last",
                &PatchBatch {
                    revision: editor.draft().revision(),
                    splices: vec![Splice {
                        offset: Offset(3),
                        delete_length: Offset(0),
                        expected_hex: String::new(),
                        insert_hex: "61".into(),
                    }],
                },
            )
            .unwrap();
        let review = editor.review(0, 128, || false).unwrap();
        assert_eq!(review.draft.range_count, 0);
        let SavePreparation::Existing(operation) = editor
            .prepare_save("checkpoint", &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("identical bytes must not exchange");
        };
        assert_eq!(operation.outcome, Outcome::Acknowledged);
        assert_eq!(operation.file_effect, FileEffect::None);
        assert_eq!(editor.target().file, original_id);
        assert!(!editor.draft().can_undo());
        assert!(editor.publish_last_version().unwrap().is_none());
        assert_eq!(std::fs::read(owner.root.join("bytes")).unwrap(), b"aaaa");
    }

    #[test]
    fn unacknowledged_intents_and_corrupt_existing_versions_are_not_published_or_repaired() {
        let (_temp, mut editor, owner) = fixture();
        patch(&mut editor, "patch_one", &[0], &[1]);
        let review = editor.review(0, 128, || false).unwrap();
        let SavePreparation::Ready(prepared) = editor
            .prepare_save("pending", &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("prepare");
        };
        editor.record.pending = Some(prepared.intent);
        persist(&mut editor.store, &editor.record).unwrap();
        assert!(editor.publish_last_version().unwrap().is_none());
        assert_eq!(
            std::fs::read(owner.root.join("bytes")).unwrap(),
            [0, 255, 128, 27]
        );
        // Separate fixture: corrupt metadata must not be silently replaced.
        let (_temp, mut editor, owner) = fixture();
        patch(&mut editor, "patch_one", &[0], &[1]);
        save(&mut editor, "saved");
        let version = editor.publish_last_version().unwrap().unwrap();
        let name = format!("{}.json", version.version_id);
        editor
            .store
            .record(&name, 2, |writer| {
                writer.write_all(b"{}").map_err(|_| Error::Io)
            })
            .unwrap();
        assert!(editor.publish_last_version().is_err());
        assert_eq!(editor.store.read_record(&name).unwrap(), b"{}");
        assert_eq!(
            std::fs::read(owner.root.join("bytes")).unwrap(),
            [1, 255, 128, 27]
        );
    }
}
