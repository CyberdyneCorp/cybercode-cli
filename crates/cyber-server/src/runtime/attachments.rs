//! Prompt attachments (`session-runtime` → Attachment normalization, `storage-events` →
//! Attachment storage): media parts are sniffed, size-checked, normalized and written once
//! as immutable content-addressed blobs at `<data>/attachments/<session_id>/<sha256>`
//! before their prompt is admitted, so events carry only [`AttachmentRef`]s. `file` parts
//! are read at admission through the `read` tool's checks ([`Runtime::attach_files`]),
//! and a prompt's media must suit the Session's model ([`Runtime::check_modalities`]). A
//! Turn reads the blobs back ([`AttachmentStore::project`]); a reference resolves only
//! inside the Session that owns it.

mod copies;
mod normalize;

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use cyber_core::config::AttachmentSettings;
use cyber_llm::{AttachmentRef, Content, Message};
use sha2::{Digest, Sha256};

use super::host::{FileReadError, ResolvedModel};
use super::{Handle, Runtime, RuntimeError};

pub use copies::{ExportedBlob, ImportedBlob, history_parts, referenced};
pub use normalize::{Normalized, normalize};

/// At most this many media parts per prompt.
pub const MAX_MEDIA_PARTS: usize = 20;
const MIB: u64 = 1024 * 1024;
const MAX_NAME_CHARS: usize = 255;
const PDF: &str = "application/pdf";

/// A refused attachment; nothing of its prompt is admitted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AttachmentError {
    /// `AttachmentTooLargeError` (413).
    #[error("Attachment exceeds {}", size_label(*limit))]
    TooLarge { size: u64, limit: u64 },
    /// `UnsupportedAttachmentError` (415): not a PNG, JPEG, GIF, WebP or PDF, or an image
    /// over the decode limits.
    #[error("{reason}")]
    Unsupported { media_type: String, reason: String },
    /// `UnsupportedModalityError` (422).
    #[error("{model} has no {modality} input; switch models with /model or remove the attachment")]
    Modality { model: String, modality: String },
    /// `InvalidRequestError` (400).
    #[error("{0}")]
    Invalid(String),
    /// The blob could not be written or read.
    #[error("attachment storage failed: {0}")]
    Storage(String),
}

fn size_label(bytes: u64) -> String {
    if bytes.is_multiple_of(MIB) {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{bytes} bytes")
    }
}

fn storage(error: std::io::Error) -> AttachmentError {
    AttachmentError::Storage(error.to_string())
}

/// The media type of `bytes` from their magic bytes: the only source of truth.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    const SIGNATURES: [(&[u8], &str); 5] = [
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"%PDF-", "application/pdf"),
    ];
    if let Some((_, media_type)) = SIGNATURES
        .iter()
        .find(|(magic, _)| bytes.starts_with(magic))
    {
        return Some(media_type);
    }
    let webp = bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP";
    webp.then_some("image/webp")
}

/// `attachments.max_bytes`, checked before anything is decoded or read.
pub fn check_size(size: u64, limit: u64) -> Result<(), AttachmentError> {
    if size > limit {
        return Err(AttachmentError::TooLarge { size, limit });
    }
    Ok(())
}

/// A blob written by [`AttachmentStore::write`]; `created` is false when it already existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub sha256: String,
    pub created: bool,
}

/// One part of a prompt before storage.
#[derive(Debug, Clone)]
pub enum InputPart {
    /// Admitted unchanged (text).
    Content(Content),
    /// Decoded bytes from an inline `image` or `document` part.
    Inline {
        bytes: Vec<u8>,
        name: Option<String>,
    },
    /// A blob this Session already stores.
    Stored { sha256: String },
    /// A path the server reads at admission ([`Runtime::attach_files`]).
    File { path: String, name: Option<String> },
}

/// A `file` part to read at admission, inserted before the admitted part at `index`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FilePart {
    pub index: usize,
    pub path: String,
    pub name: Option<String>,
}

