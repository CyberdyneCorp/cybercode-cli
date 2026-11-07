//! A resolution is an independent intent. Filesystem success is not native acknowledgement.
use super::Store;
use super::ownership::{Effect, ImageObservation, Observations, Ownership, observe_image};
use super::restore::{Change, Journal};
use super::storage::{self, Result, fail};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use cyber_server::runtime::{
    RestoreError, SnapshotBinding, SnapshotOrigin, SnapshotRecoveryAction as Action,
    SnapshotRecoveryConfirmation as Confirmation, SnapshotRecoveryImage as Image,
    SnapshotRecoveryPath as PathView, SnapshotRecoveryReceipt as Receipt,
    SnapshotRecoveryReview as Review, SnapshotWork,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Resolution {
    pub request: Confirmation,
    pub original: SnapshotOrigin,
    pub original_digest: String,
    pub review: Observations,
    pub outcome: Option<Observations>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_state: Option<ReviewedState>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReviewedState {
    pub(super) completed: bool,
    pub(super) acknowledged: bool,
}

impl Resolution {
    pub fn validate(&self, id: &str, record: &Journal) -> Result<()> {
        validate_journal_id(&self.request.journal_id)?;
        if self.original.version != 1
            || id == self.request.journal_id
            || self.original.binding.journal_id != self.request.journal_id
            || resolution_id(&self.original, &self.request.request_id)? != id
            || !storage::valid_digest(&self.original_digest)
            || !storage::valid_digest(&self.request.confirmation)
            || self.outcome.is_some() && !record.completed
        {
            return Err("Invalid recovery resolution intent".into());
        }
        if let Some(origin) = &record.origin
            && (origin.session_id != self.original.session_id
                || origin.session_created_id != self.original.session_created_id
                || origin.worktree_id != self.original.worktree_id
                || origin.activity_id == self.original.activity_id)
        {
            return Err("Resolution origin differs from original Session/checkout".into());
        }
        self.review.validate()?;
        if let Some(outcome) = &self.outcome {
            outcome.validate()?;
        }
        Ok(())
    }
}

struct Plan {
    review: Review,
    observations: Observations,
    changes: Vec<Change>,
    digest: String,
}

pub(super) struct Preimages<'a> {
    dir: Dir,
    parent: Journal,
    paths: std::collections::BTreeMap<String, usize>,
    review: &'a Observations,
}
impl Preimages<'_> {
    pub fn verify_review(&self, store: &Store) -> Result<()> {
        if &observe(store, &self.dir, &self.parent)? != self.review {
            return Err("Recovery owner state changed during private preparation".into());
        }
        Ok(())
    }
    pub fn check_source(&self, store: &Store, path: &str) -> Result<()> {
        let index = *self.paths.get(path).ok_or("Unreviewed recovery source")?;
        let parent = self
            .parent
            .ownership
            .as_ref()
            .ok_or("Missing original ownership")?
            .parent(store, &self.dir, path)?;
        let leaf = std::path::Path::new(path)
            .file_name()
            .ok_or("Invalid recovery source")?;
        if observe_image(store, parent.as_ref(), leaf)? != self.review.files[index].source {
            return Err("Recovery source differs from its reviewed native image".into());
        }
        Ok(())
    }
    pub fn bytes(&self, store: &Store, path: &str, expected: &super::Entry) -> Result<Vec<u8>> {
        let index = *self
            .paths
            .get(path)
            .ok_or("Resolution path is absent from original intent")?;
        if self.parent.changes[index].before.as_ref() != Some(expected) {
            return Err("Resolution image differs from original preimage".into());
        }
        let observed = self
            .review
            .files
            .get(index)
            .and_then(|slots| slots.stage.as_ref())
            .ok_or("Original native preimage is unavailable")?;
        let name = format!("stage_{index}");
        if observe_image(store, Some(&self.dir), std::ffi::OsStr::new(&name))?.as_ref()
            != Some(observed)
        {
            return Err("Original native preimage changed before preparation".into());
        }
        let bytes = storage::read(&self.dir, &name, expected.length)?;
        if bytes.len() as u64 != expected.length
            || storage::digest(&bytes) != expected.object
            || observe_image(store, Some(&self.dir), std::ffi::OsStr::new(&name))?.as_ref()
                != Some(observed)
        {
            return Err("Original native preimage changed while preparing resolution".into());
        }
        Ok(bytes)
    }
}

