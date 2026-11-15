//! Read-only journal listing for a Location whose directory was removed (owner decision
//! 2026-11-15). The namespace key binds the Location's path to its directory identity,
//! which can no longer be read, so retained journals are found from their own records:
//! a journal belongs to the removed Location when its recorded path equals the Location
//! and the namespace key is the digest of that path and the recorded identity. Nothing is
//! locked, created, adopted or removed, and no listed journal can be restored.
use super::preparation::{FILE as CANCELLATION, LIMIT as CANCELLATION_LIMIT};
use super::restore::Journal;
use super::storage::{self, Result};
use super::{ENTRY_LIMIT, RECORD_LIMIT, Recovery};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use cyber_server::runtime::SnapshotPreparationReceipt;
use std::path::{Path, PathBuf};

/// Namespaces one listing examines; a larger store fails instead of truncating.
const NAMESPACE_LIMIT: usize = 4096;

/// Whether the Location directory is gone (not merely unreadable or replaced).
pub(crate) fn location_removed(location: &str) -> bool {
    matches!(
        std::fs::symlink_metadata(location),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

/// The retained journals recorded for the removed `location`, each marked
/// `location_removed`. Cancelled preparations stay with the preparation listing.
pub(crate) fn orphaned_recovery(data: &Path, location: &str) -> Result<Vec<Recovery>> {
    let Some(global) = global(data)? else {
        return Ok(vec![]);
    };
    let location = canonical_removed(Path::new(location));
    let mut result = vec![];
    for key in storage::names(&global, NAMESPACE_LIMIT)? {
        if !storage::valid_digest(&key) {
            continue;
        }
        let namespace = storage::existing_child(&global, &key, true)?;
        result.extend(namespace_journals(&namespace, &key, &location)?);
    }
    Ok(result)
}

fn global(data: &Path) -> Result<Option<Dir>> {
    if !data.exists() {
        return Ok(None);
    }
    let data = Dir::open_ambient_dir(data, cap_std::ambient_authority()).map_err(storage::fail)?;
    if data.symlink_metadata("snapshot").is_err() {
        return Ok(None);
    }
    let snapshot = storage::existing_child(&data, "snapshot", false)?;
    if snapshot.symlink_metadata("global").is_err() {
        return Ok(None);
    }
    storage::existing_child(&snapshot, "global", true).map(Some)
}

/// The removed path as the store recorded it: its nearest existing ancestor resolved
/// (for example `/var` to `/private/var`) and the missing components appended.
fn canonical_removed(location: &Path) -> PathBuf {
    let mut missing = vec![];
    let mut current = location;
    loop {
        if let Ok(resolved) = current.canonicalize() {
            return missing
                .iter()
                .rev()
                .fold(resolved, |path, name| path.join(name));
        }
        match (current.file_name(), current.parent()) {
            (Some(name), Some(parent)) => {
                missing.push(name.to_owned());
                current = parent;
            }
            _ => return location.to_path_buf(),
        }
    }
}

fn namespace_journals(namespace: &Dir, key: &str, location: &Path) -> Result<Vec<Recovery>> {
    if namespace.symlink_metadata("journals").is_err() {
        return Ok(vec![]);
    }
    let journals = storage::existing_child(namespace, "journals", true)?;
    let mut result = vec![];
    for id in storage::names(&journals, ENTRY_LIMIT)? {
        let directory = journals.open_dir_nofollow(&id).map_err(storage::fail)?;
        let record: Journal =
            serde_json::from_slice(&storage::read(&directory, "intent.json", RECORD_LIMIT)?)
                .map_err(storage::fail)?;
        if !belongs(&record, key, location) {
            // Every journal in a namespace shares its Location: stop at a foreign one.
            if result.is_empty() {
                return Ok(vec![]);
            }
            continue;
        }
        if cancelled(&directory, &id)? {
            continue;
        }
        result.push(Recovery {
            id,
            filesystem_complete: record.completed,
            paths: record.changes.iter().map(|c| c.path.clone()).collect(),
            applied: record.applied,
            location_removed: true,
        });
    }
    Ok(result)
}

fn belongs(record: &Journal, key: &str, location: &Path) -> bool {
    record.location == location
        && storage::digest(format!("{}\0{}", location.display(), record.root).as_bytes()) == key
}

/// A cancellation receipt bound to this journal hides it, as in the ordinary listing.
/// Its native proof cannot be checked without the Location, so only the binding is.
fn cancelled(directory: &Dir, id: &str) -> Result<bool> {
    if directory.symlink_metadata(CANCELLATION).is_err() {
        return Ok(false);
    }
    let receipt: SnapshotPreparationReceipt =
        serde_json::from_slice(&storage::read(directory, CANCELLATION, CANCELLATION_LIMIT)?)
            .map_err(storage::fail)?;
    Ok(receipt.source_admitted == Some(false)
        && receipt
            .origin
            .as_ref()
            .is_some_and(|origin| origin.binding.journal_id == id))
}