impl InputPart {
    pub fn is_media(&self) -> bool {
        !matches!(self, Self::Content(_))
    }
}

/// Parts ready for admission, and the blobs written for them, which a refused admission
/// removes ([`Runtime::discard_attachments`]).
#[derive(Debug, Default)]
pub struct StoredParts {
    pub parts: Vec<Content>,
    pub created: Vec<String>,
    /// `file` parts, read when the prompt is admitted.
    pub files: Vec<FilePart>,
}

/// Blobs written during an admission, removed when it is dropped before [`NewBlobs::keep`]:
/// a prompt refused after its `file` parts were stored leaves nothing behind.
#[derive(Default)]
pub struct NewBlobs {
    owner: Option<(Arc<AttachmentStore>, String)>,
    blobs: Vec<String>,
}

impl NewBlobs {
    /// The prompt was committed (or an exact retry found it): its blobs stay.
    pub fn keep(mut self) {
        self.blobs.clear();
    }
}

impl Drop for NewBlobs {
    fn drop(&mut self) {
        if let Some((store, session_id)) = &self.owner {
            store.remove(session_id, &self.blobs);
        }
    }
}

impl std::fmt::Debug for NewBlobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewBlobs")
            .field("blobs", &self.blobs)
            .finish()
    }
}

/// Resolved attachments of one Drain, by SHA-256.
pub type Cache = HashMap<String, Content>;

/// The media a Turn's model accepts (`capabilities.vision`, `capabilities.pdf`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modalities {
    pub image: bool,
    pub pdf: bool,
}

impl Modalities {
    pub fn of(model: &ResolvedModel) -> Self {
        Self {
            image: model.vision,
            pdf: model.pdf,
        }
    }

    /// `image` or `pdf` when the model lacks the input `media_type` needs.
    fn missing(self, media_type: &str) -> Option<&'static str> {
        if media_type == PDF {
            (!self.pdf).then_some("pdf")
        } else {
            (!self.image).then_some("image")
        }
    }
}

/// `[Attached <mime>: <name>]` (or `[Attached <mime>]` without a name) for a media part,
/// the text media is reduced to after an overflow compaction (`compaction` → Media
/// stripping on overflow).
pub(crate) fn stripped(content: &Content) -> Option<String> {
    let name = match content {
        Content::Document { name, .. } => name.as_deref(),
        Content::Attachment(reference) => reference.name.as_deref(),
        _ => None,
    };
    let media_type = media_type(content)?;
    Some(match name {
        Some(name) => format!("[Attached {media_type}: {name}]"),
        None => format!("[Attached {media_type}]"),
    })
}

/// The media type of a media part, inline or stored.
fn media_type(content: &Content) -> Option<&str> {
    match content {
        Content::Image { media_type, .. } | Content::Document { media_type, .. } => {
            Some(media_type)
        }
        Content::Attachment(reference) => Some(&reference.media_type),
        _ => None,
    }
}

/// The blob store of every Session of one server.
pub struct AttachmentStore {
    root: PathBuf,
    settings: AttachmentSettings,
    /// The private directory of an in-memory store, removed with the server.
    _ephemeral: Option<tempfile::TempDir>,
}

impl AttachmentStore {
    /// Blobs under `root` (`<data>/attachments`).
    pub fn durable(root: PathBuf, settings: AttachmentSettings) -> Self {
        Self {
            root,
            settings,
            _ephemeral: None,
        }
    }

    /// Blobs in a private temporary directory removed when the store is dropped, for
    /// `durability: ephemeral`.
    pub fn ephemeral(settings: AttachmentSettings) -> Result<Self, AttachmentError> {
        let dir = tempfile::Builder::new()
            .prefix("cyber-attachments-")
            .tempdir()
            .map_err(storage)?;
        Ok(Self {
            root: dir.path().to_path_buf(),
            settings,
            _ephemeral: Some(dir),
        })
    }

    pub fn settings(&self) -> &AttachmentSettings {
        &self.settings
    }

