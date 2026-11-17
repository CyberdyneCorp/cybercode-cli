//! Policy-bound fallback collection; unresolved worker/journal evidence stays fenced.
use super::{ENTRY_LIMIT, ImageKind, Manifest, RECORD_LIMIT, now, storage};
use cap_std::fs::Dir;
use std::{collections::BTreeSet, path::Path, sync::Arc};
use storage::{Result, fail};
mod provenance;

/// Prune only unreferenced, old manifests. Journals keep all their objects pinned;
/// an unavailable durable-reference view never authorizes deletion.
pub(crate) fn gc(
    data: &Path,
    config: &Arc<crate::ConfigFn>,
    protected: &BTreeSet<String>,
) -> Result<()> {
    if !data.exists() {
        return Ok(());
    }
    let data = Dir::open_ambient_dir(data, cap_std::ambient_authority()).map_err(fail)?;
    if data.symlink_metadata("snapshot").is_err() {
        return Ok(());
    }
    let snapshot = storage::existing_child(&data, "snapshot", false)?;
    if snapshot.symlink_metadata("global").is_err() {
        return Ok(());
    }
    let root = storage::existing_child(&snapshot, "global", true)?;
    verify_references(&root, protected)?;
    for location in storage::names(&root, 100_000)? {
        if !storage::valid_digest(&location) {
            return Err("Invalid snapshot namespace".into());
        }
        let dir = storage::existing_child(&root, &location, true)?;
        let _lock = match storage::lock(&dir, false) {
            Ok(lock) => lock,
            Err(_) => continue,
        };
        gc_location(&dir, &location, config.as_ref(), protected)?;
    }
    Ok(())
}
/// A durable root whose manifest has disappeared has an unknown object set.
/// Resolve all protected roots before any namespace can collect orphan objects.
fn verify_references(root: &Dir, protected: &BTreeSet<String>) -> Result<()> {
    if protected.len() > 100_000 {
        return Err("Snapshot collection reference limit exceeded".into());
    }
    let mut pending = protected
        .iter()
        .filter(|id| id.starts_with("copy:"))
        .map(|id| {
            id.strip_prefix("copy:")
                .filter(|id| storage::valid_digest(id))
                .ok_or_else(|| "Invalid snapshot collection reference".to_string())
        })
        .collect::<Result<BTreeSet<_>>>()?;
    if pending.is_empty() {
        return Ok(());
    }
    for namespace in storage::names(root, 100_000)? {
        if !storage::valid_digest(&namespace) {
            return Err("Invalid snapshot namespace".into());
        }
        let dir = storage::existing_child(root, &namespace, true)?;
        let manifests = storage::existing_child(&dir, "manifests", true)?;
        for name in storage::names(&manifests, 100_000)? {
            if pending.contains(name.as_str()) {
                read_manifest(&manifests, &name, &namespace)?;
                pending.remove(name.as_str());
            }
        }
        if pending.is_empty() {
            return Ok(());
        }
    }
    Err("Snapshot collection reference manifest is unavailable".into())
}
pub(super) fn gc_location(
    dir: &Dir,
    namespace: &str,
    config: &crate::ConfigFn,
    protected: &BTreeSet<String>,
) -> Result<()> {
    match dir.symlink_metadata("workers") {
        Ok(_) => {
            let workers = storage::existing_child(dir, "workers", true)?;
            if !storage::names(&workers, ENTRY_LIMIT)?.is_empty() {
                return Ok(());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(fail(error)),
    }
    let manifests = storage::existing_child(dir, "manifests", true)?;
    let journals = storage::existing_child(dir, "journals", true)?;
    // Until an owner resolves/acknowledges native evidence, no journal is evicted.
    if !storage::names(&journals, ENTRY_LIMIT)?.is_empty() {
        return Ok(());
    }
    let objects = storage::existing_child(dir, "files", true)?;
    let names = storage::names(&manifests, 100_000)?;
    if names.is_empty() && storage::names(&objects, 100_000)?.is_empty() {
        return Ok(());
    }
    let provenance = provenance::Provenance::inspect(dir, &manifests, &names, namespace)?;
    let Some(mut retention) = Retention::inspect(provenance, config)? else {
        return Ok(());
    };
    let object_names = object_inventory(&objects)?;
    retention.provenance.publish()?;
    let mut keep = BTreeSet::new();
    for name in names {
        if temporary(&name) {
            prune_temporary(&manifests, &name, &retention, config)?;
            continue;
        }
        let manifest = read_manifest(&manifests, &name, namespace)?;
        let old = now().saturating_sub(manifest.accessed) > retention.seconds();
        if old && !protected.contains(&format!("copy:{name}")) {
            retention.verify(config)?;
            manifests.remove_file(&name).map_err(fail)?;
        } else {
            keep.extend(
                manifest
                    .tree
                    .files
                    .values()
                    .map(|entry| entry.object.clone()),
            );
        }
    }
    storage::sync(&manifests)?;
    for name in object_names {
        if temporary(&name) {
            prune_temporary(&objects, &name, &retention, config)?;
            continue;
        }
        if !keep.contains(&name) {
            let metadata = storage::open(&objects, Path::new(&name))?
                .metadata()
                .map_err(fail)?;
            let old = metadata
                .modified()
                .ok()
                .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
                .is_some_and(|age| age.as_secs() > retention.seconds());
            if old {
                retention.verify(config)?;
                objects.remove_file(&name).map_err(fail)?;
            }
        }
    }
    storage::sync(&objects)
}

/// A complete validated manifest inventory identifies the Location even when its
/// source directory is gone. Never infer a default policy from orphan names.
struct Retention {
    provenance: provenance::Provenance,
    settings: serde_json::Value,
    days: u64,
}
impl Retention {
    fn inspect(
        provenance: provenance::Provenance,
        config: &crate::ConfigFn,
    ) -> Result<Option<Self>> {
        let settings = config(provenance.location());
        // The App configuration adapter returns null when resolution fails.
        // Missing policy authority cannot select the default retention period.
        if !settings.is_object() {
            return Err("Snapshot retention configuration is unavailable".into());
        }
        let days = settings
            .pointer("/snapshots/retention_days")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(7);
        if settings.get("snapshots") == Some(&serde_json::Value::Bool(false)) || days == 0 {
            return Ok(None);
        }
        Ok(Some(Self {
            provenance,
            settings,
            days,
        }))
    }
    fn seconds(&self) -> u64 {
        self.days.saturating_mul(86400)
    }
    fn verify(&self, config: &crate::ConfigFn) -> Result<()> {
        if config(self.provenance.location()) != self.settings {
            return Err("Snapshot retention policy changed during collection".into());
        }
        self.provenance.verify()
    }
}
fn object_inventory(objects: &Dir) -> Result<Vec<String>> {
    let names = storage::names(objects, 100_000)?;
    for name in &names {
        if !temporary(name) && !storage::valid_digest(name) {
            return Err("Unrecognized snapshot object entry".into());
        }
        storage::open(objects, Path::new(name))?;
    }
    Ok(names)
}
fn read_manifest(manifests: &Dir, name: &str, namespace: &str) -> Result<Manifest> {
    if !storage::valid_digest(name) {
        return Err("Unrecognized snapshot manifest".into());
    }
    let manifest: Manifest =
        serde_json::from_slice(&storage::read(manifests, name, RECORD_LIMIT)?).map_err(fail)?;
    if manifest.tree.version != 1
        || manifest
            .tree
            .files
            .values()
            .any(|entry| entry.kind != ImageKind::File)
        || storage::digest(&serde_json::to_vec(&manifest.tree).map_err(fail)?) != name
        || storage::digest(
            format!(
                "{}\0{}",
                manifest.tree.location.display(),
                manifest.tree.root
            )
            .as_bytes(),
        ) != namespace
    {
        return Err("Corrupt or foreign snapshot manifest".into());
    }
    Ok(manifest)
}

fn temporary(name: &str) -> bool {
    name.strip_prefix("tmp_")
        .is_some_and(|id| id.len() == 26 && id.parse::<ulid::Ulid>().is_ok())
}
fn prune_temporary(
    dir: &Dir,
    name: &str,
    retention: &Retention,
    config: &crate::ConfigFn,
) -> Result<()> {
    let file = storage::open(dir, Path::new(name))?;
    let old = file
        .metadata()
        .map_err(fail)?
        .modified()
        .ok()
        .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age.as_secs() > retention.seconds());
    if old {
        retention.verify(config)?;
        dir.remove_file(name).map_err(fail)?;
        storage::sync(dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_collected_namespace_needs_no_invented_location_policy() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        for name in ["manifests", "journals", "files"] {
            storage::private_child(&dir, name).unwrap();
        }
        let config = |_: &Path| panic!("empty collection must not invent a Location");
        for _ in 0..2 {
            gc_location(&dir, &"a".repeat(64), &config, &BTreeSet::new()).unwrap();
        }
    }
}
