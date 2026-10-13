//! MCP OAuth credential storage keyed by server name, exact canonical server URL and
//! issuer, so a token is never presented to another URL or minted by another issuer.
//!
//! The file backend keeps every credential in one owner-only JSON document
//! (`<data>/mcp-auth.json`, mode 0600). Each operation takes an exclusive lock on a sibling
//! lock file (bounded retry), reads the document once, and replaces it atomically. A file
//! readable by other users, a non-regular file or an unknown format is refused rather than
//! trusted. Windows has no owner-only ACL support here yet, so the backend refuses there.
//!
//! Beside credentials the store keeps per server/URL [`AuthMarker`]s: conditions that
//! `cyber mcp auth` or a refused request recorded (no client registration available, scopes
//! the resource asked for), so the runtime reports them without running discovery itself.
//! Markers hold no secrets.
//!
//! Token refreshes of one credential (server and URL) are serialized by a separate refresh
//! lock that a holder keeps across the token request, so connections and processes sharing
//! a credential never present the same rotating refresh token twice, while a slow
//! authorization server never delays another server's refresh.
//!
//! The OS keyring backend ([`super::VaultCredentialStore`]) shares the locks and the
//! owner-only document handling ([`super::lock`]).
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use super::{OAuthClient, OAuthError, TokenSet, lock};

const VERSION: u32 = 1;

/// Which credential: the server name, its canonical URL and the token's issuer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialKey {
    pub server: String,
    pub url: String,
    pub issuer: String,
}

/// Tokens with the client that obtained them and where to refresh them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCredential {
    pub tokens: TokenSet,
    pub client: OAuthClient,
    pub token_endpoint: String,
}

/// What the next `cyber mcp auth` for one server/URL must address. No secrets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthMarker {
    /// The authorization server offers no dynamic client registration and no `client_id`
    /// was configured when it was discovered.
    pub needs_client_registration: bool,
    /// Scopes the resource required (`insufficient_scope`), unioned and sorted.
    pub required_scopes: Vec<String>,
}

impl AuthMarker {
    /// Set what `other` sets; scopes are unioned.
    pub(super) fn merge(&mut self, other: &Self) {
        self.needs_client_registration |= other.needs_client_registration;
        let scopes: std::collections::BTreeSet<_> = self
            .required_scopes
            .iter()
            .chain(&other.required_scopes)
            .cloned()
            .collect();
        self.required_scopes = scopes.into_iter().collect();
    }
}

/// The exclusive refresh lock of one credential, released on drop.
pub struct RefreshLock(#[allow(dead_code)] pub(super) Box<dyn Send + Sync>);

impl RefreshLock {
    /// A lock held until `guard` is dropped, for stores implemented elsewhere.
    pub fn new(guard: impl Send + Sync + 'static) -> Self {
        Self(Box::new(guard))
    }
}

impl std::fmt::Debug for RefreshLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RefreshLock")
    }
}