    /// `<root>/<session_id>`; refuses IDs that could leave the root.
    pub fn session_dir(&self, session_id: &str) -> Result<PathBuf, AttachmentError> {
        let safe = !session_id.is_empty()
            && session_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !safe {
            return Err(AttachmentError::Invalid(format!(
                "invalid Session ID {session_id:?}"
            )));
        }
        Ok(self.root.join(session_id))
    }

    fn blob_path(&self, session_id: &str, sha256: &str) -> Result<PathBuf, AttachmentError> {
        let valid = sha256.len() == 64
            && sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !valid {
            return Err(AttachmentError::Invalid(format!(
                "attachment {sha256:?} is not a lowercase SHA-256"
            )));
        }
        Ok(self.session_dir(session_id)?.join(sha256))
    }

    /// Write `bytes` durably (temporary file, sync, rename, directory sync), or reuse the
    /// blob that already holds them.
    pub fn write(&self, session_id: &str, bytes: &[u8]) -> Result<Stored, AttachmentError> {
        let sha256 = format!("{:x}", Sha256::digest(bytes));
        let path = self.blob_path(session_id, &sha256)?;
        if path.is_file() {
            return Ok(Stored {
                sha256,
                created: false,
            });
        }
        let dir = path.parent().expect("blob paths have a Session directory");
        std::fs::create_dir_all(dir).map_err(storage)?;
        let temp = dir.join(format!(".{}.tmp", cyber_core::ids::new_id("blob")));
        let written = write_synced(&temp, bytes).and_then(|()| std::fs::rename(&temp, &path));
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temp);
            return Err(storage(error));
        }
        sync_dir(dir).map_err(storage)?;
        Ok(Stored {
            sha256,
            created: true,
        })
    }

    /// The bytes of a blob of this Session.
    pub fn read(&self, session_id: &str, sha256: &str) -> Result<Vec<u8>, AttachmentError> {
        std::fs::read(self.blob_path(session_id, sha256)?).map_err(storage)
    }

    /// Remove blobs written for a prompt that was then refused.
    pub fn remove(&self, session_id: &str, blobs: &[String]) {
        for sha256 in blobs {
            if let Ok(path) = self.blob_path(session_id, sha256) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// Normalize and store one decoded attachment; `created` collects new blobs.
    fn ingest(
        &self,
        session_id: &str,
        bytes: Vec<u8>,
        name: Option<String>,
        created: &mut Vec<String>,
    ) -> Result<(AttachmentRef, Option<String>), AttachmentError> {
        let normalized = normalize(bytes, &self.settings, name.as_deref())?;
        let mut keep = |bytes: &[u8]| -> Result<String, AttachmentError> {
            let stored = self.write(session_id, bytes)?;
            if stored.created {
                created.push(stored.sha256.clone());
            }
            Ok(stored.sha256)
        };
        let original = normalized.original.as_deref().map(&mut keep).transpose()?;
        let reference = AttachmentRef {
            sha256: keep(&normalized.bytes)?,
            media_type: normalized.media_type.into(),
            name,
            size: normalized.bytes.len() as u64,
            width: normalized.width,
            height: normalized.height,
            pages: normalized.pages,
            original,
        };
        Ok((reference, normalized.note))
    }

    /// A reference to a blob this Session already stores (an `attachment` part).
    fn existing(&self, session_id: &str, sha256: &str) -> Result<AttachmentRef, AttachmentError> {
        let unknown = || AttachmentError::Invalid(format!("unknown attachment {sha256}"));
        let bytes = self.read(session_id, sha256).map_err(|_| unknown())?;
        let media_type = sniff(&bytes).ok_or_else(unknown)?;
        Ok(AttachmentRef {
            sha256: sha256.into(),
            media_type: media_type.into(),
            name: None,
            size: bytes.len() as u64,
            width: None,
            height: None,
            pages: None,
            original: None,
        })
    }

    /// Store every media part of a prompt and return the parts to admit, with a PDF's
    /// page note after its reference. Blobs already written are removed on a refusal.
    pub fn store_parts(
        &self,
        session_id: &str,
        input: Vec<InputPart>,
    ) -> Result<StoredParts, AttachmentError> {
        let mut out = StoredParts::default();
        for part in input {
            if let Err(error) = self.store_part(session_id, part, &mut out) {
                self.remove(session_id, &out.created);
                return Err(error);
            }
        }
        Ok(out)
    }

    fn store_part(
        &self,
        session_id: &str,
        part: InputPart,
        out: &mut StoredParts,
    ) -> Result<(), AttachmentError> {
        match part {
            InputPart::Content(content) => out.parts.push(content),
            InputPart::Stored { sha256 } => out
                .parts
                .push(Content::Attachment(self.existing(session_id, &sha256)?)),
            InputPart::File { path, name } => out.files.push(FilePart {
                index: out.parts.len(),
                path,
                name,
            }),
            InputPart::Inline { bytes, name } => {
                check_size(bytes.len() as u64, self.settings.max_bytes)?;
                let name = name.map(|name| name.chars().take(MAX_NAME_CHARS).collect());
                let (reference, note) = self.ingest(session_id, bytes, name, &mut out.created)?;
                out.parts.push(Content::Attachment(reference));
                out.parts.extend(note.map(|text| Content::Text { text }));
            }
        }
        Ok(())
    }

    /// Replace every reference in `messages` with the stored bytes, or with a placeholder
    /// when the Turn's model lacks the input (`tool-registry` → Tool result projection) or
    /// the blob is gone, so the model never sees a silent gap. The media type comes from
    /// the bytes, never from the reference. `cache` keeps the blobs already read in this
    /// Drain.
    pub fn project(
        &self,
        session_id: &str,
        messages: &mut [Message],
        modalities: Modalities,
        cache: &mut Cache,
    ) {
        for content in messages.iter_mut().flat_map(|m| m.content.iter_mut()) {
            let Content::Attachment(reference) = content else {
                continue;
            };
            let resolved = match cache.get(&reference.sha256) {
                Some(resolved) => resolved.clone(),
                None => {
                    let resolved = self.resolve(session_id, reference);
                    if !matches!(resolved, Content::Text { .. }) {
                        cache.insert(reference.sha256.clone(), resolved.clone());
                    }
                    resolved
                }
            };
            *content = match media_type(&resolved).and_then(|media| {
                modalities
                    .missing(media)
                    .map(|modality| (media.to_owned(), modality))
            }) {
                Some((media, modality)) => Content::Text {
                    text: format!("[{media} omitted: model has no {modality} input]"),
                },
                None => resolved,
            };
        }
    }

    fn resolve(&self, session_id: &str, reference: &AttachmentRef) -> Content {
        let name = reference.name.as_deref().unwrap_or(&reference.sha256);
        let bytes = self.read(session_id, &reference.sha256).ok();
        let Some((bytes, media_type)) = bytes.and_then(|b| sniff(&b).map(|media| (b, media)))
        else {
            return Content::Text {
                text: format!("[{} {name} missing from storage]", reference.media_type),
            };
        };
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        if media_type == PDF {
            Content::Document {
                media_type: media_type.into(),
                data,
                name: reference.name.clone(),
            }
        } else {
            Content::Image {
                media_type: media_type.into(),
                data,
            }
        }
    }
}

/// A refused `file` part: a denial is `PermissionDeniedError`, an oversized file
/// `AttachmentTooLargeError`, anything else `InvalidRequestError`.
fn read_error(error: FileReadError, limit: u64) -> RuntimeError {
    match error {
        FileReadError::Denied(reason) => RuntimeError::PermissionDenied(reason),
        FileReadError::TooLarge { size } => AttachmentError::TooLarge { size, limit }.into(),
        FileReadError::Invalid(reason) => AttachmentError::Invalid(reason).into(),
    }
}

/// The last component of a client path, on either separator.
fn file_name(path: &str) -> Option<String> {
    path.rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Make a rename durable. Windows has no directory handle to sync, and its rename is
/// journaled by NTFS.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

impl Runtime {
    /// Use `store` for every Session's attachments; false when one is already set. An
    /// unconfigured runtime keeps them in a private temporary directory.
    pub fn configure_attachments(&self, store: AttachmentStore) -> bool {
        self.inner.attachments.set(Ok(Arc::new(store))).is_ok()
    }

    /// The attachment store of this server.
    pub fn attachments(&self) -> Result<Arc<AttachmentStore>, RuntimeError> {
        self.inner.attachment_store()
    }

    /// Sniff, check, normalize and store the media parts of a prompt for Session `id`
    /// before it is admitted.
    pub async fn store_prompt_parts(
        &self,
        id: &str,
        input: Vec<InputPart>,
    ) -> Result<StoredParts, RuntimeError> {
        self.inner.handle(id).await?;
        if !input.iter().any(InputPart::is_media) {
            let parts = input
                .into_iter()
                .filter_map(|part| match part {
                    InputPart::Content(content) => Some(content),
                    _ => None,
                })
                .collect();
            return Ok(StoredParts {
                parts,
                ..StoredParts::default()
            });
        }
        let store = self.attachments()?;
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || store.store_parts(&id, input))
            .await
            .map_err(|error| AttachmentError::Storage(error.to_string()))?
            .map_err(RuntimeError::from)
    }

    /// Read each `file` part through the host's `read` checks, normalize and store it, and
    /// insert its reference at the part's index. An ask reaches the attached client; a
    /// denial refuses the prompt. The blobs written go to `blobs`.
    pub(super) async fn attach_files(
        &self,
        handle: &Handle,
        mut parts: Vec<Content>,
        files: &[FilePart],
        blobs: &mut NewBlobs,
    ) -> Result<Vec<Content>, RuntimeError> {
        let store = self.attachments()?;
        let turn = super::drain::turn_context_for(&*handle.state.lock().await, false);
        let asker = self.operation_asker(&turn.session_id).await?;
        blobs.owner = Some((Arc::clone(&store), turn.session_id.clone()));
        let limit = store.settings().max_bytes;
        let mut inserted = 0;
        for file in files {
            let bytes = self
                .inner
                .tools
                .read_attachment(
                    &turn,
                    &file.path,
                    limit,
                    asker.clone(),
                    self.inner.closed.child_token(),
                )
                .await
                .map_err(|error| read_error(error, limit))?;
            let name = file.name.clone().or_else(|| file_name(&file.path));
            let input = vec![InputPart::Inline { bytes, name }];
            let (writer, id) = (Arc::clone(&store), turn.session_id.clone());
            let stored = tokio::task::spawn_blocking(move || writer.store_parts(&id, input))
                .await
                .map_err(|error| AttachmentError::Storage(error.to_string()))??;
            blobs.blobs.extend(stored.created);
            let at = (file.index + inserted).min(parts.len());
            inserted += stored.parts.len();
            parts.splice(at..at, stored.parts);
        }
        Ok(parts)
    }

    /// Refuse a prompt whose media the Session's model cannot take: `UnsupportedModalityError`
    /// naming the model, and nothing is admitted.
    pub(super) async fn check_modalities(
        &self,
        handle: &Handle,
        parts: &[Content],
    ) -> Result<(), RuntimeError> {
        let media: Vec<&str> = parts.iter().filter_map(media_type).collect();
        if media.is_empty() {
            return Ok(());
        }
        let model = {
            let state = handle.state.lock().await;
            self.inner
                .resolve_selection(&state.info, &state.model_selection)?
                .model
        };
        let modalities = Modalities::of(&model);
        match media.iter().find_map(|media| modalities.missing(media)) {
            Some(modality) => Err(AttachmentError::Modality {
                model: format!("{}/{}", model.provider, model.model),
                modality: modality.into(),
            }
            .into()),
            None => Ok(()),
        }
    }

    /// Remove the blobs a refused prompt wrote.
    pub fn discard_attachments(&self, id: &str, created: &[String]) {
        if created.is_empty() {
            return;
        }
        if let Ok(store) = self.attachments() {
            store.remove(id, created);
        }
    }
}

impl Handle {
    pub(crate) fn clear_attachment_cache(&self) {
        self.attachment_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

impl super::Inner {
    pub(crate) fn attachment_store(&self) -> Result<Arc<AttachmentStore>, RuntimeError> {
        self.attachments
            .get_or_init(|| {
                AttachmentStore::ephemeral(AttachmentSettings::default())
                    .map(Arc::new)
                    .map_err(|error| error.to_string())
            })
            .clone()
            .map_err(|error| AttachmentError::Storage(error).into())
    }

    /// Drop the blobs an ended Drain cached.
    pub(crate) async fn release_attachment_cache(&self, session_id: &str) {
        if let Ok(handle) = self.handle(session_id).await {
            handle.clear_attachment_cache();
        }
    }

    /// Resolve the references of a Turn's messages from the Session's blobs, for the
    /// Turn's model; `handle` caches them for its Drain.
    pub(crate) fn project_attachments(
        &self,
        session_id: &str,
        messages: &mut [Message],
        model: &ResolvedModel,
        handle: Option<&Handle>,
    ) {
        if let Some(computer) = self.computer.get() {
            computer.project(session_id, messages, model.vision);
        }
        let referenced = messages
            .iter()
            .flat_map(|m| &m.content)
            .any(|content| matches!(content, Content::Attachment(_)));
        if !referenced {
            return;
        }
        match self.attachment_store() {
            Ok(store) => {
                let mut local = Cache::new();
                let mut shared = handle.map(|handle| {
                    handle
                        .attachment_cache
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                });
                let cache = shared.as_deref_mut().unwrap_or(&mut local);
                store.project(session_id, messages, Modalities::of(model), cache);
            }
            // Unresolved references fail the request in the adapter.
            Err(error) => cyber_core::log::warn(
                "attachments",
                &error.to_string(),
                serde_json::json!({ "session_id": session_id }),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 7])
        });
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn store(root: &Path) -> AttachmentStore {
        AttachmentStore::durable(root.to_path_buf(), AttachmentSettings::default())
    }

    #[test]
    fn media_types_come_from_magic_bytes() {
        assert_eq!(sniff(&png(2, 2)), Some("image/png"));
        assert_eq!(sniff(b"\xff\xd8\xff\xe0rest"), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a...."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"%PDF-1.7\n"), Some("application/pdf"));
        // A ZIP named photo.png is still a ZIP.
        assert_eq!(sniff(b"PK\x03\x04photo.png"), None);
        assert_eq!(sniff(b"RIFF\x10\0\0\0WAVE"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn the_size_limit_reports_size_and_limit() {
        assert_eq!(check_size(20 * MIB, 20 * MIB), Ok(()));
        let error = check_size(25 * MIB, 20 * MIB).unwrap_err();
        assert_eq!(error.to_string(), "Attachment exceeds 20 MiB");
        assert_eq!(
            error,
            AttachmentError::TooLarge {
                size: 25 * MIB,
                limit: 20 * MIB
            }
        );
    }

    #[test]
    fn blobs_are_written_once_and_reused() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(temp.path());
        let first = store.write("ses_1", b"%PDF-1.4 one").unwrap();
        assert!(first.created);
        assert_eq!(
            first.sha256,
            format!("{:x}", Sha256::digest(b"%PDF-1.4 one"))
        );
        let path = temp.path().join("ses_1").join(&first.sha256);
        assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.4 one");
        let again = store.write("ses_1", b"%PDF-1.4 one").unwrap();
        assert_eq!(again.sha256, first.sha256);
        assert!(!again.created);
        // No temporary file is left behind.
        let names: Vec<_> = std::fs::read_dir(temp.path().join("ses_1"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(&first.sha256)]);
        // A reference resolves only inside its own Session.
        assert!(store.read("ses_2", &first.sha256).is_err());
        assert!(store.write("../escape", b"x").is_err());
        assert!(store.read("ses_1", "../ses_1").is_err());
    }

    #[test]
    fn a_refused_prompt_removes_the_blobs_it_wrote() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(temp.path());
        let kept = store.write("ses_1", &png(3, 3)).unwrap();
        let input = vec![
            InputPart::Inline {
                bytes: png(4, 4),
                name: Some("new.png".into()),
            },
            InputPart::Inline {
                bytes: png(3, 3),
                name: None,
            },
            InputPart::Inline {
                bytes: b"PK\x03\x04not an image".to_vec(),
                name: Some("photo.png".into()),
            },
        ];
        let error = store.store_parts("ses_1", input).unwrap_err();
        assert!(matches!(error, AttachmentError::Unsupported { .. }));
        let left: Vec<_> = std::fs::read_dir(temp.path().join("ses_1"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        // The blob that existed before the prompt stays.
        assert_eq!(left, vec![kept.sha256]);
    }

    #[test]
    fn stored_parts_hold_references_and_project_back_to_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(temp.path());
        let shot = png(8, 6);
        let stored = store
            .store_parts(
                "ses_1",
                vec![
                    InputPart::Content(Content::Text {
                        text: "look".into(),
                    }),
                    InputPart::Inline {
                        bytes: shot.clone(),
                        name: Some("shot.png".into()),
                    },
                ],
            )
            .unwrap();
        let Content::Attachment(reference) = stored.parts[1].clone() else {
            panic!("{:?}", stored.parts);
        };
        assert_eq!(reference.media_type, "image/png");
        assert_eq!((reference.width, reference.height), (Some(8), Some(6)));
        assert_eq!(reference.size, shot.len() as u64);
        assert_eq!(stored.created, vec![reference.sha256.clone()]);
        let again = store
            .store_parts(
                "ses_1",
                vec![InputPart::Stored {
                    sha256: reference.sha256.clone(),
                }],
            )
            .unwrap();
        assert!(again.created.is_empty());
        let mut messages = vec![Message {
            role: cyber_llm::Role::User,
            content: stored.parts.clone(),
        }];
        let all = Modalities {
            image: true,
            pdf: true,
        };
        let mut cache = Cache::new();
        store.project("ses_1", &mut messages, all, &mut cache);
        assert!(cache.contains_key(&reference.sha256));
        let data = base64::engine::general_purpose::STANDARD.encode(&shot);
        assert_eq!(
            messages[0].content[1],
            Content::Image {
                media_type: "image/png".into(),
                data
            }
        );
        let mut elsewhere = vec![Message {
            role: cyber_llm::Role::User,
            content: stored.parts,
        }];
        let text_only = Modalities {
            image: false,
            pdf: true,
        };
        let mut omitted = elsewhere.clone();
        store.project("ses_1", &mut omitted, text_only, &mut Cache::new());
        assert_eq!(
            omitted[0].content[1],
            Content::Text {
                text: "[image/png omitted: model has no image input]".into()
            }
        );
        store.project("ses_2", &mut elsewhere, all, &mut Cache::new());
        assert_eq!(
            elsewhere[0].content[1],
            Content::Text {
                text: "[image/png shot.png missing from storage]".into()
            }
        );
        let unknown = store.store_parts(
            "ses_2",
            vec![InputPart::Stored {
                sha256: reference.sha256.clone(),
            }],
        );
        assert!(matches!(unknown, Err(AttachmentError::Invalid(_))));
    }

    #[test]
    fn an_ephemeral_store_is_removed_with_the_server() {
        let store = AttachmentStore::ephemeral(AttachmentSettings::default()).unwrap();
        let root = store.root.clone();
        store.write("ses_1", b"%PDF-1.4").unwrap();
        assert!(root.join("ses_1").is_dir());
        drop(store);
        assert!(!root.exists());
    }
}
