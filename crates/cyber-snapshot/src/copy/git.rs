//! Import verified Git images into the retained native journal, with original IDs.
use super::restore::{Change, RestoreContext};
use super::storage::{self, Result};
use super::{ENTRY_LIMIT, Entry, Store};
use crate::git::prepared::{Image, PreparedRestore};
use cyber_server::runtime::{RestoreError, SnapshotWork};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GitContext {
    pub worktree: PathBuf,
    pub prefix: String,
}

impl RestoreContext {
    pub(super) fn validate(&self, location: &Path, version: u32) -> Result<()> {
        let ids = [&self.target, &self.recorded, &self.expected];
        if let Some(git) = &self.git {
            if !matches!(version, 4 | 5)
                || !git.worktree.is_absolute()
                || git
                    .worktree
                    .components()
                    .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
                || git.worktree.join(&git.prefix) != location
                || !ids.iter().all(|id| git_id(id) && id.len() == ids[0].len())
            {
                return Err("Invalid Git recovery snapshot context".into());
            }
            if !git.prefix.is_empty() {
                super::tree::safe_path(&git.prefix)?;
            }
        } else if !ids
            .iter()
            .all(|id| id.strip_prefix("copy:").is_some_and(storage::valid_digest))
        {
            return Err("Invalid recovery snapshot context".into());
        }
        if self.exclusions.len() > ENTRY_LIMIT * 3 {
            return Err("Recovery exclusion inventory exceeds limit".into());
        }
        Ok(())
    }
}

fn git_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64)
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn matches(entry: Option<&Entry>, image: Option<&Image>) -> bool {
    match (entry, image) {
        (None, None) => true,
        (Some(entry), Some(image)) => {
            entry.length == image.bytes.len() as u64
                && entry.object == storage::digest(&image.bytes)
                && match entry.kind {
                    super::ImageKind::File => {
                        image.mode != "120000"
                            && (entry.mode & 0o111 != 0) == (image.mode == "100755")
                    }
                    super::ImageKind::Symlink => image.mode == "120000",
                }
        }
        _ => false,
    }
}

impl Store {
    /// Called only inside a retained synchronous worker. Ordinary Git admission
    /// stays disabled until full state/lifetime and native acceptance gates are ready.
    pub(crate) fn restore_git_worker(
        &self,
        prepared: PreparedRestore,
        worker: Option<&mut SnapshotWork<'_>>,
    ) -> std::result::Result<Vec<String>, RestoreError> {
        if let Some(worker) = worker.as_ref() {
            worker.authorize().map_err(RestoreError::Failed)?;
        }
        let (changes, context) = self.plan_git(&prepared)?;
        if changes.is_empty() {
            return Ok(vec![]);
        }
        let id = format!("restore_{}", ulid::Ulid::new());
        let mut journal = self
            .prepare_journal(changes, &id, worker.as_deref(), Some(context), None)
            .map_err(RestoreError::Failed)?;
        self.execute_journal(&id, &mut journal, &|| Ok(()), worker)
    }

