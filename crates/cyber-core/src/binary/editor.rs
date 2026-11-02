//! Durable document operations and recoverable native save. Callers retain fresh
//! owner/read/edit/Mode authority and the checkout mutation lease around commit.
//! A native intent is never automatically re-executed after interruption.
use super::{
    Document, Error, Offset,
    draft::{Draft, DraftState, ImmutableBase, PatchBatch},
    native::{self, Target},
    storage::{Base, Store},
};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::OpenOptions;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

#[path = "editor/catalog.rs"]
mod catalog;
#[path = "editor/changes.rs"]
mod changes;
#[path = "editor/history.rs"]
mod history;
#[path = "editor/restore.rs"]
mod restore;
#[path = "editor/retention.rs"]
mod retention;
#[path = "editor/versions.rs"]
mod versions;
pub use catalog::{StoredDocument, StoredDocuments};
pub use changes::AcknowledgedChange;
pub use retention::Cleanup;
pub use versions::VersionView;

const MAX_OPERATIONS: usize = 4096;
const EFFECTS: &str = "Permissions, executable bits, owner/group, supported extended attributes and ACL are preserved. Native replacement changes file identity and updates modification time. Save checkpoints draft undo/redo; Restore Last Save uses the retained preimage. External checksums, signatures and irreversible observer effects are not repaired. Uncooperative writers can leave an unknown outcome; evidence is retained.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub session_id: String,
    /// Exact canonical Session Location, which can be below the project root.
    pub directory: PathBuf,
    pub root: PathBuf,
    pub scope: DraftScope,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DraftScope {
    Manual,
    Model { operation: String },
}
pub use super::receipts::{FileEffect, Operation, Outcome, SaveReview};

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
    #[serde(default)]
    evidence: Option<versions::Evidence>,
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
    #[serde(default)]
    evidence: Option<versions::Evidence>,
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
    _pins: native::Pins,
    draft: Draft<Base>,
    record: Record,
}
/// Borrows the actual exclusive document store lock, not a boolean supplied by
/// a caller. This proves filesystem-only execution ownership, never a file outcome.
pub struct RecoveryProof<'a> {
    editor: &'a Editor,
}
impl RecoveryProof<'_> {
    pub fn document_id(&self) -> &str {
        self.editor.store.document_id()
    }
    pub fn owner(&self) -> &Owner {
        self.editor.owner()
    }
    pub fn validate(&self, storage: &super::storage::StorageRoot) -> Result<(), Error> {
        self.editor.store.belongs_to(storage)
    }
}

