//! Private BLAKE3 snapshots for exact non-Git Locations; no user repository required.
mod ownership;
mod preparation;
mod resolution;
mod restore;
mod storage;
mod tree;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
pub use cyber_server::runtime::SnapshotRecovery as Recovery;
use cyber_server::runtime::{FileDiff, Snapshot};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use storage::{Result, fail};

const RECORD_LIMIT: u64 = 8 * 1024 * 1024;
const ENTRY_LIMIT: usize = 4096;
#[derive(Clone)]
pub(crate) struct Options {
    pub max_file: u64,
    pub max_total: u64,
    pub ignore: Vec<String>,
    pub private_roots: Vec<PathBuf>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    object: String,
    length: u64,
    mode: u32,
    uid: u32,
    gid: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tree {
    version: u32,
    location: PathBuf,
    root: String,
    files: BTreeMap<String, Entry>,
    skipped: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    accessed: u64,
    tree: Tree,
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) struct Store {
    root: PathBuf,
    root_id: String,
    data_path: PathBuf,
    work: Dir,
    namespace: Dir,

    objects: Dir,
    manifests: Dir,
    journals: Dir,
    _lock: Option<std::fs::File>,
    options: Options,
    usage: std::cell::Cell<Option<u64>>,
}

mod lifetime;

pub(crate) struct WorkerGuard {
    pub store: Store,
    pub workspace: tokio::sync::OwnedMutexGuard<()>,
    pub origin: Option<cyber_server::runtime::SnapshotOrigin>,
    pub digest: Option<String>,
    pub receipt: Option<cyber_server::runtime::SnapshotRecoveryReceipt>,
}

impl cyber_server::runtime::SnapshotNativeGuard for WorkerGuard {
    fn intent_digest(&self) -> Option<&str> {
        self.digest.as_deref()
    }
    fn verify(&self, origin: Option<&cyber_server::runtime::SnapshotOrigin>) -> Result<()> {
        let _retained_workspace = &self.workspace;
        if origin != self.origin.as_ref() {
            return Err("Snapshot receipt belongs to another worker".into());
        }
        if self.store.worker_digest(origin)? != self.digest {
            return Err("Snapshot immutable intent changed before consumption".into());
        }
        if let Some(receipt) = &self.receipt {
            self.store.verify_receipt(origin, receipt)?;
        }
        self.store.verify_worker_origin(origin)
    }
    fn acknowledge(
        &mut self,
        origin: Option<&cyber_server::runtime::SnapshotOrigin>,
    ) -> Result<()> {
        self.verify(origin)?;
        self.store.acknowledge_worker(origin)
    }
}
impl Store {
    pub fn open(data: &Path, location: &str, options: Options) -> Result<Self> {
        Self::open_mode(data, location, options, true, true)
    }
    pub fn existing(data: &Path, location: &str, options: Options) -> Result<Option<Self>> {
        Self::existing_mode(data, location, options, false)
    }
    pub fn recover_existing(data: &Path, location: &str, options: Options) -> Result<Self> {
        Self::existing_mode(data, location, options, true)?
            .ok_or("No retained file-copy namespace for this Location".into())
    }
    fn existing_mode(
        data: &Path,
        location: &str,
        options: Options,
        own: bool,
    ) -> Result<Option<Self>> {
        if !data.exists() {
            return Ok(None);
        }
        let root = Path::new(location).canonicalize().map_err(fail)?;
        let work = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).map_err(fail)?;
        let id = storage::dir_identity(&work)?;
        let key = storage::digest(format!("{}\0{}", root.display(), id).as_bytes());
        let data_dir = Dir::open_ambient_dir(data, cap_std::ambient_authority()).map_err(fail)?;
        if data_dir.symlink_metadata("snapshot").is_err() {
            return Ok(None);
        }
        let snapshot = storage::existing_child(&data_dir, "snapshot", false)?;
        if snapshot.symlink_metadata("global").is_err() {
            return Ok(None);
        }
        let global = storage::existing_child(&snapshot, "global", true)?;
        if global.symlink_metadata(&key).is_err() {
            return Ok(None);
        }
        Self::open_mode(data, location, options, false, own).map(Some)
    }
    fn open_mode(
        data: &Path,
        location: &str,
        mut options: Options,
        create: bool,
        own: bool,
    ) -> Result<Self> {
        options.private_roots = options
            .private_roots
            .into_iter()
            .map(|p| p.canonicalize().unwrap_or(p))
            .collect();
        let root = Path::new(location).canonicalize().map_err(fail)?;
        if !root.is_dir() || root.parent().is_none() {
            return Err("Snapshot Location must be a project directory".into());
        }
        if create {
            std::fs::create_dir_all(data).map_err(fail)?;
        }
        let data_path = data.canonicalize().map_err(fail)?;
        if root.starts_with(&data_path) {
            return Err("Private owner state cannot be a snapshot Location".into());
        }
        let work = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).map_err(fail)?;
        let root_id = storage::dir_identity(&work)?;
        if options.private_roots.iter().any(|p| root.starts_with(p)) {
            return Err("Private owner state cannot be a snapshot Location".into());
        }
        let data = Dir::open_ambient_dir(&data_path, cap_std::ambient_authority()).map_err(fail)?;
        use std::os::unix::fs::MetadataExt;
        let data_metadata = data
            .try_clone()
            .map_err(fail)?
            .into_std_file()
            .metadata()
            .map_err(fail)?;
        if data_metadata.mode() & 0o022 != 0 {
            return Err("Snapshot data directory is writable by other users".into());
        }
        let child = |parent: &Dir, name: &str, private: bool| {
            if !create {
                storage::existing_child(parent, name, private)
            } else if private {
                storage::private_child(parent, name)
            } else {
                storage::shared_child(parent, name)
            }
        };
        let snapshot = child(&data, "snapshot", false)?;
        let copy = child(&snapshot, "global", true)?;
        let key = storage::digest(format!("{}\0{}", root.display(), root_id).as_bytes());
        let dir = child(&copy, &key, true)?;
        let lock = if own {
            Some(storage::lock(&dir, create)?)
        } else {
            None
        };
        let objects = child(&dir, "files", true)?;
        let manifests = child(&dir, "manifests", true)?;
        let journals = child(&dir, "journals", true)?;
        Ok(Self {
            root,
            root_id,
            data_path,
            work,
            namespace: dir,
            objects,
            manifests,
            journals,
            _lock: lock,
            options,
            usage: std::cell::Cell::new(None),
        })
    }
    fn verify_root(&self) -> Result<()> {
        if !std::fs::symlink_metadata(&self.root)
            .map_err(fail)?
            .file_type()
            .is_dir()
        {
            return Err("Snapshot Location identity changed".into());
        }
        let fresh =
            Dir::open_ambient_dir(&self.root, cap_std::ambient_authority()).map_err(fail)?;
        if storage::dir_identity(&fresh)? != self.root_id {
            return Err("Snapshot Location identity changed".into());
        }
        Ok(())
    }
    pub(super) fn lock_identity(&self) -> Result<String> {
        storage::identity(
            self._lock
                .as_ref()
                .ok_or("Snapshot mutation lock is not held")?,
        )
    }
    pub(super) fn verify_lock_route(&self, expected: &str) -> Result<()> {
        let current = storage::open(&self.namespace, Path::new("lock"))?;
        storage::validate_lock(&current)?;
        if storage::identity(&current)? != expected
            || self
                ._lock
                .as_ref()
                .is_some_and(|held| storage::identity(held).as_deref() != Ok(expected))
        {
            return Err("Snapshot private lock identity changed".into());
        }
        Ok(())
    }
    fn usage(&self) -> Result<u64> {
        if let Some(size) = self.usage.get() {
            return Ok(size);
        }
        let mut size = 0u64;
        for dir in [&self.objects, &self.manifests] {
            for name in storage::names(dir, 100_000)? {
                size = size
                    .checked_add(
                        storage::open(dir, Path::new(&name))?
                            .metadata()
                            .map_err(fail)?
                            .len(),
                    )
                    .ok_or("Snapshot storage overflow")?;
            }
        }
        // Journals include displaced preimages/staging files and are never quota-evicted.
        for name in storage::names(&self.journals, ENTRY_LIMIT)? {
            let journal = self.journals.open_dir_nofollow(&name).map_err(fail)?;
            for file in storage::names(&journal, ENTRY_LIMIT * 3)? {
                if journal.symlink_metadata(&file).map_err(fail)?.is_dir() {
                    let parent = journal.open_dir_nofollow(&file).map_err(fail)?;
                    if !file.starts_with("parent_") || !storage::names(&parent, 1)?.is_empty() {
                        return Err("Unexpected snapshot journal directory".into());
                    }
                    continue;
                }
                size = size
                    .checked_add(
                        storage::open(&journal, Path::new(&file))?
                            .metadata()
                            .map_err(fail)?
                            .len(),
                    )
                    .ok_or("Snapshot storage overflow")?;
            }
        }
        match self.namespace.symlink_metadata("workers") {
            Ok(_) => {
                let workers = storage::existing_child(&self.namespace, "workers", true)?;
                for name in storage::names(&workers, ENTRY_LIMIT)? {
                    let file = storage::open(&workers, Path::new(&name))?;
                    storage::validate_lock(&file)?;
                    size = size
                        .checked_add(file.metadata().map_err(fail)?.len())
                        .ok_or("Snapshot storage overflow")?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(fail(error)),
        }
        self.usage.set(Some(size));
        Ok(size)
    }
    fn reserve(&self, size: u64) -> Result<()> {
        if self.options.max_total == 0
            || self
                .usage()?
                .checked_add(size)
                .is_none_or(|n| n > self.options.max_total)
        {
            return Err(
                "Non-Git snapshot storage cap reached; conversation rewind remains available"
                    .into(),
            );
        }
        self.usage.set(Some(
            self.usage()?
                .checked_add(size)
                .ok_or("Snapshot storage overflow")?,
        ));
        Ok(())
    }
    fn object(&self, bytes: &[u8]) -> Result<Entry> {
        let object = storage::digest(bytes);
        match storage::read(&self.objects, &object, self.options.max_total) {
            Ok(existing) if existing == bytes => {}
            Ok(_) => return Err("Snapshot object identity mismatch".into()),
            Err(_) if self.objects.symlink_metadata(&object).is_ok() => {
                return Err("Unreadable snapshot object".into());
            }
            Err(_) => {
                self.reserve(bytes.len() as u64)?;
                storage::atomic(&self.objects, &object, bytes)?;
            }
        }
        Ok(Entry {
            object,
            length: bytes.len() as u64,
            mode: 0o644,
            uid: 0,
            gid: 0,
        })
    }
    fn bytes(&self, entry: &Entry) -> Result<Vec<u8>> {
        if !storage::valid_digest(&entry.object) || entry.length > self.options.max_total {
            return Err("Invalid snapshot object".into());
        }
        let bytes = storage::read(&self.objects, &entry.object, entry.length)?;
        if bytes.len() as u64 != entry.length || storage::digest(&bytes) != entry.object {
            return Err("Corrupt snapshot object".into());
        }
        Ok(bytes)
    }
    fn save(&self, tree: Tree) -> Result<Snapshot> {
        let digest = storage::digest(&serde_json::to_vec(&tree).map_err(fail)?);
        let bytes = serde_json::to_vec(&Manifest {
            accessed: now(),
            tree: tree.clone(),
        })
        .map_err(fail)?;
        if bytes.len() as u64 > RECORD_LIMIT {
            return Err("Snapshot manifest exceeds limit".into());
        }
        let old = self
            .manifests
            .symlink_metadata(&digest)
            .ok()
            .map_or(0, |m| m.len());
        if bytes.len() as u64 > old {
            self.reserve(bytes.len() as u64 - old)?;
        }
        storage::atomic(&self.manifests, &digest, &bytes)?;
        Ok(Snapshot {
            tree: format!("copy:{digest}"),
            skipped: tree.skipped,
        })
    }
    fn load(&self, id: &str) -> Result<Tree> {
        self.verify_root()?;
        let digest = id
            .strip_prefix("copy:")
            .filter(|s| storage::valid_digest(s))
            .ok_or("Invalid non-Git snapshot identity")?;
        let manifest: Manifest =
            serde_json::from_slice(&storage::read(&self.manifests, digest, RECORD_LIMIT)?)
                .map_err(fail)?;
        let tree = manifest.tree;
        if tree.version != 1
            || tree.location != self.root
            || tree.root != self.root_id
            || tree.files.len() > ENTRY_LIMIT
            || storage::digest(&serde_json::to_vec(&tree).map_err(fail)?) != digest
        {
            return Err("Snapshot belongs to a different Location or is corrupt".into());
        }
        // A preview/recovery lease keeps an otherwise unreferenced manifest recent.
        self.save(tree.clone())?;
        Ok(tree)
    }
    pub fn track(&self) -> Result<Snapshot> {
        self.ensure_recovered()?;
        self.save(self.capture()?)
    }
    pub fn changed(&self, from: &str, to: &str) -> Result<Vec<String>> {
        let (from, to) = (self.load(from)?, self.load(to)?);
        Ok(changes(&from, &to))
    }
    pub fn diff(&self, from: &str, to: &str) -> Result<Vec<FileDiff>> {
        let (from, to) = (self.load(from)?, self.load(to)?);
        changes(&from, &to)
            .into_iter()
            .map(|path| self.file_diff(&path, from.files.get(&path), to.files.get(&path)))
            .collect()
    }
    fn file_diff(
        &self,
        path: &str,
        before: Option<&Entry>,
        after: Option<&Entry>,
    ) -> Result<FileDiff> {
        let a = before
            .map(|e| self.bytes(e))
            .transpose()?
            .unwrap_or_default();
        let b = after
            .map(|e| self.bytes(e))
            .transpose()?
            .unwrap_or_default();
        let (mut patch, additions, deletions) =
            match (std::str::from_utf8(&a), std::str::from_utf8(&b)) {
                (Ok(a), Ok(b)) if !a.contains('\0') && !b.contains('\0') => {
                    let diff = similar::TextDiff::from_lines(a, b);
                    let additions = diff
                        .iter_all_changes()
                        .filter(|c| c.tag() == similar::ChangeTag::Insert)
                        .count() as u32;
                    let deletions = diff
                        .iter_all_changes()
                        .filter(|c| c.tag() == similar::ChangeTag::Delete)
                        .count() as u32;
                    (
                        diff.unified_diff()
                            .header(&format!("a/{path}"), &format!("b/{path}"))
                            .to_string(),
                        additions,
                        deletions,
                    )
                }
                _ => ("Binary files differ\n".into(), 0, 0),
            };
        if let (Some(a), Some(b)) = (before, after)
            && a.mode != b.mode
        {
            patch = format!("old mode {:04o}\nnew mode {:04o}\n{patch}", a.mode, b.mode);
        }
        Ok(FileDiff {
            file: path.into(),
            status: match (before, after) {
                (None, _) => "added",
                (_, None) => "deleted",
                _ => "modified",
            }
            .into(),
            patch,
            additions,
            deletions,
            binary: None,
        })
    }
}
fn changes(a: &Tree, b: &Tree) -> Vec<String> {
    a.files
        .keys()
        .chain(b.files.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|p| a.files.get(*p) != b.files.get(*p) && !excluded(a, p) && !excluded(b, p))
        .cloned()
        .collect()
}

/// Prune only unreferenced, old manifests. Journals keep all their objects pinned;
/// an unavailable durable-reference view never authorizes deletion.
pub(crate) fn gc(
    data: &Path,
    config: &std::sync::Arc<crate::ConfigFn>,
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
    for location in storage::names(&root, 100_000)? {
        if !storage::valid_digest(&location) {
            return Err("Invalid snapshot namespace".into());
        }
        let dir = storage::private_child(&root, &location)?;
        let _lock = match storage::lock(&dir, false) {
            Ok(lock) => lock,
            Err(_) => continue,
        };
        gc_location(&dir, config, protected)?;
    }
    Ok(())
}
fn gc_location(
    dir: &Dir,
    config: &std::sync::Arc<crate::ConfigFn>,
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
    let manifests = storage::private_child(dir, "manifests")?;
    let journals = storage::private_child(dir, "journals")?;
    // Until an owner resolves/acknowledges native evidence, no journal is evicted.
    if !storage::names(&journals, ENTRY_LIMIT)?.is_empty() {
        return Ok(());
    }
    let objects = storage::private_child(dir, "files")?;
    let mut keep = BTreeSet::new();
    for name in storage::names(&manifests, 100_000)? {
        if temporary(&name) {
            prune_temporary(&manifests, &name)?;
            continue;
        }
        if !storage::valid_digest(&name) {
            return Err("Unrecognized snapshot manifest".into());
        }
        let manifest: Manifest =
            serde_json::from_slice(&storage::read(&manifests, &name, RECORD_LIMIT)?)
                .map_err(fail)?;
        if manifest.tree.version != 1
            || storage::digest(&serde_json::to_vec(&manifest.tree).map_err(fail)?) != name
        {
            return Err("Corrupt snapshot manifest".into());
        }
        let days = config(&manifest.tree.location)
            .pointer("/snapshots/retention_days")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(7);
        let old = days != 0 && now().saturating_sub(manifest.accessed) > days.saturating_mul(86400);
        if old && !protected.contains(&format!("copy:{name}")) {
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
    for name in storage::names(&objects, 100_000)? {
        if temporary(&name) {
            prune_temporary(&objects, &name)?;
            continue;
        }
        if !storage::valid_digest(&name) {
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
                .is_some_and(|age| age.as_secs() > 7 * 86400);
            if old {
                objects.remove_file(&name).map_err(fail)?;
            }
        }
    }
    storage::sync(&objects)
}

fn excluded(tree: &Tree, path: &str) -> bool {
    tree.skipped
        .iter()
        .filter_map(|p| p.split_once(": ").map(|(_, p)| p))
        .any(|p| path == p || path.starts_with(&format!("{p}/")))
}
impl Store {
    pub fn exclusions(&self, ids: &[String]) -> Result<Vec<String>> {
        let mut skipped = BTreeSet::new();
        for id in ids {
            let tree = self.load(id)?;
            skipped.extend(
                tree.skipped
                    .into_iter()
                    .map(|path| format!("historical {id}: {path}")),
            );
        }
        Ok(skipped.into_iter().collect())
    }
}

fn temporary(name: &str) -> bool {
    name.strip_prefix("tmp_")
        .is_some_and(|id| id.len() == 26 && id.parse::<ulid::Ulid>().is_ok())
}
fn prune_temporary(dir: &Dir, name: &str) -> Result<()> {
    let file = storage::open(dir, Path::new(name))?;
    let old = file
        .metadata()
        .map_err(fail)?
        .modified()
        .ok()
        .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age.as_secs() > 7 * 86400);
    if old {
        dir.remove_file(name).map_err(fail)?;
        storage::sync(dir)?;
    }
    Ok(())
}
