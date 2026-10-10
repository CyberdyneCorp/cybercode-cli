//! Retained journal preparation, installation and fingerprint-governed recovery.
mod recovery;
#[cfg(windows)]
#[path = "transaction/windows.rs"]
mod windows;
use super::*;
use crate::memory::render_metadata_index;
#[cfg(not(windows))]
use cap_fs_ext::MetadataExt;
pub use recovery::MemoryRecoveryReview;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
#[cfg(windows)]
use windows::{
    apply_target, archive_journal, create_file, create_journal_directory, normalize_pair,
};

const INDEX_LIMIT: u64 = 8 * NOTE_LIMIT;
const INTENT_LIMIT: u64 = 4 * NOTE_LIMIT;
const HISTORY: &str = ".memory-history";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    id: String,
    name: String,
    before_note: Option<String>,
    after_note: Option<String>,
    before_index: Option<String>,
    after_index: String,
    catalog: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct MemoryMutation {
    pub id: String,
    pub name: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryJournalIdentity {
    pub receipt: MemoryMutation,
    pub intent_fingerprint: String,
}

/// Disposal retains the journal and does not acknowledge or roll back effects.
pub struct PreparedMemory<'guard, 'store> {
    scope: &'guard mut MemoryScope<'store>,
    dir: Dir,
    intent: Intent,
}

impl<'store> MemoryScope<'store> {
    pub fn write(&mut self, text: &str) -> Result<MemoryMutation, MemoryStorageError> {
        self.prepare_write(text)?.commit()
    }
    pub fn delete(&mut self, name: &str) -> Result<MemoryMutation, MemoryStorageError> {
        self.prepare_delete(name)?.commit()
    }
    pub fn prepare_write<'guard>(
        &'guard mut self,
        text: &str,
    ) -> Result<PreparedMemory<'guard, 'store>, MemoryStorageError> {
        mutation_platform()?;
        if text.len() as u64 > NOTE_LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        let document = MemoryDocument::for_write(text)?;
        let rendered = document.render_for_write()?;
        if rendered.len() as u64 > NOTE_LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        self.prepare(
            &document.metadata.name.clone(),
            Some((document, rendered)),
            None,
            None,
        )
    }
    pub fn prepare_delete<'guard>(
        &'guard mut self,
        name: &str,
    ) -> Result<PreparedMemory<'guard, 'store>, MemoryStorageError> {
        mutation_platform()?;
        validate_name(name)?;
        self.ready()?;
        self.read_file(&format!("{name}.md"), NOTE_LIMIT)?;
        self.prepare(name, None, None, None)
    }
    fn prepare<'guard>(
        &'guard mut self,
        name: &str,
        desired: Option<(MemoryDocument, String)>,
        expected: Option<&EditBefore>,
        expected_review: Option<&str>,
    ) -> Result<PreparedMemory<'guard, 'store>, MemoryStorageError> {
        self.ready()?;
        verify_private_directory(&self.store.dir)?;
        let before_note = optional_bytes(&self.store.dir, &format!("{name}.md"), NOTE_LIMIT)?;
        let before_index = optional_bytes(&self.store.dir, "MEMORY.md", INDEX_LIMIT)?;
        if let Some(expected) = expected
            && (before_note.as_deref().map(hash) != expected.note
                || before_index.as_deref().map(hash) != expected.index)
        {
            return Err(MemoryStorageError::ReviewConflict);
        }
        reserve_entries(
            &self.store.dir,
            before_note.is_none(),
            before_index.is_none(),
        )?;
        let catalog = self.fingerprints(name)?;
        let index = self.desired_index(name, desired.as_ref().map(|(doc, _)| &doc.metadata))?;
        if index.len() as u64 > INDEX_LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        if let Some(fingerprint) = expected_review {
            self.verify_edit_review(name, fingerprint)?;
        }
        let intent = Intent {
            version: 1,
            id: crate::ids::new_id("mem"),
            name: name.into(),
            before_note: before_note.as_deref().map(hash),
            after_note: desired.as_ref().map(|(_, text)| hash(text.as_bytes())),
            before_index: before_index.as_deref().map(hash),
            after_index: hash(index.as_bytes()),
            catalog,
        };
        let json = serde_json::to_vec(&intent)
            .map_err(|_| MemoryStorageError::Unsafe("transaction encoding failed"))?;
        if json.len() as u64 > INTENT_LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        let dir = create_journal_directory(&self.store.dir)?;
        sync_dir(&self.store.dir)?;
        if let Some((_, text)) = desired {
            create_file(&dir, "note.after", text.as_bytes())?;
        }
        create_file(&dir, "index.after", index.as_bytes())?;
        create_file(&dir, "intent.json", &json)?;
        sync_dir(&dir)?;
        Ok(PreparedMemory {
            scope: self,
            dir,
            intent,
        })
    }
    pub fn recover(&mut self) -> Result<Option<MemoryMutation>, MemoryStorageError> {
        self.recovery_prepared()?
            .map(PreparedMemory::commit)
            .transpose()
    }
    fn fingerprints(&self, excluded: &str) -> Result<BTreeMap<String, String>, MemoryStorageError> {
        self.catalog_unchecked()?
            .memories
            .into_iter()
            .filter(|note| note.name != excluded)
            .map(|note| {
                Ok((
                    note.name.clone(),
                    hash(&self.read_file(&format!("{}.md", note.name), NOTE_LIMIT)?),
                ))
            })
            .collect()
    }
    fn desired_index(
        &self,
        name: &str,
        desired: Option<&MemoryMetadata>,
    ) -> Result<String, MemoryStorageError> {
        let mut metadata = self.catalog_unchecked()?.memories;
        metadata.retain(|note| note.name != name);
        if let Some(desired) = desired {
            metadata.push(desired.clone());
        }
        Ok(render_metadata_index(&metadata)?)
    }
}