pub trait McpCredentialStore: Send + Sync {
    fn get(&self, key: &CredentialKey) -> Result<Option<StoredCredential>, OAuthError>;
    /// The credential for `server` at exactly `url`, whichever issuer minted it.
    fn find(
        &self,
        server: &str,
        url: &str,
    ) -> Result<Option<(CredentialKey, StoredCredential)>, OAuthError>;
    /// Whether a credential is stored for `server` at exactly `url`, without its secrets:
    /// a store backed by the OS keyring answers from its index, never waiting on the
    /// keyring.
    fn contains(&self, server: &str, url: &str) -> Result<bool, OAuthError> {
        self.find(server, url).map(|found| found.is_some())
    }
    /// Insert or replace; a server/URL pair keeps one credential, so a new issuer replaces
    /// the old one.
    fn put(&self, key: &CredentialKey, credential: &StoredCredential) -> Result<(), OAuthError>;
    /// Replace the credential stored under exactly `key` only while it still holds
    /// `previous`, so a refresh never restores a credential that was removed (`cyber mcp
    /// logout`) or replaced (`cyber mcp auth`) meanwhile. Whether it was replaced.
    fn replace(
        &self,
        key: &CredentialKey,
        previous: &TokenSet,
        credential: &StoredCredential,
    ) -> Result<bool, OAuthError>;
    /// Take the exclusive refresh lock of the credential for `server` at `url`, or `None`
    /// while another connection or process holds it. Other credentials have their own.
    fn try_lock_refresh(&self, server: &str, url: &str) -> Result<Option<RefreshLock>, OAuthError>;
    /// Remove every credential for `server` at `url`; whether any existed.
    fn remove(&self, server: &str, url: &str) -> Result<bool, OAuthError>;
    /// Remove every credential and marker for `server`, at any URL and from any issuer;
    /// how many credentials were removed.
    fn remove_server(&self, server: &str) -> Result<usize, OAuthError>;
    /// The marker for `server` at exactly `url` (empty when none).
    fn marker(&self, server: &str, url: &str) -> Result<AuthMarker, OAuthError>;
    /// Merge `marker` into the one for `server` at `url`.
    fn mark(&self, server: &str, url: &str, marker: &AuthMarker) -> Result<(), OAuthError>;
    /// Forget the marker for `server` at `url`, after a successful authorization.
    fn clear_marker(&self, server: &str, url: &str) -> Result<(), OAuthError>;
}

/// One stored credential, flat on disk.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    server: String,
    url: String,
    issuer: String,
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    issued_at: Option<u64>,
    #[serde(default)]
    scopes: Vec<String>,
    client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    token_endpoint: String,
}

impl Entry {
    fn new(key: &CredentialKey, credential: &StoredCredential) -> Self {
        let StoredCredential {
            tokens,
            client,
            token_endpoint,
        } = credential.clone();
        Self {
            server: key.server.clone(),
            url: key.url.clone(),
            issuer: key.issuer.clone(),
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_at: tokens.expires_at,
            issued_at: tokens.issued_at,
            scopes: tokens.scopes,
            client_id: client.client_id,
            client_secret: client.client_secret,
            token_endpoint,
        }
    }

    fn key(&self) -> CredentialKey {
        CredentialKey {
            server: self.server.clone(),
            url: self.url.clone(),
            issuer: self.issuer.clone(),
        }
    }

    fn credential(&self) -> StoredCredential {
        StoredCredential {
            tokens: TokenSet {
                access_token: self.access_token.clone(),
                refresh_token: self.refresh_token.clone(),
                expires_at: self.expires_at,
                issued_at: self.issued_at,
                scopes: self.scopes.clone(),
            },
            client: OAuthClient {
                client_id: self.client_id.clone(),
                client_secret: self.client_secret.clone(),
            },
            token_endpoint: self.token_endpoint.clone(),
        }
    }

    fn at(&self, server: &str, url: &str) -> bool {
        self.server == server && self.url == url
    }
}

/// One server/URL marker, flat on disk.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marked {
    server: String,
    url: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    needs_client_registration: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    required_scopes: Vec<String>,
}

impl Marked {
    fn new(server: &str, url: &str, marker: AuthMarker) -> Self {
        Self {
            server: server.into(),
            url: url.into(),
            needs_client_registration: marker.needs_client_registration,
            required_scopes: marker.required_scopes,
        }
    }

    fn at(&self, server: &str, url: &str) -> bool {
        self.server == server && self.url == url
    }

