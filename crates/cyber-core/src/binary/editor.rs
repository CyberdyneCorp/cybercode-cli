//! Durable document operations and recoverable native save. Callers retain fresh
//! owner/read/edit/Mode authority and the checkout mutation lease around commit.
//! A native intent is never automatically re-executed after interruption.
use super::{
    Document, Error, Offset,
    draft::{Draft, DraftState, ImmutableBase, PatchBatch, Review},
    native::{self, Target},
    storage::{Base, Store},
};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptions;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

#[path = "editor/restore.rs"]
mod restore;

const MAX_OPERATIONS: usize = 4096;
const EFFECTS: &str = "Permissions, executable bits, owner/group, supported extended attributes and ACL are preserved. Native replacement changes file identity and updates modification time. Save checkpoints draft undo/redo; Restore Last Save uses the retained preimage. External checksums, signatures and irreversible observer effects are not repaired. Uncooperative writers can leave an unknown outcome; evidence is retained.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub session_id: String,
    pub directory: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Acknowledged,
    Conflict,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileEffect {
    None,
    Changed,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Operation {
    pub id: String,
    pub kind: String,
    pub payload_sha256: String,
    pub outcome: Outcome,
    pub revision: String,
    pub file_effect: FileEffect,
    pub reason: Option<String>,
    pub notification_pending: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SaveReview {
    pub draft: Review,
    pub permissions: u32,
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub metadata_sha256: String,
    pub effects: String,
    pub fingerprint: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaseRef {
    name: String,
    length: Offset,
    sha256: String,
}
impl BaseRef {
    fn of(base: &Base) -> Self {
        Self {
            name: base.name().into(),
            length: Offset(base.length()),
            sha256: base.sha256().into(),
        }
    }
    fn open(&self, store: &Store) -> Result<Base, Error> {
        store.open_base(&self.name, self.length.0, &self.sha256)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LastSave {
    preimage: BaseRef,
    installed_sha256: String,
    operation: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    operation: Operation,
    prior: Target,
    after: Target,
    new_base: BaseRef,
    after_draft: DraftState,
    stage: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    owner: Owner,
    target: Target,
    base: BaseRef,
    draft: DraftState,
    operations: Vec<Operation>,
    pending: Option<Intent>,
    last_save: Option<LastSave>,
}
pub struct Editor {
    store: Store,
    draft: Draft<Base>,
    record: Record,
}
pub struct PreparedSave {
    intent: Intent,
    after: Draft<Base>,
    expected_revision: String,
}
pub enum SavePreparation {
    Existing(Operation),
    Ready(Box<PreparedSave>),
}

fn signature<T: Serialize>(value: &T) -> Result<String, Error> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Invalid)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn operation_id(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
struct Counter(u64);
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| std::io::Error::other("binary record limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn persist(store: &mut Store, record: &Record) -> Result<(), Error> {
    let mut count = Counter(0);
    serde_json::to_writer(&mut count, record).map_err(|_| Error::Io)?;
    store.record("document.json", count.0, |writer| {
        serde_json::to_writer(writer, record).map_err(|_| Error::Io)
    })
}
fn stage_file(store: &Store, name: &str) -> Result<File, Error> {
    if name.len() > 100
        || !name.starts_with("stage_")
        || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err(Error::Invalid);
    }
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = store
        .directory()
        .open_with(name, &options)
        .map_err(|_| Error::File)?
        .into_std();
    native::metadata(&file)?;
    Ok(file)
}
impl Editor {
    pub fn create(
        mut store: Store,
        owner: Owner,
        document: &mut Document,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, Error> {
        if owner.session_id.is_empty() || !owner.directory.is_absolute() {
            return Err(Error::Invalid);
        }
        // Never replace an existing document, including one with unresolved evidence.
        if store.directory().symlink_metadata("document.json").is_ok() {
            return Err(Error::Changed);
        }
        let (base, target) = document.prepare(&store, cancelled)?;
        if target.root != owner.directory {
            return Err(Error::File);
        }
        let draft = Draft::new(base)?;
        let record = Record {
            version: 1,
            owner,
            target,
            base: BaseRef::of(draft.base()),
            draft: draft.snapshot(),
            operations: vec![],
            pending: None,
            last_save: None,
        };
        persist(&mut store, &record)?;
        Ok(Self {
            store,
            draft,
            record,
        })
    }
    pub fn resume(store: Store, owner: &Owner) -> Result<Self, Error> {
        let record: Record = serde_json::from_slice(&store.read_record("document.json")?)
            .map_err(|_| Error::Invalid)?;
        if record.version != 1
            || &record.owner != owner
            || record.operations.len() > MAX_OPERATIONS
            || record.target.root != owner.directory
        {
            return Err(Error::File);
        }
        let draft = Draft::resume(record.base.open(&store)?, record.draft.clone())?;
        Ok(Self {
            store,
            draft,
            record,
        })
    }
    pub fn draft(&self) -> &Draft<Base> {
        &self.draft
    }
    pub fn target(&self) -> &Target {
        &self.record.target
    }
    pub fn owner(&self) -> &Owner {
        &self.record.owner
    }
    pub fn unresolved(&self) -> bool {
        self.record.pending.is_some()
    }
    fn available(&self) -> Result<(), Error> {
        self.store.validate()?;
        if self.unresolved() {
            return Err(Error::Changed);
        }
        Ok(())
    }
    pub fn operation(&self, id: &str) -> Option<Operation> {
        self.record
            .operations
            .iter()
            .find(|op| op.id == id)
            .cloned()
            .or_else(|| {
                self.record
                    .pending
                    .as_ref()
                    .filter(|pending| pending.operation.id == id)
                    .map(|pending| {
                        let mut op = pending.operation.clone();
                        op.outcome = Outcome::Unknown;
                        op.reason = Some("native_outcome_requires_reconciliation".into());
                        op
                    })
            })
    }
    fn prior(&self, id: &str, payload: &str) -> Result<Option<Operation>, Error> {
        operation_id(id)?;
        if let Some(op) = self.operation(id) {
            if op.payload_sha256 != payload {
                return Err(Error::Changed);
            }
            return Ok(Some(op));
        }
        if self.record.operations.len() >= MAX_OPERATIONS {
            return Err(Error::Limit);
        }
        Ok(None)
    }
    pub fn patch(&mut self, id: &str, batch: &PatchBatch) -> Result<Operation, Error> {
        let batch = super::draft::canonical(batch)?;
        let payload = signature(&("patch-v1", &batch))?;
        if let Some(prior) = self.prior(id, &payload)? {
            return Ok(prior);
        }
        self.available()?;
        let mut candidate = self.draft.clone();
        candidate.patch(&batch)?;
        self.commit_draft(id, "patch", payload, candidate)
    }
    pub fn history(&mut self, id: &str, revision: &str, redo: bool) -> Result<Operation, Error> {
        let kind = if redo { "redo" } else { "undo" };
        let payload = signature(&(kind, revision))?;
        if let Some(prior) = self.prior(id, &payload)? {
            return Ok(prior);
        }
        self.available()?;
        let mut candidate = self.draft.clone();
        if redo {
            candidate.redo(revision)?;
        } else {
            candidate.undo(revision)?;
        }
        self.commit_draft(id, kind, payload, candidate)
    }
    fn commit_draft(
        &mut self,
        id: &str,
        kind: &str,
        payload: String,
        candidate: Draft<Base>,
    ) -> Result<Operation, Error> {
        let operation = Operation {
            id: id.into(),
            kind: kind.into(),
            payload_sha256: payload,
            outcome: Outcome::Acknowledged,
            revision: candidate.revision(),
            file_effect: FileEffect::None,
            reason: None,
            notification_pending: false,
        };
        self.record.draft = candidate.snapshot();
        self.record.operations.push(operation.clone());
        if let Err(error) = persist(&mut self.store, &self.record) {
            self.record.operations.pop();
            self.record.draft = self.draft.snapshot();
            return Err(error);
        }
        self.draft = candidate;
        Ok(operation)
    }
    pub fn review(
        &self,
        start: usize,
        limit: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<SaveReview, Error> {
        self.store.validate()?;
        let draft = self
            .draft
            .review(&self.draft.revision(), start, limit, cancelled)?;
        let meta = &self.record.target.metadata;
        let fingerprint = signature(&(
            "save-review-v1",
            &draft.fingerprint,
            &self.record.target,
            EFFECTS,
        ))?;
        Ok(SaveReview {
            draft,
            permissions: meta.mode,
            owner_uid: meta.uid,
            owner_gid: meta.gid,
            metadata_sha256: meta.evidence.clone(),
            effects: EFFECTS.into(),
            fingerprint,
        })
    }
    /// Caller may release the blocking worker after preparation, refresh all
    /// authority asynchronously, then commit while retaining its checkout lease.
    pub fn prepare_save(
        &mut self,
        id: &str,
        review: &str,
        cancelled: impl Fn() -> bool,
    ) -> Result<SavePreparation, Error> {
        let payload = signature(&("save-v1", review))?;
        if let Some(prior) = self.prior(id, &payload)? {
            return Ok(SavePreparation::Existing(prior));
        }
        self.available()?;
        let current = self.review(0, 128, &cancelled)?;
        if current.fingerprint != review {
            return Err(Error::Changed);
        }
        let target = self.record.target.clone();
        let (_parent, source) = match target.validate() {
            Ok(source) => source,
            Err(Error::Changed | Error::File) => {
                return self
                    .conflict(id, "save", payload, "source_binding_changed")
                    .map(SavePreparation::Existing);
            }
            Err(error) => return Err(error),
        };
        if native::content(&source, &cancelled)? != self.draft.base().sha256() {
            return self
                .conflict(id, "save", payload, "source_content_changed")
                .map(SavePreparation::Existing);
        }
        if current.draft.range_count == 0 {
            let checkpoint = Draft::new(self.draft.base().clone())?;
            return self
                .commit_draft(id, "save", payload, checkpoint)
                .map(SavePreparation::Existing);
        }
        let base = self.store.base_cancellable(
            self.draft.length().0,
            |writer| self.draft.write_to(writer, &cancelled),
            &cancelled,
        )?;
        if base.sha256() != current.draft.proposed_sha256 {
            return Err(Error::Changed);
        }
        let (stage_name, mut stage) = self.store.stage(base.length())?;
        let digest = self.draft.write_to(&mut stage, &cancelled)?;
        if digest != base.sha256() {
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
            length: Offset(base.length()),
            metadata: native::metadata(&stage)?,
            observed: String::new(), // The installed change stamp is captured after exchange.
        };
        let after_draft = Draft::new(base)?;
        let operation = Operation {
            id: id.into(),
            kind: "save".into(),
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
    fn conflict(
        &mut self,
        id: &str,
        kind: &str,
        payload: String,
        reason: &str,
    ) -> Result<Operation, Error> {
        let operation = Operation {
            id: id.into(),
            kind: kind.into(),
            payload_sha256: payload,
            outcome: Outcome::Conflict,
            revision: self.draft.revision(),
            file_effect: FileEffect::None,
            reason: Some(reason.into()),
            notification_pending: false,
        };
        self.record.operations.push(operation.clone());
        if let Err(error) = persist(&mut self.store, &self.record) {
            self.record.operations.pop();
            return Err(error);
        }
        Ok(operation)
    }
    /// After durable intent, every failure preserves evidence and returns unknown.
    /// No response loss or restart can cause an automatic second exchange.
    pub fn commit_save(&mut self, prepared: PreparedSave) -> Result<Operation, Error> {
        self.commit_save_cancellable(prepared, || false)
    }
    /// Cancellation is honored before intent publication. After publication the
    /// caller must retain its leases until outcome settlement finishes.
    pub fn commit_save_cancellable(
        &mut self,
        prepared: PreparedSave,
        cancelled: impl Fn() -> bool,
    ) -> Result<Operation, Error> {
        if let Some(prior) = self.prior(
            &prepared.intent.operation.id,
            &prepared.intent.operation.payload_sha256,
        )? {
            return Ok(prior);
        }
        self.available()?;
        if cancelled() {
            return Err(Error::Cancelled);
        }
        if self.draft.revision() != prepared.expected_revision {
            return self.conflict(
                &prepared.intent.operation.id,
                &prepared.intent.operation.kind,
                prepared.intent.operation.payload_sha256.clone(),
                "draft_changed_since_preparation",
            );
        }
        if prepared.intent.prior.validate().is_err() {
            return self.conflict(
                &prepared.intent.operation.id,
                &prepared.intent.operation.kind,
                prepared.intent.operation.payload_sha256.clone(),
                "source_changed_since_preparation",
            );
        }
        let (_, source) = prepared.intent.prior.validate()?;
        if native::content(&source, &cancelled)? != self.record.base.sha256 {
            return self.conflict(
                &prepared.intent.operation.id,
                &prepared.intent.operation.kind,
                prepared.intent.operation.payload_sha256.clone(),
                "source_content_changed_since_preparation",
            );
        }
        self.store.validate()?;
        if cancelled() {
            return Err(Error::Cancelled);
        }
        self.record.pending = Some(prepared.intent);
        persist(&mut self.store, &self.record)?;
        let intent = self.record.pending.as_ref().ok_or(Error::Invalid)?;
        let _effect = native::exchange(self.store.directory(), &intent.stage, &intent.prior);
        // Even an error may follow a native effect. Reconcile by identities and
        // content rather than retrying or attempting an unreviewed rollback.
        self.settle(Some(prepared.after))
    }
    pub fn reconcile(&mut self) -> Result<Operation, Error> {
        self.settle(None)
    }
    fn settle(&mut self, after: Option<Draft<Base>>) -> Result<Operation, Error> {
        self.store.validate()?;
        let intent = self.record.pending.as_ref().ok_or(Error::Invalid)?;
        let mut unknown = intent.operation.clone();
        unknown.outcome = Outcome::Unknown;
        unknown.reason = Some("native_outcome_unknown_evidence_retained".into());
        let verified = (|| {
            let installed_target = Target::capture(&intent.after.root, &intent.after.relative)?;
            if installed_target.file != intent.after.file
                || installed_target.parents != intent.after.parents
                || installed_target.metadata != intent.after.metadata
                || installed_target.length != intent.after.length
            {
                return Err(Error::Changed);
            }
            let (parent, installed) = installed_target.validate()?;
            let displaced = stage_file(&self.store, &intent.stage)?;
            let displaced_stamp = native::generation(&displaced)?;
            if native::id(&displaced)? != intent.prior.file
                || native::metadata(&displaced)? != intent.prior.metadata
                || native::content(&installed, || false)? != intent.new_base.sha256
                || native::content(&displaced, || false)? != self.record.base.sha256
            {
                return Err(Error::Changed);
            }
            native::sync(&installed)?;
            parent
                .try_clone()
                .map_err(|_| Error::Io)?
                .into_std_file()
                .sync_all()
                .map_err(|_| Error::Io)?;
            self.store.sync()?;
            installed_target.validate()?;
            if native::generation(&displaced)? != displaced_stamp {
                return Err(Error::Changed);
            }
            Ok(installed_target)
        })();
        let installed_target = match verified {
            Ok(target) => target,
            Err(_) => return Ok(unknown),
        };
        let draft = match after {
            Some(draft) => draft,
            None => Draft::resume(
                intent.new_base.open(&self.store)?,
                intent.after_draft.clone(),
            )?,
        };
        let mut operation = intent.operation.clone();
        operation.outcome = Outcome::Acknowledged;
        operation.file_effect = FileEffect::Changed;
        operation.reason = None;
        operation.notification_pending = true;
        // Build the next durable record without discarding the prior intent if a
        // record write fails after the file was installed.
        let next = Record {
            version: 1,
            owner: self.record.owner.clone(),
            target: installed_target,
            base: intent.new_base.clone(),
            draft: draft.snapshot(),
            operations: self
                .record
                .operations
                .iter()
                .cloned()
                .chain(Some(operation.clone()))
                .collect(),
            pending: None,
            last_save: Some(LastSave {
                preimage: self.record.base.clone(),
                installed_sha256: intent.new_base.sha256.clone(),
                operation: operation.id.clone(),
            }),
        };
        if persist(&mut self.store, &next).is_err() {
            return Ok(unknown);
        }
        self.record = next;
        self.draft = draft;
        Ok(operation)
    }
    /// Called only after the runtime durably publishes the acknowledged observation.
    /// The operation ID is the producer's deduplication identity, not just a path.
    pub fn notified(&mut self, id: &str) -> Result<(), Error> {
        self.store.validate()?;
        let operation = self
            .record
            .operations
            .iter_mut()
            .find(|op| {
                op.id == id
                    && op.outcome == Outcome::Acknowledged
                    && op.file_effect == FileEffect::Changed
            })
            .ok_or(Error::Invalid)?;
        operation.notification_pending = false;
        persist(&mut self.store, &self.record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binary::{
        draft::{Splice, encode_hex},
        storage::Store,
    };
    use std::path::Path;
    fn setup() -> (tempfile::TempDir, Editor, PathBuf, Owner) {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let project = root.path().join("project");
        std::fs::create_dir(&data).unwrap();
        std::fs::create_dir(&project).unwrap();
        let path = project.join("bytes");
        std::fs::write(&path, b"original").unwrap();
        let owner = Owner {
            session_id: "ses_one".into(),
            directory: project.canonicalize().unwrap(),
        };
        let mut document = Document::open(&owner.directory, "bytes").unwrap();
        let editor = Editor::create(
            Store::open(&data, "bin_one").unwrap(),
            owner.clone(),
            &mut document,
            || false,
        )
        .unwrap();
        (root, editor, path, owner)
    }
    fn patch(editor: &mut Editor, id: &str) -> Operation {
        let batch = PatchBatch {
            revision: editor.draft().revision(),
            splices: vec![Splice {
                offset: Offset(0),
                delete_length: Offset(8),
                expected_hex: encode_hex(b"original"),
                insert_hex: encode_hex(&[0, 255, 128, 27]),
            }],
        };
        editor.patch(id, &batch).unwrap()
    }
    fn ready(editor: &mut Editor, id: &str) -> Box<PreparedSave> {
        let review = editor.review(0, 128, || false).unwrap();
        match editor
            .prepare_save(id, &review.fingerprint, || false)
            .unwrap()
        {
            SavePreparation::Ready(ready) => ready,
            _ => panic!("new save must prepare"),
        }
    }
    #[test]
    fn creating_a_document_cannot_bind_a_different_owner_location() {
        let (root, _editor, path, mut owner) = setup();
        let mut document = Document::open(&owner.directory, "bytes").unwrap();
        owner.directory = root.path().canonicalize().unwrap();
        let result = Editor::create(
            Store::open(&root.path().join("data"), "bin_wrong_location").unwrap(),
            owner,
            &mut document,
            || false,
        );
        assert!(matches!(result, Err(Error::File)));
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }

    #[test]
    fn cancellation_before_intent_leaves_source_and_receipts_unchanged() {
        let (_root, mut editor, path, _owner) = setup();
        patch(&mut editor, "patch_cancel");
        let prepared = ready(&mut editor, "save_cancel");
        let identity = editor.target().file.clone();
        assert!(matches!(
            editor.commit_save_cancellable(*prepared, || true),
            Err(Error::Cancelled)
        ));
        assert!(!editor.unresolved());
        assert!(editor.operation("save_cancel").is_none());
        assert_eq!(editor.target().file, identity);
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let prepared = ready(&mut editor, "save_cancel");
        assert_eq!(
            editor.commit_save(*prepared).unwrap().outcome,
            Outcome::Acknowledged
        );
        assert_eq!(std::fs::read(path).unwrap(), [0, 255, 128, 27]);
    }

    #[test]
    fn equivalent_patch_encodings_reuse_the_durable_receipt() {
        let (_root, mut editor, _path, _owner) = setup();
        let mut batch = PatchBatch {
            revision: editor.draft().revision(),
            splices: vec![
                Splice {
                    offset: Offset(0),
                    delete_length: Offset(1),
                    expected_hex: "6F".into(),
                    insert_hex: "FF".into(),
                },
                Splice {
                    offset: Offset(7),
                    delete_length: Offset(1),
                    expected_hex: "6C".into(),
                    insert_hex: "AA".into(),
                },
            ],
        };
        let first = editor.patch("canonical_patch", &batch).unwrap();
        batch.splices.reverse();
        for splice in &mut batch.splices {
            splice.expected_hex.make_ascii_lowercase();
            splice.insert_hex.make_ascii_lowercase();
        }
        let retry = editor.patch("canonical_patch", &batch).unwrap();
        assert_eq!(retry.revision, first.revision);
        assert_eq!(retry.payload_sha256, first.payload_sha256);
        assert_eq!(
            editor.draft().read(0, 8).unwrap(),
            [255, 114, 105, 103, 105, 110, 97, 170]
        );
    }

    #[test]
    fn native_save_is_durable_and_retry_returns_the_original_receipt_without_another_exchange() {
        let (root, mut editor, path, owner) = setup();
        patch(&mut editor, "patch_one");
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let review = editor.review(0, 128, || false).unwrap();
        let ready = ready(&mut editor, "save_one");
        let receipt = editor.commit_save(*ready).unwrap();
        assert_eq!(receipt.outcome, Outcome::Acknowledged);
        assert!(receipt.file_effect == FileEffect::Changed && receipt.notification_pending);
        assert_eq!(std::fs::read(&path).unwrap(), [0, 255, 128, 27]);
        let identity = editor.target().file.clone();
        match editor
            .prepare_save("save_one", &review.fingerprint, || false)
            .unwrap()
        {
            SavePreparation::Existing(prior) => assert_eq!(prior.revision, receipt.revision),
            _ => panic!("save retry prepared another exchange"),
        }
        assert_eq!(editor.target().file, identity);
        assert!(
            editor
                .prepare_save("save_one", "other_payload", || false)
                .is_err()
        );
        drop(editor);
        let editor = Editor::resume(
            Store::open(&root.path().join("data"), "bin_one").unwrap(),
            &owner,
        )
        .unwrap();
        assert_eq!(
            editor.operation("save_one").unwrap().outcome,
            Outcome::Acknowledged
        );
        assert!(!editor.unresolved());
        assert_eq!(editor.draft().read(0, 4).unwrap(), [0, 255, 128, 27]);
    }
    #[test]
    fn dirty_commands_and_history_survive_restart_and_operation_ids_cannot_change_payload() {
        let (root, mut editor, path, owner) = setup();
        let prior = editor.draft().revision();
        let batch = PatchBatch {
            revision: prior.clone(),
            splices: vec![Splice {
                offset: Offset(0),
                delete_length: Offset(1),
                expected_hex: "6f".into(),
                insert_hex: "4f".into(),
            }],
        };
        let applied = editor.patch("patch_one", &batch).unwrap();
        assert_eq!(
            editor.patch("patch_one", &batch).unwrap().revision,
            applied.revision
        );
        let different = PatchBatch {
            revision: prior,
            splices: vec![Splice {
                offset: Offset(0),
                delete_length: Offset(1),
                expected_hex: "6f".into(),
                insert_hex: "58".into(),
            }],
        };
        assert!(editor.patch("patch_one", &different).is_err());
        drop(editor);
        let mut editor = Editor::resume(
            Store::open(&root.path().join("data"), "bin_one").unwrap(),
            &owner,
        )
        .unwrap();
        assert_eq!(editor.draft().read(0, 8).unwrap(), b"Original");
        let revision = editor.draft().revision();
        let undo = editor.history("undo_one", &revision, false).unwrap();
        assert_eq!(editor.draft().read(0, 8).unwrap(), b"original");
        editor.history("redo_one", &undo.revision, true).unwrap();
        assert_eq!(editor.draft().read(0, 8).unwrap(), b"Original");
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }
    #[test]
    fn equal_size_equal_mtime_external_bytes_conflict_without_a_write_or_losing_the_draft() {
        let (_root, mut editor, path, _owner) = setup();
        patch(&mut editor, "patch_one");
        let review = editor.review(0, 128, || false).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, b"external").unwrap();
        std::fs::File::open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let result = editor
            .prepare_save("save_conflict", &review.fingerprint, || false)
            .unwrap();
        match result {
            SavePreparation::Existing(op) => {
                assert_eq!(op.outcome, Outcome::Conflict);
                assert_eq!(op.file_effect, FileEffect::None);
            }
            _ => panic!("stale bytes were accepted"),
        }
        assert_eq!(std::fs::read(path).unwrap(), b"external");
        assert_eq!(editor.draft().read(0, 4).unwrap(), [0, 255, 128, 27]);
    }
    #[test]
    fn unchanged_save_checkpoints_without_replacing_the_file_or_emitting_a_change() {
        let (_root, mut editor, path, _owner) = setup();
        let before = editor.target().file.clone();
        let review = editor.review(0, 128, || false).unwrap();
        let SavePreparation::Existing(op) = editor
            .prepare_save("no_change", &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("unchanged content created a replacement")
        };
        assert_eq!(op.outcome, Outcome::Acknowledged);
        assert_eq!(op.file_effect, FileEffect::None);
        assert!(!op.notification_pending);
        assert_eq!(editor.target().file, before);
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }
    #[test]
    fn failed_receipt_flush_after_native_effect_is_unknown_until_durable_reopen() {
        let (root, mut editor, path, owner) = setup();
        patch(&mut editor, "patch_one");
        let review = editor.review(0, 128, || false).unwrap();
        let prepared = ready(&mut editor, "save_flush");
        editor.store.fail_after_publication = Some(editor.store.publications + 2);
        assert_eq!(
            editor.commit_save(*prepared).unwrap().outcome,
            Outcome::Unknown
        );
        assert!(editor.store.validate().is_err());
        assert!(editor.unresolved());
        let identity = Target::capture(&owner.directory, Path::new("bytes"))
            .unwrap()
            .file;
        assert_eq!(std::fs::read(&path).unwrap(), [0, 255, 128, 27]);
        let SavePreparation::Existing(receipt) = editor
            .prepare_save("save_flush", &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("uncertain retry repeated a native effect")
        };
        assert_eq!(receipt.outcome, Outcome::Unknown);
        drop(editor);
        let resumed = Editor::resume(
            Store::open(&root.path().join("data"), "bin_one").unwrap(),
            &owner,
        )
        .unwrap();
        assert_eq!(
            resumed.operation("save_flush").unwrap().outcome,
            Outcome::Acknowledged
        );
        assert_eq!(resumed.target().file, identity);
    }
    #[test]
    fn failed_intent_flush_fences_the_editor_without_changing_the_target() {
        let (root, mut editor, path, owner) = setup();
        patch(&mut editor, "patch_one");
        let prepared = ready(&mut editor, "save_flush");
        editor.store.fail_after_publication = Some(editor.store.publications + 1);
        assert!(editor.commit_save(*prepared).is_err());
        assert!(editor.store.validate().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        drop(editor);
        let mut resumed = Editor::resume(
            Store::open(&root.path().join("data"), "bin_one").unwrap(),
            &owner,
        )
        .unwrap();
        assert_eq!(resumed.reconcile().unwrap().outcome, Outcome::Unknown);
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }
    #[test]
    fn a_same_size_same_mtime_change_after_preparation_is_refused_before_exchange() {
        let (_root, mut editor, path, _owner) = setup();
        patch(&mut editor, "patch_one");
        let prepared = ready(&mut editor, "save_race");
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, b"external").unwrap();
        File::open(&path).unwrap().set_modified(modified).unwrap();
        assert_eq!(
            editor.commit_save(*prepared).unwrap().outcome,
            Outcome::Conflict
        );
        assert_eq!(std::fs::read(path).unwrap(), b"external");
        assert!(!editor.unresolved());
    }
    #[test]
    fn equivalent_hex_case_and_independent_splice_order_share_one_operation_identity() {
        let (_root, mut editor, _path, _owner) = setup();
        let revision = editor.draft().revision();
        let first = PatchBatch {
            revision: revision.clone(),
            splices: vec![
                crate::binary::draft::Splice {
                    offset: Offset(0),
                    delete_length: Offset(1),
                    expected_hex: "6F".into(),
                    insert_hex: "FF".into(),
                },
                crate::binary::draft::Splice {
                    offset: Offset(7),
                    delete_length: Offset(1),
                    expected_hex: "6C".into(),
                    insert_hex: "EE".into(),
                },
            ],
        };
        let result = editor.patch("canonical_patch", &first).unwrap();
        let equivalent = PatchBatch {
            revision,
            splices: first
                .splices
                .iter()
                .rev()
                .cloned()
                .map(|mut splice| {
                    splice.expected_hex.make_ascii_lowercase();
                    splice.insert_hex.make_ascii_lowercase();
                    splice
                })
                .collect(),
        };
        assert_eq!(
            editor
                .patch("canonical_patch", &equivalent)
                .unwrap()
                .revision,
            result.revision
        );
        assert_eq!(editor.record.operations.len(), 1);
    }
    #[test]
    fn source_aliases_and_wrong_session_cannot_prepare_or_resume_mutation() {
        let (root, editor, path, owner) = setup();
        drop(editor);
        let store = Store::open(&root.path().join("data"), "bin_one").unwrap();
        assert!(
            Editor::resume(
                store,
                &Owner {
                    session_id: "ses_other".into(),
                    directory: owner.directory.clone()
                }
            )
            .is_err()
        );
        let alias = owner.directory.join("alias");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let mut doc = Document::open(&owner.directory, "alias").unwrap();
        let store = Store::open(&root.path().join("data"), "bin_alias").unwrap();
        assert!(Editor::create(store, owner, &mut doc, || false).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }
    #[test]
    fn durable_intent_after_exchange_recovers_acknowledgement_without_repeating_effect() {
        let (root, mut editor, path, owner) = setup();
        patch(&mut editor, "patch_one");
        let ready = ready(&mut editor, "save_cut");
        editor.record.pending = Some(ready.intent);
        persist(&mut editor.store, &editor.record).unwrap();
        let intent = editor.record.pending.as_ref().unwrap();
        native::exchange(editor.store.directory(), &intent.stage, &intent.prior).unwrap();
        let installed = Target::capture(&owner.directory, Path::new("bytes"))
            .unwrap()
            .file;
        drop(editor);
        let mut resumed = Editor::resume(
            Store::open(&root.path().join("data"), "bin_one").unwrap(),
            &owner,
        )
        .unwrap();
        assert_eq!(
            resumed.operation("save_cut").unwrap().outcome,
            Outcome::Unknown
        );
        assert_eq!(resumed.reconcile().unwrap().outcome, Outcome::Acknowledged);
        assert_eq!(resumed.target().file, installed);
        assert_eq!(std::fs::read(path).unwrap(), [0, 255, 128, 27]);
    }
    #[test]
    fn an_unexecuted_or_uncertain_intent_preserves_evidence_and_is_never_automatically_retried() {
        let (root, mut editor, path, owner) = setup();
        patch(&mut editor, "patch_one");
        let ready = ready(&mut editor, "save_cut");
        let stage = ready.intent.stage.clone();
        editor.record.pending = Some(ready.intent);
        persist(&mut editor.store, &editor.record).unwrap();
        drop(editor);
        let mut resumed = Editor::resume(
            Store::open(&root.path().join("data"), "bin_one").unwrap(),
            &owner,
        )
        .unwrap();
        assert_eq!(resumed.reconcile().unwrap().outcome, Outcome::Unknown);
        assert!(resumed.unresolved());
        assert_eq!(std::fs::read(path).unwrap(), b"original");
        assert!(
            root.path()
                .join("data/binary/bin_one")
                .join(stage)
                .is_file()
        );
        let review = resumed.review(0, 128, || false).unwrap();
        assert!(
            resumed
                .prepare_save("another_save", &review.fingerprint, || false)
                .is_err()
        );
    }
}

#[cfg(test)]
#[path = "editor/death_tests.rs"]
mod death_tests;
