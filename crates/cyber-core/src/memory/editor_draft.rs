//! Private editor results are retained evidence, separate from note mutation authority.
use super::{MemoryError, MemoryStorageError, windows as native};
use std::os::windows::fs::OpenOptionsExt;
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

const LIMIT: usize = 1_048_576;

pub struct MemoryEditorDraft {
    base: File,
    base_path: PathBuf,
    base_id: native::FileIdentity,
    directory: File,
    directory_id: native::FileIdentity,
    name: String,
    path: PathBuf,
}

fn open_base(path: &Path) -> Result<File, MemoryStorageError> {
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    native::identity(&file)?;
    if !file.metadata()?.is_dir() {
        return Err(MemoryStorageError::Unsafe(
            "Editor data namespace must be a directory",
        ));
    }
    Ok(file)
}

impl MemoryEditorDraft {
    /// The caller owns the data namespace. Draft bytes confer no memory write authority.
    pub fn new(data: &Path, text: &str) -> Result<Self, MemoryStorageError> {
        if text.len() > LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        if !data.is_absolute() {
            return Err(MemoryStorageError::Unsafe(
                "Editor data namespace must be absolute",
            ));
        }
        let base = open_base(data)?;
        let base_id = native::identity(&base)?;
        let name = format!(".memory-edit-{}", ulid::Ulid::new());
        let created = native::create_private_directory(&base, &name)?;
        let directory_id = native::verify_private(&created)?;
        let mut note = native::create_private_file(&created, "note.md")?;
        note.write_all(text.as_bytes())?;
        native::sync_private(&note)?;
        drop(note);
        native::sync_private(&created)?;
        native::sync_namespace_directory(&base)?;
        // The creation handle has delete access, which conflicts with the no-delete pin.
        drop(created);
        let directory = native::open_pinned_private_directory(&base, &name, false)?;
        if native::verify_private(&directory)? != directory_id {
            return Err(MemoryStorageError::ReviewConflict);
        }
        let draft = Self {
            base,
            base_path: data.to_owned(),
            base_id,
            directory,
            directory_id,
            path: data.join(&name).join("note.md"),
            name,
        };
        draft.verify_directory()?;
        Ok(draft)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn verify_directory(&self) -> Result<(), MemoryStorageError> {
        if native::identity(&self.base)? != self.base_id
            || native::identity(&open_base(&self.base_path)?)? != self.base_id
            || native::verify_private(&self.directory)? != self.directory_id
            || native::verify_private(&native::open_private_directory(
                &self.base,
                &self.name,
                native::Access::Read,
            )?)? != self.directory_id
        {
            return Err(MemoryStorageError::ReviewConflict);
        }
        Ok(())
    }

    /// Accepts a private editor replacement, then freezes that exact file for snapshot reading.
    pub fn read(&self) -> Result<String, MemoryStorageError> {
        self.verify_directory()?;
        let observed = native::open_private_file(&self.directory, "note.md", native::Access::Read)?;
        let expected = native::verify_private(&observed)?;
        drop(observed);
        let frozen = native::freeze_private_file(&self.directory, "note.md", expected)?;
        let mut bytes = Vec::new();
        (&frozen).take((LIMIT + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > LIMIT {
            return Err(MemoryStorageError::TooLarge);
        }
        self.verify_directory()?;
        let named = native::open_private_file(&self.directory, "note.md", native::Access::Read)?;
        if native::verify_private(&frozen)? != expected
            || native::verify_private(&named)? != expected
        {
            return Err(MemoryStorageError::ReviewConflict);
        }
        String::from_utf8(bytes)
            .map_err(|_| MemoryError::Invalid("Editor draft must be UTF-8").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_draft_private_creation_inherited_container_and_changed_content() {
        let data = tempfile::tempdir().unwrap();
        let draft = MemoryEditorDraft::new(data.path(), "Original").unwrap();
        assert!(native::verify_private(&draft.base).is_err());
        assert_eq!(
            native::verify_private(&draft.directory).unwrap(),
            draft.directory_id
        );
        assert_eq!(draft.read().unwrap(), "Original");
        std::fs::write(draft.path(), "Edited").unwrap();
        assert_eq!(draft.read().unwrap(), "Edited");
        assert!(!data.path().join("memory").exists());
        let path = draft.path().to_owned();
        drop(draft);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "Edited");
    }

    #[test]
    fn native_draft_creator_handoff_allows_compatible_directory_reopening() {
        let data = tempfile::tempdir().unwrap();
        let draft = MemoryEditorDraft::new(data.path(), "Original").unwrap();
        let compatible =
            native::open_private_directory(&draft.base, &draft.name, native::Access::DataWrite)
                .unwrap();
        assert_eq!(
            native::verify_private(&compatible).unwrap(),
            draft.directory_id
        );
        std::fs::write(draft.path(), "Changed!").unwrap();
        assert_eq!(draft.read().unwrap(), "Changed!");
        drop(draft);
        native::verify_private(&compatible).unwrap();
    }

    #[test]
    fn native_draft_pins_directory_until_owner_release() {
        let data = tempfile::tempdir().unwrap();
        let draft = MemoryEditorDraft::new(data.path(), "Original").unwrap();
        let directory = draft.path().parent().unwrap().to_owned();
        let moved = data.path().join("moved");
        assert!(std::fs::rename(&directory, &moved).is_err());
        assert_eq!(draft.read().unwrap(), "Original");
        drop(draft);
        std::fs::rename(directory, &moved).unwrap();
        assert_eq!(
            std::fs::read_to_string(moved.join("note.md")).unwrap(),
            "Original"
        );
    }

    #[test]
    fn native_draft_accepts_private_atomic_editor_replacement() {
        let data = tempfile::tempdir().unwrap();
        let draft = MemoryEditorDraft::new(data.path(), "Original").unwrap();
        let mut replacement = native::create_private_file(&draft.directory, "replacement").unwrap();
        replacement.write_all(b"Replacement").unwrap();
        native::sync_private(&replacement).unwrap();
        drop(replacement);
        let parent = draft.path().parent().unwrap();
        std::fs::rename(draft.path(), parent.join("original")).unwrap();
        std::fs::rename(parent.join("replacement"), draft.path()).unwrap();
        assert_eq!(draft.read().unwrap(), "Replacement");
        assert_eq!(
            std::fs::read_to_string(parent.join("original")).unwrap(),
            "Original"
        );
    }

    #[test]
    fn native_draft_refuses_hard_link_alias_and_retains_both_names() {
        let data = tempfile::tempdir().unwrap();
        let draft = MemoryEditorDraft::new(data.path(), "Original").unwrap();
        let alias = data.path().join("alias");
        std::fs::hard_link(draft.path(), &alias).unwrap();
        assert!(draft.read().is_err());
        assert_eq!(std::fs::read_to_string(&alias).unwrap(), "Original");
        assert_eq!(std::fs::read_to_string(draft.path()).unwrap(), "Original");
        std::fs::remove_file(alias).unwrap();
        assert_eq!(draft.read().unwrap(), "Original");
    }

    #[test]
    fn native_draft_refuses_inherited_replacement_without_privacy_repair() {
        let data = tempfile::tempdir().unwrap();
        let draft = MemoryEditorDraft::new(data.path(), "Original").unwrap();
        let replacement = data.path().join("inherited");
        std::fs::write(&replacement, "User replacement").unwrap();
        let inherited = open_base(data.path()).unwrap();
        let file = std::fs::File::open(&replacement).unwrap();
        assert!(native::verify_private(&file).is_err());
        drop(file);
        drop(inherited);
        std::fs::remove_file(draft.path()).unwrap();
        std::fs::rename(&replacement, draft.path()).unwrap();
        assert!(draft.read().is_err());
        let file = std::fs::File::open(draft.path()).unwrap();
        assert!(native::verify_private(&file).is_err());
        assert_eq!(
            std::fs::read_to_string(draft.path()).unwrap(),
            "User replacement"
        );
    }

    #[test]
    fn native_draft_refuses_live_writer_and_recovers_after_release() {
        let data = tempfile::tempdir().unwrap();
        let draft = MemoryEditorDraft::new(data.path(), "Original").unwrap();
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .open(draft.path())
            .unwrap();
        writer.write_all(b"Changed!").unwrap();
        assert!(draft.read().is_err());
        assert_eq!(std::fs::read_to_string(draft.path()).unwrap(), "Changed!");
        drop(writer);
        assert_eq!(draft.read().unwrap(), "Changed!");
    }

    #[test]
    fn native_draft_refuses_oversized_invalid_utf8_and_initial_size() {
        let data = tempfile::tempdir().unwrap();
        assert!(matches!(
            MemoryEditorDraft::new(data.path(), &"x".repeat(LIMIT + 1)),
            Err(MemoryStorageError::TooLarge)
        ));
        let draft = MemoryEditorDraft::new(data.path(), "Original").unwrap();
        std::fs::write(draft.path(), [0xff]).unwrap();
        assert!(matches!(draft.read(), Err(MemoryStorageError::Format(_))));
        std::fs::write(draft.path(), vec![b'x'; LIMIT + 1]).unwrap();
        assert!(matches!(draft.read(), Err(MemoryStorageError::TooLarge)));
        assert_eq!(
            std::fs::metadata(draft.path()).unwrap().len(),
            (LIMIT + 1) as u64
        );
    }
}