fn image(value: &ImageObservation) -> Image {
    expected_image(&value.entry, Some(value.identity.clone()))
}
fn expected_image(entry: &super::Entry, native_identity: Option<String>) -> Image {
    Image {
        hash: entry.object.clone(),
        bytes: entry.length,
        mode: entry.mode,
        uid: entry.uid,
        gid: entry.gid,
        native_identity,
    }
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.contains('\0')
}
fn resolution_id(original: &SnapshotOrigin, request: &str) -> Result<String> {
    if !valid_id(request) {
        return Err("Invalid snapshot recovery request ID".into());
    }
    let key = serde_json::to_vec(&(original.session_created_id.as_str(), request)).map_err(fail)?;
    Ok(format!("restore_resolution_{}", storage::digest(&key)))
}
fn immutable_digest(record: &Journal) -> Result<String> {
    Ok(storage::digest(
        &serde_json::to_vec(&(
            record.version,
            &record.location,
            &record.root,
            record.created,
            &record.changes,
            &record.origin,
            &record.ownership,
            &record.context,
            &record.resolution,
        ))
        .map_err(fail)?,
    ))
}

fn fingerprint(
    action: Action,
    binding: &SnapshotBinding,
    digest: &str,
    observations: &Observations,
    state: &ReviewedState,
) -> Result<String> {
    Ok(storage::digest(
        &serde_json::to_vec(&(
            1u32,
            action,
            binding,
            digest,
            observations,
            state.completed,
            state.acknowledged,
        ))
        .map_err(fail)?,
    ))
}

fn verify_fingerprint(resolution: &Resolution) -> Result<()> {
    let state = resolution
        .reviewed_state
        .as_ref()
        .ok_or("Legacy resolution has no frozen review state")?;
    if fingerprint(
        resolution.request.action,
        &resolution.original.binding,
        &resolution.original_digest,
        &resolution.review,
        state,
    )? != resolution.request.confirmation
    {
        return Err("Resolution differs from its confirmed immutable review".into());
    }
    Ok(())
}

impl Store {
    pub(crate) fn worker_digest(&self, origin: Option<&SnapshotOrigin>) -> Result<Option<String>> {
        origin
            .map(|origin| {
                let (_, journal) = self.journal(&origin.binding.journal_id)?;
                if journal.origin.as_ref() != Some(origin) {
                    return Err("Snapshot native origin changed".into());
                }
                immutable_digest(&journal)
            })
            .transpose()
    }
    pub(super) fn verify_receipt(
        &self,
        origin: Option<&SnapshotOrigin>,
        expected: &Receipt,
    ) -> Result<()> {
        let origin = origin.ok_or("Recovery completion has no reserved native worker origin")?;
        let (_, journal) = self.journal(&origin.binding.journal_id)?;
        let mut observed = receipt(&origin.binding.journal_id, &journal)?;
        if expected.resolution_digest.is_none() {
            observed.resolution_digest = None;
        }
        if observed != *expected {
            return Err("Native resolution receipt changed".into());
        }
        Ok(())
    }
    pub(super) fn verify_settlement(&self, parent: &Journal) -> Result<()> {
        let id = parent
            .settled_by
            .as_deref()
            .ok_or("Missing native settlement reference")?;
        let (_, journal) = self.journal(id)?;
        let resolution = journal
            .resolution
            .as_ref()
            .ok_or("Native settlement reference is not a resolution")?;
        verify_fingerprint(resolution)?;
        if !parent.acknowledged
            || !journal.acknowledged
            || !journal.completed
            || resolution.outcome.is_none()
            || parent.origin.as_ref() != Some(&resolution.original)
            || immutable_digest(parent)? != resolution.original_digest
        {
            return Err("Native settlement reference differs from acknowledged resolution".into());
        }
        Ok(())
    }

    /// Called only after the runtime consumed the exact two-owner receipt. This
    /// deliberately checks immutable native proof, not later owner file edits.
    pub(super) fn acknowledge_resolution(
        &self,
        origin: &SnapshotOrigin,
        dir: Dir,
        journal: Journal,
    ) -> Result<()> {
        self.acknowledge_resolution_authorized(origin, dir, journal, &|| Ok(()))
    }

