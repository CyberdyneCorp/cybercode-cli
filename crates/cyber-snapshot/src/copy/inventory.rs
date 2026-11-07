//! Read-only pre-effect namespace validation, distinct from native journal ownership.
use super::Store;
use super::storage::fail;
use crate::git::prepared::PreparedRestore;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use cyber_server::runtime::RestoreError;
use std::collections::BTreeSet;
use std::path::Path;

const NODE_LIMIT: usize = 65536;
const BYTE_LIMIT: usize = 8 * 1024 * 1024;

struct DirectoryMark {
    path: String,
    generation: String,
}
pub(super) struct Inventory {
    directories: Vec<DirectoryMark>,
    nodes: usize,
    bytes: usize,
    found: BTreeSet<String>,
    rules: super::git_rules::Rules,
    ignored: BTreeSet<String>,
}

impl Inventory {
    /// These marks detect planning races. They never authorize source mutation,
    /// acknowledgement, rollback, or removal of a directory.
    pub(super) fn verify(&self, store: &Store) -> Result<(), RestoreError> {
        store.verify_root().map_err(RestoreError::Failed)?;
        for mark in &self.directories {
            let mut dir = store.work.try_clone().map_err(failed)?;
            for part in Path::new(&mark.path).components() {
                dir = dir
                    .open_dir_nofollow(part.as_os_str())
                    .map_err(|_| RestoreError::Stale)?;
            }
            if generation(&dir)? != mark.generation {
                return Err(RestoreError::Stale);
            }
        }
        self.rules.verify()?;
        store.verify_root().map_err(RestoreError::Failed)
    }

    fn charge(&mut self, bytes: usize) -> Result<(), RestoreError> {
        self.nodes += 1;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| failed("Git inventory size overflow"))?;
        if self.nodes > NODE_LIMIT || self.bytes > BYTE_LIMIT {
            return Err(failed("Git namespace inventory exceeds limit"));
        }
        Ok(())
    }

    fn walk(
        &mut self,
        store: &Store,
        prepared: &PreparedRestore,
        dir: &Dir,
        prefix: &str,
        depth: usize,
    ) -> Result<(), RestoreError> {
        if depth > 64 {
            return Err(failed("Git inventory directory depth exceeds limit"));
        }
        let rule_scope = if prepared.prefix.is_empty() {
            prefix.to_owned()
        } else if prefix.is_empty() {
            prepared.prefix.clone()
        } else {
            format!("{}/{prefix}", prepared.prefix)
        };
        self.rules.enter(&rule_scope)?;
        let before = generation(dir)?;
        self.charge(prefix.len() + before.len())?;
        for entry in dir.entries().map_err(failed)? {
            let entry = entry.map_err(failed)?;
            let leaf = entry.file_name();
            let name = leaf
                .to_str()
                .ok_or_else(|| failed("Non-UTF-8 Git inventory path is unsupported"))?;
            let path = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };
            self.charge(path.len())?;
            let full = if prepared.prefix.is_empty() {
                path.clone()
            } else {
                format!("{}/{path}", prepared.prefix)
            };
            super::tree::safe_path(&path).map_err(RestoreError::Failed)?;
            // Syntactic validation is separate so only protected paths are skipped.
            if store.check_source_path(&path).is_err() {
                continue;
            }
            let metadata = dir.symlink_metadata(&leaf).map_err(failed)?;
            if self.rules.ignored(
                &full,
                metadata.is_dir() && !metadata.file_type().is_symlink(),
            ) {
                self.ignored
                    .insert(if metadata.is_dir() && !metadata.file_type().is_symlink() {
                        format!("{full}/")
                    } else {
                        full
                    });
                continue;
            }
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                let child = dir
                    .open_dir_nofollow(&leaf)
                    .map_err(|_| RestoreError::Stale)?;
                self.walk(store, prepared, &child, &path, depth + 1)?;
            } else {
                // Unknown leaf kinds are stale without opening them. In particular,
                // a new FIFO, socket or directory link is never read or traversed.
                if !prepared.expected.images.contains_key(&path) {
                    return Err(RestoreError::Stale);
                }
                self.found.insert(path);
            }
        }
        if generation(dir)? != before {
            return Err(RestoreError::Stale);
        }
        self.rules.leave();
        self.directories.push(DirectoryMark {
            path: prefix.into(),
            generation: before,
        });
        Ok(())
    }
}

impl Store {
    pub(super) fn git_inventory(
        &self,
        prepared: &PreparedRestore,
    ) -> Result<Inventory, RestoreError> {
        self.verify_root().map_err(RestoreError::Failed)?;
        self.check_expected_exclusions(prepared)?;
        let mut inventory = Inventory {
            directories: vec![],
            nodes: 0,
            bytes: 0,
            found: BTreeSet::new(),
            rules: super::git_rules::Rules::new(prepared)?,
            ignored: BTreeSet::new(),
        };
        inventory.walk(self, prepared, &self.work, "", 0)?;
        if !inventory.found.iter().eq(prepared.expected.images.keys()) {
            return Err(RestoreError::Stale);
        }
        inventory.verify(self)?;
        if let Some(expected) = &prepared.expected_policy
            && &inventory.rules.fingerprint(&inventory.ignored)? != expected
        {
            return Err(RestoreError::Stale);
        }
        Ok(inventory)
    }
    fn check_expected_exclusions(&self, prepared: &PreparedRestore) -> Result<(), RestoreError> {
        for excluded in &prepared.expected_excluded {
            let full = excluded.trim_end_matches('/');
            let relative = if prepared.prefix.is_empty() {
                full
            } else {
                let Some(path) = full.strip_prefix(&format!("{}/", prepared.prefix)) else {
                    continue;
                };
                path
            };
            super::tree::safe_path(relative).map_err(RestoreError::Failed)?;
            if self.check_source_path(relative).is_err() {
                continue;
            }
            let parent = self.parent(relative).map_err(|_| RestoreError::Stale)?;
            let leaf = Path::new(relative)
                .file_name()
                .ok_or_else(|| failed("Invalid excluded Git path"))?;
            let metadata = parent
                .symlink_metadata(leaf)
                .map_err(|_| RestoreError::Stale)?;
            if (metadata.is_dir() && !metadata.file_type().is_symlink()) != excluded.ends_with('/')
            {
                return Err(RestoreError::Stale);
            }
        }
        Ok(())
    }
}
fn generation(dir: &Dir) -> Result<String, RestoreError> {
    let file = dir.try_clone().map_err(failed)?.into_std_file();
    super::ownership::generation(&file).map_err(RestoreError::Failed)
}
fn failed(error: impl std::fmt::Display) -> RestoreError {
    RestoreError::Failed(fail(error))
}
