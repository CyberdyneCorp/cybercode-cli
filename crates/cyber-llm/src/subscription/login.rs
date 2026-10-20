//! A stored login: one owner-only document (mode 0600, replaced atomically, at most
//! 64 KiB, versioned) and its cross-process refresh lock (`<file>.refresh.lock`).
//!
//! A refresh re-reads the document under the lock, so a login another process already
//! renewed is adopted without a request and a rotating refresh token is never presented
//! twice. The renewed login is written before the lock is released and before it is used.
//! A rejected renewal leaves the document as it is, so a concurrent login is never
//! overwritten. A document readable by other users, not regular or larger than 64 KiB is
//! refused rather than trusted. Windows has no owner-only file here yet, so storing a
//! login is refused there.
use std::fs::File;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{StoreError, blocking};

const FILE_LIMIT: u64 = 64 * 1024;
const LOCK_WAIT: Duration = Duration::from_secs(60);
const LOCK_RETRY: Duration = Duration::from_millis(50);

/// A login a [`LoginFile`] stores.
pub trait Credential: Clone + Serialize + DeserializeOwned + Send + 'static {
    /// Whether a loaded login is usable; an unusable one is refused as an unknown format.
    fn valid(&self) -> bool;
}

#[derive(Serialize, Deserialize)]
struct Document<T> {
    version: u32,
    #[serde(flatten)]
    login: T,
}

/// One provider's stored-login document, at `path`, in format `version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginFile<T> {
    path: PathBuf,
    version: u32,
    login: PhantomData<fn() -> T>,
}

impl<T: Credential> LoginFile<T> {
    pub fn new(path: impl Into<PathBuf>, version: u32) -> Self {
        Self {
            path: path.into(),
            version,
            login: PhantomData,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// The stored login, or `None` when there is none.
    pub fn load(&self) -> Result<Option<T>, StoreError> {
        let metadata = match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(StoreError::Storage("login file unavailable")),
        };
        if !metadata.is_file() || metadata.len() > FILE_LIMIT {
            return Err(StoreError::Storage(
                "login file is not a bounded regular file",
            ));
        }
        private(&metadata)?;
        let bytes =
            std::fs::read(&self.path).map_err(|_| StoreError::Storage("login file unavailable"))?;
        let document: Document<T> = serde_json::from_slice(&bytes)
            .map_err(|_| StoreError::Storage("login file has an unknown format"))?;
        if document.version != self.version || !document.login.valid() {
            return Err(StoreError::Storage("login file has an unknown format"));
        }
        Ok(Some(document.login))
    }

    /// Replace the stored login atomically with an owner-only file.
    pub fn save(&self, login: &T) -> Result<(), StoreError> {
        if cfg!(not(unix)) {
            return Err(StoreError::Storage(
                "subscription login is not supported on Windows yet",
            ));
        }
        let document = Document {
            version: self.version,
            login: login.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&document)
            .map_err(|_| StoreError::Storage("login encoding failed"))?;
        cyber_core::trust::write_private_atomic(&self.path, &bytes)
            .map_err(|_| StoreError::Storage("login file write failed"))
    }

    /// Forget the login; whether one was stored. Tokens are not revoked remotely.
    pub fn remove(&self) -> Result<bool, StoreError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(StoreError::Storage("login file could not be removed")),
        }
    }

    /// The stored login, read on the blocking pool; [`StoreError::NotSignedIn`] when there
    /// is none.
    pub async fn load_blocking(&self) -> Result<T, StoreError> {
        let file = self.clone();
        blocking(move || file.load())
            .await?
            .ok_or(StoreError::NotSignedIn)
    }

    async fn lock_refresh(&self) -> Result<File, StoreError> {
        let deadline = tokio::time::Instant::now() + LOCK_WAIT;
        let file = self.lock_file()?;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => {}
                Err(std::fs::TryLockError::Error(_)) => {
                    return Err(StoreError::Storage("refresh lock failed"));
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(StoreError::Busy);
            }
            tokio::time::sleep(LOCK_RETRY).await;
        }
    }

    fn lock_file(&self) -> Result<File, StoreError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|_| StoreError::Storage("login directory unavailable"))?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut name = self.path.clone().into_os_string();
        name.push(".refresh.lock");
        options
            .open(PathBuf::from(name))
            .map_err(|_| StoreError::Storage("refresh lock unavailable"))
    }
}