    fn acknowledge_resolution_authorized(
        &self,
        origin: &SnapshotOrigin,
        dir: Dir,
        mut journal: Journal,
        authorize: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        let resolution = journal
            .resolution
            .as_ref()
            .ok_or("Missing resolution intent")?;
        verify_fingerprint(resolution)?;
        let _receipt = receipt(&origin.binding.journal_id, &journal)?;
        self.verify_worker_namespace(&resolution.original)?;
        let (parent_dir, mut parent) = self.journal(&resolution.original.binding.journal_id)?;
        if parent.origin.as_ref() != Some(&resolution.original)
            || immutable_digest(&parent)? != resolution.original_digest
            || parent
                .settled_by
                .as_ref()
                .is_some_and(|id| id != &origin.binding.journal_id)
        {
            return Err("Original intent differs from consuming resolution acknowledgement".into());
        }
        // The child acknowledgement is durable before publishing the parent's
        // settlement link. A cut between writes stays fenced and is idempotent.
        authorize()?;
        self.verify_worker_namespace(origin)?;
        journal.acknowledged = true;
        Self::record(&dir, &journal)?;
        authorize()?;
        self.verify_worker_namespace(&resolution.original)?;
        parent.acknowledged = true;
        parent.settled_by = Some(origin.binding.journal_id.clone());
        Self::record(&parent_dir, &parent)
    }

