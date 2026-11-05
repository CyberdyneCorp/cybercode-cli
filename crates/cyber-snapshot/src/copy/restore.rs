//! A durable intent precedes every namespace mutation; incomplete intents fence reuse.
use super::storage::{self, Result, fail};
use super::{ENTRY_LIMIT, Entry, RECORD_LIMIT, Store, Tree, now};
use cap_fs_ext::DirExt;
use cyber_server::runtime::{FileDiff, RestoreError};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    path: String,
    before: Option<Entry>,
    after: Option<Entry>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    location: std::path::PathBuf,
    root: String,
    created: u64,
    completed: bool,
    changes: Vec<Change>,
    applied: Vec<String>,
}

impl Store {
    pub(super) fn ensure_recovered(&self) -> Result<()> {
        if self.recovery()?.iter().any(|r| !r.filesystem_complete) {
            return Err(
                "Snapshot restore requires recovery; private journal and preimages retained".into(),
            );
        }
        Ok(())
    }
    pub fn recovery(&self) -> Result<Vec<super::Recovery>> {
        let mut result = vec![];
        let mut response_bytes = 32usize;
        for id in storage::names(&self.journals, ENTRY_LIMIT)? {
            let dir = self.journals.open_dir_nofollow(&id).map_err(fail)?;
            let record: Journal =
                serde_json::from_slice(&storage::read(&dir, "intent.json", RECORD_LIMIT)?)
                    .map_err(fail)?;
            if record.version != 1
                || record.location != self.root
                || record.root != self.root_id
                || record.changes.len() > ENTRY_LIMIT
            {
                return Err("Invalid snapshot recovery journal".into());
            }
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
    ) -> std::result::Result<(Tree, Vec<Change>), RestoreError> {
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
        Ok((before, changes))
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
        let (_, changes) = self.plan(target, recorded, expected, paths)?;
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
        authorize().map_err(RestoreError::Failed)?;
        let (_, changes) = self.plan(target, recorded, expected, paths)?;
        if changes.is_empty() {
            return Ok(vec![]);
        }
        let record_bytes = serde_json::to_vec(&changes)
            .map_err(|e| RestoreError::Failed(e.to_string()))?
            .len() as u64;
        let reserve = changes
            .iter()
            .map(|c| {
                c.before
                    .as_ref()
                    .map_or(0, |e| e.length)
                    .max(c.after.as_ref().map_or(0, |e| e.length))
            })
            .try_fold(
                record_bytes.saturating_mul(2).saturating_add(4096),
                |sum, length| sum.checked_add(length).ok_or("Restore size overflow"),
            )
            .map_err(|e| RestoreError::Failed(e.into()))?;
        self.reserve(reserve).map_err(RestoreError::Failed)?;
        // The directory is unique. A crash before publishing its intent is also
        // unreadable recovery evidence and fails closed on the next open.
        let id = format!("restore_{}", ulid::Ulid::new());
        let dir = storage::private_child(&self.journals, &id).map_err(RestoreError::Failed)?;
        let mut journal = Journal {
            version: 1,
            location: self.root.clone(),
            root: self.root_id.clone(),
            created: now(),
            completed: false,
            changes,
            applied: vec![],
        };

        Self::record(&dir, &journal).map_err(RestoreError::Failed)?;
        for (i, change) in journal.changes.iter().enumerate() {
            if let Some(after) = &change.after {
                let file = storage::create(
                    &dir,
                    &format!("stage_{i}"),
                    &self.bytes(after).map_err(RestoreError::Failed)?,
                )
                .map_err(RestoreError::Failed)?;
                Self::stage_metadata(&file, after).map_err(RestoreError::Failed)?;
            }
        }
        storage::sync(&dir).map_err(RestoreError::Failed)?;
        for i in 0..journal.changes.len() {
            authorize().map_err(RestoreError::Failed)?;
            self.install(&dir, i, &journal.changes[i])
                .map_err(RestoreError::Failed)?;
            journal.applied.push(journal.changes[i].path.clone());
            Self::record(&dir, &journal).map_err(RestoreError::Failed)?;
        }
        self.verify_root().map_err(RestoreError::Failed)?;
        journal.completed = true;
        Self::record(&dir, &journal).map_err(RestoreError::Failed)?;
        Ok(journal.applied)
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
        file.sync_all().map_err(fail)
    }
    fn record(dir: &cap_std::fs::Dir, journal: &Journal) -> Result<()> {
        let bytes = serde_json::to_vec(journal).map_err(fail)?;
        if bytes.len() as u64 > RECORD_LIMIT {
            return Err("Restore journal exceeds limit".into());
        }
        storage::atomic(dir, "intent.json", &bytes)
    }
    fn install(&self, dir: &cap_std::fs::Dir, index: usize, change: &Change) -> Result<()> {
        if self.current(&change.path)? != change.before {
            return Err(format!(
                "Restore source changed; journal retained: {}",
                change.path
            ));
        }
        let parent = self.parent(&change.path, true)?;
        let parent_id = storage::dir_identity(&parent)?;
        let leaf = Path::new(&change.path)
            .file_name()
            .ok_or("Invalid restore leaf")?;
        let stage = format!("stage_{index}");
        match (&change.before, &change.after) {
            (Some(before), Some(after)) => {
                let target = cyber_core::binary::native::Target::capture(
                    &self.root,
                    Path::new(&change.path),
                )
                .map_err(fail)?;
                if target.length.0 != before.length
                    || target.metadata.mode != before.mode
                    || target.metadata.uid != before.uid
                    || target.metadata.gid != before.gid
                {
                    return Err("Restore source metadata changed; journal retained".into());
                }
                let (_, original) = target.validate().map_err(fail)?;
                let stage_file = storage::open(dir, Path::new(&stage))?;
                cyber_core::binary::native::copy_metadata(&original, &stage_file).map_err(fail)?;
                Self::stage_metadata(&stage_file, after)?;
                cyber_core::binary::native::exchange(dir, &stage, &target).map_err(fail)?;
                if self.read_entry(dir, std::ffi::OsStr::new(&stage))? != *before {
                    return Err("Displaced restore preimage changed; recovery required".into());
                }
            }
            (Some(before), None) => {
                rustix::fs::renameat_with(
                    &parent,
                    leaf,
                    dir,
                    &stage,
                    rustix::fs::RenameFlags::NOREPLACE,
                )
                .map_err(fail)?;
                if self.read_entry(dir, std::ffi::OsStr::new(&stage))? != *before {
                    return Err("Displaced deletion preimage changed; recovery required".into());
                }
            }
            (None, Some(_)) => rustix::fs::renameat_with(
                dir,
                &stage,
                &parent,
                leaf,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(fail)?,
            (None, None) => {}
        }
        storage::sync(&parent)?;
        storage::sync(dir)?;
        self.verify_root()?;
        if storage::dir_identity(&self.parent(&change.path, false)?)? != parent_id {
            return Err("Restore parent identity changed; journal retained".into());
        }
        if self.current(&change.path)? != change.after {
            return Err("Restore result changed; recovery required".into());
        }
        Ok(())
    }
}