/// The stored login, renewed by `renew` under the refresh lock when `due` says so. The
/// login is re-read under the lock, so one another process renewed meanwhile is returned
/// without calling `renew`; a renewed login is saved before the lock is released. When
/// `renew` fails the file is left untouched.
pub async fn refresh_locked<T, E, R>(
    file: &LoginFile<T>,
    due: impl Fn(&T) -> bool,
    renew: impl FnOnce(T) -> R,
) -> Result<T, E>
where
    T: Credential,
    E: From<StoreError>,
    R: Future<Output = Result<T, E>>,
{
    let login = file.load_blocking().await?;
    if !due(&login) {
        return Ok(login);
    }
    let lock = file.lock_refresh().await?;
    let login = file.load_blocking().await?;
    if !due(&login) {
        return Ok(login);
    }
    let next = renew(login).await?;
    let (file, stored) = (file.clone(), next.clone());
    // The lock is released only once the write has ended, even if this future is dropped
    // meanwhile, so no other holder reads a rotated-away refresh token.
    blocking(move || {
        let _lock = lock;
        file.save(&stored)
    })
    .await?;
    Ok(next)
}

#[cfg(unix)]
fn private(metadata: &std::fs::Metadata) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(StoreError::Storage(
            "login file is accessible to other users",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn private(_metadata: &std::fs::Metadata) -> Result<(), StoreError> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Token {
        secret: String,
        expires_at: u64,
    }

    impl Credential for Token {
        fn valid(&self) -> bool {
            !self.secret.is_empty()
        }
    }

    fn token(secret: &str, expires_at: u64) -> Token {
        Token {
            secret: secret.into(),
            expires_at,
        }
    }

    fn file(dir: &Path) -> LoginFile<Token> {
        LoginFile::new(dir.join("test-auth.json"), 3)
    }

    #[test]
    fn save_is_owner_only_versioned_and_a_widened_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = file(dir.path());
        assert_eq!(file.load(), Ok(None));
        file.save(&token("s", 9)).unwrap();
        let mode = std::fs::metadata(file.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file.path()).unwrap()).unwrap();
        assert_eq!(raw["version"], 3);
        assert_eq!(file.load().unwrap(), Some(token("s", 9)));
        assert!(LoginFile::<Token>::new(file.path(), 4).load().is_err());
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(file.load().is_err());
        assert_eq!(file.remove(), Ok(true));
        assert_eq!(file.remove(), Ok(false));
    }

    #[test]
    fn invalid_oversized_and_non_regular_documents_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = file(dir.path());
        let write = |bytes: &[u8]| {
            cyber_core::trust::write_private_atomic(file.path(), bytes).unwrap();
        };
        write(br#"{"version":3,"secret":"","expires_at":1}"#);
        assert!(file.load().is_err());
        write(&vec![b' '; FILE_LIMIT as usize + 1]);
        assert!(file.load().is_err());
        std::fs::remove_file(file.path()).unwrap();
        std::fs::create_dir(file.path()).unwrap();
        assert!(file.load().is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_refreshes_renew_once_and_adopt_the_stored_result() {
        let dir = tempfile::tempdir().unwrap();
        let file = file(dir.path());
        file.save(&token("old", 1)).unwrap();
        let renewals = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = (0..4)
            .map(|_| {
                let (file, renewals) = (file.clone(), renewals.clone());
                tokio::spawn(async move {
                    refresh_locked::<_, StoreError, _>(
                        &file,
                        |t| t.expires_at < 100,
                        |_| async move {
                            renewals.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            Ok(token("new", 1_000))
                        },
                    )
                    .await
                })
            })
            .collect();
        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap(), token("new", 1_000));
        }
        assert_eq!(renewals.load(Ordering::SeqCst), 1);
        assert_eq!(file.load().unwrap(), Some(token("new", 1_000)));
    }

    #[tokio::test]
    async fn a_rejected_renewal_leaves_the_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let file = file(dir.path());
        file.save(&token("old", 1)).unwrap();
        let before = std::fs::read(file.path()).unwrap();
        let result = refresh_locked(
            &file,
            |_| true,
            |_| async { Err::<Token, _>(StoreError::Storage("rejected")) },
        )
        .await;
        assert_eq!(result, Err(StoreError::Storage("rejected")));
        assert_eq!(std::fs::read(file.path()).unwrap(), before);
    }

    #[tokio::test]
    async fn a_missing_login_is_not_signed_in_and_a_fresh_one_is_not_renewed() {
        let dir = tempfile::tempdir().unwrap();
        let file = file(dir.path());
        let never = |_: Token| async { Err(StoreError::Storage("renewed")) };
        assert_eq!(
            refresh_locked::<_, StoreError, _>(&file, |_| true, never).await,
            Err(StoreError::NotSignedIn)
        );
        file.save(&token("s", 1)).unwrap();
        assert_eq!(
            refresh_locked::<_, StoreError, _>(&file, |_| false, never).await,
            Ok(token("s", 1))
        );
    }
}