    fn marker(&self) -> AuthMarker {
        AuthMarker {
            needs_client_registration: self.needs_client_registration,
            required_scopes: self.required_scopes.clone(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    credentials: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    markers: Vec<Marked>,
}

impl Document {
    fn get(&self, key: &CredentialKey) -> Option<StoredCredential> {
        self.credentials
            .iter()
            .find(|entry| entry.key() == *key)
            .map(Entry::credential)
    }

    fn find(&self, server: &str, url: &str) -> Option<(CredentialKey, StoredCredential)> {
        self.credentials
            .iter()
            .find(|entry| entry.at(server, url))
            .map(|entry| (entry.key(), entry.credential()))
    }

    fn put(&mut self, key: &CredentialKey, credential: &StoredCredential) {
        self.remove(&key.server, &key.url);
        self.credentials.push(Entry::new(key, credential));
    }

    fn replace(
        &mut self,
        key: &CredentialKey,
        previous: &TokenSet,
        credential: &StoredCredential,
    ) -> bool {
        let current = self
            .credentials
            .iter()
            .any(|entry| entry.key() == *key && entry.credential().tokens == *previous);
        if current {
            self.put(key, credential);
        }
        current
    }

    fn remove(&mut self, server: &str, url: &str) -> bool {
        let before = self.credentials.len();
        self.credentials.retain(|entry| !entry.at(server, url));
        self.credentials.len() != before
    }

    /// Credentials removed; markers for `server` go too. Whether anything changed.
    fn remove_server(&mut self, server: &str) -> (usize, bool) {
        let (credentials, markers) = (self.credentials.len(), self.markers.len());
        self.credentials.retain(|entry| entry.server != server);
        self.markers.retain(|marked| marked.server != server);
        let removed = credentials - self.credentials.len();
        (removed, removed > 0 || markers != self.markers.len())
    }

    fn marker(&self, server: &str, url: &str) -> AuthMarker {
        self.markers
            .iter()
            .find(|marked| marked.at(server, url))
            .map(Marked::marker)
            .unwrap_or_default()
    }

    /// Whether the merge changed anything.
    fn mark(&mut self, server: &str, url: &str, marker: &AuthMarker) -> bool {
        let mut merged = self.marker(server, url);
        let before = merged.clone();
        merged.merge(marker);
        if merged == before {
            return false;
        }
        self.clear_marker(server, url);
        self.markers.push(Marked::new(server, url, merged));
        true
    }

    /// Whether a marker was removed.
    fn clear_marker(&mut self, server: &str, url: &str) -> bool {
        let before = self.markers.len();
        self.markers.retain(|marked| !marked.at(server, url));
        self.markers.len() != before
    }
}

/// One credential's in-memory refresh lock.
type RefreshMutex = Arc<tokio::sync::Mutex<()>>;

/// An in-memory store for tests and embedders.
#[derive(Default)]
pub struct MemoryCredentialStore {
    document: Mutex<Document>,
    refresh: Mutex<HashMap<(String, String), RefreshMutex>>,
}

impl MemoryCredentialStore {
    fn with<T>(&self, operation: impl FnOnce(&mut Document) -> T) -> T {
        operation(&mut self.document.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl McpCredentialStore for MemoryCredentialStore {
    fn get(&self, key: &CredentialKey) -> Result<Option<StoredCredential>, OAuthError> {
        Ok(self.with(|document| document.get(key)))
    }
    fn find(
        &self,
        server: &str,
        url: &str,
    ) -> Result<Option<(CredentialKey, StoredCredential)>, OAuthError> {
        Ok(self.with(|document| document.find(server, url)))
    }
    fn put(&self, key: &CredentialKey, credential: &StoredCredential) -> Result<(), OAuthError> {
        self.with(|document| document.put(key, credential));
        Ok(())
    }
    fn replace(
        &self,
        key: &CredentialKey,
        previous: &TokenSet,
        credential: &StoredCredential,
    ) -> Result<bool, OAuthError> {
        Ok(self.with(|document| document.replace(key, previous, credential)))
    }
    fn try_lock_refresh(&self, server: &str, url: &str) -> Result<Option<RefreshLock>, OAuthError> {
        let lock = self
            .refresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((server.into(), url.into()))
            .or_default()
            .clone();
        Ok(lock
            .try_lock_owned()
            .ok()
            .map(|guard| RefreshLock(Box::new(guard))))
    }
    fn remove(&self, server: &str, url: &str) -> Result<bool, OAuthError> {
        Ok(self.with(|document| document.remove(server, url)))
    }
    fn remove_server(&self, server: &str) -> Result<usize, OAuthError> {
        Ok(self.with(|document| document.remove_server(server).0))
    }
    fn marker(&self, server: &str, url: &str) -> Result<AuthMarker, OAuthError> {
        Ok(self.with(|document| document.marker(server, url)))
    }
    fn mark(&self, server: &str, url: &str, marker: &AuthMarker) -> Result<(), OAuthError> {
        self.with(|document| document.mark(server, url, marker));
        Ok(())
    }
    fn clear_marker(&self, server: &str, url: &str) -> Result<(), OAuthError> {
        self.with(|document| document.clear_marker(server, url));
        Ok(())
    }
}

/// The owner-only credential file (`Paths::mcp_auth_file`).
pub struct FileCredentialStore {
    path: PathBuf,
}

impl FileCredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Whether anything is at the path; a symlink or an unreadable entry counts, so it is
    /// refused rather than skipped.
    pub(super) fn present(&self) -> bool {
        match std::fs::symlink_metadata(&self.path) {
            Ok(_) => true,
            Err(error) => error.kind() != std::io::ErrorKind::NotFound,
        }
    }

    /// Lock, read once, apply `operation`, and write back only when it reports a change.
    fn with<T>(&self, operation: impl FnOnce(&mut Document) -> (T, bool)) -> Result<T, OAuthError> {
        supported()?;
        let _lock = lock::lock_document(&self.path)?;
        let mut document = read(&self.path)?;
        let (result, changed) = operation(&mut document);
        if changed {
            lock::write_private(&self.path, &document)?;
        }
        Ok(result)
    }
}

impl McpCredentialStore for FileCredentialStore {
    fn get(&self, key: &CredentialKey) -> Result<Option<StoredCredential>, OAuthError> {
        self.with(|document| (document.get(key), false))
    }
    fn find(
        &self,
        server: &str,
        url: &str,
    ) -> Result<Option<(CredentialKey, StoredCredential)>, OAuthError> {
        self.with(|document| (document.find(server, url), false))
    }
    fn put(&self, key: &CredentialKey, credential: &StoredCredential) -> Result<(), OAuthError> {
        self.with(|document| (document.put(key, credential), true))
    }
    fn replace(
        &self,
        key: &CredentialKey,
        previous: &TokenSet,
        credential: &StoredCredential,
    ) -> Result<bool, OAuthError> {
        self.with(|document| {
            let replaced = document.replace(key, previous, credential);
            (replaced, replaced)
        })
    }
    /// A sibling lock file per credential, separate from the document lock, so short store
    /// operations (including `cyber mcp logout`) never wait for a token request. Its name
    /// is a digest of the server name and URL, never the URL itself.
    fn try_lock_refresh(&self, server: &str, url: &str) -> Result<Option<RefreshLock>, OAuthError> {
        supported()?;
        lock::try_lock_refresh(&self.path, server, url)
    }
    fn remove(&self, server: &str, url: &str) -> Result<bool, OAuthError> {
        self.with(|document| {
            let removed = document.remove(server, url);
            (removed, removed)
        })
    }
    fn remove_server(&self, server: &str) -> Result<usize, OAuthError> {
        self.with(|document| document.remove_server(server))
    }
    fn marker(&self, server: &str, url: &str) -> Result<AuthMarker, OAuthError> {
        self.with(|document| (document.marker(server, url), false))
    }
    fn mark(&self, server: &str, url: &str, marker: &AuthMarker) -> Result<(), OAuthError> {
        self.with(|document| ((), document.mark(server, url, marker)))
    }
    fn clear_marker(&self, server: &str, url: &str) -> Result<(), OAuthError> {
        self.with(|document| ((), document.clear_marker(server, url)))
    }
}

/// A short hex digest naming one credential (server and URL) without revealing the URL:
/// its refresh lock, and its OS keyring entries and index records.
pub(super) fn credential_digest(server: &str, url: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::new()
        .chain_update(server.as_bytes())
        .chain_update([0])
        .chain_update(url.as_bytes())
        .finalize();
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn storage(reason: &str) -> OAuthError {
    OAuthError::Storage(reason.into())
}

#[cfg(windows)]
pub(super) fn supported() -> Result<(), OAuthError> {
    Err(storage(
        "owner-only credential files are not supported on Windows yet",
    ))
}

#[cfg(not(windows))]
pub(super) fn supported() -> Result<(), OAuthError> {
    Ok(())
}

/// An absent file is an empty document; anything not owner-only and regular is refused.
fn read(path: &std::path::Path) -> Result<Document, OAuthError> {
    let Some(bytes) = lock::read_private(path)? else {
        return Ok(Document {
            version: VERSION,
            ..Document::default()
        });
    };
    let document: Document =
        serde_json::from_slice(&bytes).map_err(|_| storage("credential file is invalid"))?;
    if document.version != VERSION {
        return Err(storage("credential file version is unsupported"));
    }
    Ok(document)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    fn markers_merge_per_url_and_go_with_the_server() {
        let store = MemoryCredentialStore::default();
        let scopes = |list: &[&str]| AuthMarker {
            required_scopes: list.iter().map(|scope| (*scope).to_owned()).collect(),
            ..AuthMarker::default()
        };
        store
            .mark("remote", "https://a.test/mcp", &scopes(&["b", "a"]))
            .unwrap();
        store
            .mark("remote", "https://a.test/mcp", &scopes(&["c", "a"]))
            .unwrap();
        let registration = AuthMarker {
            needs_client_registration: true,
            ..AuthMarker::default()
        };
        store
            .mark("remote", "https://b.test/mcp", &registration)
            .unwrap();
        assert_eq!(
            store.marker("remote", "https://a.test/mcp").unwrap(),
            scopes(&["a", "b", "c"])
        );
        assert_eq!(
            store.marker("remote", "https://b.test/mcp").unwrap(),
            registration
        );
        assert_eq!(
            store.marker("other", "https://a.test/mcp").unwrap(),
            AuthMarker::default()
        );
        store.clear_marker("remote", "https://a.test/mcp").unwrap();
        assert_eq!(
            store.marker("remote", "https://a.test/mcp").unwrap(),
            AuthMarker::default()
        );
        assert_eq!(store.remove_server("remote").unwrap(), 0);
        assert_eq!(
            store.marker("remote", "https://b.test/mcp").unwrap(),
            AuthMarker::default()
        );
    }

    /// A refresh of one credential holds only its own lock, so a slow authorization server
    /// never makes another server's refresh wait (and time out into `needs_auth`).
    pub(in super::super) fn refresh_locks_are_per_credential(store: &dyn McpCredentialStore) {
        let held = store
            .try_lock_refresh("a", "https://a.test/mcp")
            .unwrap()
            .expect("first lock");
        assert!(
            store
                .try_lock_refresh("a", "https://a.test/mcp")
                .unwrap()
                .is_none()
        );
        let other = store.try_lock_refresh("b", "https://b.test/mcp").unwrap();
        assert!(other.is_some(), "another server's refresh waited");
        let same_name = store.try_lock_refresh("a", "https://a.test/other").unwrap();
        assert!(same_name.is_some(), "another URL's refresh waited");
        drop(held);
        assert!(
            store
                .try_lock_refresh("a", "https://a.test/mcp")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn memory_refresh_locks_are_per_credential() {
        refresh_locks_are_per_credential(&MemoryCredentialStore::default());
    }

    #[cfg(not(windows))]
    #[test]
    fn file_refresh_locks_are_per_credential_and_never_name_the_url() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileCredentialStore::new(dir.path().join("mcp-auth.json"));
        refresh_locks_are_per_credential(&store);
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(!name.contains("test"), "{name}");
        }
    }
}
