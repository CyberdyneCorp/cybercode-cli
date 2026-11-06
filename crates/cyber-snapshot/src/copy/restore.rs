//! A durable intent precedes every namespace mutation; incomplete intents fence reuse.
use super::ownership::{Effect, Ownership};
use super::resolution::Resolution;
use super::storage::{self, Result, fail};
use super::{ENTRY_LIMIT, Entry, RECORD_LIMIT, Store, Tree, now};
use cap_fs_ext::DirExt;
use cyber_server::runtime::{
    FileDiff, RestoreError, SnapshotBinding, SnapshotOrigin, SnapshotWork,
};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Change {
    pub(super) path: String,
    pub(super) before: Option<Entry>,
    pub(super) after: Option<Entry>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    // Version 3 adds prepared native file/parent ownership. Version 2 binds
    // only the worker and cannot acquire that historical proof retroactively.
    pub version: u32,
    pub location: std::path::PathBuf,
    pub root: String,
    pub created: u64,
    pub completed: bool,
    pub changes: Vec<Change>,
    pub applied: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<SnapshotOrigin>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub acknowledged: bool,
    /// A failed restore remains incomplete. Only an acknowledged, independently
    /// retained resolution can settle its native fence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<Ownership>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<RestoreContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RestoreContext {
    pub target: String,
    pub recorded: String,
    pub expected: String,
    pub exclusions: Vec<String>,
}

impl Store {
    pub(super) fn ensure_recovered(&self) -> Result<()> {
        for id in storage::names(&self.journals, ENTRY_LIMIT)? {
            let (_, record) = self.journal(&id)?;
            if record.settled_by.is_some() {
                self.verify_settlement(&record)?;
                continue;
            }
            if !record.completed
                || ((record.origin.is_some() || record.resolution.is_some())
                    && !record.acknowledged)
            {
                return Err(
                    "Snapshot restore requires recovery; private journal and preimages retained"
                        .into(),
                );
            }
        }
        Ok(())
    }

