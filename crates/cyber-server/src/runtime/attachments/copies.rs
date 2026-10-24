//! Blobs shared with other Sessions (`storage-events` → Attachment storage, Copies): a
//! fork, a child task and an import get their own copies before their events commit,
//! hard-linked when the filesystem allows and copied otherwise, so deleting the source
//! never takes them away. Exports carry the blobs ([`ExportedBlob`]) and an import
//! verifies each one before anything is written ([`AttachmentStore::verify_import`]).

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::Engine as _;
use cyber_llm::{AttachmentRef, Content};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{AttachmentError, AttachmentStore, NewBlobs, check_size, sniff, storage, sync_dir};
use crate::runtime::{Entry, Handle, Inner, PromptAttachments, Runtime, RuntimeError};

/// One blob of a JSON export: `{ sha256, media_type, size, name, data }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExportedBlob {
    pub sha256: String,
    pub media_type: String,
    pub size: u64,
    /// The name of the first part referencing the blob.
    #[serde(default)]
    pub name: Option<String>,
    /// Base64 of the bytes.
    pub data: String,
}

/// A verified blob of an import, ready to be written.
#[derive(Debug)]
pub struct ImportedBlob {
    pub sha256: String,
    pub bytes: Vec<u8>,
}

/// The references a set of parts carries, normalized blobs and originals alike.
pub fn references<'a>(parts: impl IntoIterator<Item = &'a Content>) -> Vec<&'a AttachmentRef> {
    parts
        .into_iter()
        .filter_map(|part| match part {
            Content::Attachment(reference) => Some(reference),
            _ => None,
        })
        .collect()
}

/// The SHA-256 of every blob `parts` references, in order, without repeats.
pub fn referenced<'a>(parts: impl IntoIterator<Item = &'a Content>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    references(parts)
        .into_iter()
        .flat_map(|reference| std::iter::once(&reference.sha256).chain(&reference.original))
        .filter(|sha256| seen.insert(sha256.as_str()))
        .cloned()
        .collect()
}

/// The parts of every user entry of a history.
pub fn history_parts(entries: &[Entry]) -> impl Iterator<Item = &Content> {
    entries.iter().flat_map(|entry| match entry {
        Entry::User { parts, .. } => parts.as_slice(),
        _ => &[],
    })
}

impl AttachmentStore {
    /// Give Session `to` its own copy of blob `sha256` of Session `from`: a hard link,
    /// or a durable copy where links are not possible (another filesystem). True when a
    /// blob was added; a blob `to` already has, or one `from` no longer has, adds none
    /// (the copied reference then shows the missing-blob placeholder, as in the source).
    pub fn link(&self, from: &str, to: &str, sha256: &str) -> Result<bool, AttachmentError> {
        let target = self.blob_path(to, sha256)?;
        let source = self.blob_path(from, sha256)?;
        if target.is_file() || !source.is_file() {
            return Ok(false);
        }
        let dir = target
            .parent()
            .expect("blob paths have a Session directory");
        std::fs::create_dir_all(dir).map_err(storage)?;
        match std::fs::hard_link(&source, &target) {
            Ok(()) => sync_dir(dir).map(|()| true).map_err(storage),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(_) => {
                let bytes = std::fs::read(&source).map_err(storage)?;
                Ok(self.write(to, &bytes)?.created)
            }
        }
    }

    /// Link every blob in `blobs` from `from` into `to`; the ones added are removed
    /// again when the returned guard drops before [`NewBlobs::keep`].
    pub fn link_all(
        self: &Arc<Self>,
        from: &str,
        to: &str,
        blobs: &[String],
    ) -> Result<NewBlobs, AttachmentError> {
        let mut added = NewBlobs {
            owner: Some((Arc::clone(self), to.to_owned())),
            blobs: Vec::new(),
        };
        for sha256 in blobs {
            if self.link(from, to, sha256)? {
                added.blobs.push(sha256.clone());
            }
        }
        Ok(added)
    }