    fn plan_git(
        &self,
        prepared: &PreparedRestore,
    ) -> std::result::Result<(Vec<Change>, RestoreContext), RestoreError> {
        let context = RestoreContext {
            target: prepared.target.id.clone(),
            recorded: prepared.recorded.id.clone(),
            expected: prepared.expected.id.clone(),
            exclusions: prepared.exclusions.clone(),
            git: Some(GitContext {
                worktree: prepared.worktree.clone(),
                prefix: prepared.prefix.clone(),
            }),
        };
        context
            .validate(&self.root, 4)
            .map_err(RestoreError::Failed)?;
        if prepared.directory != self.root {
            return Err(RestoreError::Failed(
                "Git handoff belongs to another native Location".into(),
            ));
        }
        self.ensure_recovered().map_err(RestoreError::Failed)?;
        // Known image validation precedes any source effect. Full state scanning
        // and actual runtime lifetime integration remain required before activation.
        for tree in [&prepared.target, &prepared.recorded, &prepared.expected] {
            for (path, image) in &tree.images {
                image.verify().map_err(RestoreError::Failed)?;
                self.check_source_path(path).map_err(RestoreError::Failed)?;
                if !matches!(image.mode.as_str(), "100644" | "100755" | "120000") {
                    return Err(RestoreError::Failed(
                        "Unsupported native Git image mode".into(),
                    ));
                }
                if image.mode == "120000"
                    && (image.bytes.is_empty()
                        || image.bytes.len() > super::links::TARGET_LIMIT
                        || image.bytes.contains(&0))
                {
                    return Err(RestoreError::Failed("Invalid symbolic-link target".into()));
                }
                if image.bytes.len() as u64 > self.options.max_file {
                    return Err(RestoreError::Failed(
                        "Git journal image exceeds file limit".into(),
                    ));
                }
            }
        }
        let paths: BTreeSet<_> = prepared
            .target
            .images
            .keys()
            .chain(prepared.recorded.images.keys())
            .chain(prepared.expected.images.keys())
            .cloned()
            .collect();
        if paths.len() > ENTRY_LIMIT {
            return Err(RestoreError::Failed(
                "Git journal path count exceeds limit".into(),
            ));
        }
        let inventory = self.git_inventory(prepared)?;
        let mut changes = Vec::new();
        let mut conflicts = Vec::new();
        for path in paths {
            let current = self.current(&path).map_err(RestoreError::Failed)?;
            if !matches(current.as_ref(), prepared.expected.images.get(&path)) {
                return Err(RestoreError::Stale);
            }
            let base = prepared.recorded.images.get(&path);
            let want = prepared.target.images.get(&path);
            if base == want
                || prepared
                    .allowed
                    .as_ref()
                    .is_some_and(|paths| !paths.contains(&path))
                || matches(current.as_ref(), want)
            {
                continue;
            }
            let next = if matches(current.as_ref(), base) {
                want.map(|image| self.git_entry(image, current.as_ref()))
                    .transpose()
                    .map_err(RestoreError::Failed)?
            } else {
                let base = base
                    .map(|image| self.git_entry(image, current.as_ref()))
                    .transpose()
                    .map_err(RestoreError::Failed)?;
                let want = want
                    .map(|image| self.git_entry(image, current.as_ref()))
                    .transpose()
                    .map_err(RestoreError::Failed)?;
                match self
                    .merged(current.as_ref(), base.as_ref(), want.as_ref())
                    .map_err(RestoreError::Failed)?
                {
                    Some(entry) => Some(entry),
                    None => {
                        conflicts.push(path);
                        continue;
                    }
                }
            };
            if let Some(entry) = &next
                && entry.kind == super::ImageKind::Symlink
            {
                self.check_link_target(
                    &path,
                    &self.bytes(entry).map_err(RestoreError::Failed)?,
                    &prepared.worktree,
                )
                .map_err(RestoreError::Failed)?;
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
        inventory.verify(self)?;
        Ok((changes, context))
    }

    fn git_entry(&self, image: &Image, current: Option<&Entry>) -> Result<Entry> {
        let mut entry = self.object(&image.bytes)?;
        let owner = self
            .work
            .try_clone()
            .map_err(storage::fail)?
            .into_std_file()
            .metadata()
            .map_err(storage::fail)?;
        if image.mode == "120000" {
            entry.kind = super::ImageKind::Symlink;
            entry.mode = current
                .filter(|entry| entry.kind == super::ImageKind::Symlink)
                .map_or(0o777, |entry| entry.mode);
        } else {
            entry.mode = current
                .filter(|entry| entry.kind == super::ImageKind::File)
                .map_or(0o644, |entry| entry.mode);
        }
        if entry.kind == super::ImageKind::File {
            entry.mode = if image.mode == "100755" {
                entry.mode | 0o111
            } else {
                entry.mode & !0o111
            };
        }
        entry.uid = current.map_or(owner.uid(), |entry| entry.uid);
        entry.gid = current.map_or(owner.gid(), |entry| entry.gid);
        Ok(entry)
    }
}

#[cfg(test)]
mod tests;