    pub(crate) fn reconcile_acknowledgement(
        &self,
        work: &cyber_server::runtime::SnapshotAcknowledgementWork<'_>,
    ) -> Result<()> {
        self.reconcile_receipt(work.origin(), work.receipt(), &|| work.authorize())
    }
    pub(super) fn reconcile_receipt(
        &self,
        origin: &SnapshotOrigin,
        expected: &Receipt,
        authorize: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        authorize()?;
        self.verify_worker_namespace(origin)?;
        self.verify_receipt(Some(origin), expected)?;
        let (dir, journal) = self.journal(&origin.binding.journal_id)?;
        if expected.resolution_digest.is_none() {
            let (_, parent) = self.journal(&expected.journal_id)?;
            if journal.acknowledged
                && parent.acknowledged
                && parent.settled_by.as_deref() == Some(origin.binding.journal_id.as_str())
            {
                self.verify_settlement(&parent)?;
                return Ok(());
            }
            return Err("Legacy receipt has no persisted immutable resolution digest; retained evidence cannot authorize new acknowledgement writes".into());
        }
        self.acknowledge_resolution_authorized(origin, dir, journal, authorize)
    }
    pub(super) fn resolution_preimages<'a>(
        &self,
        resolution: Option<&'a Resolution>,
    ) -> Result<Option<Preimages<'a>>> {
        let Some(resolution) = resolution.filter(|value| value.request.action == Action::Rollback)
        else {
            return Ok(None);
        };
        self.verify_worker_namespace(&resolution.original)?;
        let (dir, parent) = self.journal(&resolution.original.binding.journal_id)?;
        if immutable_digest(&parent)? != resolution.original_digest {
            return Err("Original intent changed before preimage preparation".into());
        }
        let paths = parent
            .changes
            .iter()
            .enumerate()
            .map(|(index, change)| (change.path.clone(), index))
            .collect();
        Ok(Some(Preimages {
            dir,
            parent,
            paths,
            review: &resolution.review,
        }))
    }
    pub fn recovery_review(&self, id: &str, action: Action) -> Result<Review> {
        Ok(self.resolution_plan(id, action)?.review)
    }

    fn resolution_plan(&self, id: &str, action: Action) -> Result<Plan> {
        validate_journal_id(id)?;
        self.verify_root()?;
        let (dir, record) = self.journal(id)?;
        let observations = observe(self, &dir, &record)?;
        let digest = immutable_digest(&record)?;
        let mut review = Review {
            journal_id: id.into(),
            action,
            origin: record.origin.clone(),
            filesystem_complete: record.completed,
            acknowledged: record.acknowledged,
            confirmation: None,
            paths: vec![],
            exclusions: record
                .context
                .as_ref()
                .map(|context| context.exclusions.clone()),
            recorded_created_parents: record.ownership.as_ref().map(Ownership::created_parents),
            conflicts: vec![],
        };
        if record.acknowledged {
            review
                .conflicts
                .push("This restore has already been acknowledged".into());
        }
        if record.origin.is_none() {
            review
                .conflicts
                .push("Legacy intent has no exact native worker origin".into());
        }
        if let Some(origin) = &record.origin
            && let Err(error) = self.verify_worker_namespace(origin)
        {
            review.conflicts.push(error);
        }
        if action == Action::Rollback {
            match &record.ownership {
                None => review
                    .conflicts
                    .push("Rollback requires prepared native file ownership".into()),
                Some(proof) => {
                    if let Err(error) = proof.inspect(self, &dir, &record.changes) {
                        review.conflicts.push(error);
                    }
                }
            }
        }
        let mut changes = vec![];
        for (index, (change, slots)) in record.changes.iter().zip(&observations.files).enumerate() {
            let effect = record
                .ownership
                .as_ref()
                .and_then(|proof| proof.inspect_change(self, &dir, index, change).ok());
            let reverse = action == Action::Rollback && effect == Some(Effect::Applied);
            let current = slots.source.as_ref().map(image);
            let proposed = if reverse {
                change
                    .before
                    .as_ref()
                    .map(|entry| expected_image(entry, None))
            } else {
                current.clone()
            };
            review.paths.push(PathView {
                path: change.path.clone(),
                state: match effect {
                    Some(Effect::Applied) => "applied",
                    Some(Effect::Unapplied) => "unapplied",
                    None => "foreign",
                }
                .into(),
                current,
                recorded_before: change
                    .before
                    .as_ref()
                    .map(|entry| expected_image(entry, None)),
                recorded_after: change
                    .after
                    .as_ref()
                    .map(|entry| expected_image(entry, None)),
                retained: slots.stage.as_ref().map(image),
                proposed,
                changes: reverse,
            });
            if reverse {
                changes.push(Change {
                    path: change.path.clone(),
                    before: slots.source.as_ref().map(|value| value.entry.clone()),
                    after: change.before.clone(),
                });
            }
        }
        let binding = SnapshotBinding {
            directory: self.root.clone(),
            root_identity: self.root_id.clone(),
            namespace_identity: storage::dir_identity(&self.journals)?,
            journal_id: id.into(),
            journal_identity: storage::dir_identity(&dir)?,
            lock_identity: record
                .origin
                .as_ref()
                .and_then(|origin| origin.binding.lock_identity.clone()),
        };
        if review.conflicts.is_empty() {
            review.confirmation = Some(fingerprint(
                action,
                &binding,
                &digest,
                &observations,
                &ReviewedState {
                    completed: record.completed,
                    acknowledged: record.acknowledged,
                },
            )?);
        }
        if serde_json::to_vec(&review).map_err(fail)?.len() > 16 * 1024 * 1024 {
            return Err("Recovery review exceeds response limit".into());
        }
        // Re-read control data; a live writer may have advanced preparation or progress.
        let (_, current) = self.journal(id)?;
        if immutable_digest(&current)? != digest
            || current.completed != record.completed
            || current.applied != record.applied
            || current.acknowledged != record.acknowledged
        {
            return Err("Recovery intent changed while reviewing".into());
        }
        Ok(Plan {
            review,
            observations,
            changes,
            digest,
        })
    }

    /// Native backend only. Consuming runtime acknowledgement is a separate owner transaction.
    pub fn resolve_recovery(
        &self,
        request: &Confirmation,
        authorize: &(dyn Fn() -> Result<()> + Send + Sync),
        worker: Option<&mut SnapshotWork<'_>>,
    ) -> std::result::Result<Receipt, RestoreError> {
        validate_journal_id(&request.journal_id).map_err(RestoreError::Failed)?;
        let (parent_dir, parent) = self
            .journal(&request.journal_id)
            .map_err(RestoreError::Failed)?;
        let original = parent
            .origin
            .clone()
            .ok_or_else(|| RestoreError::Failed("Recovery requires exact native origin".into()))?;
        let id = resolution_id(&original, &request.request_id).map_err(RestoreError::Failed)?;
        // A durable prior request is lookup-only, including failed/partial outcomes.
        if self.journals.symlink_metadata(&id).is_ok() {
            return self
                .lookup_resolution(&id, request, &original)
                .map_err(RestoreError::Failed);
        }
        authorize().map_err(RestoreError::Failed)?;
        self.verify_worker_namespace(&original)
            .map_err(RestoreError::Failed)?;
        let plan = self
            .resolution_plan(&request.journal_id, request.action)
            .map_err(RestoreError::Failed)?;
        if plan.review.confirmation.as_deref() != Some(&request.confirmation) {
            return Err(RestoreError::Stale);
        }
        let resolution = Resolution {
            request: request.clone(),
            original: original.clone(),
            original_digest: plan.digest,
            review: plan.observations.clone(),
            outcome: None,
            reviewed_state: Some(ReviewedState {
                completed: parent.completed,
                acknowledged: parent.acknowledged,
            }),
        };
        let mut record = self
            .prepare_journal(
                plan.changes,
                &id,
                worker.as_deref(),
                parent.context.clone(),
                Some(resolution),
            )
            .map_err(RestoreError::Failed)?;
        if let Some(worker) = worker.as_deref() {
            let source = worker.preview_origin(original.binding.clone());
            if source.session_id != original.session_id
                || source.session_created_id != original.session_created_id
                || source.worktree_id != original.worktree_id
            {
                return Err(RestoreError::Failed(
                    "Recovery worker differs from the original Session/checkout".into(),
                ));
            }
        }
        // Revalidate after storage waits/reservation, before publishing the distinct intent.
        let fresh = self
            .resolution_plan(&request.journal_id, request.action)
            .map_err(RestoreError::Failed)?;
        if fresh.review.confirmation.as_deref() != Some(&request.confirmation) {
            return Err(RestoreError::Stale);
        }
        self.execute_journal(&id, &mut record, authorize, worker)?;
        let dir = self
            .journals
            .open_dir_nofollow(&id)
            .map_err(|e| RestoreError::Failed(fail(e)))?;
        let outcome = observe(self, &parent_dir, &parent).map_err(RestoreError::Failed)?;
        verify_unchanged_slots(
            &plan.observations,
            &outcome,
            &parent.changes,
            &record.changes,
        )
        .map_err(RestoreError::Failed)?;
        record
            .resolution
            .as_mut()
            .ok_or_else(|| RestoreError::Failed("Missing resolution intent".into()))?
            .outcome = Some(outcome);
        Self::record(&dir, &record).map_err(RestoreError::Failed)?;
        self.verify_resolution(&dir, &record)
            .map_err(RestoreError::Failed)?;
        receipt(&id, &record).map_err(RestoreError::Failed)
    }

    fn lookup_resolution(
        &self,
        id: &str,
        request: &Confirmation,
        original: &SnapshotOrigin,
    ) -> Result<Receipt> {
        let (_, record) = self.journal(id)?;
        let resolution = record
            .resolution
            .as_ref()
            .ok_or("Recovery request is not a resolution")?;
        if &resolution.request != request || &resolution.original != original {
            return Err(
                "Recovery request ID was reused with different ownership or content".into(),
            );
        }
        receipt(id, &record)
    }

    pub(super) fn verify_resolution(&self, _dir: &Dir, record: &Journal) -> Result<()> {
        let Some(resolution) = &record.resolution else {
            return Ok(());
        };
        verify_fingerprint(resolution)?;
        self.verify_worker_namespace(&resolution.original)?;
        let (parent_dir, parent) = self.journal(&resolution.original.binding.journal_id)?;
        if immutable_digest(&parent)? != resolution.original_digest {
            return Err("Original recovery intent changed".into());
        }
        let expected = resolution
            .outcome
            .as_ref()
            .ok_or("Recovery filesystem outcome is incomplete; lookup without replay")?;
        if &observe(self, &parent_dir, &parent)? != expected {
            return Err("Recovery outcome changed before acknowledgement".into());
        }
        Ok(())
    }
}

