//! Native journal operations; public mutation admission still requires lifecycle acceptance.
use super::*;
use crate::memory::windows as native;
#[path = "windows/objects.rs"]
mod objects;
pub(super) use objects::{Objects, persist_intent, target_ids, validate_shape, verify_objects};

pub(super) fn create_journal_directory(root: &Dir) -> Result<Dir, MemoryStorageError> {
    let parent = root.try_clone()?.into_std_file();
    native::verify_private(&parent)?;
    let created = native::create_private_directory(&parent, TRANSACTION)?;
    let expected = native::identity(&created)?;
    drop(created);
    let pinned = native::open_pinned_private_directory(&parent, TRANSACTION, true)?;
    if native::identity(&pinned)? != expected {
        return Err(MemoryStorageError::ReviewConflict);
    }
    Ok(Dir::from_std_file(pinned))
}

pub(super) fn create_file(dir: &Dir, name: &str, bytes: &[u8]) -> Result<(), MemoryStorageError> {
    let mut file = native::create_private_file(&dir.try_clone()?.into_std_file(), name)?;
    file.write_all(bytes)?;
    native::sync_private(&file)
}

pub(super) fn archive_journal(
    root: &Dir,
    journal: Dir,
    history: &Dir,
    id: &str,
) -> Result<(), MemoryStorageError> {
    let expected = native::identity(&journal.try_clone()?.into_std_file())?;
    // The exclusive guard cannot coexist with this journal's writable handle.
    drop(journal);
    let source = native::retain_private_directory(
        &root.try_clone()?.into_std_file(),
        TRANSACTION,
        expected,
    )?;
    source.rename_to_durable(&history.try_clone()?.into_std_file(), id)
}

fn move_file(
    source: &Dir,
    name: &str,
    destination: &Dir,
    target: &str,
    limit: u64,
    expected: &str,
    expected_identity: native::FileIdentity,
) -> Result<(), MemoryStorageError> {
    let file = optional_file(source, name)?.ok_or(MemoryStorageError::Conflict)?;
    let identity = native::identity(&file)?;
    if identity != expected_identity {
        return Err(MemoryStorageError::Conflict);
    }
    // Release the shared write-capable opening before acquiring the exclusive source.
    drop(file);
    let retained = native::retain_private_file(
        &source.try_clone()?.into_std_file(),
        name,
        expected_identity,
    )?;
    let mut bytes = Vec::new();
    retained.file().take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit || hash(&bytes) != expected {
        return Err(MemoryStorageError::Conflict);
    }
    retained.rename_to_durable(&destination.try_clone()?.into_std_file(), target)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_target(
    root: &Dir,
    stage: &Dir,
    name: &str,
    prefix: &str,
    before: Option<&str>,
    after: Option<&str>,
    limit: u64,
    identities: (Option<native::FileIdentity>, Option<native::FileIdentity>),
) -> Result<(), MemoryStorageError> {
    objects::verify_slots(root, stage, name, prefix, identities)?;
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
    if archived.is_none()
        && let Some(before) = before
    {
        if current_hash.as_deref() != Some(before) {
            return Err(MemoryStorageError::Conflict);
        }
        move_file(
            root,
            name,
            stage,
            &backup,
            limit,
            before,
            identities.0.ok_or(MemoryStorageError::RecoveryRequired)?,
        )?;
        verify_backup(stage, &backup, limit, Some(before))?;
    } else if current.is_some() {
        return Err(MemoryStorageError::Conflict);
    }
    if let Some(expected) = after {
        move_file(
            stage,
            &staged,
            root,
            name,
            limit,
            expected,
            identities.1.ok_or(MemoryStorageError::RecoveryRequired)?,
        )?;
    }
    verify_hash(root, name, limit, after)
}

pub(super) fn normalize_pair(
    stage: &Dir,
    staged: &str,
    root: &Dir,
    name: &str,
    _: Option<&str>,
) -> Result<(), MemoryStorageError> {
    // Native rename creates no staging hard-link pair. Reject aliases before effects;
    // never unlink a user-created alias as Unix interrupted-link normalization would.
    optional_file(stage, staged)?;
    optional_file(root, name)?;
    Ok(())
}

#[cfg(test)]
#[path = "windows/tests.rs"]
mod tests;