    /// The export entry of each blob Session `session_id` references in `parts`; a blob
    /// gone from storage is left out.
    pub fn exported<'a>(
        &self,
        session_id: &str,
        parts: impl IntoIterator<Item = &'a Content>,
        out: &mut Vec<ExportedBlob>,
    ) {
        let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        for reference in references(parts) {
            let blobs = std::iter::once(&reference.sha256).chain(&reference.original);
            for sha256 in blobs {
                if out.iter().any(|blob| &blob.sha256 == sha256) {
                    continue;
                }
                let Ok(bytes) = self.read(session_id, sha256) else {
                    continue;
                };
                out.push(ExportedBlob {
                    sha256: sha256.clone(),
                    media_type: sniff(&bytes).unwrap_or(&reference.media_type).into(),
                    size: bytes.len() as u64,
                    name: reference.name.clone(),
                    data: encode(&bytes),
                });
            }
        }
    }

    /// Decode and check the blobs of an import before anything is written: each must
    /// match its `sha256` and `size`, fit `attachments.max_bytes` and be a supported
    /// media type, and every reference in `parts` must have its blob.
    pub fn verify_import<'a>(
        &self,
        blobs: &[ExportedBlob],
        parts: impl IntoIterator<Item = &'a Content>,
    ) -> Result<Vec<ImportedBlob>, AttachmentError> {
        let mut verified = Vec::with_capacity(blobs.len());
        for blob in blobs {
            let invalid = |reason: &str| {
                AttachmentError::Invalid(format!("attachment {}: {reason}", blob.sha256))
            };
            check_size(blob.size, self.settings.max_bytes)?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&blob.data)
                .map_err(|_| invalid("data is not base64"))?;
            check_size(bytes.len() as u64, self.settings.max_bytes)?;
            if bytes.len() as u64 != blob.size {
                return Err(invalid("size does not match its data"));
            }
            if format!("{:x}", Sha256::digest(&bytes)) != blob.sha256 {
                return Err(invalid("SHA-256 does not match its data"));
            }
            if sniff(&bytes).is_none() {
                return Err(invalid("not a PNG, JPEG, GIF, WebP or PDF"));
            }
            verified.push(ImportedBlob {
                sha256: blob.sha256.clone(),
                bytes,
            });
        }
        if let Some(missing) = referenced(parts)
            .into_iter()
            .find(|sha256| !verified.iter().any(|blob| &blob.sha256 == sha256))
        {
            return Err(AttachmentError::Invalid(format!(
                "attachment {missing} is referenced but not in the export"
            )));
        }
        Ok(verified)
    }

    /// Write verified import blobs into Session `session_id`; removed again when the
    /// returned guard drops before [`NewBlobs::keep`].
    pub fn write_imported(
        self: &Arc<Self>,
        session_id: &str,
        blobs: &[ImportedBlob],
    ) -> Result<NewBlobs, AttachmentError> {
        let mut added = NewBlobs {
            owner: Some((Arc::clone(self), session_id.to_owned())),
            blobs: Vec::new(),
        };
        for blob in blobs {
            if self.write(session_id, &blob.bytes)?.created {
                added.blobs.push(blob.sha256.clone());
            }
        }
        Ok(added)
    }
}

impl Runtime {
    /// A child prompt carrying references of its parent's blobs (a delegated user
    /// attachment) gets the child's own copies before it is admitted; `blobs` removes
    /// them if the prompt is refused. Only the parent's blobs are linked: a reference
    /// to any other Session's blob stays unresolved.
    pub(in crate::runtime) async fn adopt_parent_blobs(
        &self,
        handle: &Handle,
        parts: &[Content],
        blobs: &mut NewBlobs,
    ) -> Result<(), RuntimeError> {
        let wanted = referenced(parts);
        if wanted.is_empty() {
            return Ok(());
        }
        let (id, parent) = {
            let state = handle.state.lock().await;
            (state.info.id.clone(), state.info.parent_id.clone())
        };
        let Some(parent) = parent else {
            return Ok(());
        };
        let store = self.attachments()?;
        let added = tokio::task::spawn_blocking(move || store.link_all(&parent, &id, &wanted))
            .await
            .map_err(|error| AttachmentError::Storage(error.to_string()))??;
        blobs.absorb(added);
        Ok(())
    }
}

impl Runtime {
    /// The attachments of input admitted without prompt hooks (a child task's prompt):
    /// only the child's copies of its parent's blobs, whose media the child's model must
    /// take.
    pub(in crate::runtime) async fn checked_attachments(
        &self,
        session_id: &str,
        parts: &[Content],
    ) -> Result<PromptAttachments, RuntimeError> {
        let mut attachments = PromptAttachments::default();
        if parts
            .iter()
            .any(|part| matches!(part, Content::Attachment(_)))
        {
            let handle = self.inner.handle(session_id).await?;
            self.adopt_parent_blobs(&handle, parts, &mut attachments.blobs)
                .await?;
            self.check_modalities(&handle, parts).await?;
        }
        Ok(attachments)
    }
}

