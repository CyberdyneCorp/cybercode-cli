//! Durable native evidence for source effects, independent of progress records.
use super::restore::Change;
use super::storage::{self, Result, fail};
use super::{ENTRY_LIMIT, Entry, Store};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImageObservation {
    pub identity: String,
    pub entry: Entry,
    pub generation: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Slots {
    pub source: Option<ImageObservation>,
    pub stage: Option<ImageObservation>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Observations {
    pub files: Vec<Slots>,
    parents: Vec<ParentObservation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentObservation {
    path: String,
    source: Option<DirectoryObservation>,
    stage: Option<DirectoryObservation>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryObservation {
    identity: String,
    generation: String,
}

impl Observations {
    pub(super) fn has_links(&self) -> bool {
        self.files
            .iter()
            .flat_map(|slots| [&slots.source, &slots.stage])
            .flatten()
            .any(|image| image.entry.kind == super::ImageKind::Symlink)
    }
    pub fn validate(&self) -> Result<()> {
        if self.files.len() > ENTRY_LIMIT || self.parents.len() > ENTRY_LIMIT + 1 {
            return Err("Recovery observation limit exceeded".into());
        }
        for slots in &self.files {
            for image in [&slots.source, &slots.stage].into_iter().flatten() {
                if !valid_identity(&image.identity)
                    || !valid_entry(&image.entry)
                    || !storage::valid_digest(&image.generation)
                {
                    return Err("Invalid recovery file observation".into());
                }
            }
        }
        for parent in &self.parents {
            if !parent.path.is_empty() {
                super::tree::safe_path(&parent.path)?;
            }
            for directory in [&parent.source, &parent.stage].into_iter().flatten() {
                if !valid_identity(&directory.identity)
                    || !storage::valid_digest(&directory.generation)
                {
                    return Err("Invalid recovery parent observation".into());
                }
            }
        }
        Ok(())
    }
    pub fn budget(&self) -> Self {
        let mut value = self.clone();
        let image = ImageObservation {
            identity: "0".repeat(storage::IDENTITY_MAX_BYTES),
            generation: "0".repeat(64),
            entry: Entry {
                kind: super::ImageKind::File,
                object: "0".repeat(64),
                length: u64::MAX,
                mode: u32::MAX,
                uid: u32::MAX,
                gid: u32::MAX,
            },
        };
        for slots in &mut value.files {
            let mut bound = image.clone();
            if [&slots.source, &slots.stage]
                .into_iter()
                .flatten()
                .any(|image| image.entry.kind == super::ImageKind::Symlink)
            {
                bound.entry.kind = super::ImageKind::Symlink;
            }
            slots.source = Some(bound.clone());
            slots.stage = Some(bound);
        }
        for parent in &mut value.parents {
            let bound = DirectoryObservation {
                identity: "0".repeat(storage::IDENTITY_MAX_BYTES),
                generation: "0".repeat(64),
            };
            parent.source = Some(bound.clone());
            parent.stage = Some(bound);
        }
        value
    }
}

pub(super) fn generation(file: &std::fs::File) -> Result<String> {
    use std::os::unix::fs::MetadataExt;
    let m = file.metadata().map_err(fail)?;
    let value = (
        m.dev(),
        m.ino(),
        m.mode(),
        m.uid(),
        m.gid(),
        m.nlink(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    );
    Ok(storage::digest(&serde_json::to_vec(&value).map_err(fail)?))
}

pub(super) fn observe_image(
    store: &Store,
    parent: Option<&Dir>,
    name: &std::ffi::OsStr,
) -> Result<Option<ImageObservation>> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    match parent.symlink_metadata(name) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(fail(error)),
        Ok(_) => {}
    }
    let pin = super::links::pin_image(parent, name)?;
    let before = generation(&pin)?;
    let (identity, entry) =
        observed(store, Some(parent), name)?.ok_or("Recovery image disappeared")?;
    let current = super::links::pin_image(parent, name)?;
    if storage::identity(&pin)? != identity
        || storage::identity(&current)? != identity
        || generation(&pin)? != before
        || generation(&current)? != before
    {
        return Err("Recovery image changed while reviewing".into());
    }
    Ok(Some(ImageObservation {
        identity,
        entry,
        generation: before,
    }))
}

fn observe_directory(dir: &Dir) -> Result<DirectoryObservation> {
    let file = dir.try_clone().map_err(fail)?.into_std_file();
    Ok(DirectoryObservation {
        identity: storage::identity(&file)?,
        generation: generation(&file)?,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ownership {
    version: u32,
    parents: Vec<Parent>,
    files: Vec<File>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Parent {
    path: String,
    identity: String,
    stage: Option<String>,
    metadata: Option<cyber_core::binary::native::Metadata>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    source: Option<String>,
    stage: Option<String>,
}

impl Parent {
    fn validate(&self, index: usize, path: &str, identities: &mut BTreeSet<String>) -> Result<()> {
        if self.path != path
            || !valid_identity(&self.identity)
            || !identities.insert(self.identity.clone())
            || self.metadata.is_some() != self.stage.is_some()
            || self
                .metadata
                .as_ref()
                .is_some_and(|m| m.mode != 0o700 || !storage::valid_digest(&m.evidence))
            || self
                .stage
                .as_ref()
                .is_some_and(|s| s != &format!("parent_{index}"))
        {
            return Err("Invalid restore parent evidence".into());
        }
        Ok(())
    }
    fn verify(&self, dir: &Dir) -> Result<()> {
        if storage::dir_identity(dir)? != self.identity {
            return Err("Restore parent identity changed; journal retained".into());
        }
        if let Some(metadata) = &self.metadata
            && storage::parent_metadata(dir)? != *metadata
        {
            return Err("Restore created-parent metadata changed; journal retained".into());
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Effect {
    Unapplied,
    Applied,
}

fn parent_paths(changes: &[Change]) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::new();
    let mut files = BTreeSet::new();
    for change in changes {
        super::tree::safe_path(&change.path)?;
        if !files.insert(change.path.clone()) || change.before == change.after {
            return Err("Invalid restore change inventory".into());
        }
        let mut path = Path::new(&change.path).parent();
        while let Some(parent) = path.filter(|p| !p.as_os_str().is_empty()) {
            paths.insert(parent.to_str().ok_or("Invalid restore parent")?.to_owned());
            if paths.len() > ENTRY_LIMIT {
                return Err("Restore parent count exceeds limit".into());
            }
            path = parent.parent();
        }
    }
    if paths.iter().any(|path| files.contains(path)) {
        return Err("Restore file/parent paths overlap".into());
    }
    Ok(paths)
}

fn valid_identity(value: &str) -> bool {
    if value.is_empty() || value.len() > storage::IDENTITY_MAX_BYTES {
        return false;
    }
    let parts: Vec<_> = value.split(':').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && parts[..3].iter().all(|p| p.parse::<u64>().is_ok())
        && parts[3].parse::<u32>().is_ok_and(|n| n < 1_000_000_000)
}

fn existing_parent(store: &Store, path: &str) -> Result<Option<Dir>> {
    let mut dir = store.work.try_clone().map_err(fail)?;
    for component in Path::new(path).components() {
        let Component::Normal(name) = component else {
            return Err("Invalid restore parent".into());
        };
        match dir.symlink_metadata(name) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(fail(error)),
            Ok(_) => dir = dir.open_dir_nofollow(name).map_err(fail)?,
        }
    }
    Ok(Some(dir))
}

fn same_filesystem(dir: &Dir, private: &Dir) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let source = dir.try_clone().map_err(fail)?.into_std_file();
    let private = private.try_clone().map_err(fail)?.into_std_file();
    if source.metadata().map_err(fail)?.dev() != private.metadata().map_err(fail)?.dev() {
        return Err("Restore requires same-filesystem parents".into());
    }
    Ok(())
}

fn observed(
    store: &Store,
    dir: Option<&Dir>,
    name: &std::ffi::OsStr,
) -> Result<Option<(String, Entry)>> {
    let Some(dir) = dir else { return Ok(None) };
    match dir.symlink_metadata(name) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(fail(error)),
        Ok(_) => {}
    }
    // Keep the first inode pinned through hashing and the second route lookup.
    let pin = super::links::pin_image(dir, name)?;
    let identity = storage::identity(&pin)?;
    let entry = store.inspect_entry(dir, name)?;
    let current = super::links::pin_image(dir, name)?;
    if storage::identity(&current)? != identity {
        return Err("Restore file identity changed while inspecting".into());
    }
    Ok(Some((identity, entry)))
}

fn matches(
    observed: &Option<(String, Entry)>,
    identity: &Option<String>,
    entry: &Option<Entry>,
) -> bool {
    match (observed, identity, entry) {
        (None, None, None) => true,
        (Some((id, value)), Some(want), Some(expected)) => id == want && value == expected,
        _ => false,
    }
}

fn valid_entry(entry: &Entry) -> bool {
    storage::valid_digest(&entry.object)
        && entry.length <= 64 * 1024 * 1024
        && entry.mode & !0o777 == 0
        && match entry.kind {
            super::ImageKind::File => entry.mode & 0o222 != 0,
            super::ImageKind::Symlink => {
                entry.length > 0 && entry.length <= super::links::TARGET_LIMIT as u64
            }
        }
}

impl File {
    fn prepare(
        store: &Store,
        journal: &Dir,
        parent: Option<&Dir>,
        index: usize,
        change: &Change,
    ) -> Result<Self> {
        let leaf = Path::new(&change.path)
            .file_name()
            .ok_or("Invalid restore leaf")?;
        let source = observed(store, parent, leaf)?;
        let staged = observed(
            store,
            Some(journal),
            std::ffi::OsStr::new(&format!("stage_{index}")),
        )?;
        if source.as_ref().map(|(_, entry)| entry) != change.before.as_ref()
            || staged.as_ref().map(|(_, entry)| entry) != change.after.as_ref()
        {
            return Err("Restore source/stage changed before ownership preparation".into());
        }
        if let (Some((identity, _)), Some(parent)) = (&source, parent) {
            let pin = super::links::pin_image(parent, leaf)?;
            if storage::identity(&pin)? != *identity {
                return Err("Restore original inode changed".into());
            }
            // Make the actual displaced preimage durable before any source rename.
            // Read-only inspection deliberately never performs this preparation.
            if pin.metadata().map_err(fail)?.file_type().is_symlink() {
                storage::sync(parent)?;
            } else {
                storage::sync_file(&pin)?;
            }
        }
        Ok(Self {
            source: source.map(|(id, _)| id),
            stage: staged.map(|(id, _)| id),
        })
    }
}

impl Ownership {
    pub(super) fn created_parents(&self) -> Vec<String> {
        self.parents
            .iter()
            .filter(|parent| parent.stage.is_some())
            .map(|parent| parent.path.clone())
            .collect()
    }
    /// Capture ordinary current files without assigning foreign files to this restore.
    /// Keep can preserve these images; rollback still requires strict `inspect` proof.
    pub(super) fn observe(
        &self,
        store: &Store,
        journal: &Dir,
        changes: &[Change],
    ) -> Result<Observations> {
        self.validate(changes)?;
        self.observe_current(store, journal, changes)
    }
    pub(super) fn observe_unprepared(
        store: &Store,
        journal: &Dir,
        changes: &[Change],
    ) -> Result<Observations> {
        Self::budget(changes)?.observe_current(store, journal, changes)
    }
    fn observe_current(
        &self,
        store: &Store,
        journal: &Dir,
        changes: &[Change],
    ) -> Result<Observations> {
        store.verify_root()?;
        let mut parents = vec![ParentObservation {
            path: String::new(),
            source: Some(observe_directory(&store.work)?),
            stage: None,
        }];
        for parent in &self.parents {
            let live = existing_parent(store, &parent.path)?
                .as_ref()
                .map(observe_directory)
                .transpose()?;
            let stage = parent
                .stage
                .as_ref()
                .map(|stage| match journal.symlink_metadata(stage) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(fail(error)),
                    Ok(_) => observe_directory(&journal.open_dir_nofollow(stage).map_err(fail)?)
                        .map(Some),
                })
                .transpose()?
                .flatten();
            parents.push(ParentObservation {
                path: parent.path.clone(),
                source: live,
                stage,
            });
        }
        let mut files = vec![];
        for (index, change) in changes.iter().enumerate() {
            store.check_source_path(&change.path)?;
            let parent_path = Path::new(&change.path)
                .parent()
                .ok_or("Invalid recovery parent")?;
            let parent = existing_parent(
                store,
                parent_path.to_str().ok_or("Invalid recovery parent")?,
            )?;
            let leaf = Path::new(&change.path)
                .file_name()
                .ok_or("Invalid recovery path")?;
            files.push(Slots {
                source: observe_image(store, parent.as_ref(), leaf)?,
                stage: observe_image(
                    store,
                    Some(journal),
                    std::ffi::OsStr::new(&format!("stage_{index}")),
                )?,
            });
        }
        // Detect route changes during the bounded scan, including parent edits
        // that do not replace any of the tracked leaf files.
        for parent in &parents {
            let now = existing_parent(store, &parent.path)?
                .as_ref()
                .map(observe_directory)
                .transpose()?;
            if now != parent.source {
                return Err("Recovery parent changed while reviewing".into());
            }
        }
        store.verify_root()?;
        Ok(Observations { files, parents })
    }
    pub(super) fn original_identity(&self, index: usize) -> Option<&str> {
        self.files
            .get(index)
            .and_then(|file| file.source.as_deref())
    }
    pub(super) fn verify_original(&self, index: usize, file: &std::fs::File) -> Result<()> {
        if self.files.get(index).and_then(|file| file.source.as_ref())
            != Some(&storage::identity(file)?)
        {
            return Err("Restore original inode changed".into());
        }
        Ok(())
    }
    /// Maximum encoded shape, before any private stages or source parents exist.
    pub(super) fn budget(changes: &[Change]) -> Result<Self> {
        let identity = "0".repeat(storage::IDENTITY_MAX_BYTES);
        let parents = parent_paths(changes)?
            .into_iter()
            .enumerate()
            .map(|(index, path)| Parent {
                path,
                identity: identity.clone(),
                stage: Some(format!("parent_{index}")),
                metadata: Some(cyber_core::binary::native::Metadata {
                    mode: u32::MAX,
                    uid: u32::MAX,
                    gid: u32::MAX,
                    evidence: "0".repeat(64),
                }),
            })
            .collect();
        let files = changes
            .iter()
            .map(|change| File {
                source: change.before.as_ref().map(|_| identity.clone()),
                stage: change.after.as_ref().map(|_| identity.clone()),
            })
            .collect();
        Ok(Self {
            version: 1,
            parents,
            files,
        })
    }

    pub(super) fn prepare(store: &Store, journal: &Dir, changes: &[Change]) -> Result<Self> {
        store.verify_root()?;
        same_filesystem(&store.work, journal)?;
        for change in changes {
            store.check_source_path(&change.path)?;
        }
        let mut proof = Self::budget(changes)?;
        for parent in &mut proof.parents {
            let dir = if let Some(dir) = existing_parent(store, &parent.path)? {
                parent.stage = None;
                parent.metadata = None;
                dir
            } else {
                storage::private_child(
                    journal,
                    parent.stage.as_deref().ok_or("Missing parent stage")?,
                )?
            };
            same_filesystem(&dir, journal)?;
            parent.identity = storage::dir_identity(&dir)?;
            if parent.stage.is_some() {
                parent.metadata = Some(storage::parent_metadata(&dir)?);
                storage::sync(&dir)?;
            }
        }
        for (index, change) in changes.iter().enumerate() {
            let parent = proof.parent(store, journal, &change.path)?;
            proof.files[index] = File::prepare(store, journal, parent.as_ref(), index, change)?;
        }
        storage::sync(journal)?;
        store.verify_root()?;
        proof.validate(changes)?;
        if proof
            .parents
            .iter()
            .any(|parent| parent.identity == store.root_id)
        {
            return Err("Restore parent aliases the Location root".into());
        }
        Ok(proof)
    }

    pub(super) fn validate(&self, changes: &[Change]) -> Result<()> {
        let paths = parent_paths(changes)?;
        if self.version != 1
            || self.files.len() != changes.len()
            || self.parents.len() != paths.len()
        {
            return Err("Invalid restore ownership inventory".into());
        }
        let mut identities = BTreeSet::new();
        for (index, (parent, path)) in self.parents.iter().zip(paths).enumerate() {
            parent.validate(index, &path, &mut identities)?;
        }
        for (file, change) in self.files.iter().zip(changes) {
            for (identity, entry) in [(&file.source, &change.before), (&file.stage, &change.after)]
            {
                if identity.is_some() != entry.is_some()
                    || entry.as_ref().is_some_and(|entry| !valid_entry(entry))
                    || identity.as_ref().is_some_and(|id| !valid_identity(id))
                    || identity
                        .as_ref()
                        .is_some_and(|id| !identities.insert(id.clone()))
                {
                    return Err("Invalid restore file evidence".into());
                }
            }
        }
        Ok(())
    }

    pub(super) fn parent(&self, store: &Store, journal: &Dir, path: &str) -> Result<Option<Dir>> {
        self.resolve_parent(store, journal, path, None)
    }

    pub(super) fn create_parents(
        &self,
        store: &Store,
        journal: &Dir,
        path: &str,
        authorize: &dyn Fn() -> Result<()>,
    ) -> Result<Dir> {
        self.resolve_parent(store, journal, path, Some(authorize))?
            .ok_or("Missing restore parent".into())
    }

    fn resolve_parent(
        &self,
        store: &Store,
        journal: &Dir,
        path: &str,
        authorize: Option<&dyn Fn() -> Result<()>>,
    ) -> Result<Option<Dir>> {
        store.verify_root()?;
        store.check_source_path(path)?;
        let mut dir = store.work.try_clone().map_err(fail)?;
        let mut relative = std::path::PathBuf::new();
        for component in Path::new(path)
            .parent()
            .ok_or("Invalid restore parent")?
            .components()
        {
            let Component::Normal(name) = component else {
                return Err("Invalid restore parent".into());
            };
            relative.push(name);
            let key = relative.to_str().ok_or("Invalid restore parent")?;
            let index = self
                .parents
                .binary_search_by(|parent| parent.path.as_str().cmp(key))
                .map_err(|_| "Unrecorded restore parent")?;
            let proof = &self.parents[index];
            if proof.identity == store.root_id {
                return Err("Restore parent aliases the Location root".into());
            }
            match dir.symlink_metadata(name) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let Some(authorize) = authorize else {
                        return Ok(None);
                    };
                    authorize()?;
                    let fresh = self
                        .parent(store, journal, key)?
                        .ok_or("Restore parent disappeared")?;
                    if storage::dir_identity(&fresh)? != storage::dir_identity(&dir)? {
                        return Err("Restore parent route changed".into());
                    }
                    self.publish_parent(journal, &dir, name, proof)?;
                }
                Err(error) => return Err(fail(error)),
                Ok(_) => {}
            }
            dir = dir.open_dir_nofollow(name).map_err(fail)?;
            proof.verify(&dir)?;
        }
        Ok(Some(dir))
    }

    fn publish_parent(
        &self,
        journal: &Dir,
        parent: &Dir,
        name: &std::ffi::OsStr,
        proof: &Parent,
    ) -> Result<()> {
        let stage = proof
            .stage
            .as_deref()
            .ok_or("Previously present restore parent is missing")?;
        let staged = journal.open_dir_nofollow(stage).map_err(fail)?;
        proof.verify(&staged)?;
        if !storage::names(&staged, 1)?.is_empty() {
            return Err("Restore parent stage changed".into());
        }
        rustix::fs::renameat_with(
            journal,
            stage,
            parent,
            name,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(fail)?;
        storage::sync(parent)?;
        storage::sync(journal)
    }

    pub(super) fn inspect_change(
        &self,
        store: &Store,
        journal: &Dir,
        index: usize,
        change: &Change,
    ) -> Result<Effect> {
        let file = self
            .files
            .get(index)
            .ok_or("Missing restore file evidence")?;
        let parent = self.parent(store, journal, &change.path)?;
        let leaf = Path::new(&change.path)
            .file_name()
            .ok_or("Invalid restore leaf")?;
        let source = observed(store, parent.as_ref(), leaf)?;
        let staged = observed(
            store,
            Some(journal),
            std::ffi::OsStr::new(&format!("stage_{index}")),
        )?;
        if matches(&source, &file.source, &change.before)
            && matches(&staged, &file.stage, &change.after)
        {
            return Ok(Effect::Unapplied);
        }
        if matches(&source, &file.stage, &change.after)
            && matches(&staged, &file.source, &change.before)
        {
            return Ok(Effect::Applied);
        }
        Err(format!(
            "Restore native ownership conflicts: {}",
            change.path
        ))
    }

    pub(super) fn inspect(
        &self,
        store: &Store,
        journal: &Dir,
        changes: &[Change],
    ) -> Result<Vec<Effect>> {
        self.validate(changes)?;
        store.verify_root()?;
        for change in changes {
            store.check_source_path(&change.path)?;
        }
        // Every prepared parent must remain in exactly one recorded slot, even
        // when no file below it was published before interruption.
        for parent in &self.parents {
            let live = existing_parent(store, &parent.path)?;
            if let Some(dir) = &live {
                parent.verify(dir)?;
            }
            let live_id = live.as_ref().map(storage::dir_identity).transpose()?;
            let staged_id = parent
                .stage
                .as_ref()
                .map(|stage| match journal.symlink_metadata(stage) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(fail(error)),
                    Ok(_) => {
                        let dir = journal.open_dir_nofollow(stage).map_err(fail)?;
                        parent.verify(&dir)?;
                        if !storage::names(&dir, 1)?.is_empty() {
                            return Err("Restore parent stage changed".into());
                        }
                        storage::dir_identity(&dir).map(Some)
                    }
                })
                .transpose()?
                .flatten();
            if !((live_id.as_ref() == Some(&parent.identity) && staged_id.is_none())
                || (parent.stage.is_some()
                    && live_id.is_none()
                    && staged_id.as_ref() == Some(&parent.identity)))
            {
                return Err("Restore parent ownership conflicts".into());
            }
        }
        changes
            .iter()
            .enumerate()
            .map(|(index, change)| self.inspect_change(store, journal, index, change))
            .collect()
    }
}
