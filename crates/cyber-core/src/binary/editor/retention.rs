//! Retire only acknowledged history, then reclaim its unreferenced immutable
//! images. Compact tombstones retain candidate references across interruption.
//! Unassociated preparation files and uncertain native stages are never guessed.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Displaced {
    name: String,
    identity: String,
    metadata: native::Metadata,
    base: BaseRef,
}
impl Displaced {
    pub fn prepared(editor: &Editor, name: &str) -> Self {
        Self {
            name: name.into(),
            identity: editor.record.target.file.clone(),
            metadata: editor.record.target.metadata.clone(),
            base: editor.record.base.clone(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "BinaryCleanup")]
pub struct Cleanup {
    pub versions_retired: usize,
    pub bases_removed: usize,
    pub stages_removed: usize,
    pub stages_retained: usize,
    pub preparations_removed: usize,
    pub bytes_reclaimed: Offset,
}
impl Default for Cleanup {
    fn default() -> Self {
        Self {
            versions_retired: 0,
            bases_removed: 0,
            stages_removed: 0,
            stages_retained: 0,
            preparations_removed: 0,
            bytes_reclaimed: Offset(0),
        }
    }
}
struct Plan {
    retire: Vec<String>,
    retained: BTreeSet<String>,
    candidates: BTreeMap<String, BaseRef>,
    stages: Vec<Stage>,
    recognized: BTreeSet<String>,
    preparation_contract: bool,
}
struct Stage {
    version: String,
    ticket: Displaced,
    images: [String; 2],
}
fn add_candidate(plan: &mut Plan, base: &BaseRef) -> Result<(), Error> {
    if !versions::valid_base(base) {
        return Err(Error::Changed);
    }
    if let Some(prior) = plan.candidates.get(&base.name)
        && (prior.length != base.length || prior.sha256 != base.sha256)
    {
        return Err(Error::Changed);
    }
    plan.candidates.insert(base.name.clone(), base.clone());
    Ok(())
}
fn gate(
    cancelled: &impl Fn() -> bool,
    authorize: &impl Fn() -> Result<(), Error>,
) -> Result<(), Error> {
    if cancelled() {
        return Err(Error::Cancelled);
    }
    authorize()?;
    if cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(())
}
impl Editor {
    /// Caller supplies the configured retention cutoff and live checkpoint refs,
    /// retains native mutation ownership, and refreshes trusted authority before
    /// every namespace change. No source bytes are modified by this operation.
    pub fn prune_saved_versions(
        &mut self,
        older_than_ms: i64,
        protected_versions: &[String],
        cancelled: impl Fn() -> bool,
        authorize: impl Fn() -> Result<(), Error>,
    ) -> Result<Cleanup, Error> {
        self.available()?;
        if protected_versions.len() > 4096
            || protected_versions
                .iter()
                .any(|id| !versions::valid_version(id))
        {
            return Err(Error::Limit);
        }
        let _readers = self.store.history_lease(true)?;
        let mut report = Cleanup::default();
        if let Some(displaced) = self
            .record
            .last_save
            .as_ref()
            .and_then(|last| last.evidence.as_ref())
            .and_then(|evidence| evidence.displaced.clone())
        {
            self.reclaim_stage(&displaced, &cancelled, &authorize, &mut report)?;
        }
        self.publish_last_version_locked()?;
        let mut plan = self.retention_plan(older_than_ms, protected_versions, &cancelled)?;
        // Resolve known duplicates before retiring their provenance. A changed
        // displaced inode keeps the complete version and both exact images.
        for stage in &plan.stages {
            let retained = report.stages_retained;
            self.reclaim_stage(&stage.ticket, &cancelled, &authorize, &mut report)?;
            if report.stages_retained > retained {
                plan.retained.extend(stage.images.clone());
                plan.retire.retain(|id| id != &stage.version);
            }
        }
        for version in &plan.retire {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let mut evidence =
                versions::read_evidence(self.store.directory(), self.store.document_id(), version)?;
            gate(&cancelled, &authorize)?;
            evidence.retired = true;
            evidence.ranges.clear();
            evidence.revision.clear();
            self.store.record(
                &format!("{version}.json"),
                encoded_size(&evidence)?,
                |writer| serde_json::to_writer(writer, &evidence).map_err(|_| Error::Io),
            )?;
            report.versions_retired += 1;
        }
        // Only references durably recorded in retired history become candidates.
        // An interrupted cleanup can repeat this pass without losing provenance.
        for (name, base) in &plan.candidates {
            if plan.retained.contains(name) {
                continue;
            }
            if cancelled() {
                return Err(Error::Cancelled);
            }
            match self.store.directory().symlink_metadata(name) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return Err(Error::Io),
                Ok(_) => {}
            }
            let image =
                self.store
                    .open_base_cancellable(name, base.length.0, &base.sha256, &cancelled)?;
            gate(&cancelled, &authorize)?;
            if self.store.remove_base(&image)? {
                report.bases_removed += 1;
                report.bytes_reclaimed.0 += base.length.0;
            }
        }
        if plan.preparation_contract {
            self.reclaim_preparations(&plan.recognized, &cancelled, &authorize, &mut report)?;
        }
        Ok(report)
    }
    fn retention_plan(
        &self,
        cutoff: i64,
        protected: &[String],
        cancelled: &impl Fn() -> bool,
    ) -> Result<Plan, Error> {
        let mut plan = Plan {
            retire: vec![],
            retained: BTreeSet::from([self.record.base.name.clone()]),
            candidates: BTreeMap::new(),
            stages: vec![],
            recognized: BTreeSet::from([self.record.base.name.clone()]),
            preparation_contract: true,
        };
        if let Some(last) = &self.record.last_save {
            plan.retained.insert(last.preimage.name.clone());
            plan.recognized.insert(last.preimage.name.clone());
        }
        let mut seen = BTreeSet::new();
        for (count, entry) in self
            .store
            .directory()
            .entries()
            .map_err(|_| Error::Io)?
            .enumerate()
        {
            if count >= 4096 {
                return Err(Error::Limit);
            }
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let name = entry.map_err(|_| Error::Io)?.file_name();
            let name = name.to_str().ok_or(Error::File)?;
            let Some(id) = name
                .strip_suffix(".json")
                .filter(|id| versions::valid_version(id))
            else {
                continue;
            };
            let evidence =
                versions::read_any_evidence(self.store.directory(), self.store.document_id(), id)?;
            if evidence.owner != self.record.owner
                || evidence.target.relative != self.record.target.relative
            {
                return Err(Error::Changed);
            }
            let operation = self
                .record
                .operations
                .iter()
                .find(|operation| operation.id == evidence.info.operation_id)
                .ok_or(Error::Changed)?;
            if operation.outcome != Outcome::Acknowledged
                || operation.file_effect != FileEffect::Changed
                || operation.payload_sha256 != evidence.operation.payload_sha256
            {
                return Err(Error::Changed);
            }
            seen.insert(operation.id.clone());
            plan.recognized
                .extend([evidence.before.name.clone(), evidence.after.name.clone()]);
            let keep = protected.iter().any(|value| value == id)
                || operation.notification_pending
                || self
                    .record
                    .last_save
                    .as_ref()
                    .is_some_and(|last| last.operation == operation.id)
                || evidence.info.created_ms >= cutoff;
            if evidence.retired && keep && protected.iter().any(|value| value == id) {
                return Err(Error::Changed); // A checkpoint cannot silently bind expired data.
            }
            if !evidence.retired && keep {
                plan.retained
                    .extend([evidence.before.name.clone(), evidence.after.name.clone()]);
            } else {
                if !evidence.retired {
                    plan.retire.push(id.into());
                }
                add_candidate(&mut plan, &evidence.before)?;
                add_candidate(&mut plan, &evidence.after)?;
            }
            if let Some(displaced) = evidence.displaced {
                plan.recognized.insert(displaced.name.clone());
                plan.stages.push(Stage {
                    version: id.into(),
                    ticket: displaced,
                    images: [evidence.before.name.clone(), evidence.after.name.clone()],
                });
            } else {
                plan.preparation_contract = false;
            }
        }
        if self.record.operations.iter().any(|operation| {
            operation.outcome == Outcome::Acknowledged
                && operation.file_effect == FileEffect::Changed
                && !seen.contains(&operation.id)
        }) {
            return Err(Error::Changed); // Incomplete older evidence cannot authorize collection.
        }
        Ok(plan)
    }
    fn reclaim_preparations(
        &mut self,
        recognized: &BTreeSet<String>,
        cancelled: &impl Fn() -> bool,
        authorize: &impl Fn() -> Result<(), Error>,
        report: &mut Cleanup,
    ) -> Result<(), Error> {
        // Every acknowledged save has an exact stage ticket, no native intent
        // remains, and the exclusive history lease excludes every prepared token.
        // Other allocator-owned images therefore never reached native execution.
        let mut files = vec![];
        for (count, entry) in self
            .store
            .directory()
            .entries()
            .map_err(|_| Error::Io)?
            .enumerate()
        {
            if count >= 4096 {
                return Err(Error::Limit);
            }
            let name = entry.map_err(|_| Error::Io)?.file_name();
            let name = name.to_str().ok_or(Error::File)?;
            if !recognized.contains(name) && crate::binary::storage::preparation_name(name) {
                files.push(name.to_owned());
            }
        }
        for name in files {
            gate(cancelled, authorize)?;
            if let Some(bytes) = self
                .store
                .remove_preparation(&name, self.record.target.metadata.uid)?
            {
                report.preparations_removed += 1;
                report.bytes_reclaimed.0 += bytes;
            }
        }
        Ok(())
    }
    fn reclaim_stage(
        &mut self,
        displaced: &Displaced,
        cancelled: &impl Fn() -> bool,
        authorize: &impl Fn() -> Result<(), Error>,
        report: &mut Cleanup,
    ) -> Result<(), Error> {
        if cancelled() {
            return Err(Error::Cancelled);
        }
        match self.store.directory().symlink_metadata(&displaced.name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(Error::Io),
            Ok(_) => {}
        }
        // The immutable preimage must still exist and verify before its duplicate
        // can be removed. Changed displaced data is retained as uncertain evidence.
        self.store.open_base_cancellable(
            &displaced.base.name,
            displaced.base.length.0,
            &displaced.base.sha256,
            cancelled,
        )?;
        let verified = (|| {
            let stage = stage_file(&self.store, &displaced.name)?;
            if native::id(&stage)? != displaced.identity
                || native::metadata(&stage)? != displaced.metadata
                || stage.metadata().map_err(|_| Error::Io)?.len() != displaced.base.length.0
                || native::content(&stage, cancelled)? != displaced.base.sha256
            {
                return Err(Error::Changed);
            }
            Ok(())
        })();
        if let Err(error) = verified {
            if matches!(error, Error::Cancelled) {
                return Err(error);
            }
            report.stages_retained += 1;
            return Ok(());
        }
        gate(cancelled, authorize)?;
        if self
            .store
            .remove_known(&displaced.name, &displaced.identity)?
        {
            report.stages_removed += 1;
            report.bytes_reclaimed.0 += displaced.base.length.0;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::draft::{PatchBatch, Splice, encode_hex};
    fn save_value(editor: &mut Editor, value: u8) -> crate::binary::receipts::Version {
        let id = format!("patch_{value}");
        editor
            .patch(
                &id,
                &PatchBatch {
                    revision: editor.draft().revision(),
                    splices: vec![Splice {
                        offset: Offset(0),
                        delete_length: Offset(1),
                        expected_hex: encode_hex(&editor.draft().read(0, 1).unwrap()),
                        insert_hex: encode_hex(&[value]),
                    }],
                },
            )
            .unwrap();
        let id = format!("save_{value}");
        let prepared = super::super::tests::ready(editor, &id);
        editor.commit_save(*prepared).unwrap();
        let version = editor.publish_last_version().unwrap().unwrap();
        editor.notified(&id).unwrap();
        version
    }
    fn fixture() -> (
        tempfile::TempDir,
        Editor,
        PathBuf,
        Owner,
        Vec<crate::binary::receipts::Version>,
    ) {
        let (temp, mut editor, path, owner) = super::super::tests::setup();
        let versions = (1..=3)
            .map(|value| save_value(&mut editor, value))
            .collect();
        (temp, editor, path, owner, versions)
    }
    fn cutoff() -> i64 {
        chrono::Utc::now().timestamp_millis() + 1
    }
    fn abandoned_preparation(editor: &mut Editor) -> (String, String) {
        editor
            .patch(
                "abandoned",
                &PatchBatch {
                    revision: editor.draft().revision(),
                    splices: vec![Splice {
                        offset: Offset(0),
                        delete_length: Offset(1),
                        expected_hex: "03".into(),
                        insert_hex: "04".into(),
                    }],
                },
            )
            .unwrap();
        let prepared = super::super::tests::ready(editor, "abandoned_save");
        let names = (
            prepared.intent.new_base.name.clone(),
            prepared.intent.stage.clone(),
        );
        drop(prepared);
        names
    }

    #[test]
    fn cleanup_preserves_dirty_current_and_last_save_images_and_is_restart_idempotent() {
        let (temp, mut editor, path, owner, versions) = fixture();
        editor
            .patch(
                "dirty",
                &PatchBatch {
                    revision: editor.draft().revision(),
                    splices: vec![Splice {
                        offset: Offset(1),
                        delete_length: Offset(1),
                        expected_hex: "72".into(),
                        insert_hex: "ff".into(),
                    }],
                },
            )
            .unwrap();
        let dirty = editor.draft().read(0, 8).unwrap();
        let report = editor
            .prune_saved_versions(cutoff(), &[], || false, || Ok(()))
            .unwrap();
        assert_eq!(report.versions_retired, 2);
        assert_eq!(report.bases_removed, 2);
        assert_eq!(report.stages_removed, 3);
        assert_eq!(editor.draft().read(0, 8).unwrap(), dirty);
        assert!(editor.draft().can_undo());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            [3, b'r', b'i', b'g', b'i', b'n', b'a', b'l']
        );
        let evidence = versions::read_evidence(
            editor.store.directory(),
            editor.store.document_id(),
            &versions[2].version_id,
        )
        .unwrap();
        evidence.before.open(&editor.store).unwrap();
        evidence.after.open(&editor.store).unwrap();
        drop(editor);
        let storage =
            crate::binary::storage::StorageRoot::prepare(&temp.path().join("data")).unwrap();
        let mut editor = Editor::resume(storage.open_existing("bin_one").unwrap(), &owner).unwrap();
        assert_eq!(
            editor
                .prune_saved_versions(cutoff(), &[], || false, || Ok(()))
                .unwrap()
                .bases_removed,
            0
        );
        let source = Document::open(&owner.root, "bytes").unwrap();
        let listed = storage
            .versions(&source, "bin_one", None, 64, || false)
            .unwrap();
        assert_eq!(listed.versions.len(), 1);
        assert!(
            storage
                .open_version(&source, "bin_one", &versions[0].version_id, || false)
                .is_err()
        );
        assert_eq!(
            editor.lookup_operation("save_1").unwrap().unwrap().outcome,
            Outcome::Acknowledged
        );
    }
    #[test]
    fn checkpoint_refs_and_live_prepared_tokens_prevent_history_loss() {
        let (temp, mut editor, _path, owner, versions) = fixture();
        editor
            .patch(
                "future",
                &PatchBatch {
                    revision: editor.draft().revision(),
                    splices: vec![Splice {
                        offset: Offset(0),
                        delete_length: Offset(1),
                        expected_hex: "03".into(),
                        insert_hex: "04".into(),
                    }],
                },
            )
            .unwrap();
        let prepared = super::super::tests::ready(&mut editor, "future_save");
        assert!(
            editor
                .prune_saved_versions(cutoff(), &[], || false, || Ok(()))
                .is_err()
        );
        drop(prepared);
        editor
            .store
            .directory()
            .write("stage_unclassified", b"retain unknown evidence")
            .unwrap();
        let report = editor
            .prune_saved_versions(
                cutoff(),
                &[versions[0].version_id.clone()],
                || false,
                || Ok(()),
            )
            .unwrap();
        assert_eq!(report.versions_retired, 1);
        assert_eq!(report.bases_removed, 0);
        assert_eq!(report.preparations_removed, 2);
        assert_eq!(
            editor.store.directory().read("stage_unclassified").unwrap(),
            b"retain unknown evidence"
        );
        let storage =
            crate::binary::storage::StorageRoot::prepare(&temp.path().join("data")).unwrap();
        let source = Document::open(&owner.root, "bytes").unwrap();
        assert!(
            storage
                .open_version(&source, "bin_one", &versions[0].version_id, || false)
                .is_ok()
        );
        assert_eq!(editor.draft().read(0, 1).unwrap(), [4]);
    }
    #[test]
    fn changed_displaced_data_retains_its_complete_version_and_preimages() {
        let (_temp, mut editor, path, _owner, versions) = fixture();
        let first = versions::read_evidence(
            editor.store.directory(),
            editor.store.document_id(),
            &versions[0].version_id,
        )
        .unwrap();
        let displaced = first.displaced.as_ref().unwrap();
        editor
            .store
            .directory()
            .write(&displaced.name, b"external")
            .unwrap();
        let report = editor
            .prune_saved_versions(cutoff(), &[], || false, || Ok(()))
            .unwrap();
        assert_eq!(report.stages_retained, 1);
        assert_eq!(report.versions_retired, 1);
        assert_eq!(report.bases_removed, 0);
        versions::read_evidence(
            editor.store.directory(),
            editor.store.document_id(),
            &versions[0].version_id,
        )
        .unwrap();
        first.before.open(&editor.store).unwrap();
        first.after.open(&editor.store).unwrap();
        assert_eq!(
            editor.store.directory().read(&displaced.name).unwrap(),
            b"external"
        );
        assert_eq!(std::fs::read(path).unwrap()[0], 3);
    }
    #[test]
    fn authorization_stop_and_unknown_intents_refuse_cleanup_without_source_effects() {
        let (_temp, mut editor, path, _owner, _versions) = fixture();
        assert!(
            editor
                .prune_saved_versions(cutoff(), &[], || false, || Err(Error::Changed))
                .is_err()
        );
        let stop = std::cell::Cell::new(false);
        assert!(matches!(
            editor.prune_saved_versions(
                cutoff(),
                &[],
                || stop.get(),
                || {
                    stop.set(true);
                    Ok(())
                }
            ),
            Err(Error::Cancelled)
        ));
        editor
            .patch(
                "future",
                &PatchBatch {
                    revision: editor.draft().revision(),
                    splices: vec![Splice {
                        offset: Offset(0),
                        delete_length: Offset(1),
                        expected_hex: "03".into(),
                        insert_hex: "04".into(),
                    }],
                },
            )
            .unwrap();
        let prepared = super::super::tests::ready(&mut editor, "unknown");
        editor.record.pending = Some(prepared.intent);
        persist(&mut editor.store, &editor.record).unwrap();
        assert!(
            editor
                .prune_saved_versions(cutoff(), &[], || false, || Ok(()))
                .is_err()
        );
        assert!(editor.unresolved());
        assert_eq!(std::fs::read(path).unwrap()[0], 3);
    }
    #[test]
    fn uncertain_retirement_publication_reopens_before_reclaiming_any_images() {
        let (temp, mut editor, path, owner, _versions) = fixture();
        editor.store.fail_after_publication = Some(editor.store.publications + 1);
        assert!(
            editor
                .prune_saved_versions(cutoff(), &[], || false, || Ok(()))
                .is_err()
        );
        assert!(editor.store.validate().is_err());
        assert_eq!(std::fs::read(&path).unwrap()[0], 3);
        drop(editor);
        let storage =
            crate::binary::storage::StorageRoot::prepare(&temp.path().join("data")).unwrap();
        let mut editor = Editor::resume(storage.open_existing("bin_one").unwrap(), &owner).unwrap();
        let report = editor
            .prune_saved_versions(cutoff(), &[], || false, || Ok(()))
            .unwrap();
        assert_eq!(report.bases_removed, 2);
        assert_eq!(std::fs::read(path).unwrap()[0], 3);
    }
    #[test]
    fn incomplete_saved_evidence_cannot_authorize_collecting_preparation_files() {
        let (_temp, mut editor, path, _owner, versions) = fixture();
        let (base, stage) = abandoned_preparation(&mut editor);
        editor
            .store
            .directory()
            .remove_file(format!("{}.json", versions[0].version_id))
            .unwrap();
        assert!(
            editor
                .prune_saved_versions(cutoff(), &[], || false, || Ok(()))
                .is_err()
        );
        assert!(editor.store.directory().try_exists(base).unwrap());
        assert!(editor.store.directory().try_exists(stage).unwrap());
        assert_eq!(editor.draft().read(0, 1).unwrap(), [4]);
        assert_eq!(std::fs::read(path).unwrap()[0], 3);
    }
    #[test]
    fn older_history_without_stage_tickets_keeps_unassociated_preparations() {
        let (_temp, mut editor, _path, _owner, versions) = fixture();
        let mut older = versions::read_evidence(
            editor.store.directory(),
            editor.store.document_id(),
            &versions[0].version_id,
        )
        .unwrap();
        older.displaced = None;
        editor
            .store
            .record(
                &format!("{}.json", older.info.version_id),
                encoded_size(&older).unwrap(),
                |writer| serde_json::to_writer(writer, &older).map_err(|_| Error::Io),
            )
            .unwrap();
        let (base, stage) = abandoned_preparation(&mut editor);
        let protected = versions
            .iter()
            .map(|version| version.version_id.clone())
            .collect::<Vec<_>>();
        let report = editor
            .prune_saved_versions(cutoff(), &protected, || false, || Ok(()))
            .unwrap();
        assert_eq!(report.preparations_removed, 0);
        assert!(editor.store.directory().try_exists(base).unwrap());
        assert!(editor.store.directory().try_exists(stage).unwrap());
    }
}