fn validate_journal_id(id: &str) -> Result<()> {
    if !id.starts_with("restore_")
        || id.len() > 256
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("Invalid snapshot recovery journal ID".into());
    }
    Ok(())
}
fn observe(store: &Store, dir: &Dir, record: &Journal) -> Result<Observations> {
    match &record.ownership {
        Some(proof) => proof.observe(store, dir, &record.changes),
        None => Ownership::observe_unprepared(store, dir, &record.changes),
    }
}
fn verify_unchanged_slots(
    before: &Observations,
    after: &Observations,
    original: &[Change],
    changes: &[Change],
) -> Result<()> {
    if before.files.len() != after.files.len() {
        return Err("Recovery inventory changed".into());
    }
    // Every original private slot is retained. Source changes are independently
    // proven by the new resolution's native ownership before its final commit.
    for ((old, new), original) in before.files.iter().zip(&after.files).zip(original) {
        if old.stage != new.stage {
            return Err("Original recovery preimages changed".into());
        }
        if !changes.iter().any(|change| change.path == original.path) && old.source != new.source {
            return Err("Unchanged owner file changed during recovery".into());
        }
    }
    if changes.is_empty() && before != after {
        return Err("Owner state changed during keep".into());
    }
    Ok(())
}
fn receipt(id: &str, record: &Journal) -> Result<Receipt> {
    let resolution = record
        .resolution
        .as_ref()
        .ok_or("Missing resolution record")?;
    if !record.completed || resolution.outcome.is_none() {
        return Err(
            "Recovery request has an incomplete outcome; inspect retained intent, do not replay"
                .into(),
        );
    }
    Ok(Receipt {
        journal_id: resolution.request.journal_id.clone(),
        resolution_id: id.into(),
        request_id: resolution.request.request_id.clone(),
        action: resolution.request.action,
        confirmation: resolution.request.confirmation.clone(),
        original: resolution.original.clone(),
        filesystem_complete: true,
        resolution_digest: Some(immutable_digest(record)?),
    })
}
