//! Persisted Windows journal object identities, including partial installation slots.
use super::*;
type Id = native::FileIdentity;
pub(crate) struct FileProof {
    pub(crate) name: String,
    pub(crate) identity: Id,
    pub(crate) digest: String,
    pub(crate) limit: u64,
    pub(crate) journal: bool,
}
pub(crate) fn terminal_files(intent: &Intent) -> Result<Vec<FileProof>, MemoryStorageError> {
    let objects = plan(intent)?;
    let mut files = Vec::new();
    for (name, id, digest, limit, journal) in [
        (
            format!("{}.md", intent.name),
            objects.after_note,
            intent.after_note.as_deref(),
            NOTE_LIMIT,
            false,
        ),
        (
            "MEMORY.md".into(),
            objects.after_index,
            Some(intent.after_index.as_str()),
            INDEX_LIMIT,
            false,
        ),
        (
            "note.before".into(),
            objects.before_note,
            intent.before_note.as_deref(),
            NOTE_LIMIT,
            true,
        ),
        (
            "index.before".into(),
            objects.before_index,
            intent.before_index.as_deref(),
            INDEX_LIMIT,
            true,
        ),
    ] {
        if let Some(identity) = id {
            files.push(FileProof {
                name,
                identity,
                digest: digest.ok_or(MemoryStorageError::RecoveryRequired)?.into(),
                limit,
                journal,
            });
        }
    }
    for (name, identity) in &objects.catalog {
        files.push(FileProof {
            name: format!("{name}.md"),
            identity: *identity,
            digest: intent
                .catalog
                .get(name)
                .ok_or(MemoryStorageError::RecoveryRequired)?
                .clone(),
            limit: NOTE_LIMIT,
            journal: false,
        });
    }
    Ok(files)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Objects {
    data: Id,
    root: Id,
    scope: Id,
    journal: Option<Id>,
    intent: Option<Id>,
    before_note: Option<Id>,
    before_index: Option<Id>,
    after_note: Option<Id>,
    after_index: Option<Id>,
    catalog: BTreeMap<String, Id>,
}
impl Objects {
    pub(crate) fn capture_original(
        store: &MemoryStore,
        name: &str,
        note: Option<&str>,
        index: Option<&str>,
        catalog: &BTreeMap<String, String>,
    ) -> Result<Self, MemoryStorageError> {
        let (data, root) = store.binding.identities()?;
        Ok(Self {
            data,
            root,
            scope: directory_id(&store.dir)?,
            journal: None,
            intent: None,
            before_note: checked_file(&store.dir, &format!("{name}.md"), note, NOTE_LIMIT)?,
            before_index: checked_file(&store.dir, "MEMORY.md", index, INDEX_LIMIT)?,
            after_note: None,
            after_index: None,
            catalog: catalog
                .iter()
                .map(|(name, digest)| {
                    Ok((
                        name.clone(),
                        checked_file(&store.dir, &format!("{name}.md"), Some(digest), NOTE_LIMIT)?
                            .ok_or(MemoryStorageError::Conflict)?,
                    ))
                })
                .collect::<Result<_, MemoryStorageError>>()?,
        })
    }
}
fn directory_id(dir: &Dir) -> Result<Id, MemoryStorageError> {
    native::identity(&dir.try_clone()?.into_std_file())
}
fn file_id(dir: &Dir, name: &str) -> Result<Option<Id>, MemoryStorageError> {
    optional_file(dir, name)?
        .map(|file| native::identity(&file))
        .transpose()
}
fn checked_file(
    dir: &Dir,
    name: &str,
    expected: Option<&str>,
    limit: u64,
) -> Result<Option<Id>, MemoryStorageError> {
    let Some(mut file) = optional_file(dir, name)? else {
        return if expected.is_none() {
            Ok(None)
        } else {
            Err(MemoryStorageError::Conflict)
        };
    };
    let id = native::identity(&file)?;
    let mut bytes = Vec::new();
    (&mut file).take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit || Some(hash(&bytes).as_str()) != expected {
        return Err(MemoryStorageError::Conflict);
    }
    if file_id(dir, name)? != Some(id) {
        return Err(MemoryStorageError::Conflict);
    }
    Ok(Some(id))
}
pub(crate) fn persist_intent(dir: &Dir, mut intent: Intent) -> Result<Intent, MemoryStorageError> {
    let mut file = native::create_private_file(&dir.try_clone()?.into_std_file(), "intent.json")?;
    let objects = intent
        .objects
        .as_mut()
        .ok_or(MemoryStorageError::RecoveryRequired)?;
    objects.journal = Some(directory_id(dir)?);
    objects.intent = Some(native::identity(&file)?);
    objects.after_note = checked_file(dir, "note.after", intent.after_note.as_deref(), NOTE_LIMIT)?;
    objects.after_index = checked_file(dir, "index.after", Some(&intent.after_index), INDEX_LIMIT)?;
    validate_shape(&intent)?;
    let json = serde_json::to_vec(&intent).map_err(|_| MemoryStorageError::RecoveryRequired)?;
    if json.len() as u64 > INTENT_LIMIT {
        return Err(MemoryStorageError::TooLarge);
    }
    file.write_all(&json)?;
    native::sync_private(&file)?;
    Ok(intent)
}
fn plan(intent: &Intent) -> Result<&Objects, MemoryStorageError> {
    intent
        .objects
        .as_ref()
        .ok_or(MemoryStorageError::RecoveryRequired)
}
pub(crate) fn validate_shape(intent: &Intent) -> Result<(), MemoryStorageError> {
    let objects = plan(intent)?;
    if objects.journal.is_none()
        || objects.intent.is_none()
        || objects.after_index.is_none()
        || objects.before_note.is_some() != intent.before_note.is_some()
        || objects.before_index.is_some() != intent.before_index.is_some()
        || objects.after_note.is_some() != intent.after_note.is_some()
        || objects.catalog.keys().ne(intent.catalog.keys())
    {
        return Err(MemoryStorageError::RecoveryRequired);
    }
    Ok(())
}
pub(crate) fn target_ids(
    intent: &Intent,
    prefix: &str,
) -> Result<(Option<Id>, Option<Id>), MemoryStorageError> {
    let objects = plan(intent)?;
    match prefix {
        "note" => Ok((objects.before_note, objects.after_note)),
        "index" => Ok((objects.before_index, objects.after_index)),
        _ => Err(MemoryStorageError::RecoveryRequired),
    }
}
pub(crate) fn verify_objects(pending: &PreparedMemory<'_, '_>) -> Result<(), MemoryStorageError> {
    let objects = plan(&pending.intent)?;
    let store = pending.scope.store;
    let (data, root) = store.binding.identities()?;
    if data != objects.data
        || root != objects.root
        || directory_id(&store.dir)? != objects.scope
        || Some(directory_id(&pending.dir)?) != objects.journal
        || Some(native::identity(intent_descriptor(&pending.intent_file))?) != objects.intent
    {
        return Err(MemoryStorageError::Conflict);
    }
    for (name, expected) in &objects.catalog {
        if file_id(&store.dir, &format!("{name}.md"))? != Some(*expected) {
            return Err(MemoryStorageError::Conflict);
        }
    }
    verify_slots(
        &store.dir,
        &pending.dir,
        &format!("{}.md", pending.intent.name),
        "note",
        target_ids(&pending.intent, "note")?,
    )?;
    verify_slots(
        &store.dir,
        &pending.dir,
        "MEMORY.md",
        "index",
        target_ids(&pending.intent, "index")?,
    )
}
pub(crate) fn verify_slots(
    root: &Dir,
    stage: &Dir,
    name: &str,
    prefix: &str,
    expected: (Option<Id>, Option<Id>),
) -> Result<(), MemoryStorageError> {
    let (before, after) = expected;
    let current = file_id(root, name)?;
    let backup = file_id(stage, &format!("{prefix}.before"))?;
    let staged = file_id(stage, &format!("{prefix}.after"))?;
    let original = current == before && backup.is_none();
    let captured = current.is_none() && backup == before;
    let installed = current == after && staged.is_none() && backup == before;
    if backup.is_some() && backup != before
        || staged.is_some() && staged != after
        || before.is_some() && current != before && backup != before
        || after.is_some() && current != after && staged != after
        || !(original || captured || installed)
    {
        return Err(MemoryStorageError::Conflict);
    }
    Ok(())
}