pub struct PreparedSave {
    intent: Intent,
    after: Draft<Base>,
    expected_revision: String,
    _history: File,
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
fn encoded_size(value: &impl Serialize) -> Result<u64, Error> {
    let mut count = Counter(0);
    serde_json::to_writer(&mut count, value).map_err(|_| Error::Io)?;
    Ok(count.0)
}
fn persist(store: &mut Store, record: &Record) -> Result<(), Error> {
    store.record("document.json", encoded_size(record)?, |writer| {
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
        if owner.session_id.is_empty()
            || !owner.directory.is_absolute()
            || !owner.root.is_absolute()
        {
            return Err(Error::Invalid);
        }
        if document.root() != owner.root || !owner.directory.starts_with(&owner.root) {
            return Err(Error::File);
        }
        // Never replace an existing document, including one with unresolved evidence.
        match store.directory().symlink_metadata("document.json") {
            Ok(_) => return Err(Error::Changed),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Io),
        }
        store.ensure_history_lock()?;
        catalog::publish_header(&mut store, &owner, document)?;
        let (base, target) = document.prepare(&store, cancelled)?;
        if target.root != owner.root {
            return Err(Error::File);
        }
        let pins = document.retain_pins()?;
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
            _pins: pins,
            draft,
            record,
        })
    }
    pub fn resume(mut store: Store, owner: &Owner) -> Result<Self, Error> {
        let record: Record = serde_json::from_slice(&store.read_record("document.json")?)
            .map_err(|_| Error::Invalid)?;
        if record.version != 1
            || &record.owner != owner
            || record.operations.len() > MAX_OPERATIONS
            || record.target.root != owner.root
        {
            return Err(Error::File);
        }
        store.ensure_history_lock()?;
        let pins = record.target.pins()?;
        let draft = Draft::resume(record.base.open(&store)?, record.draft.clone())?;
        Ok(Self {
            store,
            _pins: pins,
            draft,
            record,
        })
    }
    pub fn recovery_proof(&self) -> Result<RecoveryProof<'_>, Error> {
        self.store.validate()?;
        Ok(RecoveryProof { editor: self })
    }
    pub fn draft(&self) -> &Draft<Base> {
        &self.draft
    }
    /// Refuse stale in-memory draft disclosure after an uncertain journal write.
    pub fn validate_read_resource(&self) -> Result<(), Error> {
        self.store.validate()?;
        self.record.target.validate_read_resource()
    }
    pub fn resource(&self) -> super::Resource {
        super::Resource(self.record.target.root.join(&self.record.target.relative))
    }

    pub fn target(&self) -> &Target {
        &self.record.target
    }
    pub fn owner(&self) -> &Owner {
        &self.record.owner
    }
    pub fn pending_operation(&self) -> Option<Operation> {
        self.record
            .pending
            .as_ref()
            .and_then(|pending| self.operation(&pending.operation.id))
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
    /// A fenced/uncertain journal must be reopened before absence can be asserted.
    pub fn lookup_operation(&self, id: &str) -> Result<Option<Operation>, Error> {
        self.store.validate()?;
        operation_id(id)?;
        Ok(self.operation(id))
    }
    pub fn lookup_installation(
        &self,
        id: &str,
        review: &str,
        restore: bool,
    ) -> Result<Option<Operation>, Error> {
        let payload = signature(&(if restore { "restore-v1" } else { "save-v1" }, review))?;
        let operation = self.lookup_operation(id)?;
        if operation
            .as_ref()
            .is_some_and(|operation| operation.payload_sha256 != payload)
        {
            return Err(Error::Changed);
        }
        Ok(operation)
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
    /// Explicitly discard local commands without changing the source, preimages,
    /// operation receipts or unresolved native evidence.
    pub fn discard(&mut self, id: &str, revision: &str) -> Result<Operation, Error> {
        let payload = signature(&("discard-v1", revision))?;
        if let Some(prior) = self.prior(id, &payload)? {
            return Ok(prior);
        }
        self.available()?;
        if revision != self.draft.revision() {
            return Err(Error::Changed);
        }
        let candidate = Draft::new(self.draft.base().clone())?;
        self.commit_draft(id, "discard", payload, candidate)
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
        self.publish_last_version()?;
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
        let evidence = versions::Evidence::prepared(
            self,
            &operation,
            &current.draft,
            after.clone(),
            &after_draft,
            self.draft.ranges(),
            &stage_name,
        )?;
        let intent = Intent {
            operation,
            prior: target,
            after,
            new_base: BaseRef::of(after_draft.base()),
            after_draft: after_draft.snapshot(),
            stage: stage_name,
            evidence: Some(evidence),
        };
        Ok(SavePreparation::Ready(Box::new(PreparedSave {
            intent,
            after: after_draft,
            expected_revision: self.draft.revision(),
            _history: self.store.history_lease(false)?,
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
        self.commit_save_checked(prepared, cancelled, || Ok(()))
    }
    /// The host rechecks trusted current read/edit/Mode and ancestor authority
    /// after content verification and capacity reservation, before native intent.
    /// The check must not prompt, invoke hooks/models, or release mutation owners.
    /// A refusal has no native effect and leaves the operation eligible for an
    /// explicit newly authorized attempt. It does not manufacture a save receipt.
    pub fn commit_save_checked(
        &mut self,
        mut prepared: PreparedSave,
        cancelled: impl Fn() -> bool,
        authorize: impl FnOnce() -> Result<(), Error>,
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
        let stage = stage_file(&self.store, &prepared.intent.stage)?;
        if native::id(&stage)? != prepared.intent.after.file
            || native::metadata(&stage)? != prepared.intent.after.metadata
            || stage.metadata().map_err(|_| Error::Io)?.len() != prepared.intent.after.length.0
            || native::content(&stage, &cancelled)? != prepared.intent.new_base.sha256
        {
            return Err(Error::Changed);
        }
        if cancelled() {
            return Err(Error::Cancelled);
        }
        // Only the pending JSON value changes. Count the prospective publication
        // by streaming serialization, without cloning the bounded draft history.
        // Retention starts at commit preparation, not when a caller first held
        // the prepared token. Fix the timestamp before both capacity forecasts
        // and intent publication so the reserved outcome shape stays exact.
        if let Some(evidence) = &mut prepared.intent.evidence {
            evidence.info.created_ms = chrono::Utc::now().timestamp_millis();
        }
        let pending_size = encoded_size(&Some(&prepared.intent))?;
        let prior_pending_size = encoded_size(&self.record.pending)?;
        let intent_bytes = encoded_size(&self.record)?
            .checked_sub(prior_pending_size)
            .and_then(|size| size.checked_add(pending_size))
            .ok_or(Error::Limit)?;
        // The installed observation is always a 64-character SHA-256 token. Its
        // value is captured after exchange; its encoded size is known beforehand.
        let mut installed = prepared.intent.after.clone();
        installed.observed = "0".repeat(64);
        let (outcome, _) = self.acknowledged_record(&prepared.intent, installed, &prepared.after);
        self.store.reserve_exchange(
            intent_bytes,
            encoded_size(&outcome)?,
            prepared
                .intent
                .prior
                .length
                .0
                .saturating_sub(prepared.intent.after.length.0),
        )?;
        authorize()?;
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
            super::storage::flush_dir(&parent)?;
            self.store.sync()?;
            installed_target.validate()?;
            if native::generation(&displaced)? != displaced_stamp {
                return Err(Error::Changed);
            }
            Ok((installed_target, installed))
        })();
        let (installed_target, installed_pin) = match verified {
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
        // Preserve the prior intent if publishing this outcome fails.
        let (next, operation) = self.acknowledged_record(intent, installed_target, &draft);
        if persist(&mut self.store, &next).is_err() {
            return Ok(unknown);
        }
        self.record = next;
        self.draft = draft;
        self._pins._file = Some(installed_pin);
        Ok(operation)
    }
    fn acknowledged_record(
        &self,
        intent: &Intent,
        target: Target,
        draft: &Draft<Base>,
    ) -> (Record, Operation) {
        let mut operation = intent.operation.clone();
        operation.outcome = Outcome::Acknowledged;
        operation.file_effect = FileEffect::Changed;
        operation.reason = None;
        operation.notification_pending = true;
        let mut evidence = intent.evidence.clone();
        if let Some(evidence) = &mut evidence {
            evidence.target = target.clone();
            evidence.operation = operation.clone();
        }
        let record = Record {
            version: 1,
            owner: self.record.owner.clone(),
            target,
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
                evidence,
            }),
        };
        (record, operation)
    }
    /// Called only after the runtime durably publishes the acknowledged observation.
    /// The operation ID is the producer's deduplication identity, not just a path.
    pub fn notified(&mut self, id: &str) -> Result<(), Error> {
        self.finish_notification(id, None)
    }
    /// A durable producer disposition can close notification work without
    /// claiming that a handler ran or changing the acknowledged source outcome.
    pub fn notification_suppressed(&mut self, id: &str, reason: &str) -> Result<(), Error> {
        if reason.is_empty()
            || reason.len() > 64
            || !reason.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        {
            return Err(Error::Invalid);
        }
        self.finish_notification(id, Some(format!("notification_suppressed:{reason}")))
    }
    fn finish_notification(&mut self, id: &str, reason: Option<String>) -> Result<(), Error> {
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
        if !operation.notification_pending {
            return Ok(());
        }
        operation.notification_pending = false;
        operation.reason = reason;
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
    pub(super) fn setup() -> (tempfile::TempDir, Editor, PathBuf, Owner) {
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
            root: project.canonicalize().unwrap(),
            scope: DraftScope::Manual,
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
    pub(super) fn patch(editor: &mut Editor, id: &str) -> Operation {
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
    pub(super) fn ready(editor: &mut Editor, id: &str) -> Box<PreparedSave> {
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
    fn installation_retries_bind_the_original_review_and_notification_disposition() {
        let (_root, mut editor, path, _owner) = setup();
        patch(&mut editor, "patch_once");
        let review = editor.review(0, 128, || false).unwrap();
        let prepared = ready(&mut editor, "saved_once");
        editor.commit_save(*prepared).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let notification = editor.notification_key("saved_once").unwrap().unwrap();
        assert_eq!(
            editor
                .lookup_installation("saved_once", &review.fingerprint, false)
                .unwrap()
                .unwrap()
                .outcome,
            Outcome::Acknowledged
        );
        assert!(
            editor
                .lookup_installation("saved_once", &"0".repeat(64), false)
                .is_err()
        );
        assert!(
            editor
                .lookup_installation("saved_once", &review.fingerprint, true)
                .is_err()
        );
        assert!(
            editor
                .notification_suppressed("saved_once", "invalid reason")
                .is_err()
        );
        editor
            .notification_suppressed("saved_once", "native_unavailable")
            .unwrap();
        editor.notified("saved_once").unwrap();
        let receipt = editor.lookup_operation("saved_once").unwrap().unwrap();
        assert!(!receipt.notification_pending);
        assert_eq!(receipt.outcome, Outcome::Acknowledged);
        assert_eq!(receipt.file_effect, FileEffect::Changed);
        assert_eq!(
            receipt.reason.as_deref(),
            Some("notification_suppressed:native_unavailable")
        );
        std::fs::write(&path, b"later external source").unwrap();
        assert_eq!(
            editor.notification_key("saved_once").unwrap().as_deref(),
            Some(notification.as_str())
        );
        assert!(
            editor
                .acknowledged_change("saved_once")
                .unwrap()
                .unwrap()
                .observed_stamp()
                .is_err(),
            "old metadata must not confirm a fresh source observation"
        );
        assert_ne!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn final_save_authority_refusal_preserves_source_draft_and_receipt_absence() {
        let (_root, mut editor, path, _owner) = setup();
        patch(&mut editor, "patch_one");
        let revision = editor.draft().revision();
        let prepared = ready(&mut editor, "save_checked");
        let called = std::cell::Cell::new(false);
        let result = editor.commit_save_checked(
            *prepared,
            || false,
            || {
                called.set(true);
                Err(Error::Changed)
            },
        );
        assert!(called.get());
        assert!(matches!(result, Err(Error::Changed)));
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(editor.draft().revision(), revision);
        assert_eq!(editor.draft().read(0, 4).unwrap(), [0, 255, 128, 27]);
        assert!(!editor.unresolved());
        assert!(editor.lookup_operation("save_checked").unwrap().is_none());
        // A stricter check can also observe Stop while checking authority.
        let cancelled = std::cell::Cell::new(false);
        let prepared = ready(&mut editor, "save_checked");
        assert!(matches!(
            editor.commit_save_checked(
                *prepared,
                || cancelled.get(),
                || {
                    cancelled.set(true);
                    Ok(())
                }
            ),
            Err(Error::Cancelled)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert!(!editor.unresolved());
        let prepared = ready(&mut editor, "save_checked");
        assert_eq!(
            editor
                .commit_save_checked(*prepared, || false, || Ok(()))
                .unwrap()
                .outcome,
            Outcome::Acknowledged
        );
        assert_eq!(std::fs::read(path).unwrap(), [0, 255, 128, 27]);
    }
    #[test]
    fn prepared_editors_keep_native_source_and_directory_pins_after_the_live_view_is_dropped() {
        use std::os::unix::fs::MetadataExt;
        let (_root, editor, _path, owner) = setup();
        let original_root = editor.target().parents[0].clone();
        let original_file = editor.target().file.clone();
        std::fs::remove_dir_all(&owner.root).unwrap();
        std::fs::create_dir(&owner.root).unwrap();
        std::fs::write(owner.root.join("bytes"), b"replacement").unwrap();
        assert_eq!(
            native::id(&editor._pins._directories[0]).unwrap(),
            original_root
        );
        let source = editor._pins._file.as_ref().unwrap();
        assert_eq!(native::id(source).unwrap(), original_file);
        assert_eq!(source.metadata().unwrap().nlink(), 0);
        assert!(editor.target().validate_read_resource().is_err());
    }

    #[test]
    fn uncertain_draft_publication_refuses_stale_reads_and_receipt_absence_until_reopen() {
        let (root, mut editor, path, owner) = setup();
        let batch = PatchBatch {
            revision: editor.draft().revision(),
            splices: vec![crate::binary::draft::Splice {
                offset: Offset(0),
                delete_length: Offset(1),
                expected_hex: "6f".into(),
                insert_hex: "ff".into(),
            }],
        };
        editor.store.fail_after_publication = Some(editor.store.publications + 1);
        assert!(editor.patch("patch_uncertain", &batch).is_err());
        assert!(editor.validate_read_resource().is_err());
        assert!(editor.lookup_operation("patch_uncertain").is_err());
        assert!(editor.lookup_operation("never_seen").is_err());
        drop(editor);
        let resumed = Editor::resume(
            Store::open(&root.path().join("data"), "bin_one").unwrap(),
            &owner,
        )
        .unwrap();
        resumed.validate_read_resource().unwrap();
        assert_eq!(
            resumed
                .lookup_operation("patch_uncertain")
                .unwrap()
                .unwrap()
                .outcome,
            Outcome::Acknowledged
        );
        assert!(resumed.lookup_operation("never_seen").unwrap().is_none());
        assert_eq!(resumed.draft().read(0, 1).unwrap(), [255]);
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }

    #[test]
    fn recovered_editor_retains_all_parent_pins_across_later_directory_removal() {
        let (root, editor, _path, owner) = setup();
        drop(editor);
        std::fs::create_dir(owner.root.join("nested")).unwrap();
        std::fs::write(owner.root.join("nested/bytes"), [0, 255]).unwrap();
        let mut doc = Document::open(&owner.root, "nested/bytes").unwrap();
        let editor = Editor::create(
            Store::open(&root.path().join("data"), "bin_nested").unwrap(),
            owner.clone(),
            &mut doc,
            || false,
        )
        .unwrap();
        drop((doc, editor));
        let editor = Editor::resume(
            Store::open(&root.path().join("data"), "bin_nested").unwrap(),
            &owner,
        )
        .unwrap();
        assert_eq!(editor._pins._directories.len(), 2);
        let original = editor.target().parents[1].clone();
        std::fs::remove_dir_all(owner.root.join("nested")).unwrap();
        std::fs::create_dir(owner.root.join("nested")).unwrap();
        assert_eq!(native::id(&editor._pins._directories[1]).unwrap(), original);
        let replacement = cap_std::fs::Dir::open_ambient_dir(
            owner.root.join("nested"),
            cap_std::ambient_authority(),
        )
        .unwrap();
        assert_ne!(native::id(&replacement.into_std_file()).unwrap(), original);
        assert!(editor.validate_read_resource().is_err());
    }

    #[test]
    fn restore_reserves_the_displaced_file_quota_before_any_native_effect() {
        let (root, mut editor, path, _owner) = setup();
        editor
            .patch(
                "grow_for_restore",
                &PatchBatch {
                    revision: editor.draft().revision(),
                    splices: vec![crate::binary::draft::Splice {
                        offset: Offset(0),
                        delete_length: Offset(8),
                        expected_hex: crate::binary::draft::encode_hex(b"original"),
                        insert_hex: "ff".repeat(48 * 1024),
                    }],
                },
            )
            .unwrap();
        let save = ready(&mut editor, "save_grown");
        assert_eq!(
            editor.commit_save(*save).unwrap().outcome,
            Outcome::Acknowledged
        );
        let review = editor.restore_review(|| false).unwrap();
        for outcome_space_missing in [false, true] {
            let SavePreparation::Ready(prepared) = editor
                .prepare_restore("restore_quota", &review.fingerprint, || false)
                .unwrap()
            else {
                panic!("restore preparation");
            };
            let intent_bytes = serde_json::to_vec(&prepared.intent).unwrap().len() as u64;
            let displaced_extra = prepared.intent.prior.length.0 - prepared.intent.after.length.0;
            let allowance = if outcome_space_missing {
                let mut installed = prepared.intent.after.clone();
                installed.observed = "0".repeat(64);
                let (outcome, _) =
                    editor.acknowledged_record(&prepared.intent, installed, &prepared.after);
                let publication = encoded_size(&editor.record).unwrap() + intent_bytes
                    - encoded_size(&editor.record.pending).unwrap();
                let growth = publication - encoded_size(&editor.record).unwrap();
                let allowance = displaced_extra + growth + encoded_size(&outcome).unwrap() / 2;
                assert!(allowance > publication, "intent publication must still fit");
                allowance
            } else {
                let allowance = intent_bytes + 4096;
                assert!(
                    allowance < displaced_extra,
                    "displacement must exceed publication allowance"
                );
                allowance
            };
            let directory = root.path().join("data/binary/bin_one");
            let used = std::fs::read_dir(&directory)
                .unwrap()
                .map(|entry| entry.unwrap().metadata().unwrap().len())
                .sum::<u64>();
            let filler_path = directory.join("quota_filler");
            let filler = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&filler_path)
                .unwrap();
            // Sparse allocation: exercise logical document quota without allocating 2 GiB.
            filler
                .set_len(super::super::storage::MAX_STORAGE - used - allowance)
                .unwrap();
            let result = editor.commit_save(*prepared);
            assert!(
                matches!(result, Err(Error::Limit)),
                "{result:?}; source length after refusal: {}",
                std::fs::metadata(&path).unwrap().len()
            );
            assert!(!editor.unresolved());
            assert!(editor.lookup_operation("restore_quota").unwrap().is_none());
            assert_eq!(std::fs::read(&path).unwrap(), vec![255; 48 * 1024]);
            drop(filler);
            std::fs::remove_file(filler_path).unwrap();
        }
        let SavePreparation::Ready(prepared) = editor
            .prepare_restore("restore_quota", &review.fingerprint, || false)
            .unwrap()
        else {
            panic!("restore after capacity release");
        };
        assert_eq!(
            editor.commit_save(*prepared).unwrap().outcome,
            Outcome::Acknowledged
        );
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }

    #[test]
    fn changed_replacement_bytes_are_refused_before_intent_or_source_effects() {
        let (root, mut editor, path, _owner) = setup();
        patch(&mut editor, "patch_replacement");
        let prepared = ready(&mut editor, "save_replacement");
        let stage = root
            .path()
            .join("data/binary/bin_one")
            .join(&prepared.intent.stage);
        std::fs::write(stage, [9; 4]).unwrap();
        assert!(matches!(editor.commit_save(*prepared), Err(Error::Changed)));
        assert!(!editor.unresolved());
        assert!(editor.operation("save_replacement").is_none());
        assert_eq!(std::fs::read(path).unwrap(), b"original");
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
    fn explicit_discard_survives_restart_without_changing_source_or_losing_receipts() {
        let (root, mut editor, path, owner) = setup();
        patch(&mut editor, "patch_discard");
        let revision = editor.draft().revision();
        assert!(editor.discard("discard_stale", "stale").is_err());
        let receipt = editor.discard("discard_one", &revision).unwrap();
        assert_eq!(receipt.file_effect, FileEffect::None);
        assert!(!receipt.notification_pending);
        assert!(!editor.draft().can_undo() && !editor.draft().can_redo());
        assert_eq!(editor.draft().read(0, 8).unwrap(), b"original");
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        drop(editor);
        let mut editor = Editor::resume(
            Store::open(&root.path().join("data"), "bin_one").unwrap(),
            &owner,
        )
        .unwrap();
        assert_eq!(
            editor.discard("discard_one", &revision).unwrap().revision,
            receipt.revision
        );
        assert!(editor.operation("patch_discard").is_some());
        assert!(!editor.draft().can_undo());
    }

    #[test]
    fn owner_namespace_and_exact_location_cannot_be_rebound_on_resume() {
        let (root, editor, _path, owner) = setup();
        drop(editor);
        let mut wrong = owner.clone();
        wrong.scope = DraftScope::Model {
            operation: "call_model".into(),
        };
        assert!(
            Editor::resume(
                Store::open(&root.path().join("data"), "bin_one").unwrap(),
                &wrong
            )
            .is_err()
        );
        let nested = owner.root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        wrong = owner.clone();
        wrong.directory = nested.clone();
        assert!(
            Editor::resume(
                Store::open(&root.path().join("data"), "bin_one").unwrap(),
                &wrong
            )
            .is_err()
        );
        // A new document can bind a nested Location without confusing it with
        // its project-root source routing authority.
        let mut document = Document::open(&owner.root, "bytes").unwrap();
        let editor = Editor::create(
            Store::open(&root.path().join("data"), "bin_nested").unwrap(),
            wrong.clone(),
            &mut document,
            || false,
        )
        .unwrap();
        assert_eq!(editor.owner().directory, nested);
        drop(editor);
        Editor::resume(
            Store::open(&root.path().join("data"), "bin_nested").unwrap(),
            &wrong,
        )
        .unwrap();
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
    fn preparation_accepts_direct_dot_paths_but_not_parent_traversal() {
        let (root, editor, path, owner) = setup();
        drop(editor);
        assert!(Document::open(&owner.root, path.to_str().unwrap()).is_err());
        {
            let id = "bin_dot";
            let requested = format!("./{}", path.file_name().unwrap().to_str().unwrap());
            let mut doc = Document::open(&owner.root, &requested).unwrap();
            let editor = Editor::create(
                Store::open(&root.path().join("data"), id).unwrap(),
                owner.clone(),
                &mut doc,
                || false,
            )
            .unwrap();
            assert_eq!(editor.resource().path(), path.canonicalize().unwrap());
        }
        std::fs::create_dir(owner.root.join("nested")).unwrap();
        let requested = format!("nested/../{}", path.file_name().unwrap().to_str().unwrap());
        let mut doc = Document::open(&owner.root, &requested).unwrap();
        assert!(
            Editor::create(
                Store::open(&root.path().join("data"), "bin_parent").unwrap(),
                owner,
                &mut doc,
                || false
            )
            .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), b"original");
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
                    directory: owner.directory.clone(),
                    root: owner.root.clone(),
                    scope: owner.scope.clone(),
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
        let storage =
            super::super::storage::StorageRoot::prepare(&root.path().join("data")).unwrap();
        resumed
            .recovery_proof()
            .unwrap()
            .validate(&storage)
            .unwrap();
        assert_eq!(
            resumed.pending_operation().unwrap().outcome,
            Outcome::Unknown,
            "exclusive execution proof must not acknowledge an uncertain file effect"
        );
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
