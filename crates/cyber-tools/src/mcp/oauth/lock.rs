//! What both credential backends share: the document lock, the per-credential refresh lock
//! files and the owner-only document file itself.
//!
//! Every lock is a sibling file of the document, so a store and its locks live together:
//! `<document>.lock` serializes reads and writes of the document (bounded retry), and
//! `<document>.refresh-<digest>.lock` serializes token refreshes of one credential. The
//! digest names the server and URL without revealing the URL.
use std::fs::File;
use std::path::Path;
use std::time::Duration;

use super::OAuthError;
use super::store::{RefreshLock, credential_digest, storage};

const LOCK_ATTEMPTS: u32 = 100;
const LOCK_RETRY: Duration = Duration::from_millis(50);
const FILE_LIMIT: u64 = 4 * 1024 * 1024;

/// The owner-only sibling `<document>.<extension>` (its parent is created).
fn lock_file(document: &Path, extension: &str) -> Result<File, OAuthError> {
    let parent = document.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|_| storage("credential directory unavailable"))?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(document.with_extension(extension))
        .map_err(|_| storage("credential lock unavailable"))
}

/// The exclusive lock of `document` (a `.json` file), held until the file is dropped.
pub(super) fn lock_document(document: &Path) -> Result<File, OAuthError> {
    let file = lock_file(document, "json.lock")?;
    for _ in 0..LOCK_ATTEMPTS {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(LOCK_RETRY),
            Err(std::fs::TryLockError::Error(_)) => break,
        }
    }
    Err(OAuthError::Busy)
}

/// The refresh lock of the credential for `server` at `url`, separate from the document
/// lock so short store operations never wait for a token request; `None` while held.
pub(super) fn try_lock_refresh(
    document: &Path,
    server: &str,
    url: &str,
) -> Result<Option<RefreshLock>, OAuthError> {
    let file = lock_file(
        document,
        &format!("json.refresh-{}.lock", credential_digest(server, url)),
    )?;
    match file.try_lock() {
        Ok(()) => Ok(Some(RefreshLock(Box::new(file)))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(_)) => Err(storage("credential refresh lock failed")),
    }
}

/// The bytes of `document`, or `None` when it does not exist. A file that is not regular,
/// is too large or is accessible to other users is refused rather than trusted.
pub(super) fn read_private(document: &Path) -> Result<Option<Vec<u8>>, OAuthError> {
    let metadata = match std::fs::symlink_metadata(document) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(storage("credential file unavailable")),
    };
    if !metadata.is_file() || metadata.len() > FILE_LIMIT {
        return Err(storage("credential file is not a bounded regular file"));
    }
    private(&metadata)?;
    std::fs::read(document)
        .map(Some)
        .map_err(|_| storage("credential file unavailable"))
}

/// Replace `document` atomically with an owner-only file holding `value`.
pub(super) fn write_private<T: serde::Serialize>(
    document: &Path,
    value: &T,
) -> Result<(), OAuthError> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|_| storage("credential encoding failed"))?;
    cyber_core::trust::write_private_atomic(document, &bytes)
        .map_err(|_| storage("credential file write failed"))
}

#[cfg(unix)]
fn private(metadata: &std::fs::Metadata) -> Result<(), OAuthError> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(storage("credential file is accessible to other users"));
    }
    Ok(())
}

#[cfg(not(unix))]
fn private(_metadata: &std::fs::Metadata) -> Result<(), OAuthError> {
    Ok(())
}