    pub(super) fn journal(&self, id: &str) -> Result<(cap_std::fs::Dir, Journal)> {
        let dir = self.journals.open_dir_nofollow(id).map_err(fail)?;
        let record: Journal =
            serde_json::from_slice(&storage::read(&dir, "intent.json", RECORD_LIMIT)?)
                .map_err(fail)?;
        if !matches!(
            (record.version, record.origin.is_some()),
            (1, false) | (2, true) | (3, _)
        ) || record.location != self.root
            || record.root != self.root_id
            || record.changes.len() > ENTRY_LIMIT
            || record.applied.len() > record.changes.len()
            || !record
                .applied
                .iter()
                .zip(&record.changes)
                .all(|(path, change)| path == &change.path)
            || (record.completed && record.applied.len() != record.changes.len())
            || (record.origin.is_none() && record.acknowledged)
            || record.settled_by.as_ref().is_some_and(|resolution| {
                !record.acknowledged
                    || record.origin.is_none()
                    || resolution == id
                    || resolution.len() > 256
                    || !resolution.starts_with("restore_")
                    || !resolution
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
            || (record.version < 3 && record.ownership.is_some())
            || (record.version == 3
                && record.ownership.is_none()
                && (record.completed || !record.applied.is_empty()))
        {
            return Err("Invalid snapshot recovery journal".into());
        }
        if let Some(ownership) = &record.ownership {
            ownership.validate(&record.changes)?;
        }
        if let Some(resolution) = &record.resolution {
            resolution.validate(id, &record)?;
        }
        if let Some(context) = &record.context {
            for id in [&context.target, &context.recorded, &context.expected] {
                if !id.strip_prefix("copy:").is_some_and(storage::valid_digest) {
                    return Err("Invalid recovery snapshot context".into());
                }
            }
            if context.exclusions.len() > ENTRY_LIMIT * 3 {
                return Err("Recovery exclusion inventory exceeds limit".into());
            }
        }
        if let Some(origin) = &record.origin {
            let binding = &origin.binding;
            if origin.version != 1
                || binding.directory != self.root
                || binding.root_identity != self.root_id
                || binding.journal_id != id
                || binding.namespace_identity != storage::dir_identity(&self.journals)?
                || binding.journal_identity != storage::dir_identity(&dir)?
            {
                return Err("Snapshot journal differs from its native origin".into());
            }
        }
        Ok((dir, record))
    }
    pub fn recovery(&self) -> Result<Vec<super::Recovery>> {
        let mut result = vec![];
        let mut response_bytes = 32usize;
        for id in storage::names(&self.journals, ENTRY_LIMIT)? {
            let (_, record) = self.journal(&id)?;
            let view = super::Recovery {
                id,
                filesystem_complete: record.completed,
                paths: record.changes.iter().map(|c| c.path.clone()).collect(),
                applied: record.applied,
            };
            response_bytes = response_bytes
                .saturating_add(serde_json::to_vec(&view).map_err(fail)?.len())
                .saturating_add(1);
            if response_bytes > 4 * 1024 * 1024 {
                return Err("Snapshot recovery catalog exceeds response limit".into());
            }
            result.push(view);
        }
        Ok(result)
    }
    fn plan(
        &self,
        target: &str,
        recorded: &str,
        expected: &str,
        paths: Option<&[String]>,
    ) -> std::result::Result<(Tree, Vec<Change>, RestoreContext), RestoreError> {
        let mut context = RestoreContext {
            target: target.into(),
            recorded: recorded.into(),
            expected: expected.into(),
            exclusions: vec![],
        };
        self.ensure_recovered().map_err(RestoreError::Failed)?;
        let before = self.capture().map_err(RestoreError::Failed)?;
        if self
            .save(before.clone())
            .map_err(RestoreError::Failed)?
            .tree
            != expected
        {
            return Err(RestoreError::Stale);
        }
        let target = self.load(target).map_err(RestoreError::Failed)?;
        let recorded = self.load(recorded).map_err(RestoreError::Failed)?;
        let mut exclusions = std::collections::BTreeSet::new();
        for (label, tree) in [
            ("current", &before),
            ("target", &target),
            ("recorded", &recorded),
        ] {
            exclusions.extend(tree.skipped.iter().map(|path| format!("{label}: {path}")));
        }
        context.exclusions = exclusions.into_iter().collect();
        let mut changes = vec![];
        let mut conflicts = vec![];
        for path in super::changes(&recorded, &target) {
            if paths.is_some_and(|allowed| !allowed.contains(&path)) {
                continue;
            }
            if before
                .skipped
                .iter()
                .filter_map(|p| p.split_once(": ").map(|(_, p)| p))
                .any(|p| path == p || path.starts_with(&format!("{p}/")))
            {
                continue;
            }
            let current = self.current(&path).map_err(RestoreError::Failed)?;
            let base = recorded.files.get(&path);
            let want = target.files.get(&path);
            let mut next = if current.as_ref() == want {
                continue;
            } else if current.as_ref() == base {
                want.cloned()
            } else {
                match self
                    .merged(current.as_ref(), base, want)
                    .map_err(RestoreError::Failed)?
                {
                    Some(entry) => Some(entry),
                    None => {
                        conflicts.push(path);
                        continue;
                    }
                }
            };
            // Existing ownership is owner state, not an agent edit to undo.
            if let (Some(current), Some(next)) = (current.as_ref(), next.as_mut()) {
                next.uid = current.uid;
                next.gid = current.gid;
            }
            if next == current {
                continue;
            }
            if current.is_some() {
                let native =
                    cyber_core::binary::native::Target::capture(&self.root, Path::new(&path))
                        .map_err(|e| {
                            RestoreError::Failed(format!(
                                "Restore metadata unsupported for {path}: {e}"
                            ))
                        })?;
                native.same_filesystem(&self.objects).map_err(|e| {
                    RestoreError::Failed(format!(
                        "Restore requires same-filesystem private storage: {e}"
                    ))
                })?;
            }
            changes.push(Change {
                path,
                before: current,
                after: next,
            });
        }
        if !conflicts.is_empty() {
            return Err(RestoreError::Conflict(conflicts));
        }
        Ok((before, changes, context))
    }
    fn merged(
        &self,
        current: Option<&Entry>,
        base: Option<&Entry>,
        want: Option<&Entry>,
    ) -> Result<Option<Entry>> {
        let (Some(current), Some(base), Some(want)) = (current, base, want) else {
            return Ok(None);
        };
        let (ours, base, theirs) = (self.bytes(current)?, self.bytes(base)?, self.bytes(want)?);
        let (Ok(ours), Ok(base), Ok(theirs)) = (
            std::str::from_utf8(&ours),
            std::str::from_utf8(&base),
            std::str::from_utf8(&theirs),
        ) else {
            return Ok(None);
        };
        if [ours, base, theirs].iter().any(|s| s.contains('\0')) {
            return Ok(None);
        }
        let Ok(merged) = diffy::merge(base, ours, theirs) else {
            return Ok(None);
        };
        if merged.len() as u64 > self.options.max_file {
            return Err("Merged snapshot exceeds file limit".into());
        }
        let mut entry = self.object(merged.as_bytes())?;
        entry.mode = current.mode;
        entry.uid = current.uid;
        entry.gid = current.gid;
        Ok(Some(entry))
    }
    pub fn preview(
        &self,
        target: &str,
        recorded: &str,
        expected: &str,
        paths: Option<&[String]>,
    ) -> std::result::Result<Vec<FileDiff>, RestoreError> {
        let (_, changes, _) = self.plan(target, recorded, expected, paths)?;
        changes
            .iter()
            .map(|c| {
                self.file_diff(&c.path, c.before.as_ref(), c.after.as_ref())
                    .map_err(RestoreError::Failed)
            })
            .collect()
    }
    pub fn restore(
        &self,
        target: &str,
        recorded: &str,
        expected: &str,
        paths: Option<&[String]>,
        authorize: &(dyn Fn() -> Result<()> + Send + Sync),
    ) -> std::result::Result<Vec<String>, RestoreError> {
        self.restore_worker(target, recorded, expected, paths, authorize, None)
    }

    pub fn restore_worker(
        &self,
        target: &str,
        recorded: &str,
        expected: &str,
        paths: Option<&[String]>,
        authorize: &(dyn Fn() -> Result<()> + Send + Sync),
        worker: Option<&mut SnapshotWork<'_>>,
    ) -> std::result::Result<Vec<String>, RestoreError> {
        authorize().map_err(RestoreError::Failed)?;
        if let Some(worker) = worker.as_ref() {
            worker.authorize().map_err(RestoreError::Failed)?;
        }
        let (_, changes, context) = self.plan(target, recorded, expected, paths)?;
        if changes.is_empty() {
            return Ok(vec![]);
        }
        let id = format!("restore_{}", ulid::Ulid::new());
        let mut journal = self
            .prepare_journal(changes, &id, worker.as_deref(), Some(context), None)
            .map_err(RestoreError::Failed)?;
        self.execute_journal(&id, &mut journal, authorize, worker)
    }

    pub(super) fn execute_journal(
        &self,
        id: &str,
        journal: &mut Journal,
        authorize: &(dyn Fn() -> Result<()> + Send + Sync),
        mut worker: Option<&mut SnapshotWork<'_>>,
    ) -> std::result::Result<Vec<String>, RestoreError> {
        // An unpublished/incomplete directory also remains recovery evidence.
        let dir = storage::private_child(&self.journals, id).map_err(RestoreError::Failed)?;
        let origin = worker
            .as_mut()
            .map(|worker| {
                worker.begin(SnapshotBinding {
                    directory: self.root.clone(),
                    root_identity: self.root_id.clone(),
                    namespace_identity: storage::dir_identity(&self.journals)?,
                    journal_id: id.to_owned(),
                    journal_identity: storage::dir_identity(&dir)?,
                    lock_identity: Some(self.lock_identity()?),
                })
            })
            .transpose()
            .map_err(RestoreError::Failed)?;
        journal.origin = origin;

        Self::record(&dir, journal).map_err(RestoreError::Failed)?;
        let preimages = self
            .resolution_preimages(journal.resolution.as_ref())
            .map_err(RestoreError::Failed)?;
        for (i, change) in journal.changes.iter().enumerate() {
            if let Some(after) = &change.after {
                let bytes = match &preimages {
                    Some(preimages) => preimages.bytes(self, &change.path, after),
                    None => self.bytes(after),
                }
                .map_err(RestoreError::Failed)?;
                let file = storage::create(&dir, &format!("stage_{i}"), &bytes)
                    .map_err(RestoreError::Failed)?;
                Self::stage_metadata(&file, after).map_err(RestoreError::Failed)?;
            }
        }
        storage::sync(&dir).map_err(RestoreError::Failed)?;
        journal.ownership =
            Some(Ownership::prepare(self, &dir, &journal.changes).map_err(RestoreError::Failed)?);
        Self::record(&dir, journal).map_err(RestoreError::Failed)?;
        if let Some(preimages) = &preimages {
            preimages
                .verify_review(self)
                .map_err(RestoreError::Failed)?;
        }
        for i in 0..journal.changes.len() {
            authorize().map_err(RestoreError::Failed)?;
            let apply = |authorize: &dyn Fn() -> Result<()>| {
                let verify = || {
                    authorize()?;
                    if let Some(preimages) = &preimages {
                        preimages.check_source(self, &journal.changes[i].path)?;
                    }
                    Ok(())
                };
                verify()?;
                self.install(
                    &dir,
                    i,
                    &journal.changes[i],
                    journal
                        .ownership
                        .as_ref()
                        .ok_or("Missing restore ownership")?,
                    &verify,
                )
            };
            if let Some(worker) = worker.as_ref() {
                worker
                    .effect(|| {
                        self.verify_worker_namespace(
                            journal
                                .origin
                                .as_ref()
                                .ok_or("Missing snapshot worker origin")?,
                        )?;
                        apply(&|| worker.authorize())
                    })
                    .map_err(RestoreError::Failed)?;
            } else {
                apply(authorize).map_err(RestoreError::Failed)?;
            }
            journal.applied.push(journal.changes[i].path.clone());
            Self::record(&dir, journal).map_err(RestoreError::Failed)?;
        }
        drop(preimages);
        self.verify_root().map_err(RestoreError::Failed)?;
        self.verify_applied(&dir, journal)
            .map_err(RestoreError::Failed)?;
        journal.completed = true;
        Self::record(&dir, journal).map_err(RestoreError::Failed)?;
        Ok(journal.applied.clone())
    }

    pub(super) fn prepare_journal(
        &self,
        changes: Vec<Change>,
        id: &str,
        worker: Option<&SnapshotWork<'_>>,
        context: Option<RestoreContext>,
        mut resolution: Option<Resolution>,
    ) -> Result<Journal> {
        let origin = worker
            .map(|worker| {
                worker.authorize()?;
                Ok::<_, String>(worker.preview_origin(SnapshotBinding {
                    directory: self.root.clone(),
                    root_identity: self.root_id.clone(),
                    namespace_identity: storage::dir_identity(&self.journals)?,
                    journal_id: id.to_owned(),
                    journal_identity: "0".repeat(storage::IDENTITY_MAX_BYTES),
                    lock_identity: Some("0".repeat(storage::IDENTITY_MAX_BYTES)),
                }))
            })
            .transpose()?;
        let ownership = Some(Ownership::budget(&changes)?);
        if let Some(resolution) = &mut resolution {
            resolution.outcome = Some(resolution.review.budget());
        }
        let mut journal = Journal {
            version: 3,
            location: self.root.clone(),
            root: self.root_id.clone(),
            created: now(),
            // Budget the largest progress/acknowledgement record, including JSON
            // escaping, plus its atomic replacement copy, before creating an intent.
            completed: false,
            applied: changes.iter().map(|change| change.path.clone()).collect(),
            changes,
            origin,
            acknowledged: worker.is_some(),
            settled_by: Some("0".repeat(256)),
            ownership,
            context,
            resolution,
        };
        let bytes = serde_json::to_vec(&journal).map_err(fail)?.len() as u64;
        if bytes > RECORD_LIMIT {
            return Err("Restore journal exceeds limit".into());
        }
        let parent_reserve = if let Some(resolution) = &journal.resolution {
            let (_, mut parent) = self.journal(&resolution.original.binding.journal_id)?;
            parent.acknowledged = true;
            parent.settled_by = Some(id.to_owned());
            let bytes = serde_json::to_vec(&parent).map_err(fail)?.len() as u64;
            if bytes > RECORD_LIMIT {
                return Err("Original settlement journal exceeds limit".into());
            }
            bytes.checked_mul(2).ok_or("Restore size overflow")?
        } else {
            0
        };
        let reserve = journal
            .changes
            .iter()
            .map(|change| {
                change
                    .before
                    .as_ref()
                    .map_or(0, |entry| entry.length)
                    .max(change.after.as_ref().map_or(0, |entry| entry.length))
            })
            .try_fold(
                bytes
                    .checked_mul(2)
                    .and_then(|bytes| bytes.checked_add(parent_reserve))
                    .ok_or("Restore size overflow")?,
                |sum, length| sum.checked_add(length).ok_or("Restore size overflow"),
            )?;
        self.reserve(reserve)?;
        journal.applied.clear();
        journal.origin = None;
        journal.acknowledged = false;
        journal.settled_by = None;
        journal.ownership = None;
        if let Some(resolution) = &mut journal.resolution {
            resolution.outcome = None;
        }
        Ok(journal)
    }

    pub(super) fn verify_worker_origin(&self, origin: Option<&SnapshotOrigin>) -> Result<()> {
        self.verify_root()?;
        let Some(origin) = origin else { return Ok(()) };
        self.verify_worker_namespace(origin)?;
        let (dir, record) = self.journal(&origin.binding.journal_id)?;
        if record.origin.as_ref() != Some(origin) || !record.completed {
            return Err("Snapshot worker completion differs from its retained intent".into());
        }
        self.verify_applied(&dir, &record)?;
        self.verify_resolution(&dir, &record)?;
        Ok(())
    }

    fn verify_applied(&self, dir: &cap_std::fs::Dir, record: &Journal) -> Result<()> {
        let ownership = record
            .ownership
            .as_ref()
            .ok_or("Restore has no prepared native ownership")?;
        if ownership
            .inspect(self, dir, &record.changes)?
            .iter()
            .any(|effect| *effect != Effect::Applied)
        {
            return Err("Restore filesystem completion differs from native ownership".into());
        }
        Ok(())
    }

    pub(super) fn verify_worker_namespace(&self, origin: &SnapshotOrigin) -> Result<()> {
        self.verify_root()?;
        let binding = &origin.binding;
        self.verify_lock_route(
            binding
                .lock_identity
                .as_deref()
                .ok_or("Legacy snapshot origin has no exact lock identity")?,
        )?;
        if binding.directory != self.root
            || binding.root_identity != self.root_id
            || binding.namespace_identity != storage::dir_identity(&self.journals)?
        {
            return Err("Snapshot worker namespace changed".into());
        }
        let fresh = Self::existing(
            &self.data_path,
            &self.root.to_string_lossy(),
            self.options.clone(),
        )?
        .ok_or("Snapshot worker namespace is missing")?;
        if storage::dir_identity(&fresh.journals)? != binding.namespace_identity {
            return Err("Snapshot worker namespace route changed".into());
        }
        let dir = self
            .journals
            .open_dir_nofollow(&binding.journal_id)
            .map_err(fail)?;
        if storage::dir_identity(&dir)? != binding.journal_identity {
            return Err("Snapshot worker journal identity changed".into());
        }
        Ok(())
    }
    pub(super) fn acknowledge_worker(&self, origin: Option<&SnapshotOrigin>) -> Result<()> {
        let Some(origin) = origin else { return Ok(()) };
        let (dir, mut journal) = self.journal(&origin.binding.journal_id)?;
        if journal.origin.as_ref() != Some(origin) || !journal.completed {
            return Err("Snapshot acknowledgement differs from its completed worker".into());
        }
        self.verify_worker_namespace(origin)?;
        if journal.resolution.is_some() {
            return self.acknowledge_resolution(origin, dir, journal);
        }
        journal.acknowledged = true;
        Self::record(&dir, &journal)
    }
    fn stage_metadata(file: &std::fs::File, entry: &Entry) -> Result<()> {
        rustix::fs::fchown(
            file,
            Some(rustix::fs::Uid::from_raw(entry.uid)),
            Some(rustix::fs::Gid::from_raw(entry.gid)),
        )
        .map_err(fail)?;
        file.set_permissions(std::fs::Permissions::from_mode(entry.mode))
            .map_err(fail)?;
        storage::sync_file(file)
    }
    pub(super) fn record(dir: &cap_std::fs::Dir, journal: &Journal) -> Result<()> {
        let bytes = serde_json::to_vec(journal).map_err(fail)?;
        if bytes.len() as u64 > RECORD_LIMIT {
            return Err("Restore journal exceeds limit".into());
        }
        storage::atomic(dir, "intent.json", &bytes)
    }
    fn install(
        &self,
        dir: &cap_std::fs::Dir,
        index: usize,
        change: &Change,
        ownership: &Ownership,
        authorize: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        self.verify_root()?;
        if ownership.inspect_change(self, dir, index, change)? != Effect::Unapplied {
            return Err(format!(
                "Restore source changed; journal retained: {}",
                change.path
            ));
        }
        let parent = ownership.create_parents(self, dir, &change.path, authorize)?;
        let parent_id = storage::dir_identity(&parent)?;
        let leaf = Path::new(&change.path)
            .file_name()
            .ok_or("Invalid restore leaf")?;
        let stage = format!("stage_{index}");
        match (&change.before, &change.after) {
            (Some(before), Some(_)) => {
                let target = cyber_core::binary::native::Target::capture(
                    &self.root,
                    Path::new(&change.path),
                )
                .map_err(fail)?;
                target.validate_root(&self.work).map_err(fail)?;
                if target.length.0 != before.length
                    || target.metadata.mode != before.mode
                    || target.metadata.uid != before.uid
                    || target.metadata.gid != before.gid
                {
                    return Err("Restore source metadata changed; journal retained".into());
                }
                let (_, original) = target.validate().map_err(fail)?;
                ownership.verify_original(index, &original)?;
                // Snapshot entries admit only plain ownership/mode. Those fields
                // are already durable on the stage before its identity is logged.
                // Copying native timestamps here would change macOS birth identity.
                if ownership.inspect_change(self, dir, index, change)? != Effect::Unapplied {
                    return Err("Restore source changed before exchange".into());
                }
                self.authorize_restore_effect(dir, ownership, &change.path, &parent_id, authorize)?;
                cyber_core::binary::native::exchange(dir, &stage, &target).map_err(fail)?;
                if self.inspect_entry(dir, std::ffi::OsStr::new(&stage))? != *before {
                    return Err("Displaced restore preimage changed; recovery required".into());
                }
            }
            (Some(before), None) => {
                self.authorize_restore_effect(dir, ownership, &change.path, &parent_id, authorize)?;
                rustix::fs::renameat_with(
                    &parent,
                    leaf,
                    dir,
                    &stage,
                    rustix::fs::RenameFlags::NOREPLACE,
                )
                .map_err(fail)?;
                if self.inspect_entry(dir, std::ffi::OsStr::new(&stage))? != *before {
                    return Err("Displaced deletion preimage changed; recovery required".into());
                }
            }
            (None, Some(_)) => {
                self.authorize_restore_effect(dir, ownership, &change.path, &parent_id, authorize)?;
                rustix::fs::renameat_with(
                    dir,
                    &stage,
                    &parent,
                    leaf,
                    rustix::fs::RenameFlags::NOREPLACE,
                )
                .map_err(fail)?;
            }
            (None, None) => {}
        }
        storage::sync(&parent)?;
        storage::sync(dir)?;
        self.verify_root()?;
        if storage::dir_identity(
            &ownership
                .parent(self, dir, &change.path)?
                .ok_or("Missing restore parent")?,
        )? != parent_id
        {
            return Err("Restore parent identity changed; journal retained".into());
        }
        if ownership.inspect_change(self, dir, index, change)? != Effect::Applied {
            return Err("Restore result changed; recovery required".into());
        }
        Ok(())
    }

    fn authorize_restore_effect(
        &self,
        dir: &cap_std::fs::Dir,
        ownership: &Ownership,
        path: &str,
        parent_id: &str,
        authorize: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        authorize()?;
        let fresh = ownership
            .parent(self, dir, path)?
            .ok_or("Restore parent disappeared")?;
        if storage::dir_identity(&fresh)? != parent_id {
            return Err("Restore parent route changed".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "ownership_tests.rs"]
mod ownership_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_includes_escaped_progress_and_atomic_record_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        let mut store = Store::open(
            &temp.path().join("data"),
            &root.to_string_lossy(),
            super::super::Options {
                max_file: 65536,
                max_total: 10000,
                ignore: vec![],
                private_roots: vec![],
            },
        )
        .unwrap();
        let path = vec!["\"".repeat(50); 24].join("/") + "/file";
        let changes = vec![Change {
            path,
            before: None,
            after: Some(Entry {
                object: blake3::hash(b"").to_hex().to_string(),
                length: 0,
                mode: 0o644,
                uid: 0,
                gid: 0,
            }),
        }];
        // The former changes-only estimate admitted this escaped-path fixture.
        let old_estimate = serde_json::to_vec(&changes).unwrap().len() * 2 + 4096;
        assert!(old_estimate < 10000);
        assert!(
            store
                .prepare_journal(changes.clone(), "restore_fixture", None, None, None)
                .is_err()
        );
        assert!(
            storage::names(&store.journals, ENTRY_LIMIT)
                .unwrap()
                .is_empty()
        );
        store.options.max_total = 100000;
        let journal = store
            .prepare_journal(changes, "restore_fixture", None, None, None)
            .unwrap();
        assert!(journal.applied.is_empty());
        assert_eq!(journal.changes.len(), 1);
    }

    #[test]
    fn filesystem_completion_alone_does_not_admit_a_managed_restore_or_foreign_ack() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("work");
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(
            &temp.path().join("data"),
            &root.to_string_lossy(),
            super::super::Options {
                max_file: 65536,
                max_total: 4 * 1024 * 1024,
                ignore: vec![],
                private_roots: vec![],
            },
        )
        .unwrap();
        let id = "restore_fixture";
        let dir = storage::private_child(&store.journals, id).unwrap();
        // This is a storage-state fixture, not runtime authority to acknowledge a process.
        let origin = SnapshotOrigin {
            version: 1,
            session_id: "ses_fixture".into(),
            session_created_id: "evt_fixture".into(),
            activity_id: "op_fixture".into(),
            worktree_id: None,
            binding: SnapshotBinding {
                directory: store.root.clone(),
                root_identity: store.root_id.clone(),
                namespace_identity: storage::dir_identity(&store.journals).unwrap(),
                journal_id: id.into(),
                journal_identity: storage::dir_identity(&dir).unwrap(),
                lock_identity: Some(store.lock_identity().unwrap()),
            },
        };
        Store::record(
            &dir,
            &Journal {
                version: 2,
                location: store.root.clone(),
                root: store.root_id.clone(),
                created: now(),
                completed: true,
                changes: vec![],
                applied: vec![],
                origin: Some(origin.clone()),
                acknowledged: false,
                settled_by: None,
                ownership: None,
                context: None,
                resolution: None,
            },
        )
        .unwrap();
        assert!(store.recovery().unwrap()[0].filesystem_complete);
        let refusal = store.preview("unused", "unused", "unused", None);
        assert!(
            matches!(refusal, Err(RestoreError::Failed(message)) if message.contains("requires recovery"))
        );
        let mut foreign = origin.clone();
        foreign.activity_id = "op_foreign".into();
        assert!(store.acknowledge_worker(Some(&foreign)).is_err());
        assert!(store.ensure_recovered().is_err());
        store.acknowledge_worker(Some(&origin)).unwrap();
        assert!(store.ensure_recovered().is_ok());
        assert!(store.journal(id).unwrap().1.acknowledged);
    }
}
