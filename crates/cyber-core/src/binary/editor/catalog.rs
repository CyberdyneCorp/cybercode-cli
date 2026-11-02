//! Bounded private owner index. Listing never opens source bytes or a draft base,
//! and never needs the active document's exclusive mutation lock.
use super::*;
use crate::binary::storage::{StorageRoot, small_record};
use cap_fs_ext::DirExt;
use schemars::JsonSchema;
use std::path::Component;
const HEADER_LIMIT: u64 = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    owner: Owner,
    relative: PathBuf,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StoredDocument {
    pub document_id: String,
    pub path: String,
    /// Presence of a journal, not a claim of successful native reconciliation.
    pub record_available: bool,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StoredDocuments {
    pub documents: Vec<StoredDocument>,
    pub next_cursor: Option<String>,
}
pub(super) fn publish_header(
    store: &mut Store,
    owner: &Owner,
    document: &Document,
) -> Result<(), Error> {
    let relative = document
        .path()
        .strip_prefix(document.root())
        .map_err(|_| Error::File)?
        .to_owned();
    match store.directory().symlink_metadata("owner.json") {
        Ok(_) => {
            let prior: Header = serde_json::from_slice(&small_record(
                store.directory(),
                "owner.json",
                HEADER_LIMIT,
            )?)
            .map_err(|_| Error::Invalid)?;
            if prior.version != 1 || &prior.owner != owner || prior.relative != relative {
                return Err(Error::Changed);
            }
            // create() already proved there is no document journal: an interrupted
            // preparation has no source effects or staged commands to overwrite.
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(Error::Io),
    }
    let header = Header {
        version: 1,
        owner: owner.clone(),
        relative,
    };
    let bytes = serde_json::to_vec(&header).map_err(|_| Error::Invalid)?;
    if bytes.len() as u64 > HEADER_LIMIT {
        return Err(Error::Limit);
    }
    store.record("owner.json", bytes.len() as u64, |writer| {
        writer.write_all(&bytes).map_err(|_| Error::Io)
    })
}
impl StorageRoot {
    /// Authenticate a small owner header before opening a potentially large draft.
    pub fn resource(&self, owner: &Owner, id: &str) -> Result<super::super::Resource, Error> {
        if !id.starts_with("bin_")
            || id.len() > 100
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(Error::File);
        }
        let root = self.directory()?;
        let dir = root.open_dir_nofollow(id).map_err(|_| Error::File)?;
        crate::binary::storage::private(
            root,
            &dir.try_clone().map_err(|_| Error::Io)?.into_std_file(),
            true,
        )?;
        let header: Header =
            serde_json::from_slice(&small_record(&dir, "owner.json", HEADER_LIMIT)?)
                .map_err(|_| Error::Invalid)?;
        if header.version != 1
            || &header.owner != owner
            || header.relative.as_os_str().is_empty()
            || header
                .relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(Error::File);
        }
        Ok(super::super::Resource(owner.root.join(header.relative)))
    }

    /// Deletion must not orphan dirty drafts or unresolved receipts, including closed
    /// documents and records from prior server processes. Locked/corrupt records fail closed.
    pub fn check_session_deletion(&self, sessions: &[String]) -> Result<(), Error> {
        let root = self.directory()?;
        for (count, entry) in root.entries().map_err(|_| Error::Io)?.enumerate() {
            if count >= 4096 {
                return Err(Error::Limit);
            }
            let name = entry.map_err(|_| Error::Io)?.file_name();
            let id = name.to_str().ok_or(Error::File)?;
            let dir = root.open_dir_nofollow(&name).map_err(|_| Error::File)?;
            if !dir.try_exists("owner.json").map_err(|_| Error::Io)? {
                continue;
            }
            let header: Header =
                serde_json::from_slice(&small_record(&dir, "owner.json", HEADER_LIMIT)?)
                    .map_err(|_| Error::Invalid)?;
            if !sessions.contains(&header.owner.session_id) {
                continue;
            }
            if !dir.try_exists("document.json").map_err(|_| Error::Io)? {
                continue;
            }
            let editor = Editor::resume(self.open_existing(id)?, &header.owner)?;
            if editor.unresolved() || editor.draft().can_undo() {
                return Err(Error::Changed);
            }
        }
        self.directory()?;
        Ok(())
    }
    /// Callers establish fresh owner authentication and Session/Location scope.
    /// Exact scope equality keeps model invocation drafts out of manual recovery.
    pub fn list(
        &self,
        owner: &Owner,
        after: Option<&str>,
        limit: usize,
    ) -> Result<StoredDocuments, Error> {
        if limit == 0
            || limit > 64
            || after.is_some_and(|id| {
                id.len() > 100 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
        {
            return Err(Error::Limit);
        }
        let root = self.directory()?;
        let mut found = vec![];
        for (count, entry) in root.entries().map_err(|_| Error::Io)?.enumerate() {
            if count >= 4096 {
                return Err(Error::Limit);
            }
            let name = entry.map_err(|_| Error::Io)?.file_name();
            let id = name.to_str().ok_or(Error::File)?;
            if !id.starts_with("bin_")
                || id.len() > 100
                || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(Error::File);
            }
            if after.is_some_and(|after| id <= after) {
                continue;
            }
            let dir = root.open_dir_nofollow(&name).map_err(|_| Error::File)?;
            crate::binary::storage::private(
                root,
                &dir.try_clone().map_err(|_| Error::Io)?.into_std_file(),
                true,
            )?;
            // Missing headers represent an allocation cancelled before capturing
            // plaintext. Existing invalid records are retained and fail closed.
            if !dir.try_exists("owner.json").map_err(|_| Error::Io)? {
                continue;
            }
            let header: Header =
                serde_json::from_slice(&small_record(&dir, "owner.json", HEADER_LIMIT)?)
                    .map_err(|_| Error::Invalid)?;
            if header.version != 1 || &header.owner != owner {
                continue;
            }
            if header.relative.as_os_str().is_empty()
                || header
                    .relative
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
            {
                return Err(Error::File);
            }
            found.push(StoredDocument {
                document_id: id.into(),
                path: header.relative.to_str().ok_or(Error::File)?.into(),
                record_available: dir.try_exists("document.json").map_err(|_| Error::Io)?,
            });
            found.sort_by(|a, b| a.document_id.cmp(&b.document_id));
            found.truncate(limit + 1);
        }
        self.directory()?;
        let next_cursor = (found.len() > limit).then(|| found[limit - 1].document_id.clone());
        found.truncate(limit);
        Ok(StoredDocuments {
            documents: found,
            next_cursor,
        })
    }
}

impl StorageRoot {
    /// Discover candidate namespaces by their small source header. No private
    /// command journal, base image or Session metadata is returned to the caller.
    pub(super) fn source_document_ids(
        &self,
        source: &Document,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<String>, Error> {
        source.validate()?;
        let root = self.directory()?;
        let mut found = Vec::new();
        for (count, entry) in root.entries().map_err(|_| Error::Io)?.enumerate() {
            if count >= 4096 {
                return Err(Error::Limit);
            }
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let name = entry.map_err(|_| Error::Io)?.file_name();
            let id = name.to_str().ok_or(Error::File)?;
            if !id.starts_with("bin_") {
                continue;
            }
            let dir = root.open_dir_nofollow(id).map_err(|_| Error::File)?;
            crate::binary::storage::private(
                root,
                &dir.try_clone().map_err(|_| Error::Io)?.into_std_file(),
                true,
            )?;
            if !dir.try_exists("owner.json").map_err(|_| Error::Io)? {
                continue;
            }
            let header: Header =
                serde_json::from_slice(&small_record(&dir, "owner.json", HEADER_LIMIT)?)
                    .map_err(|_| Error::Invalid)?;
            if header.version != 1
                || header.relative.as_os_str().is_empty()
                || header
                    .relative
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err(Error::Changed);
            }
            if header.owner.root == source.root()
                && header.owner.root.join(&header.relative) == source.path()
            {
                found.push(id.to_owned());
            }
        }
        found.sort();
        source.validate()?;
        self.directory()?;
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, StorageRoot, Owner) {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let project = root.path().join("project");
        std::fs::create_dir(&data).unwrap();
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("bytes"), [0, 255, 128]).unwrap();
        let project = project.canonicalize().unwrap();
        let owner = Owner {
            session_id: "ses_catalog".into(),
            directory: project.clone(),
            root: project,
            scope: DraftScope::Manual,
        };
        (root, StorageRoot::prepare(&data).unwrap(), owner)
    }
    fn create(storage: &StorageRoot, owner: &Owner, id: &str) -> Editor {
        let mut doc = Document::open(&owner.root, "bytes").unwrap();
        Editor::create(storage.open(id).unwrap(), owner.clone(), &mut doc, || false).unwrap()
    }
    #[test]
    fn recovery_listing_is_bounded_pageable_and_isolated_while_documents_are_active() {
        let (_root, storage, owner) = fixture();
        let first = create(&storage, &owner, "bin_a");
        let second = create(&storage, &owner, "bin_b");
        let mut model = owner.clone();
        model.scope = DraftScope::Model {
            operation: "call_model".into(),
        };
        let _model = create(&storage, &model, "bin_model");
        let mut other = owner.clone();
        other.session_id = "ses_other".into();
        let _other = create(&storage, &other, "bin_other");
        let page = storage.list(&owner, None, 1).unwrap();
        assert_eq!(page.documents.len(), 1);
        assert_eq!(page.documents[0].document_id, "bin_a");
        assert!(page.documents[0].record_available);
        assert_eq!(page.next_cursor.as_deref(), Some("bin_a"));
        let page = storage
            .list(&owner, page.next_cursor.as_deref(), 1)
            .unwrap();
        assert_eq!(page.documents[0].document_id, "bin_b");
        assert!(page.next_cursor.is_none());
        assert_eq!(
            storage.list(&model, None, 64).unwrap().documents[0].document_id,
            "bin_model"
        );
        assert_eq!(
            storage.list(&other, None, 64).unwrap().documents[0].document_id,
            "bin_other"
        );
        assert!(storage.list(&owner, None, 65).is_err());
        drop((first, second));
        Editor::resume(storage.open("bin_a").unwrap(), &owner).unwrap();
    }
    #[test]
    fn missing_recovery_ids_never_create_namespaces_and_headers_remain_owner_scoped() {
        let (_temp, storage, owner) = fixture();
        assert!(storage.resource(&owner, "bin_absent").is_err());
        assert!(storage.open_existing("bin_absent").is_err());
        assert!(!storage.path().join("bin_absent").exists());
        let _active = create(&storage, &owner, "bin_active");
        assert_eq!(
            storage.resource(&owner, "bin_active").unwrap().path(),
            owner.root.join("bytes")
        );
        let mut other = owner.clone();
        other.session_id = "ses_other".into();
        assert!(storage.resource(&other, "bin_active").is_err());
    }

    #[test]
    fn interrupted_preparation_remains_discoverable_without_a_claimed_journal() {
        let (_root, storage, owner) = fixture();
        let mut doc = Document::open(&owner.root, "bytes").unwrap();
        assert!(matches!(
            Editor::create(
                storage.open("bin_cancelled").unwrap(),
                owner.clone(),
                &mut doc,
                || true
            ),
            Err(Error::Cancelled)
        ));
        let listing = storage.list(&owner, None, 64).unwrap();
        assert_eq!(listing.documents.len(), 1);
        assert_eq!(listing.documents[0].document_id, "bin_cancelled");
        assert!(!listing.documents[0].record_available);
        let editor = Editor::create(
            storage.open("bin_cancelled").unwrap(),
            owner.clone(),
            &mut doc,
            || false,
        )
        .unwrap();
        assert_eq!(editor.owner(), &owner);
        assert!(storage.list(&owner, None, 64).unwrap().documents[0].record_available);
        assert_eq!(
            std::fs::read(owner.root.join("bytes")).unwrap(),
            [0, 255, 128]
        );
    }
}
