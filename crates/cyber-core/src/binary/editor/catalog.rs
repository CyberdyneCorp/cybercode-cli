//! Bounded private owner index. Listing never opens source bytes or a draft base,
//! and never needs the active document's exclusive mutation lock.
use super::*;
use crate::binary::storage::{StorageRoot, small_record};
use cap_fs_ext::DirExt;
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
    if store.directory().symlink_metadata("owner.json").is_ok() {
        return Err(Error::Changed);
    }
    let relative = document
        .path()
        .strip_prefix(document.root())
        .map_err(|_| Error::File)?
        .to_owned();
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
        assert!(matches!(
            Editor::create(
                storage.open("bin_cancelled").unwrap(),
                owner.clone(),
                &mut doc,
                || false
            ),
            Err(Error::Changed)
        ));
        assert_eq!(
            std::fs::read(owner.root.join("bytes")).unwrap(),
            [0, 255, 128]
        );
    }
}