impl Inner {
    /// A fork's own copies of the blobs its copied history references, linked from
    /// `source` before the fork's creation commits.
    pub(in crate::runtime) fn link_fork_blobs(
        &self,
        source: Option<&str>,
        target: &str,
        history: &[Entry],
    ) -> Result<NewBlobs, RuntimeError> {
        let wanted = referenced(history_parts(history));
        let Some(source) = source.filter(|_| !wanted.is_empty()) else {
            return Ok(NewBlobs::default());
        };
        Ok(self.attachment_store()?.link_all(source, target, &wanted)?)
    }
}

impl NewBlobs {
    /// Take over the blobs `other` added to the same Session.
    pub(crate) fn absorb(&mut self, mut other: NewBlobs) {
        if self.owner.is_none() {
            self.owner = other.owner.take();
        }
        self.blobs.append(&mut other.blobs);
    }
}

#[cfg(test)]
mod tests {
    use cyber_core::config::AttachmentSettings;

    use super::*;

    const PDF: &[u8] = b"%PDF-1.4\n1 0 obj << >> endobj\ntrailer << >>\n%%EOF\n";

    fn store(root: &std::path::Path) -> Arc<AttachmentStore> {
        Arc::new(AttachmentStore::durable(
            root.to_path_buf(),
            AttachmentSettings::default(),
        ))
    }

    fn reference(sha256: &str) -> Content {
        Content::Attachment(AttachmentRef {
            sha256: sha256.into(),
            media_type: "application/pdf".into(),
            name: Some("spec.pdf".into()),
            size: PDF.len() as u64,
            width: None,
            height: None,
            pages: None,
            original: None,
        })
    }

    #[test]
    fn links_survive_the_source_and_are_removed_with_a_refused_target() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let sha256 = store.write("ses_a", PDF).unwrap().sha256;
        let added = store
            .link_all("ses_a", "ses_b", std::slice::from_ref(&sha256))
            .unwrap();
        added.keep();
        // Linked again, or from a Session without the blob: nothing is added.
        assert!(!store.link("ses_a", "ses_b", &sha256).unwrap());
        assert!(!store.link("ses_x", "ses_c", &sha256).unwrap());
        std::fs::remove_dir_all(store.session_dir("ses_a").unwrap()).unwrap();
        assert_eq!(store.read("ses_b", &sha256).unwrap(), PDF);

        let refused = store
            .link_all("ses_b", "ses_d", std::slice::from_ref(&sha256))
            .unwrap();
        drop(refused);
        assert!(store.read("ses_d", &sha256).is_err());
        assert!(store.link("ses_b", "../ses_e", &sha256).is_err());
    }

    #[test]
    fn imports_verify_hash_size_limit_and_references_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let sha256 = format!("{:x}", Sha256::digest(PDF));
        let blob = ExportedBlob {
            sha256: sha256.clone(),
            media_type: "application/pdf".into(),
            size: PDF.len() as u64,
            name: Some("spec.pdf".into()),
            data: base64::engine::general_purpose::STANDARD.encode(PDF),
        };
        let parts = [reference(&sha256)];
        let verified = store
            .verify_import(std::slice::from_ref(&blob), &parts)
            .unwrap();
        assert_eq!(verified[0].bytes, PDF);

        let mut tampered = blob.clone();
        tampered.data = base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.5 other");
        tampered.size = 14;
        let error = store.verify_import(&[tampered], &parts).unwrap_err();
        assert!(
            error.to_string().contains("SHA-256 does not match"),
            "{error}"
        );
        let mut huge = blob.clone();
        huge.size = 21 * 1024 * 1024;
        assert!(matches!(
            store.verify_import(&[huge], &parts),
            Err(AttachmentError::TooLarge { .. })
        ));
        let error = store.verify_import(&[], &parts).unwrap_err();
        assert!(error.to_string().contains("not in the export"), "{error}");
        assert!(!dir.path().join("ses_a").exists());

        let written = store.write_imported("ses_a", &verified).unwrap();
        written.keep();
        assert_eq!(store.read("ses_a", &sha256).unwrap(), PDF);
    }
}