impl PreparedMemory<'_, '_> {
    pub fn journal_identity(&self) -> Result<MemoryJournalIdentity, MemoryStorageError> {
        let bytes =
            serde_json::to_vec(&self.intent).map_err(|_| MemoryStorageError::RecoveryRequired)?;
        Ok(MemoryJournalIdentity {
            receipt: MemoryMutation {
                id: self.intent.id.clone(),
                name: self.intent.name.clone(),
                deleted: self.intent.after_note.is_none(),
            },
            intent_fingerprint: hash(&bytes),
        })
    }

    pub fn commit(self) -> Result<MemoryMutation, MemoryStorageError> {
        self.commit_with_acknowledgement(|_| Ok(()))
    }

    /// Publish acknowledgement after synced note/index verification but before releasing
    /// journal fencing. Failure retains completed evidence for explicit reviewed recovery.
    pub fn commit_with_acknowledgement(
        mut self,
        acknowledge: impl FnOnce(&MemoryMutation) -> Result<(), MemoryStorageError>,
    ) -> Result<MemoryMutation, MemoryStorageError> {
        validate_intent(&self.intent)?;
        verify_private_directory(&self.scope.store.dir)?;
        verify_private_directory(&self.dir)?;
        verify_history(&self.scope.store.dir)?;
        if let Some(marker) = optional_bytes(&self.dir, "completed", NOTE_LIMIT)?
            && marker != b"committed\n"
        {
            return Err(MemoryStorageError::RecoveryRequired);
        }
        self.normalize_links()?;
        self.verify_catalog()?;
        self.verify_desired()?;
        self.verify_slots()?;
        self.apply_note()?;
        self.apply_index()?;
        self.verify_terminal()?;
        sync_dir(&self.scope.store.dir)?;
        if optional_bytes(&self.dir, "completed", NOTE_LIMIT)?.is_none() {
            create_file(&self.dir, "completed", b"committed\n")?;
        }
        sync_dir(&self.dir)?;
        let receipt = MemoryMutation {
            id: self.intent.id.clone(),
            name: self.intent.name.clone(),
            deleted: self.intent.after_note.is_none(),
        };
        acknowledge(&receipt)?;
        let history = private_directory(&self.scope.store.dir, HISTORY)?;
        match history.symlink_metadata(&self.intent.id) {
            Ok(_) => return Err(MemoryStorageError::Conflict),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        archive_journal(&self.scope.store.dir, self.dir, &history, &self.intent.id)?;
        sync_dir(&history)?;
        sync_dir(&self.scope.store.dir)?;
        Ok(MemoryMutation {
            id: self.intent.id.clone(),
            name: self.intent.name.clone(),
            deleted: self.intent.after_note.is_none(),
        })
    }
    fn normalize_links(&self) -> Result<(), MemoryStorageError> {
        normalize_pair(
            &self.dir,
            "note.after",
            &self.scope.store.dir,
            &format!("{}.md", self.intent.name),
            self.intent.after_note.as_deref(),
        )?;
        normalize_pair(
            &self.dir,
            "index.after",
            &self.scope.store.dir,
            "MEMORY.md",
            Some(&self.intent.after_index),
        )
    }
    fn verify_catalog(&self) -> Result<(), MemoryStorageError> {
        if self.scope.fingerprints(&self.intent.name)? != self.intent.catalog {
            return Err(MemoryStorageError::Conflict);
        }
        Ok(())
    }
    fn desired_note(&self) -> Result<Option<MemoryDocument>, MemoryStorageError> {
        let Some(expected) = &self.intent.after_note else {
            return Ok(None);
        };
        let bytes = desired_bytes(
            &self.dir,
            "note.after",
            &self.scope.store.dir,
            &format!("{}.md", self.intent.name),
            NOTE_LIMIT,
            expected,
        )?;
        let text = std::str::from_utf8(&bytes).map_err(|_| MemoryStorageError::RecoveryRequired)?;
        let document = MemoryDocument::for_write(text)?;
        if document.metadata.name != self.intent.name {
            return Err(MemoryStorageError::RecoveryRequired);
        }
        Ok(Some(document))
    }
    fn verify_desired(&self) -> Result<(), MemoryStorageError> {
        let desired = self.desired_note()?;
        let index = self
            .scope
            .desired_index(&self.intent.name, desired.as_ref().map(|doc| &doc.metadata))?;
        let bytes = desired_bytes(
            &self.dir,
            "index.after",
            &self.scope.store.dir,
            "MEMORY.md",
            INDEX_LIMIT,
            &self.intent.after_index,
        )?;
        if bytes != index.as_bytes() {
            return Err(MemoryStorageError::Conflict);
        }
        Ok(())
    }
    fn verify_slots(&self) -> Result<(), MemoryStorageError> {
        preflight_target(
            &self.scope.store.dir,
            &self.dir,
            &format!("{}.md", self.intent.name),
            "note",
            self.intent.before_note.as_deref(),
            self.intent.after_note.as_deref(),
            NOTE_LIMIT,
        )?;
        preflight_target(
            &self.scope.store.dir,
            &self.dir,
            "MEMORY.md",
            "index",
            self.intent.before_index.as_deref(),
            Some(&self.intent.after_index),
            INDEX_LIMIT,
        )
    }
    fn apply_note(&mut self) -> Result<(), MemoryStorageError> {
        apply_target(
            &self.scope.store.dir,
            &self.dir,
            &format!("{}.md", self.intent.name),
            "note",
            self.intent.before_note.as_deref(),
            self.intent.after_note.as_deref(),
            NOTE_LIMIT,
        )
    }
    fn apply_index(&mut self) -> Result<(), MemoryStorageError> {
        apply_target(
            &self.scope.store.dir,
            &self.dir,
            "MEMORY.md",
            "index",
            self.intent.before_index.as_deref(),
            Some(&self.intent.after_index),
            INDEX_LIMIT,
        )
    }
    fn verify_terminal(&self) -> Result<(), MemoryStorageError> {
        self.verify_catalog()?;
        verify_hash(
            &self.scope.store.dir,
            &format!("{}.md", self.intent.name),
            NOTE_LIMIT,
            self.intent.after_note.as_deref(),
        )?;
        verify_hash(
            &self.scope.store.dir,
            "MEMORY.md",
            INDEX_LIMIT,
            Some(&self.intent.after_index),
        )?;
        verify_backup(
            &self.dir,
            "note.before",
            NOTE_LIMIT,
            self.intent.before_note.as_deref(),
        )?;
        verify_backup(
            &self.dir,
            "index.before",
            INDEX_LIMIT,
            self.intent.before_index.as_deref(),
        )
    }
}

fn mutation_platform() -> Result<(), MemoryStorageError> {
    if cfg!(unix) {
        Ok(())
    } else {
        Err(MemoryStorageError::Unsafe(
            "memory mutations require platform privacy and durability support",
        ))
    }
}
#[cfg(not(windows))]
fn create_journal_directory(root: &Dir) -> Result<Dir, MemoryStorageError> {
    root.create_dir_with(TRANSACTION, &private_builder())?;
    Ok(root.open_dir_nofollow(TRANSACTION)?)
}
#[cfg(not(windows))]
fn archive_journal(root: &Dir, _: Dir, history: &Dir, id: &str) -> Result<(), MemoryStorageError> {
    root.rename(TRANSACTION, history, id)?;
    Ok(())
}
fn existing_journal_directory(root: &Dir) -> Result<Option<Dir>, MemoryStorageError> {
    #[cfg(windows)]
    {
        existing_private_directory(root, TRANSACTION)
    }
    #[cfg(not(windows))]
    {
        match root.open_dir_nofollow(TRANSACTION) {
            Ok(dir) => Ok(Some(dir)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}
fn validate_intent(intent: &Intent) -> Result<(), MemoryStorageError> {
    validate_name(&intent.name)?;
    let id = intent
        .id
        .strip_prefix("mem_")
        .ok_or(MemoryStorageError::RecoveryRequired)?;
    if intent.version != 1
        || id.len() != 26
        || !id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(MemoryStorageError::RecoveryRequired);
    }
    for name in intent.catalog.keys() {
        validate_name(name)?;
    }
    for digest in intent
        .catalog
        .values()
        .map(String::as_str)
        .chain(intent.before_note.as_deref())
        .chain(intent.after_note.as_deref())
        .chain(intent.before_index.as_deref())
        .chain(std::iter::once(intent.after_index.as_str()))
    {
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        {
            return Err(MemoryStorageError::RecoveryRequired);
        }
    }
    Ok(())
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn optional_bytes(
    dir: &Dir,
    name: &str,
    limit: u64,
) -> Result<Option<Vec<u8>>, MemoryStorageError> {
    let Some(file) = optional_file(dir, name)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(MemoryStorageError::TooLarge);
    }
    Ok(Some(bytes))
}
fn optional_file(dir: &Dir, name: &str) -> Result<Option<File>, MemoryStorageError> {
    match dir.symlink_metadata(name) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(MemoryStorageError::Unsafe(
                "expected a regular transaction file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    #[cfg(windows)]
    let file = super::super::windows::open_private_file(
        &dir.try_clone()?.into_std_file(),
        name,
        super::super::windows::Access::Read,
    )?;
    #[cfg(not(windows))]
    let file = {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        dir.open_with(name, &options)?.into_std()
    };
    verify_regular(&file)?;
    verify_private_file(&file)?;
    Ok(Some(file))
}
fn desired_bytes(
    stage: &Dir,
    staged: &str,
    root: &Dir,
    destination: &str,
    limit: u64,
    expected: &str,
) -> Result<Vec<u8>, MemoryStorageError> {
    let bytes = optional_bytes(stage, staged, limit)?
        .or(optional_bytes(root, destination, limit)?)
        .ok_or(MemoryStorageError::RecoveryRequired)?;
    if hash(&bytes) != expected {
        return Err(MemoryStorageError::Conflict);
    }
    Ok(bytes)
}
fn verify_hash(
    dir: &Dir,
    name: &str,
    limit: u64,
    expected: Option<&str>,
) -> Result<(), MemoryStorageError> {
    if optional_bytes(dir, name, limit)?
        .as_deref()
        .map(hash)
        .as_deref()
        != expected
    {
        return Err(MemoryStorageError::Conflict);
    }
    Ok(())
}
fn verify_backup(
    dir: &Dir,
    name: &str,
    limit: u64,
    expected: Option<&str>,
) -> Result<(), MemoryStorageError> {
    verify_hash(dir, name, limit, expected)
}

#[allow(clippy::too_many_arguments)]
fn preflight_target(
    root: &Dir,
    stage: &Dir,
    name: &str,
    prefix: &str,
    before: Option<&str>,
    after: Option<&str>,
    limit: u64,
) -> Result<(), MemoryStorageError> {
    let archived = optional_bytes(stage, &format!("{prefix}.before"), limit)?;
    if archived.as_deref().map(hash).as_deref() != before && archived.is_some() {
        return Err(MemoryStorageError::Conflict);
    }
    let current = optional_bytes(root, name, limit)?.as_deref().map(hash);
    let installed = current.as_deref() == after && (before.is_none() || archived.is_some());
    let original = archived.is_none() && current.as_deref() == before;
    let captured = archived.is_some() && current.is_none();
    if installed || original || captured {
        Ok(())
    } else {
        Err(MemoryStorageError::Conflict)
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(not(windows))]
fn apply_target(
    root: &Dir,
    stage: &Dir,
    name: &str,
    prefix: &str,
    before: Option<&str>,
    after: Option<&str>,
    limit: u64,
) -> Result<(), MemoryStorageError> {
    let backup = format!("{prefix}.before");
    let staged = format!("{prefix}.after");
    let archived = optional_bytes(stage, &backup, limit)?;
    if let Some(bytes) = &archived
        && Some(hash(bytes).as_str()) != before
    {
        return Err(MemoryStorageError::Conflict);
    }
    let current = optional_bytes(root, name, limit)?;
    let current_hash = current.as_deref().map(hash);
    if current_hash.as_deref() == after && (before.is_none() || archived.is_some()) {
        return Ok(());
    }
    if archived.is_none() && before.is_some() {
        if current_hash.as_deref() != before {
            return Err(MemoryStorageError::Conflict);
        }
        root.rename(name, stage, &backup)?;
        sync_dir(root)?;
        sync_dir(stage)?;
        verify_backup(stage, &backup, limit, before)?;
    } else if current.is_some() {
        return Err(MemoryStorageError::Conflict);
    }
    if let Some(expected) = after {
        verify_hash(stage, &staged, limit, Some(expected))?;
        match stage.hard_link(&staged, root, name) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                return Err(MemoryStorageError::Conflict);
            }
            Err(error) => return Err(error.into()),
        }
        sync_dir(root)?;
        normalize_pair(stage, &staged, root, name, after)?;
    }
    verify_hash(root, name, limit, after)
}

#[cfg(not(windows))]
fn normalize_pair(
    stage: &Dir,
    staged: &str,
    root: &Dir,
    name: &str,
    expected: Option<&str>,
) -> Result<(), MemoryStorageError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let open = |dir: &Dir, path: &str| -> Result<Option<cap_std::fs::File>, MemoryStorageError> {
        match dir.symlink_metadata(path) {
            Ok(meta) if !meta.is_file() => return Err(MemoryStorageError::Conflict),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        Ok(Some(dir.open_with(path, &options)?))
    };
    let (Some(source), Some(destination)) = (open(stage, staged)?, open(root, name)?) else {
        return Ok(());
    };
    let a = source.metadata()?;
    let b = destination.metadata()?;
    if a.dev() != b.dev() || a.ino() != b.ino() {
        return Ok(());
    }
    if a.nlink() != 2 || b.nlink() != 2 {
        return Err(MemoryStorageError::Conflict);
    }
    let mut bytes = Vec::new();
    source.take(INDEX_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > INDEX_LIMIT || hash(&bytes) != expected {
        return Err(MemoryStorageError::Conflict);
    }
    stage.remove_file(staged)?;
    sync_dir(stage)?;
    sync_dir(root)
}
#[cfg(not(windows))]
fn create_file(dir: &Dir, name: &str, bytes: &[u8]) -> Result<(), MemoryStorageError> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = dir.open_with(name, &options)?.into_std();
    verify_regular(&file)?;
    make_private_file(&file)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn sync_dir(dir: &Dir) -> Result<(), MemoryStorageError> {
    #[cfg(unix)]
    directory_file(dir)?.sync_all()?;
    #[cfg(windows)]
    super::super::windows::sync_private(&dir.try_clone()?.into_std_file())?;
    #[cfg(not(any(unix, windows)))]
    let _ = dir;
    Ok(())
}
fn reserve_entries(
    dir: &Dir,
    note_missing: bool,
    index_missing: bool,
) -> Result<(), MemoryStorageError> {
    verify_history(dir)?;
    let count = dir
        .entries()?
        .take(ENTRY_LIMIT + 1)
        .collect::<io::Result<Vec<_>>>()?
        .len();
    let history_missing = match dir.symlink_metadata(HISTORY) {
        Ok(_) => false,
        Err(e) if e.kind() == io::ErrorKind::NotFound => true,
        Err(e) => return Err(e.into()),
    };
    if count
        + usize::from(note_missing)
        + usize::from(index_missing)
        + usize::from(history_missing)
        + 1
        > ENTRY_LIMIT
    {
        return Err(MemoryStorageError::Unsafe(
            "memory directory contains too many entries",
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    pub(super) fn note(body: &str) -> String {
        format!("---\nname: rule\ndescription: useful fact\ntype: user\n---\n{body}\n")
    }

    #[test]
    fn recovery_after_note_install_finishes_index_without_replaying_note_effects() {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        scope.write(&note("Before")).unwrap();
        let mut pending = scope.prepare_write(&note("After")).unwrap();
        pending.apply_note().unwrap();
        drop(pending);
        assert!(scope.recover().unwrap().is_some());
        assert_eq!(scope.read("rule").unwrap().body, "After");
        assert_eq!(
            scope.index().unwrap().text,
            "- [rule](rule.md) — useful fact\n"
        );
    }
    #[test]
    fn recovery_normalizes_only_the_owned_two_link_staging_pair() {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        let pending = scope.prepare_write(&note("After")).unwrap();
        pending
            .dir
            .hard_link("note.after", &pending.scope.store.dir, "rule.md")
            .unwrap();
        drop(pending);
        scope.recover().unwrap();
        assert_eq!(scope.read("rule").unwrap().body, "After");
    }
    #[test]
    fn new_file_created_after_original_archive_is_not_overwritten() {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        scope.write(&note("Before")).unwrap();
        let pending = scope.prepare_write(&note("Desired")).unwrap();
        pending
            .scope
            .store
            .dir
            .rename("rule.md", &pending.dir, "note.before")
            .unwrap();
        create_file(
            &pending.scope.store.dir,
            "rule.md",
            note("User edit").as_bytes(),
        )
        .unwrap();
        drop(pending);
        assert!(matches!(scope.recover(), Err(MemoryStorageError::Conflict)));
        assert!(
            std::fs::read_to_string(store.path().join("rule.md"))
                .unwrap()
                .contains("User edit")
        );
        assert!(
            std::fs::read_to_string(store.path().join(TRANSACTION).join("note.before"))
                .unwrap()
                .contains("Before")
        );
    }
    #[test]
    fn foreign_third_link_is_not_normalized_or_deleted() {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        let pending = scope.prepare_write(&note("After")).unwrap();
        pending
            .dir
            .hard_link("note.after", &pending.scope.store.dir, "rule.md")
            .unwrap();
        let outside = Dir::open_ambient_dir(data.path(), cap_std::ambient_authority()).unwrap();
        pending
            .dir
            .hard_link("note.after", &outside, "foreign")
            .unwrap();
        drop(pending);
        assert!(matches!(scope.recover(), Err(MemoryStorageError::Conflict)));
        assert!(store.path().join(TRANSACTION).join("note.after").exists());
        assert!(data.path().join("foreign").exists());
    }
    #[test]
    fn edited_archived_original_remains_named_and_blocks_completion() {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        scope.write(&note("Before")).unwrap();
        let mut pending = scope.prepare_write(&note("Desired")).unwrap();
        pending.apply_note().unwrap();
        std::fs::write(
            store.path().join(TRANSACTION).join("note.before"),
            note("Late user edit"),
        )
        .unwrap();
        drop(pending);
        assert!(matches!(scope.recover(), Err(MemoryStorageError::Conflict)));
        assert!(
            std::fs::read_to_string(store.path().join(TRANSACTION).join("note.before"))
                .unwrap()
                .contains("Late user edit")
        );
    }
    #[test]
    fn already_installed_note_and_index_keep_their_inode_identity_on_recovery() {
        let data = tempfile::tempdir().unwrap();
        let store = MemoryStore::open(data.path(), "global").unwrap();
        let mut scope = store.claim().unwrap();
        scope.write(&note("Before")).unwrap();
        let mut pending = scope.prepare_write(&note("After")).unwrap();
        pending.apply_note().unwrap();
        pending.apply_index().unwrap();
        let metadata = pending.scope.store.dir.metadata("rule.md").unwrap();
        let identity = (metadata.dev(), metadata.ino());
        drop(pending);
        scope.recover().unwrap();
        let metadata = store.dir.metadata("rule.md").unwrap();
        assert_eq!((metadata.dev(), metadata.ino()), identity);
    }
}

fn verify_history(root: &Dir) -> Result<(), MemoryStorageError> {
    #[cfg(windows)]
    {
        if let Some(dir) = existing_private_directory(root, HISTORY)? {
            verify_private_directory(&dir)?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    match root.open_dir_nofollow(HISTORY) {
        Ok(dir) => verify_private_directory(&dir),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryEditReview {
    pub name: String,
    pub original: Option<String>,
    pub fingerprint: String,
}

impl<'store> MemoryScope<'store> {
    /// Read-only snapshot; the fingerprint grants no mutation or database authority.
    pub fn inspect_edit(&self, name: &str) -> Result<MemoryEditReview, MemoryStorageError> {
        validate_name(name)?;
        self.ready()?;
        verify_private_directory(&self.store.dir)?;
        let reviewed_note =
            recovery::reviewed_file(&self.store.dir, &format!("{name}.md"), NOTE_LIMIT)?;
        let original = optional_bytes(&self.store.dir, &format!("{name}.md"), NOTE_LIMIT)?
            .map(String::from_utf8)
            .transpose()
            .map_err(|_| MemoryStorageError::Unsafe("invalid UTF-8 memory note"))?;
        if original.as_deref().map(|text| hash(text.as_bytes()))
            != reviewed_note.as_ref().map(|file| file.digest.clone())
        {
            return Err(MemoryStorageError::ReviewConflict);
        }
        let snapshot = serde_json::to_vec(&(
            "memory-edit-review-v1",
            &self.store.path,
            recovery::directory_identity(&self.store.dir)?,
            name,
            reviewed_note,
            recovery::reviewed_file(&self.store.dir, "MEMORY.md", INDEX_LIMIT)?,
            self.fingerprints(name)?,
        ))
        .map_err(|_| MemoryStorageError::ReviewConflict)?;
        Ok(MemoryEditReview {
            name: name.into(),
            original,
            fingerprint: hash(&snapshot),
        })
    }
    pub fn verify_edit_review(
        &self,
        name: &str,
        fingerprint: &str,
    ) -> Result<(), MemoryStorageError> {
        if self.inspect_edit(name)?.fingerprint != fingerprint {
            return Err(MemoryStorageError::ReviewConflict);
        }
        Ok(())
    }
    pub fn prepare_write_reviewed<'guard>(
        &'guard mut self,
        text: &str,
        fingerprint: &str,
    ) -> Result<PreparedMemory<'guard, 'store>, MemoryStorageError> {
        mutation_platform()?;
        if text.len() as u64 > NOTE_LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        let document = MemoryDocument::for_write(text)?;
        self.verify_edit_review(&document.metadata.name, fingerprint)?;
        let rendered = document.render_for_write()?;
        if rendered.len() as u64 > NOTE_LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        self.prepare(
            &document.metadata.name.clone(),
            Some((document, rendered)),
            None,
            Some(fingerprint),
        )
    }
    pub fn prepare_delete_reviewed<'guard>(
        &'guard mut self,
        name: &str,
        fingerprint: &str,
    ) -> Result<PreparedMemory<'guard, 'store>, MemoryStorageError> {
        mutation_platform()?;
        self.verify_edit_review(name, fingerprint)?;
        self.read_file(&format!("{name}.md"), NOTE_LIMIT)?;
        self.prepare(name, None, None, Some(fingerprint))
    }
}

struct EditBefore {
    note: Option<String>,
    index: Option<String>,
}

/// The scope claim remains owned while an interactive editor operates on a separate draft.
pub struct ReviewedMemory<'guard, 'store> {
    scope: &'guard mut MemoryScope<'store>,
    name: String,
    original: Option<String>,
    before: EditBefore,
}

impl<'store> MemoryScope<'store> {
    pub fn review_edit<'guard>(
        &'guard mut self,
        name: &str,
    ) -> Result<ReviewedMemory<'guard, 'store>, MemoryStorageError> {
        mutation_platform()?;
        validate_name(name)?;
        self.ready()?;
        let note = optional_bytes(&self.store.dir, &format!("{name}.md"), NOTE_LIMIT)?;
        let index = optional_bytes(&self.store.dir, "MEMORY.md", INDEX_LIMIT)?;
        let before = EditBefore {
            note: note.as_deref().map(hash),
            index: index.as_deref().map(hash),
        };
        let original = note
            .map(String::from_utf8)
            .transpose()
            .map_err(|_| MemoryStorageError::Unsafe("invalid UTF-8 memory note"))?;
        Ok(ReviewedMemory {
            scope: self,
            name: name.into(),
            original,
            before,
        })
    }
}

impl ReviewedMemory<'_, '_> {
    pub fn original(&self) -> Option<&str> {
        self.original.as_deref()
    }

    pub fn commit(self, text: &str) -> Result<MemoryMutation, MemoryStorageError> {
        if text.len() as u64 > NOTE_LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        let document = MemoryDocument::for_write(text)?;
        if document.metadata.name != self.name {
            return Err(MemoryStorageError::Unsafe(
                "edited memory name does not match requested note",
            ));
        }
        let rendered = document.render_for_write()?;
        if rendered.len() as u64 > NOTE_LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        self.scope
            .prepare(
                &self.name,
                Some((document, rendered)),
                Some(&self.before),
                None,
            )?
            .commit()
    }
}
