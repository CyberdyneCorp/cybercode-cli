//! MCP OAuth credentials in the OS keyring (macOS Keychain, Windows Credential Manager),
//! and the compile-time choice of credential backend ([`open_store`]).
//!
//! Only secrets go to the keyring: the access token, the refresh token and the client
//! secret, each split into entries of at most [`BLOB_LIMIT`] bytes (the Windows Credential
//! Manager blob limit) under the service [`SERVICE`] and the account
//! `<namespace>.<digest>.<generation>.<field>.<n>`. `<namespace>` is a random id the index
//! document creates on its first write, so two data directories (two `CYBER_HOME`s) of one
//! OS user never share, overwrite or delete each other's entries; `<digest>` names the
//! server and URL without revealing the URL (which may carry a secret `?key=`). A secret
//! needing more than [`CHUNK_CAP`] entries is refused whole, never truncated.
//!
//! Everything else lives in an owner-only, versioned index document beside the other
//! state (`<data>/mcp-auth-index.json`): the namespace, the server name, credential digest,
//! issuer, token endpoint, client id, scopes, expiry, markers and, per secret, its entry
//! count and SHA-256. It holds no token, and never records the server URL (an issuer, token
//! endpoint or scope may share its origin). The index is read first: without an index
//! record the keyring is never queried, so a host without OAuth credentials never touches
//! it.
//!
//! The keyring also binds each record: a `<namespace>.<digest>.<generation>.meta` entry
//! holds a SHA-256 over the whole record, checked before a credential is returned. An index
//! edited without access to the keyring (another local user, on Windows where the index
//! keeps the data directory's ACL) therefore fails closed: a changed issuer, token endpoint
//! or client id can never send the refresh token or client secret elsewhere. Such an edit
//! can still delete a record or a marker, which only costs a sign-in.
//!
//! No keyring call runs under the index lock, so a slow keyring (a Keychain prompt) never
//! blocks another process's store operation. A write reserves a new generation, never
//! reused, and lists it as pending under the lock; writes its entries without the lock;
//! then swaps the record in under the lock, listing the replaced generation for deletion,
//! which runs without the lock. A crash leaves either the old credential whole or the new
//! one whole, and anything it wrote listed. Removal moves the records (and the generations
//! of writes in flight) to that list under the lock and deletes their entries without it,
//! up to the cap; a generation leaves the list only once each of its entries is confirmed
//! gone, so a failed deletion is reported, never claimed done, and the next logout finishes
//! it. A write whose reservation a removal took meanwhile is not committed: the removal
//! wins. Reads take the lock only to read the record; each secret's entry count and digest
//! and the meta binding reject a torn or foreign read, and a read that fails while the
//! record changed is retried against the new record. A process keeps the credentials it
//! read or wrote in memory by record binding, so an unchanged record (the steady state of
//! the checks before every call) is answered without any keyring call. Refreshes use the
//! same per-credential lock files as the file backend.
//!
//! Migration (macOS): an earlier release kept credentials in the owner-only file
//! (`Paths::mcp_auth_file`). While the index has no record for a server and URL, reads and
//! markers fall back to that file, which is still refused when it is not owner-only and
//! regular; such a read never queries the keyring. The next write of that credential
//! (`cyber mcp auth`, or a refresh rotating it) goes to the keyring, carries its marker into
//! the index, and then removes it from the file, which the new record already shadows; the
//! file's refresh lock is held alongside this one. Removal sweeps the file, the index and
//! the keyring and fails, rather than claiming removal, if any part fails. Windows never
//! had a file credential (the file backend refuses there, lacking an owner-only ACL), so it
//! has no fallback: nothing at the legacy path is skipped, and anything there fails every
//! operation closed rather than being ignored.
//!
//! On Windows the index and lock files keep the default ACL of the data directory. That
//! is acceptable because the index holds no secret (which debug builds assert on every
//! write) and the keyring binding above protects its integrity.
//!
//! Keyring errors are reduced to fixed diagnostics: platform text never reaches a caller.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use cyber_core::paths::Paths;
use serde::{Deserialize, Serialize};

use super::store::{
    AuthMarker, CredentialKey, FileCredentialStore, McpCredentialStore, RefreshLock,
    StoredCredential, credential_digest, storage, supported,
};
use super::{OAuthClient, OAuthError, TokenSet, lock};

/// The keyring service of every MCP OAuth entry.
pub const SERVICE: &str = "cyber-mcp-oauth";
/// The most bytes one keyring entry holds: Windows Credential Manager's blob limit.
pub const BLOB_LIMIT: usize = 2560;
/// The most entries one secret spans (20 KiB, above the 16 KiB accepted for any token).
pub const CHUNK_CAP: usize = 8;
const VERSION: u32 = 1;
const TOO_LARGE: &str = "credential too large for the OS keyring";
const ALTERED: &str = "stored credential is incomplete or altered";
const INVALID: &str = "credential index is invalid";
const DELETE_FAILED: &str = "OS keyring delete failed";
/// Domain separation of the record binding (the keyring's meta entry).
const BINDING_DOMAIN: &[u8] = b"cyber-mcp-oauth-index-record-v1\n";
/// Reads of one credential while its record keeps changing under them.
const READ_ATTEMPTS: usize = 3;
/// Random bytes of an index's keyring namespace.
const NAMESPACE_BYTES: usize = 16;

/// Raw secret bytes by account name, under one service.
pub trait SecretVault: Send + Sync {
    fn put(&self, account: &str, secret: &[u8]) -> Result<(), OAuthError>;
    /// `None` when there is no such entry.
    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, OAuthError>;
    /// Deleting an absent entry succeeds. Success is not trusted on its own: the store
    /// confirms with [`Self::get`] that the entry is gone.
    fn delete(&self, account: &str) -> Result<(), OAuthError>;
}

/// An in-memory vault enforcing a per-entry size limit ([`BLOB_LIMIT`] by default): the
/// test double of the OS keyring, and a store for embedders.
pub struct MemoryVault {
    limit: usize,
    entries: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl Default for MemoryVault {
    fn default() -> Self {
        Self::with_limit(BLOB_LIMIT)
    }
}

impl MemoryVault {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            limit,
            entries: Mutex::default(),
        }
    }

    /// Every `(account, secret)`, ordered by account.
    pub fn entries(&self) -> Vec<(String, Vec<u8>)> {
        self.map()
            .iter()
            .map(|(account, secret)| (account.clone(), secret.clone()))
            .collect()
    }

    pub fn accounts(&self) -> Vec<String> {
        self.map().keys().cloned().collect()
    }

    fn map(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SecretVault for MemoryVault {
    fn put(&self, account: &str, secret: &[u8]) -> Result<(), OAuthError> {
        if secret.len() > self.limit {
            return Err(storage("keyring entry is too large"));
        }
        self.map().insert(account.into(), secret.to_vec());
        Ok(())
    }
    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, OAuthError> {
        Ok(self.map().get(account).cloned())
    }
    fn delete(&self, account: &str) -> Result<(), OAuthError> {
        self.map().remove(account);
        Ok(())
    }
}

/// The OS keyring: the login Keychain on macOS, Credential Manager on Windows. Secrets are
/// stored as bytes (`set_secret`), never as passwords, which Windows stores as UTF-16 and
/// would halve the entry limit.
/// Each vault keeps its entries under one keyring service.
#[cfg(any(target_os = "macos", windows))]
pub struct OsVault {
    service: &'static str,
}

#[cfg(any(target_os = "macos", windows))]
impl OsVault {
    /// The MCP OAuth credentials' vault ([`SERVICE`]).
    pub const MCP: Self = Self { service: SERVICE };

    pub const fn for_service(service: &'static str) -> Self {
        Self { service }
    }

    #[cfg(target_os = "macos")]
    fn entry(&self, account: &str) -> Result<keyring::Entry, OAuthError> {
        let credential =
            keyring::macos::MacCredential::new_with_target(None, self.service, account)
                .map_err(|error| os_failure("open", &error))?;
        Ok(keyring::Entry::new_with_credential(Box::new(credential)))
    }

    #[cfg(windows)]
    fn entry(&self, account: &str) -> Result<keyring::Entry, OAuthError> {
        let credential =
            keyring::windows::WinCredential::new_with_target(None, self.service, account)
                .map_err(|error| os_failure("open", &error))?;
        Ok(keyring::Entry::new_with_credential(Box::new(credential)))
    }
}

#[cfg(any(target_os = "macos", windows))]
impl SecretVault for OsVault {
    fn put(&self, account: &str, secret: &[u8]) -> Result<(), OAuthError> {
        self.entry(account)?
            .set_secret(secret)
            .map_err(|error| os_failure("write", &error))
    }
    fn get(&self, account: &str) -> Result<Option<Vec<u8>>, OAuthError> {
        match self.entry(account)?.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(os_failure("read", &error)),
        }
    }
    /// keyring 3.6.3's macOS backend discards the status of the Keychain deletion (a
    /// denied prompt, an ACL that no longer trusts this binary) and reports success; the
    /// store's read-back (see [`SecretVault::delete`]) catches that.
    fn delete(&self, account: &str) -> Result<(), OAuthError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(os_failure("delete", &error)),
        }
    }
}

/// A fixed diagnostic: keyring errors can carry attribute values and platform text.
#[cfg(any(target_os = "macos", windows))]
fn os_failure(operation: &'static str, _error: &keyring::Error) -> OAuthError {
    OAuthError::Storage(format!("OS keyring {operation} failed"))
}

/// The credential store of this platform, chosen at compile time and never by
/// configuration or environment: the OS keyring on macOS and Windows, the owner-only file
/// (`Paths::mcp_auth_file`) elsewhere. Linux has no keyring backend without libdbus, which
/// the release builds do not link, so it keeps the file.
pub fn open_store(paths: &Paths) -> Arc<dyn McpCredentialStore> {
    // On Windows the file backend refuses, so the legacy path is never a fallback there:
    // an absent file is skipped and a present one fails closed (see `legacy_in_use`).
    #[cfg(any(target_os = "macos", windows))]
    {
        Arc::new(
            VaultCredentialStore::new(index_file(paths), Arc::new(OsVault::MCP))
                .with_legacy(FileCredentialStore::new(paths.mcp_auth_file())),
        )
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Arc::new(FileCredentialStore::new(paths.mcp_auth_file()))
    }
}

/// Which backend [`open_store`] selects: `keyring` or `file`.
pub fn backend_name() -> &'static str {
    if cfg!(any(target_os = "macos", windows)) {
        "keyring"
    } else {
        "file"
    }
}

/// The OS keyring backend's index document.
pub fn index_file(paths: &Paths) -> PathBuf {
    paths.data.join("mcp-auth-index.json")
}

/// Which secret of a credential an entry holds.
#[derive(Clone, Copy)]
enum Field {
    Access,
    Refresh,
    Secret,
}

const FIELDS: [Field; 3] = [Field::Access, Field::Refresh, Field::Secret];

impl Field {
    fn name(self) -> &'static str {
        match self {
            Field::Access => "access",
            Field::Refresh => "refresh",
            Field::Secret => "secret",
        }
    }
}

/// Where one generation of a credential lives in the keyring.
#[derive(Clone, Copy)]
struct Slot<'a> {
    namespace: &'a str,
    digest: &'a str,
    generation: u64,
}

impl Slot<'_> {
    fn account(&self, field: Field, chunk: usize) -> String {
        format!(
            "{}.{}.{}.{}.{chunk}",
            self.namespace,
            self.digest,
            self.generation,
            field.name()
        )
    }

    /// The entry binding this generation to its index record.
    fn meta(&self) -> String {
        format!(
            "{}.{}.{}.meta",
            self.namespace, self.digest, self.generation
        )
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(bytes))
}

/// Where one secret is: how many entries and the digest of their concatenation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sealed {
    chunks: usize,
    sha256: String,
}

impl Sealed {
    fn of(value: &str) -> Self {
        Self {
            chunks: value.len().div_ceil(BLOB_LIMIT),
            sha256: sha256(value.as_bytes()),
        }
    }

    fn holds(&self, value: &str) -> bool {
        *self == Self::of(value)
    }
}

fn holds_optional(sealed: Option<&Sealed>, value: Option<&str>) -> bool {
    match (sealed, value) {
        (None, None) => true,
        (Some(sealed), Some(value)) => sealed.holds(value),
        _ => false,
    }
}

/// The secrets of a credential, by field.
fn secrets(credential: &StoredCredential) -> [(Field, Option<&str>); 3] {
    [
        (Field::Access, Some(credential.tokens.access_token.as_str())),
        (Field::Refresh, credential.tokens.refresh_token.as_deref()),
        (Field::Secret, credential.client.client_secret.as_deref()),
    ]
}

/// Shorter secrets are matched only as a whole value, so the guard below never mistakes a
/// test token like `t` for a leak inside unrelated text.
const GUARD_MIN: usize = 8;

/// Index fields naming the authorization server or scopes, which may legitimately share
/// the server URL (a server that is its own authorization server, an `<url>/.default`
/// scope), of this or any other record.
const URL_FIELDS: [&str; 4] = ["issuer", "token_endpoint", "scopes", "required_scopes"];

/// Whether the serialized `index` holds a secret of `credential` anywhere, or the URL of
/// `key` outside [`URL_FIELDS`]. The index is not a secret store (on Windows it keeps the
/// default ACL), so committing one that does is a bug, which debug builds assert against.
fn reveals(index: &serde_json::Value, key: &CredentialKey, credential: &StoredCredential) -> bool {
    let secrets: Vec<&str> = secrets(credential)
        .into_iter()
        .filter_map(|(_, value)| value)
        .filter(|value| !value.is_empty())
        .collect();
    let leaks = |text: &str, url: bool| {
        (url && !key.url.is_empty() && text.contains(key.url.as_str()))
            || secrets.iter().any(|secret| {
                text == *secret || (secret.len() >= GUARD_MIN && text.contains(secret))
            })
    };
    let mut pending = vec![(index, true)];
    while let Some((value, url)) = pending.pop() {
        match value {
            serde_json::Value::String(text) if leaks(text, url) => return true,
            serde_json::Value::Array(items) => pending.extend(items.iter().map(|item| (item, url))),
            serde_json::Value::Object(fields) => pending.extend(
                fields
                    .iter()
                    .map(|(name, field)| (field, url && !URL_FIELDS.contains(&name.as_str()))),
            ),
            _ => {}
        }
    }
    false
}

/// Refuse a credential with a secret beyond [`CHUNK_CAP`] entries before anything is
/// written.
fn check_size(credential: &StoredCredential) -> Result<(), OAuthError> {
    let fits = secrets(credential)
        .iter()
        .all(|(_, value)| value.is_none_or(|value| value.len() <= BLOB_LIMIT * CHUNK_CAP));
    if fits {
        Ok(())
    } else {
        Err(storage(TOO_LARGE))
    }
}

/// One credential's index record; its secrets and its binding are in the keyring.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Indexed {
    server: String,
    /// `credential_digest(server, url)`.
    credential: String,
    issuer: String,
    generation: u64,
    access_token: Sealed,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<Sealed>,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    issued_at: Option<u64>,
    #[serde(default)]
    scopes: Vec<String>,
    client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<Sealed>,
    token_endpoint: String,
}

impl Indexed {
    fn new(key: &CredentialKey, generation: u64, credential: &StoredCredential) -> Self {
        let tokens = &credential.tokens;
        Self {
            server: key.server.clone(),
            credential: credential_digest(&key.server, &key.url),
            issuer: key.issuer.clone(),
            generation,
            access_token: Sealed::of(&tokens.access_token),
            refresh_token: tokens.refresh_token.as_deref().map(Sealed::of),
            expires_at: tokens.expires_at,
            issued_at: tokens.issued_at,
            scopes: tokens.scopes.clone(),
            client_id: credential.client.client_id.clone(),
            client_secret: credential.client.client_secret.as_deref().map(Sealed::of),
            token_endpoint: credential.token_endpoint.clone(),
        }
    }

    fn at(&self, server: &str, digest: &str) -> bool {
        self.server == server && self.credential == digest
    }

    /// What the keyring's meta entry of this generation holds: a SHA-256 over the record.
    fn binding(&self) -> Result<String, OAuthError> {
        let bytes = serde_json::to_vec(self).map_err(|_| storage("credential encoding failed"))?;
        Ok(sha256(&[BINDING_DOMAIN, &bytes].concat()))
    }

    fn key(&self, url: &str) -> CredentialKey {
        CredentialKey {
            server: self.server.clone(),
            url: url.into(),
            issuer: self.issuer.clone(),
        }
    }

    fn is(&self, key: &CredentialKey) -> bool {
        self.at(&key.server, &credential_digest(&key.server, &key.url)) && self.issuer == key.issuer
    }

    /// Whether this record holds exactly `tokens`, comparing secrets by digest.
    fn holds(&self, tokens: &TokenSet) -> bool {
        self.access_token.holds(&tokens.access_token)
            && holds_optional(self.refresh_token.as_ref(), tokens.refresh_token.as_deref())
            && self.expires_at == tokens.expires_at
            && self.issued_at == tokens.issued_at
            && self.scopes == tokens.scopes
    }

    fn sealed(&self, field: Field) -> Option<&Sealed> {
        match field {
            Field::Access => Some(&self.access_token),
            Field::Refresh => self.refresh_token.as_ref(),
            Field::Secret => self.client_secret.as_ref(),
        }
    }
}

/// A generation whose entries no record lists: a write in flight (`pending`, or one that
/// crashed before its commit), or one replaced or removed whose entries are not all
/// confirmed deleted yet.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Unlisted {
    server: String,
    credential: String,
    generation: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pending: bool,
}

impl Unlisted {
    fn of(&self, server: &str, digest: Option<&str>) -> bool {
        self.server == server && digest.is_none_or(|digest| self.credential == digest)
    }

    fn is(&self, server: &str, digest: &str, generation: u64) -> bool {
        self.of(server, Some(digest)) && self.generation == generation
    }
}

impl From<Indexed> for Unlisted {
    fn from(entry: Indexed) -> Self {
        Self {
            server: entry.server,
            credential: entry.credential,
            generation: entry.generation,
            pending: false,
        }
    }
}

/// One server/URL marker; no secrets.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexedMarker {
    server: String,
    credential: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    needs_client_registration: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    required_scopes: Vec<String>,
}

impl IndexedMarker {
    fn marker(&self) -> AuthMarker {
        AuthMarker {
            needs_client_registration: self.needs_client_registration,
            required_scopes: self.required_scopes.clone(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    version: u32,
    /// This index's keyring namespace (random hex), created by its first write.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    namespace: String,
    /// The last generation reserved; generations are never reused.
    #[serde(default)]
    generation: u64,
    credentials: Vec<Indexed>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    markers: Vec<IndexedMarker>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    unlisted: Vec<Unlisted>,
}

impl Index {
    /// The keyring namespace; an index naming keyring entries without a valid one is
    /// refused.
    fn namespace(&self) -> Result<&str, OAuthError> {
        let valid = self.namespace.len() == 2 * NAMESPACE_BYTES
            && self
                .namespace
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if valid {
            Ok(&self.namespace)
        } else {
            Err(storage(INVALID))
        }
    }

    /// A new generation for `server` at the URL whose digest is `digest`, listed as a
    /// pending write, with the namespace (created on first use).
    fn reserve(&mut self, server: &str, digest: &str) -> Result<(String, u64), OAuthError> {
        if self.namespace.is_empty() {
            let mut bytes = [0u8; NAMESPACE_BYTES];
            getrandom::fill(&mut bytes).map_err(|_| OAuthError::Random)?;
            self.namespace = hex(&bytes);
        }
        let namespace = self.namespace()?.to_owned();
        let used = self
            .credentials
            .iter()
            .map(|entry| entry.generation)
            .chain(self.unlisted.iter().map(|unlisted| unlisted.generation))
            .fold(self.generation, u64::max);
        self.generation = used.checked_add(1).ok_or_else(|| storage(INVALID))?;
        self.unlisted.push(Unlisted {
            server: server.into(),
            credential: digest.into(),
            generation: self.generation,
            pending: true,
        });
        Ok((namespace, self.generation))
    }

    /// Whether the write that reserved `generation` still owns its reservation.
    fn pending(&self, server: &str, digest: &str, generation: u64) -> bool {
        self.unlisted
            .iter()
            .any(|unlisted| unlisted.pending && unlisted.is(server, digest, generation))
    }

    /// Make `entry` the record for its server and URL: the record it replaces is listed
    /// for deletion and its own reservation is settled.
    fn commit(&mut self, entry: Indexed) {
        let replaced = self.take(&entry.server, Some(&entry.credential));
        self.unlisted.retain(|unlisted| {
            !(unlisted.pending && unlisted.is(&entry.server, &entry.credential, entry.generation))
        });
        self.unlisted
            .extend(replaced.into_iter().map(Unlisted::from));
        self.credentials.push(entry);
    }

    /// List `generation`, whose write ended without a commit, for deletion.
    fn abandon(&mut self, server: &str, digest: &str, generation: u64) {
        self.unlisted
            .retain(|unlisted| !unlisted.is(server, digest, generation));
        self.unlisted.push(Unlisted {
            server: server.into(),
            credential: digest.into(),
            generation,
            pending: false,
        });
    }

    /// Move every record of `server` matching `digest` (any when `None`), and the
    /// reservations of writes in flight, to the deletion list. Whether anything moved.
    fn unlist(&mut self, server: &str, digest: Option<&str>) -> bool {
        let taken = self.take(server, digest);
        let mut changed = !taken.is_empty();
        for unlisted in &mut self.unlisted {
            if unlisted.pending && unlisted.of(server, digest) {
                unlisted.pending = false;
                changed = true;
            }
        }
        self.unlisted.extend(taken.into_iter().map(Unlisted::from));
        changed
    }

    /// The generations of `server` (matching `digest`) listed for deletion.
    fn sweepable(&self, server: &str, digest: Option<&str>) -> Vec<Unlisted> {
        self.unlisted
            .iter()
            .filter(|unlisted| !unlisted.pending && unlisted.of(server, digest))
            .cloned()
            .collect()
    }

    /// Drop `swept` from the deletion list; whether anything was dropped.
    fn forget(&mut self, swept: &[Unlisted]) -> bool {
        let before = self.unlisted.len();
        self.unlisted
            .retain(|unlisted| unlisted.pending || !swept.contains(unlisted));
        self.unlisted.len() != before
    }

    /// The record for `server` at the URL whose digest is `digest`.
    fn entry(&self, server: &str, digest: &str) -> Option<&Indexed> {
        self.credentials
            .iter()
            .find(|entry| entry.at(server, digest))
    }

    /// Remove and return every record for `server` matching `digest` (any when `None`).
    fn take(&mut self, server: &str, digest: Option<&str>) -> Vec<Indexed> {
        let (taken, kept) = std::mem::take(&mut self.credentials)
            .into_iter()
            .partition(|entry| {
                entry.server == server && digest.is_none_or(|digest| entry.credential == digest)
            });
        self.credentials = kept;
        taken
    }

    fn marker(&self, server: &str, digest: &str) -> AuthMarker {
        self.markers
            .iter()
            .find(|marked| marked.server == server && marked.credential == digest)
            .map(IndexedMarker::marker)
            .unwrap_or_default()
    }

    /// Whether the merge changed anything.
    fn mark(&mut self, server: &str, digest: &str, marker: &AuthMarker) -> bool {
        let mut merged = self.marker(server, digest);
        let before = merged.clone();
        merged.merge(marker);
        if merged == before {
            return false;
        }
        self.clear_marker(server, digest);
        self.markers.push(IndexedMarker {
            server: server.into(),
            credential: digest.into(),
            needs_client_registration: merged.needs_client_registration,
            required_scopes: merged.required_scopes,
        });
        true
    }

    /// Whether a marker was removed.
    fn clear_marker(&mut self, server: &str, digest: &str) -> bool {
        let before = self.markers.len();
        self.markers
            .retain(|marked| !(marked.server == server && marked.credential == digest));
        self.markers.len() != before
    }
}

/// An absent index is empty; anything not owner-only, regular and of this version is
/// refused.
fn read_index(path: &Path) -> Result<Index, OAuthError> {
    let Some(bytes) = lock::read_private(path)? else {
        return Ok(Index {
            version: VERSION,
            ..Index::default()
        });
    };
    let index: Index = serde_json::from_slice(&bytes).map_err(|_| storage(INVALID))?;
    if index.version != VERSION {
        return Err(storage("credential index version is unsupported"));
    }
    Ok(index)
}

/// Every server the index at `index` names (credentials, markers and entries pending
/// deletion), for `remove_server` on each: `cyber uninstall` sweeps the keyring from
/// cyber's own records, so an absent index means no keyring call.
pub fn indexed_servers(index: &Path) -> Result<BTreeSet<String>, OAuthError> {
    let index = read_index(index)?;
    let credentials = index.credentials.iter().map(|record| &record.server);
    let markers = index.markers.iter().map(|marker| &marker.server);
    let unlisted = index.unlisted.iter().map(|entry| &entry.server);
    Ok(credentials
        .chain(markers)
        .chain(unlisted)
        .cloned()
        .collect())
}

/// Whether the legacy file takes part: never when nothing is at its path, so an absent
/// file is no obstacle even where the file backend is refused; otherwise only where
/// `supported` (the file backend's platform check) allows, and its refusal is surfaced.
fn legacy_in_use(
    present: bool,
    supported: impl FnOnce() -> Result<(), OAuthError>,
) -> Result<bool, OAuthError> {
    if !present {
        return Ok(false);
    }
    supported().map(|()| true)
}

/// What the index holds for one server and URL, read under its lock.
enum Lookup {
    /// The record, with the index's keyring namespace.
    Indexed(String, Indexed),
    /// No record: the legacy file's answer, read under the same lock so a concurrent
    /// migration out of it is never mistaken for a removal.
    Elsewhere(Option<(CredentialKey, StoredCredential)>),
}

type Opened = HashMap<String, (String, StoredCredential)>;

/// Credentials with their secrets in a [`SecretVault`] and everything else in an
/// owner-only index document.
pub struct VaultCredentialStore {
    index: PathBuf,
    vault: Arc<dyn SecretVault>,
    legacy: Option<FileCredentialStore>,
    /// By credential digest: the binding of the record last read or written and its
    /// credential, so an unchanged record needs no keyring call.
    opened: Mutex<Opened>,
}

impl VaultCredentialStore {
    /// `index` is the index document (normally [`index_file`]).
    pub fn new(index: PathBuf, vault: Arc<dyn SecretVault>) -> Self {
        Self {
            index,
            vault,
            legacy: None,
            opened: Mutex::default(),
        }
    }

    /// Also serve credentials and markers from `legacy`, the owner-only file an earlier
    /// release wrote (normally `Paths::mcp_auth_file`), while the index has no record of
    /// them; the next write of such a credential moves it here (see the module docs).
    pub fn with_legacy(mut self, legacy: FileCredentialStore) -> Self {
        self.legacy = Some(legacy);
        self
    }

    /// Lock, read the index once, apply `operation`, and write back when it reports a
    /// change. `operation` never calls the keyring.
    fn with<T>(
        &self,
        operation: impl FnOnce(&mut Index) -> Result<(T, bool), OAuthError>,
    ) -> Result<T, OAuthError> {
        let _lock = lock::lock_document(&self.index)?;
        let mut index = read_index(&self.index)?;
        let (result, changed) = operation(&mut index)?;
        if changed {
            lock::write_private(&self.index, &index)?;
        }
        Ok(result)
    }

    /// Run `operation` on the index under its lock without writing it back.
    fn read<T>(
        &self,
        operation: impl FnOnce(&Index) -> Result<T, OAuthError>,
    ) -> Result<T, OAuthError> {
        self.with(|index| Ok((operation(index)?, false)))
    }

    // Vault access with fixed diagnostics, whatever the vault reported.
    fn vault_put(&self, account: &str, secret: &[u8]) -> Result<(), OAuthError> {
        self.vault
            .put(account, secret)
            .map_err(|_| storage("OS keyring write failed"))
    }
    fn vault_get(&self, account: &str) -> Result<Option<Vec<u8>>, OAuthError> {
        self.vault
            .get(account)
            .map_err(|_| storage("OS keyring read failed"))
    }
    /// A deletion counts only once a read finds nothing: a backend that reports success
    /// for a deletion it did not make (see [`OsVault`]) fails here.
    fn vault_delete(&self, account: &str) -> Result<(), OAuthError> {
        self.vault
            .delete(account)
            .map_err(|_| storage(DELETE_FAILED))?;
        match self.vault.get(account) {
            Ok(None) => Ok(()),
            _ => Err(storage(DELETE_FAILED)),
        }
    }

    /// Write every secret of `credential` into `slot`, then the record's `binding`.
    fn seal(
        &self,
        slot: Slot<'_>,
        credential: &StoredCredential,
        binding: &str,
    ) -> Result<(), OAuthError> {
        for (field, value) in secrets(credential) {
            let Some(value) = value else { continue };
            for (n, chunk) in value.as_bytes().chunks(BLOB_LIMIT).enumerate() {
                self.vault_put(&slot.account(field, n), chunk)?;
            }
        }
        self.vault_put(&slot.meta(), binding.as_bytes())
    }

    /// One secret, whole and matching its digest, or an error; never a truncated value.
    fn reveal(&self, slot: Slot<'_>, field: Field, sealed: &Sealed) -> Result<String, OAuthError> {
        if sealed.chunks > CHUNK_CAP {
            return Err(storage(ALTERED));
        }
        let mut bytes = Vec::new();
        for n in 0..sealed.chunks {
            let chunk = self
                .vault_get(&slot.account(field, n))?
                .filter(|chunk| chunk.len() <= BLOB_LIMIT)
                .ok_or_else(|| storage(ALTERED))?;
            bytes.extend_from_slice(&chunk);
        }
        if sha256(&bytes) != sealed.sha256 {
            return Err(storage(ALTERED));
        }
        String::from_utf8(bytes).map_err(|_| storage(ALTERED))
    }

    fn reveal_field(
        &self,
        slot: Slot<'_>,
        entry: &Indexed,
        field: Field,
    ) -> Result<Option<String>, OAuthError> {
        entry
            .sealed(field)
            .map(|sealed| self.reveal(slot, field, sealed))
            .transpose()
    }

    fn opened(&self) -> std::sync::MutexGuard<'_, Opened> {
        self.opened.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn remember(&self, entry: &Indexed, binding: String, credential: &StoredCredential) {
        self.opened()
            .insert(entry.credential.clone(), (binding, credential.clone()));
    }

    /// The credential `entry` names: from memory while the record is the one last read or
    /// written here, otherwise from the keyring after checking the record's binding there.
    fn open(&self, namespace: &str, entry: &Indexed) -> Result<StoredCredential, OAuthError> {
        let binding = entry.binding()?;
        let known = self
            .opened()
            .get(&entry.credential)
            .filter(|(known, _)| *known == binding)
            .map(|(_, credential)| credential.clone());
        if let Some(credential) = known {
            return Ok(credential);
        }
        let slot = Slot {
            namespace,
            digest: &entry.credential,
            generation: entry.generation,
        };
        let bound = self.vault_get(&slot.meta())?;
        if bound.as_deref() != Some(binding.as_bytes()) {
            return Err(storage(ALTERED));
        }
        let credential = self.unseal(slot, entry)?;
        self.remember(entry, binding, &credential);
        Ok(credential)
    }

    fn unseal(&self, slot: Slot<'_>, entry: &Indexed) -> Result<StoredCredential, OAuthError> {
        let access_token = self
            .reveal_field(slot, entry, Field::Access)?
            .ok_or_else(|| storage(ALTERED))?;
        Ok(StoredCredential {
            tokens: TokenSet {
                access_token,
                refresh_token: self.reveal_field(slot, entry, Field::Refresh)?,
                expires_at: entry.expires_at,
                issued_at: entry.issued_at,
                scopes: entry.scopes.clone(),
            },
            client: OAuthClient {
                client_id: entry.client_id.clone(),
                client_secret: self.reveal_field(slot, entry, Field::Secret)?,
            },
            token_endpoint: entry.token_endpoint.clone(),
        })
    }

    /// Delete every possible entry of one generation, up to the cap, binding last.
    fn sweep(&self, slot: Slot<'_>) -> Result<(), OAuthError> {
        for field in FIELDS {
            for n in 0..CHUNK_CAP {
                self.vault_delete(&slot.account(field, n))?;
            }
        }
        self.vault_delete(&slot.meta())
    }

    /// Delete the listed generations of `server` (at the URL whose digest is `digest`, or
    /// any URL) without the index lock, then drop those deleted from the list. How many
    /// credentials they belonged to, or the first failure, which leaves the rest listed.
    fn sweep_unlisted(&self, server: &str, digest: Option<&str>) -> Result<usize, OAuthError> {
        let (namespace, targets) = self.read(|index| {
            let targets = index.sweepable(server, digest);
            let namespace = if targets.is_empty() {
                String::new()
            } else {
                index.namespace()?.to_owned()
            };
            Ok((namespace, targets))
        })?;
        let mut swept = Vec::new();
        let mut failure = None;
        for target in targets {
            let slot = Slot {
                namespace: &namespace,
                digest: &target.credential,
                generation: target.generation,
            };
            match self.sweep(slot) {
                Ok(()) => swept.push(target),
                Err(error) => failure = failure.or(Some(error)),
            }
        }
        if !swept.is_empty() {
            self.with(|index| Ok(((), index.forget(&swept))))?;
        }
        if let Some(error) = failure {
            return Err(error);
        }
        let credentials: BTreeSet<&str> = swept.iter().map(|t| t.credential.as_str()).collect();
        Ok(credentials.len())
    }

    /// Store `credential` for its server and URL; with `previous`, only while the stored
    /// credential still holds it. A new generation is reserved under the index lock, its
    /// entries are written without it, the record is swapped in under it, and the
    /// generations it replaced are deleted without it. Whether it was stored.
    fn write(
        &self,
        key: &CredentialKey,
        credential: &StoredCredential,
        previous: Option<&TokenSet>,
    ) -> Result<bool, OAuthError> {
        check_size(credential)?;
        let digest = credential_digest(&key.server, &key.url);
        let reserved = self.with(|index| {
            self.legacy()?;
            if let Some(previous) = previous
                && !self.current(index, key, &digest, previous)?
            {
                return Ok((None, false));
            }
            index
                .reserve(&key.server, &digest)
                .map(|reserved| (Some(reserved), true))
        })?;
        let Some((namespace, generation)) = reserved else {
            return Ok(false);
        };
        let entry = Indexed::new(key, generation, credential);
        // Checked before anything is written, so the index never names a secret.
        debug_assert!(
            !serde_json::to_value(&entry).is_ok_and(|value| reveals(&value, key, credential)),
            "the credential index would hold a secret or a server URL"
        );
        let binding = entry.binding()?;
        let slot = Slot {
            namespace: &namespace,
            digest: &digest,
            generation,
        };
        let written = self
            .seal(slot, credential, &binding)
            .and_then(|()| self.with(|index| self.commit(index, key, &entry, previous)));
        let outcome = match written {
            Ok(true) => {
                self.remember(&entry, binding, credential);
                self.forget_legacy(key).map(|()| true)
            }
            other => {
                // Best effort: a pending reservation is swept by the next logout anyway.
                let _ = self.with(|index| {
                    index.abandon(&key.server, &digest, generation);
                    Ok(((), true))
                });
                other
            }
        };
        // Best effort: what is not deleted stays listed for the next write or logout.
        let _ = self.sweep_unlisted(&key.server, Some(&digest));
        outcome
    }

    /// Under the index lock: swap in `entry` unless a removal took its reservation (the
    /// removal wins) or, with `previous`, the stored credential changed meanwhile. The
    /// legacy file's marker is carried into the index.
    fn commit(
        &self,
        index: &mut Index,
        key: &CredentialKey,
        entry: &Indexed,
        previous: Option<&TokenSet>,
    ) -> Result<(bool, bool), OAuthError> {
        if !index.pending(&entry.server, &entry.credential, entry.generation) {
            return match previous {
                Some(_) => Ok((false, false)),
                None => Err(storage("credential was removed while it was written")),
            };
        }
        if let Some(previous) = previous
            && !self.current(index, key, &entry.credential, previous)?
        {
            return Ok((false, false));
        }
        if let Some(legacy) = self.legacy()? {
            let marker = legacy.marker(&key.server, &key.url)?;
            index.mark(&key.server, &entry.credential, &marker);
        }
        index.commit(entry.clone());
        self.opened().remove(&entry.credential);
        Ok((true, true))
    }

    /// Whether the stored credential under exactly `key` holds `previous`.
    fn current(
        &self,
        index: &Index,
        key: &CredentialKey,
        digest: &str,
        previous: &TokenSet,
    ) -> Result<bool, OAuthError> {
        match index.entry(&key.server, digest) {
            Some(entry) => Ok(entry.is(key) && entry.holds(previous)),
            None => self.legacy_holds(key, previous),
        }
    }

    /// The legacy file store, when there is anything at its path. A file that fails the
    /// owner-only or regular-file checks is still returned, so every use refuses it; where
    /// the file backend is unsupported (Windows), a present file fails closed here.
    fn legacy(&self) -> Result<Option<&FileCredentialStore>, OAuthError> {
        let Some(legacy) = self.legacy.as_ref() else {
            return Ok(None);
        };
        Ok(legacy_in_use(legacy.present(), supported)?.then_some(legacy))
    }

    /// Whether the legacy file holds exactly `previous` under `key`.
    fn legacy_holds(&self, key: &CredentialKey, previous: &TokenSet) -> Result<bool, OAuthError> {
        let Some(legacy) = self.legacy()? else {
            return Ok(false);
        };
        Ok(legacy
            .get(key)?
            .is_some_and(|stored| stored.tokens == *previous))
    }

    /// Forget `key`'s server and URL in the legacy file after the index took it over (its
    /// record shadows the file already). A failure is reported, though the keyring holds
    /// the new credential.
    fn forget_legacy(&self, key: &CredentialKey) -> Result<(), OAuthError> {
        let Some(legacy) = self.legacy()? else {
            return Ok(());
        };
        legacy.remove(&key.server, &key.url)?;
        legacy.clear_marker(&key.server, &key.url)
    }

    /// The record for `server` at `url`, or the legacy file's answer when there is none.
    fn lookup(&self, server: &str, url: &str, digest: &str) -> Result<Lookup, OAuthError> {
        self.read(|index| match index.entry(server, digest) {
            Some(entry) => Ok(Lookup::Indexed(
                index.namespace()?.to_owned(),
                entry.clone(),
            )),
            None => self
                .legacy()?
                .map_or(Ok(None), |legacy| legacy.find(server, url))
                .map(Lookup::Elsewhere),
        })
    }

    /// The credential for `server` at `url`. The index lock is held only to read the
    /// record; the keyring is read without it. A read that fails while the record changed
    /// (a concurrent write or removal) is retried against the new record; one that fails
    /// against an unchanged record fails closed.
    fn resolve(
        &self,
        server: &str,
        url: &str,
    ) -> Result<Option<(CredentialKey, StoredCredential)>, OAuthError> {
        let digest = credential_digest(server, url);
        let (mut namespace, mut entry) = match self.lookup(server, url, &digest)? {
            Lookup::Indexed(namespace, entry) => (namespace, entry),
            Lookup::Elsewhere(found) => return Ok(found),
        };
        for _ in 1..READ_ATTEMPTS {
            let error = match self.open(&namespace, &entry) {
                Ok(credential) => return Ok(Some((entry.key(url), credential))),
                Err(error) => error,
            };
            match self.lookup(server, url, &digest)? {
                Lookup::Indexed(now_namespace, now)
                    if (&now_namespace, &now) != (&namespace, &entry) =>
                {
                    (namespace, entry) = (now_namespace, now);
                }
                Lookup::Indexed(..) => return Err(error),
                Lookup::Elsewhere(found) => return Ok(found),
            }
        }
        Ok(Some((entry.key(url), self.open(&namespace, &entry)?)))
    }
}

impl McpCredentialStore for VaultCredentialStore {
    /// An index record answers for its server and URL; without one, the legacy file does.
    fn get(&self, key: &CredentialKey) -> Result<Option<StoredCredential>, OAuthError> {
        Ok(self
            .resolve(&key.server, &key.url)?
            .filter(|(found, _)| found.issuer == key.issuer)
            .map(|(_, credential)| credential))
    }
    fn find(
        &self,
        server: &str,
        url: &str,
    ) -> Result<Option<(CredentialKey, StoredCredential)>, OAuthError> {
        self.resolve(server, url)
    }
    /// The index (or the legacy file) answers; the keyring is never read.
    fn contains(&self, server: &str, url: &str) -> Result<bool, OAuthError> {
        let digest = credential_digest(server, url);
        Ok(match self.lookup(server, url, &digest)? {
            Lookup::Indexed(..) => true,
            Lookup::Elsewhere(found) => found.is_some(),
        })
    }
    fn put(&self, key: &CredentialKey, credential: &StoredCredential) -> Result<(), OAuthError> {
        self.write(key, credential, None).map(|_| ())
    }
    fn replace(
        &self,
        key: &CredentialKey,
        previous: &TokenSet,
        credential: &StoredCredential,
    ) -> Result<bool, OAuthError> {
        self.write(key, credential, Some(previous))
    }
    /// With a legacy file, its refresh lock is held too, so a process still using the file
    /// backend never refreshes the same credential concurrently.
    fn try_lock_refresh(&self, server: &str, url: &str) -> Result<Option<RefreshLock>, OAuthError> {
        let Some(own) = lock::try_lock_refresh(&self.index, server, url)? else {
            return Ok(None);
        };
        let Some(legacy) = self.legacy()? else {
            return Ok(Some(own));
        };
        Ok(legacy
            .try_lock_refresh(server, url)?
            .map(|old| RefreshLock(Box::new((own, old)))))
    }
    /// The legacy file is cleared and the record (with any write in flight) moved to the
    /// deletion list under the index lock, so the credential is unusable at once; its
    /// entries are then deleted without the lock. A deletion that fails is reported and
    /// stays listed for the next logout, never claimed removed.
    fn remove(&self, server: &str, url: &str) -> Result<bool, OAuthError> {
        let digest = credential_digest(server, url);
        let legacy = self.with(|index| {
            let legacy = match self.legacy()? {
                Some(legacy) => legacy.remove(server, url)?,
                None => false,
            };
            Ok((legacy, index.unlist(server, Some(&digest))))
        })?;
        self.opened().clear();
        Ok(self.sweep_unlisted(server, Some(&digest))? > 0 || legacy)
    }
    /// As [`Self::remove`] for every URL; the count adds the legacy file's credentials.
    fn remove_server(&self, server: &str) -> Result<usize, OAuthError> {
        let legacy = self.with(|index| {
            let legacy = match self.legacy()? {
                Some(legacy) => legacy.remove_server(server)?,
                None => 0,
            };
            let unlisted = index.unlist(server, None);
            let markers = index.markers.len();
            index.markers.retain(|marked| marked.server != server);
            Ok((legacy, unlisted || markers != index.markers.len()))
        })?;
        self.opened().clear();
        Ok(legacy + self.sweep_unlisted(server, None)?)
    }
    /// The index's marker merged with the legacy file's.
    fn marker(&self, server: &str, url: &str) -> Result<AuthMarker, OAuthError> {
        let digest = credential_digest(server, url);
        self.read(|index| {
            let mut marker = index.marker(server, &digest);
            if let Some(legacy) = self.legacy()? {
                marker.merge(&legacy.marker(server, url)?);
            }
            Ok(marker)
        })
    }
    fn mark(&self, server: &str, url: &str, marker: &AuthMarker) -> Result<(), OAuthError> {
        let digest = credential_digest(server, url);
        self.with(|index| Ok(((), index.mark(server, &digest, marker))))
    }
    fn clear_marker(&self, server: &str, url: &str) -> Result<(), OAuthError> {
        let digest = credential_digest(server, url);
        self.with(|index| {
            if let Some(legacy) = self.legacy()? {
                legacy.clear_marker(server, url)?;
            }
            Ok(((), index.clear_marker(server, &digest)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://mcp.example.test/mcp?key=QUERY-SECRET";

    fn key(server: &str, url: &str, issuer: &str) -> CredentialKey {
        CredentialKey {
            server: server.into(),
            url: url.into(),
            issuer: issuer.into(),
        }
    }

    fn credential(access: &str) -> StoredCredential {
        StoredCredential {
            tokens: TokenSet {
                access_token: access.into(),
                refresh_token: Some(format!("{access}-refresh")),
                expires_at: Some(4_000_000_000),
                issued_at: None,
                scopes: vec!["mcp.read".into()],
            },
            client: OAuthClient {
                client_id: "dyn-client".into(),
                client_secret: Some("CLIENT-SECRET".into()),
            },
            token_endpoint: "https://as.example.test/token".into(),
        }
    }

    fn store(dir: &tempfile::TempDir) -> (VaultCredentialStore, Arc<MemoryVault>) {
        let vault = Arc::new(MemoryVault::default());
        let store =
            VaultCredentialStore::new(dir.path().join("mcp-auth-index.json"), vault.clone());
        (store, vault)
    }

    /// A second store over the same index and vault, as another process opens it.
    fn reader(dir: &tempfile::TempDir, vault: Arc<dyn SecretVault>) -> VaultCredentialStore {
        VaultCredentialStore::new(dir.path().join("mcp-auth-index.json"), vault)
    }

    fn index_text(dir: &tempfile::TempDir) -> String {
        std::fs::read_to_string(dir.path().join("mcp-auth-index.json")).unwrap_or_default()
    }

    fn field_accounts(vault: &MemoryVault, field: &str) -> Vec<String> {
        vault
            .accounts()
            .into_iter()
            .filter(|account| account.contains(&format!(".{field}.")))
            .collect()
    }

    #[test]
    fn a_large_token_is_split_into_bounded_entries_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let (store, vault) = store(&dir);
        let access = "a".repeat(6000);
        let key = key("remote", URL, "https://as.example.test/");
        store.put(&key, &credential(&access)).unwrap();
        assert_eq!(field_accounts(&vault, "access").len(), 3);
        assert!(
            vault
                .entries()
                .iter()
                .all(|(_, secret)| secret.len() <= BLOB_LIMIT)
        );
        let stored = store.get(&key).unwrap().unwrap();
        assert_eq!(stored, credential(&access));
        assert_eq!(
            store.find("remote", URL).unwrap().unwrap(),
            (key, credential(&access))
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_index_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = store(&dir);
        store
            .put(
                &key("remote", URL, "https://as.example.test/"),
                &credential("t"),
            )
            .unwrap();
        let path = dir.path().join("mcp-auth-index.json");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            store.find("remote", URL),
            Err(OAuthError::Storage(_))
        ));
    }

    #[test]
    fn a_credential_over_the_cap_is_refused_whole() {
        let dir = tempfile::tempdir().unwrap();
        let (store, vault) = store(&dir);
        let key = key("remote", URL, "https://as.example.test/");
        store.put(&key, &credential("small")).unwrap();
        let before = (index_text(&dir), vault.entries());
        let huge = "h".repeat(BLOB_LIMIT * CHUNK_CAP + 1);
        let error = store.put(&key, &credential(&huge)).unwrap_err();
        assert!(
            matches!(&error, OAuthError::Storage(reason) if reason == TOO_LARGE),
            "{error}"
        );
        let mut refresh = credential("small");
        refresh.tokens.refresh_token = Some(huge.clone());
        assert!(
            store
                .replace(&key, &credential("small").tokens, &refresh)
                .is_err()
        );
        assert_eq!((index_text(&dir), vault.entries()), before);
        // Exactly at the cap fits.
        let full = "f".repeat(BLOB_LIMIT * CHUNK_CAP);
        let mut at_cap = credential(&full);
        at_cap.tokens.refresh_token = None;
        store.put(&key, &at_cap).unwrap();
        assert_eq!(field_accounts(&vault, "access").len(), CHUNK_CAP);
        assert_eq!(store.get(&key).unwrap().unwrap().tokens.access_token, full);
    }

    #[test]
    fn a_missing_or_altered_entry_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let (store, vault) = store(&dir);
        let key = key("remote", URL, "https://as.example.test/");
        store.put(&key, &credential(&"m".repeat(6000))).unwrap();
        // Another process, which has not read this credential yet.
        let (store, _) = (reader(&dir, vault.clone()), store);
        let accounts = field_accounts(&vault, "access");
        let last = accounts
            .iter()
            .find(|account| account.ends_with(".2"))
            .unwrap();
        vault.delete(last).unwrap();
        let incomplete = store.get(&key).unwrap_err();
        assert!(matches!(&incomplete, OAuthError::Storage(reason) if reason == ALTERED));
        // A foreign value of the right size in the right place is refused too.
        vault.put(last, &vec![b'x'; 6000 - 2 * BLOB_LIMIT]).unwrap();
        assert!(matches!(
            store.find("remote", URL),
            Err(OAuthError::Storage(_))
        ));
        assert!(
            !store
                .find("remote", URL)
                .unwrap_err()
                .to_string()
                .contains("mmm")
        );
    }

    #[test]
    fn replace_after_a_concurrent_put_keeps_the_newer_generation() {
        let dir = tempfile::tempdir().unwrap();
        let (store, vault) = store(&dir);
        let key = key("remote", URL, "https://as.example.test/");
        let first = credential(&"1".repeat(5000));
        store.put(&key, &first).unwrap();
        // `cyber mcp auth` replaced it while a refresh of `first` was in flight.
        let newer = credential(&"2".repeat(5000));
        store.put(&key, &newer).unwrap();
        let refreshed = credential(&"3".repeat(5000));
        assert!(!store.replace(&key, &first.tokens, &refreshed).unwrap());
        assert_eq!(store.get(&key).unwrap().unwrap(), newer);
        // Only the newer generation's entries remain.
        let generations: std::collections::BTreeSet<_> = vault
            .accounts()
            .iter()
            .map(|account| account.split('.').nth(2).unwrap().to_owned())
            .collect();
        assert_eq!(generations.len(), 1, "{:?}", vault.accounts());
        assert!(store.replace(&key, &newer.tokens, &refreshed).unwrap());
        assert_eq!(store.get(&key).unwrap().unwrap(), refreshed);
        let mut other_issuer = key.clone();
        other_issuer.issuer = "https://elsewhere.test/".into();
        assert!(
            !store
                .replace(&other_issuer, &refreshed.tokens, &first)
                .unwrap()
        );
    }

    #[test]
    fn indexed_servers_names_every_server_with_a_record_or_marker() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("mcp-auth-index.json");
        assert!(indexed_servers(&index).unwrap().is_empty());
        let (store, _vault) = store(&dir);
        store
            .put(
                &key("remote", URL, "https://as.example.test/"),
                &credential("a"),
            )
            .unwrap();
        store.mark("marked", URL, &AuthMarker::default()).unwrap();
        store
            .mark(
                "marked",
                URL,
                &AuthMarker {
                    needs_client_registration: true,
                    ..AuthMarker::default()
                },
            )
            .unwrap();
        let servers: Vec<_> = indexed_servers(&index).unwrap().into_iter().collect();
        assert_eq!(servers, ["marked", "remote"]);
    }

    #[test]
    fn remove_server_deletes_every_entry_of_every_url_and_issuer() {
        let dir = tempfile::tempdir().unwrap();
        let (store, vault) = store(&dir);
        let old = "https://old.example.test/mcp";
        store
            .put(
                &key("remote", URL, "https://as.example.test/"),
                &credential(&"r".repeat(3000)),
            )
            .unwrap();
        store
            .put(
                &key("remote", old, "https://other-as.test/"),
                &credential("old"),
            )
            .unwrap();
        store
            .put(
                &key("other", URL, "https://as.example.test/"),
                &credential("kept"),
            )
            .unwrap();
        let marker = AuthMarker {
            needs_client_registration: true,
            ..AuthMarker::default()
        };
        store.mark("remote", URL, &marker).unwrap();
        // An orphan of a write that crashed before its commit: reserved, never listed.
        let digest = credential_digest("remote", old);
        let (namespace, generation) = store
            .with(|index| Ok((index.reserve("remote", &digest)?, true)))
            .unwrap();
        let orphan = Slot {
            namespace: &namespace,
            digest: &digest,
            generation,
        };
        vault
            .put(&orphan.account(Field::Access, 0), b"orphan")
            .unwrap();
        assert_eq!(store.remove_server("remote").unwrap(), 2);
        let other = credential_digest("other", URL);
        assert!(
            vault
                .accounts()
                .iter()
                .all(|account| account.contains(&other))
        );
        assert_eq!(store.marker("remote", URL).unwrap(), AuthMarker::default());
        assert_eq!(
            store
                .find("other", URL)
                .unwrap()
                .unwrap()
                .1
                .tokens
                .access_token,
            "kept"
        );
        assert!(store.find("remote", old).unwrap().is_none());
        assert_eq!(store.remove_server("remote").unwrap(), 0);
        assert!(store.remove("other", URL).unwrap());
        assert!(vault.entries().is_empty());
    }

    #[test]
    fn no_account_or_index_byte_names_the_url_or_a_secret() {
        let dir = tempfile::tempdir().unwrap();
        let (store, vault) = store(&dir);
        store
            .put(
                &key("remote", URL, "https://as.example.test/"),
                &credential("ACCESS-SECRET"),
            )
            .unwrap();
        store
            .mark(
                "remote",
                URL,
                &AuthMarker {
                    needs_client_registration: true,
                    ..AuthMarker::default()
                },
            )
            .unwrap();
        let text = index_text(&dir);
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        let stored = credential("ACCESS-SECRET");
        assert!(!reveals(
            &parsed,
            &key("remote", URL, "https://as.example.test/"),
            &stored
        ));
        for secret in [
            "mcp.example.test",
            "QUERY-SECRET",
            "key=",
            "ACCESS-SECRET",
            "CLIENT-SECRET",
        ] {
            assert!(!text.contains(secret), "{secret}: {text}");
            for account in vault.accounts() {
                assert!(!account.contains(secret), "{account}");
            }
        }
    }

    /// Any vault call panics: the index alone answers for credentials it does not list.
    struct UntouchableVault;

    impl SecretVault for UntouchableVault {
        fn put(&self, _: &str, _: &[u8]) -> Result<(), OAuthError> {
            panic!("the vault was written")
        }
        fn get(&self, _: &str) -> Result<Option<Vec<u8>>, OAuthError> {
            panic!("the vault was read")
        }
        fn delete(&self, _: &str) -> Result<(), OAuthError> {
            panic!("the vault was deleted from")
        }
    }

    #[test]
    fn an_empty_index_never_touches_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let store = VaultCredentialStore::new(
            dir.path().join("mcp-auth-index.json"),
            Arc::new(UntouchableVault),
        );
        let key = key("remote", URL, "https://as.example.test/");
        assert!(store.get(&key).unwrap().is_none());
        assert!(store.find("remote", URL).unwrap().is_none());
        assert_eq!(store.marker("remote", URL).unwrap(), AuthMarker::default());
        assert!(!store.remove("remote", URL).unwrap());
        assert_eq!(store.remove_server("remote").unwrap(), 0);
        assert!(
            !store
                .replace(&key, &credential("x").tokens, &credential("y"))
                .unwrap()
        );
        // Markers live in the index only.
        store
            .mark(
                "remote",
                URL,
                &AuthMarker {
                    needs_client_registration: true,
                    ..AuthMarker::default()
                },
            )
            .unwrap();
        store.clear_marker("remote", URL).unwrap();
        drop(store.try_lock_refresh("remote", URL).unwrap());
    }

    #[test]
    fn refresh_locks_are_per_credential_and_never_name_the_url() {
        let dir = tempfile::tempdir().unwrap();
        let store = VaultCredentialStore::new(
            dir.path().join("mcp-auth-index.json"),
            Arc::new(UntouchableVault),
        );
        super::super::store::tests::refresh_locks_are_per_credential(&store);
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            assert!(!name.contains("test"), "{name}");
        }
    }

    /// A vault whose errors carry attribute values, as platform errors can.
    struct LeakyVault(MemoryVault);

    impl SecretVault for LeakyVault {
        fn put(&self, account: &str, secret: &[u8]) -> Result<(), OAuthError> {
            self.0.put(account, secret)
        }
        fn get(&self, _: &str) -> Result<Option<Vec<u8>>, OAuthError> {
            Err(OAuthError::Storage("attribute value TOKEN-LEAK".into()))
        }
        fn delete(&self, _: &str) -> Result<(), OAuthError> {
            Err(OAuthError::Storage("attribute value TOKEN-LEAK".into()))
        }
    }

    #[test]
    fn vault_errors_become_fixed_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let vault: Arc<dyn SecretVault> = Arc::new(LeakyVault(MemoryVault::default()));
        reader(&dir, vault.clone())
            .put(
                &key("remote", URL, "https://as.example.test/"),
                &credential("TOKEN-LEAK"),
            )
            .unwrap();
        let store = reader(&dir, vault);
        let read = store.find("remote", URL).unwrap_err();
        assert!(matches!(&read, OAuthError::Storage(reason) if reason == "OS keyring read failed"));
        let delete = store.remove("remote", URL).unwrap_err();
        assert!(
            matches!(&delete, OAuthError::Storage(reason) if reason == "OS keyring delete failed")
        );
        for error in [read, delete] {
            assert!(!error.to_string().contains("TOKEN"), "{error}");
        }
        // A failed deletion keeps the credential listed for the next logout.
        assert!(index_text(&dir).contains("\"remote\""));
    }

    /// A [`MemoryVault`] whose `n`th next deletion fails (`0`: none does).
    #[derive(Default)]
    struct FlakyVault {
        inner: MemoryVault,
        fail_in: std::sync::atomic::AtomicUsize,
    }

    impl FlakyVault {
        fn fail_delete(&self, n: usize) {
            self.fail_in.store(n, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl SecretVault for FlakyVault {
        fn put(&self, account: &str, secret: &[u8]) -> Result<(), OAuthError> {
            self.inner.put(account, secret)
        }
        fn get(&self, account: &str) -> Result<Option<Vec<u8>>, OAuthError> {
            self.inner.get(account)
        }
        fn delete(&self, account: &str) -> Result<(), OAuthError> {
            use std::sync::atomic::Ordering;
            let left = self.fail_in.load(Ordering::SeqCst);
            if left > 0 {
                self.fail_in.store(left - 1, Ordering::SeqCst);
                if left == 1 {
                    return Err(storage("vault unavailable"));
                }
            }
            self.inner.delete(account)
        }
    }

    /// The owner-only file an earlier release wrote. Windows never had one: its file
    /// backend refuses, so these cases run where the file store works.
    #[cfg(not(windows))]
    mod legacy_file {
        use super::*;

        const OLD: &str = "https://old.example.test/mcp";

        fn legacy_path(dir: &tempfile::TempDir) -> PathBuf {
            dir.path().join("mcp-auth.json")
        }

        /// The file a previous release wrote, as `cyber mcp auth` stored it.
        fn legacy(dir: &tempfile::TempDir) -> FileCredentialStore {
            FileCredentialStore::new(legacy_path(dir))
        }

        /// A vault store that also reads and migrates the legacy file.
        fn migrating(dir: &tempfile::TempDir, vault: Arc<dyn SecretVault>) -> VaultCredentialStore {
            VaultCredentialStore::new(dir.path().join("mcp-auth-index.json"), vault)
                .with_legacy(legacy(dir))
        }

        fn registration() -> AuthMarker {
            AuthMarker {
                needs_client_registration: true,
                ..AuthMarker::default()
            }
        }

        fn legacy_text(dir: &tempfile::TempDir) -> String {
            std::fs::read_to_string(legacy_path(dir)).unwrap_or_default()
        }

        #[test]
        fn a_legacy_file_credential_is_read_until_a_rotation_moves_it_to_the_vault() {
            let dir = tempfile::tempdir().unwrap();
            let key = key("remote", URL, "https://as.example.test/");
            legacy(&dir).put(&key, &credential("LEGACY-ONE")).unwrap();
            legacy(&dir).mark("remote", URL, &registration()).unwrap();
            let vault = Arc::new(MemoryVault::default());
            let store = migrating(&dir, vault.clone());
            // Lazy reads leave both places as they are.
            assert_eq!(
                store.find("remote", URL).unwrap().unwrap(),
                (key.clone(), credential("LEGACY-ONE"))
            );
            assert_eq!(store.get(&key).unwrap().unwrap(), credential("LEGACY-ONE"));
            assert_eq!(store.marker("remote", URL).unwrap(), registration());
            assert!(vault.entries().is_empty());
            assert!(store.find("remote", OLD).unwrap().is_none());

            // A refresh rotation (compare-and-swap) moves it, marker included.
            let stale = credential("STALE");
            assert!(
                !store
                    .replace(&key, &stale.tokens, &credential("X"))
                    .unwrap()
            );
            let rotated = credential("ROTATED-ONE");
            let previous = credential("LEGACY-ONE").tokens;
            assert!(store.replace(&key, &previous, &rotated).unwrap());
            assert_eq!(store.get(&key).unwrap().unwrap(), rotated);
            assert!(legacy(&dir).find("remote", URL).unwrap().is_none());
            let left = legacy(&dir).marker("remote", URL).unwrap();
            assert_eq!(left, AuthMarker::default());
            assert_eq!(store.marker("remote", URL).unwrap(), registration());
            assert!(!legacy_text(&dir).contains("LEGACY-ONE"));
            assert!(index_text(&dir).contains("needs_client_registration"));
            assert!(!vault.entries().is_empty());
        }

        #[test]
        fn auth_moves_a_legacy_credential_which_never_shadows_the_vault_again() {
            let dir = tempfile::tempdir().unwrap();
            let key = key("remote", URL, "https://as.example.test/");
            legacy(&dir).put(&key, &credential("LEGACY-TWO")).unwrap();
            let store = migrating(&dir, Arc::new(MemoryVault::default()));
            // `cyber mcp auth` stores with `put`.
            store.put(&key, &credential("NEW-TWO")).unwrap();
            assert!(legacy(&dir).find("remote", URL).unwrap().is_none());
            assert!(!legacy_text(&dir).contains("LEGACY-TWO"));
            assert_eq!(store.get(&key).unwrap().unwrap(), credential("NEW-TWO"));
            // A later legacy write (an older binary) is shadowed by the index record.
            legacy(&dir).put(&key, &credential("LEGACY-AGAIN")).unwrap();
            assert_eq!(store.get(&key).unwrap().unwrap(), credential("NEW-TWO"));
        }

        #[test]
        fn remove_server_sweeps_the_legacy_file_the_index_and_the_vault() {
            let dir = tempfile::tempdir().unwrap();
            legacy(&dir)
                .put(
                    &key("remote", OLD, "https://as.example.test/"),
                    &credential("L1"),
                )
                .unwrap();
            legacy(&dir)
                .put(
                    &key("other", OLD, "https://as.example.test/"),
                    &credential("KEPT"),
                )
                .unwrap();
            legacy(&dir).mark("remote", OLD, &registration()).unwrap();
            let vault = Arc::new(MemoryVault::default());
            let store = migrating(&dir, vault.clone());
            store
                .put(
                    &key("remote", URL, "https://as.example.test/"),
                    &credential("V1"),
                )
                .unwrap();
            store.mark("remote", URL, &registration()).unwrap();
            assert_eq!(store.remove_server("remote").unwrap(), 2);
            assert!(vault.entries().is_empty());
            assert!(!index_text(&dir).contains("\"remote\""));
            assert!(!legacy_text(&dir).contains("\"remote\""));
            for url in [URL, OLD] {
                assert!(store.find("remote", url).unwrap().is_none());
                assert_eq!(store.marker("remote", url).unwrap(), AuthMarker::default());
            }
            assert_eq!(
                store.find("other", OLD).unwrap().unwrap().1,
                credential("KEPT")
            );
            assert_eq!(store.remove_server("remote").unwrap(), 0);
            // `remove` covers both places too.
            assert!(store.remove("other", OLD).unwrap());
            assert!(!legacy_text(&dir).contains("KEPT"));
        }

        #[cfg(unix)]
        #[test]
        fn a_legacy_file_open_to_others_is_refused_not_migrated() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let key = key("remote", URL, "https://as.example.test/");
            legacy(&dir).put(&key, &credential("EXPOSED")).unwrap();
            let path = legacy_path(&dir);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            let vault = Arc::new(MemoryVault::default());
            let store = migrating(&dir, vault.clone());
            assert!(matches!(
                store.find("remote", URL),
                Err(OAuthError::Storage(_))
            ));
            assert!(matches!(store.get(&key), Err(OAuthError::Storage(_))));
            assert!(
                store
                    .replace(&key, &credential("EXPOSED").tokens, &credential("NEW"))
                    .is_err()
            );
            assert!(store.remove_server("remote").is_err());
            assert!(vault.entries().is_empty());
            assert!(index_text(&dir).is_empty() || !index_text(&dir).contains("\"remote\""));
            assert!(legacy_text(&dir).contains("EXPOSED"));
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o644);
        }

        #[test]
        fn a_refresh_lock_also_holds_the_legacy_one() {
            let dir = tempfile::tempdir().unwrap();
            legacy(&dir).mark("remote", URL, &registration()).unwrap();
            let store = migrating(&dir, Arc::new(UntouchableVault));
            let held = store.try_lock_refresh("remote", URL).unwrap().unwrap();
            // A process still using the file backend waits for this refresh.
            assert!(
                legacy(&dir)
                    .try_lock_refresh("remote", URL)
                    .unwrap()
                    .is_none()
            );
            assert!(store.try_lock_refresh("remote", URL).unwrap().is_none());
            drop(held);
            let old = legacy(&dir)
                .try_lock_refresh("remote", URL)
                .unwrap()
                .unwrap();
            assert!(store.try_lock_refresh("remote", URL).unwrap().is_none());
            drop(old);
            assert!(store.try_lock_refresh("remote", URL).unwrap().is_some());
        }

        #[test]
        fn a_vault_error_during_remove_server_fails_and_a_retry_leaves_no_orphan() {
            let dir = tempfile::tempdir().unwrap();
            legacy(&dir)
                .put(
                    &key("remote", OLD, "https://as.example.test/"),
                    &credential("L1"),
                )
                .unwrap();
            let vault = Arc::new(FlakyVault::default());
            let store = migrating(&dir, vault.clone());
            store
                .put(
                    &key("remote", URL, "https://as.example.test/"),
                    &credential("V1"),
                )
                .unwrap();
            store
                .put(
                    &key(
                        "remote",
                        "https://third.example.test/mcp",
                        "https://as.example.test/",
                    ),
                    &credential(&"V2".repeat(2000)),
                )
                .unwrap();
            // Fails inside the second credential's sweep (25 deletions each).
            vault.fail_delete(30);
            let error = store.remove_server("remote").unwrap_err();
            assert!(
                matches!(&error, OAuthError::Storage(reason) if reason == "OS keyring delete failed"),
                "{error}"
            );
            // Nothing is claimed removed: the index still lists what may remain.
            assert!(index_text(&dir).contains("\"remote\""));
            assert!(store.remove_server("remote").unwrap() >= 1);
            assert!(
                vault.inner.entries().is_empty(),
                "{:?}",
                vault.inner.accounts()
            );
            assert!(!index_text(&dir).contains("\"remote\""));
            assert!(!legacy_text(&dir).contains("\"remote\""));
        }
    }

    #[test]
    fn a_failed_delete_of_a_replaced_generation_is_finished_by_logout() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(FlakyVault::default());
        let store =
            VaultCredentialStore::new(dir.path().join("mcp-auth-index.json"), vault.clone());
        let key = key("remote", URL, "https://as.example.test/");
        store.put(&key, &credential("FIRST")).unwrap();
        // The new generation is stored even though the old one's deletion failed.
        vault.fail_delete(1);
        store.put(&key, &credential("SECOND")).unwrap();
        assert_eq!(store.get(&key).unwrap().unwrap(), credential("SECOND"));
        assert_eq!(store.remove_server("remote").unwrap(), 1);
        assert!(
            vault.inner.entries().is_empty(),
            "{:?}",
            vault.inner.accounts()
        );
    }

    /// The Windows index and lock files keep their default ACL, which is acceptable only
    /// while the index holds no secret and no URL: the commit guard catches either.
    #[test]
    fn the_index_guard_finds_any_secret_or_the_url() {
        let key = key("remote", URL, "https://as.example.test/");
        let stored = credential("ACCESS-SECRET");
        let clean = serde_json::json!({
            "server": "remote",
            "issuer": "https://as.example.test/",
            "token_endpoint": "https://as.example.test/token",
            "nested": [{"count": 1, "sha256": sha256(b"ACCESS-SECRET")}],
        });
        assert!(!reveals(&clean, &key, &stored));
        // A server that is its own authorization server, or a scope named after it.
        let shared = serde_json::json!({
            "credentials": [{
                "issuer": URL,
                "token_endpoint": format!("{URL}/token"),
                "scopes": [format!("{URL}/.default")],
            }],
            "markers": [{"required_scopes": [URL]}],
        });
        assert!(!reveals(&shared, &key, &stored));
        for leak in [
            serde_json::json!({"a": "ACCESS-SECRET"}),
            serde_json::json!({"a": ["prefix ACCESS-SECRET-refresh"]}),
            serde_json::json!({"b": {"c": "CLIENT-SECRET"}}),
            serde_json::json!({"u": format!("at {URL}")}),
            serde_json::json!({"server": URL}),
            // Secrets are refused in the fields that may share the URL too.
            serde_json::json!({"issuer": "https://as.test/CLIENT-SECRET"}),
        ] {
            assert!(reveals(&leak, &key, &stored), "{leak}");
        }
        // A short secret is matched whole, never as a substring of an unrelated value.
        let short = credential("t");
        assert!(!reveals(&clean, &key, &short));
        assert!(reveals(&serde_json::json!(["t"]), &key, &short));
    }

    /// The cfg-independent rule behind the legacy fallback: an absent file is skipped, a
    /// present one is used only where the file backend works and otherwise fails closed.
    #[test]
    fn a_legacy_file_counts_only_when_present_and_a_refusal_is_surfaced() {
        let refused = || Err(storage("owner-only credential files are not supported"));
        assert!(!legacy_in_use(false, refused).unwrap());
        assert!(!legacy_in_use(false, || Ok(())).unwrap());
        assert!(legacy_in_use(true, || Ok(())).unwrap());
        assert!(matches!(
            legacy_in_use(true, refused),
            Err(OAuthError::Storage(_))
        ));
    }

    /// Windows: the file backend refuses, so a file at the legacy path fails every
    /// operation closed instead of being ignored, and an absent one is no obstacle.
    #[cfg(windows)]
    #[test]
    fn a_present_legacy_file_fails_closed_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-auth.json");
        let vault = Arc::new(MemoryVault::default());
        let store =
            VaultCredentialStore::new(dir.path().join("mcp-auth-index.json"), vault.clone())
                .with_legacy(FileCredentialStore::new(path.clone()));
        let key = key("remote", URL, "https://as.example.test/");
        std::fs::write(&path, "{}").unwrap();
        assert!(matches!(
            store.find("remote", URL),
            Err(OAuthError::Storage(_))
        ));
        assert!(matches!(store.get(&key), Err(OAuthError::Storage(_))));
        assert!(matches!(
            store.marker("remote", URL),
            Err(OAuthError::Storage(_))
        ));
        assert!(store.put(&key, &credential("NEW")).is_err());
        assert!(store.try_lock_refresh("remote", URL).is_err());
        assert!(store.remove_server("remote").is_err());
        assert!(vault.entries().is_empty());
        std::fs::remove_file(&path).unwrap();
        assert!(store.find("remote", URL).unwrap().is_none());
        store.put(&key, &credential("NEW")).unwrap();
        assert_eq!(store.get(&key).unwrap().unwrap(), credential("NEW"));
    }

    /// Regression: the guard took an issuer, token endpoint or scope sharing the server URL
    /// for a URL leak, so a debug build panicked after the keyring entries were written
    /// and before the index named them, leaving entries no logout could find.
    #[test]
    fn a_server_that_is_its_own_authorization_server_is_stored_and_removed_whole() {
        let dir = tempfile::tempdir().unwrap();
        let (store, vault) = store(&dir);
        let root = "https://mcp.example.test/";
        let mut own = credential("ROOT-ACCESS");
        own.token_endpoint = format!("{root}token");
        own.tokens.scopes = vec![format!("{root}mcp/.default")];
        store.put(&key("remote", root, root), &own).unwrap();
        // Another server whose issuer is the first one's URL.
        store
            .put(&key("other", URL, root), &credential("OTHER-ACCESS"))
            .unwrap();
        store
            .mark(
                "remote",
                root,
                &AuthMarker {
                    required_scopes: vec![root.into()],
                    ..AuthMarker::default()
                },
            )
            .unwrap();
        let (found, stored) = store.find("remote", root).unwrap().unwrap();
        assert_eq!((found.issuer.as_str(), stored), (root, own));
        assert_eq!(store.remove_server("remote").unwrap(), 1);
        assert_eq!(store.remove_server("other").unwrap(), 1);
        assert!(vault.entries().is_empty(), "{:?}", vault.accounts());
        assert!(!index_text(&dir).contains("\"remote\""));
    }

    /// The index is not a secret store, yet what it says is bound in the keyring: an edited
    /// issuer, token endpoint, client id or binding field fails closed rather than sending
    /// the refresh token or client secret elsewhere, even to the process that last wrote it.
    #[test]
    fn an_edited_index_record_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let (store, vault) = store(&dir);
        let key = key("remote", URL, "https://as.example.test/");
        store.put(&key, &credential("BOUND")).unwrap();
        let path = dir.path().join("mcp-auth-index.json");
        let original = index_text(&dir);
        for (field, value) in [
            ("issuer", serde_json::json!("https://evil.test/")),
            ("token_endpoint", serde_json::json!("https://evil.test/t")),
            ("client_id", serde_json::json!("evil-client")),
            ("scopes", serde_json::json!(["admin"])),
            ("expires_at", serde_json::json!(1)),
        ] {
            let mut edited: serde_json::Value = serde_json::from_str(&original).unwrap();
            edited["credentials"][0][field] = value;
            std::fs::write(&path, edited.to_string()).unwrap();
            for reader in [&store, &reader(&dir, vault.clone())] {
                let error = reader.find("remote", URL).unwrap_err();
                assert!(
                    matches!(&error, OAuthError::Storage(reason) if reason == ALTERED),
                    "{field}: {error}"
                );
            }
            assert!(store.get(&key).is_err(), "{field}");
        }
        std::fs::write(&path, &original).unwrap();
        assert_eq!(store.get(&key).unwrap().unwrap(), credential("BOUND"));
    }

    /// A [`MemoryVault`] counting reads; armed, its next read waits at `gate` twice (on
    /// entry and before returning).
    #[derive(Default)]
    struct ObservedVault {
        inner: MemoryVault,
        reads: std::sync::atomic::AtomicUsize,
        armed: std::sync::atomic::AtomicBool,
        gate: Option<Arc<std::sync::Barrier>>,
    }

    impl SecretVault for ObservedVault {
        fn put(&self, account: &str, secret: &[u8]) -> Result<(), OAuthError> {
            self.inner.put(account, secret)
        }
        fn get(&self, account: &str) -> Result<Option<Vec<u8>>, OAuthError> {
            use std::sync::atomic::Ordering;
            self.reads.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.gate
                && self.armed.swap(false, Ordering::SeqCst)
            {
                gate.wait();
                gate.wait();
            }
            self.inner.get(account)
        }
        fn delete(&self, account: &str) -> Result<(), OAuthError> {
            self.inner.delete(account)
        }
    }

    /// The checks before every call find an unchanged record: no keyring call, in the
    /// process that wrote it or one that read it once. A rotation elsewhere is read again.
    #[test]
    fn an_unchanged_record_is_answered_without_the_keyring() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(ObservedVault::default());
        let writer = reader(&dir, vault.clone());
        let key = key("remote", URL, "https://as.example.test/");
        writer.put(&key, &credential("FIRST")).unwrap();
        assert_eq!(writer.get(&key).unwrap().unwrap(), credential("FIRST"));
        assert_eq!(vault.reads.load(Ordering::SeqCst), 0);
        let other = reader(&dir, vault.clone());
        assert_eq!(
            other.find("remote", URL).unwrap().unwrap().1,
            credential("FIRST")
        );
        let once = vault.reads.load(Ordering::SeqCst);
        assert!(once > 0);
        for _ in 0..3 {
            assert!(other.find("remote", URL).unwrap().is_some());
        }
        assert_eq!(vault.reads.load(Ordering::SeqCst), once);
        writer.put(&key, &credential("SECOND")).unwrap();
        assert_eq!(
            other.find("remote", URL).unwrap().unwrap().1,
            credential("SECOND")
        );
        assert!(vault.reads.load(Ordering::SeqCst) > once);
        // A removal is seen at once.
        assert!(writer.remove("remote", URL).unwrap());
        assert!(other.find("remote", URL).unwrap().is_none());
    }

    /// A keyring read that blocks (a Keychain prompt) holds no index lock, so another
    /// process's store operations go on meanwhile instead of failing as locked.
    #[test]
    fn a_blocked_keyring_read_never_holds_the_index_lock() {
        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(std::sync::Barrier::new(2));
        let vault = Arc::new(ObservedVault {
            gate: Some(gate.clone()),
            ..ObservedVault::default()
        });
        let key = key("remote", URL, "https://as.example.test/");
        reader(&dir, vault.clone())
            .put(&key, &credential("GATED"))
            .unwrap();
        vault.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let blocked = {
            let store = reader(&dir, vault.clone());
            std::thread::spawn(move || store.find("remote", URL))
        };
        gate.wait();
        let started = std::time::Instant::now();
        let other = reader(&dir, Arc::new(MemoryVault::default()));
        let marker = AuthMarker {
            required_scopes: vec!["mcp.write".into()],
            ..AuthMarker::default()
        };
        other.mark("other", URL, &marker).unwrap();
        assert_eq!(other.marker("other", URL).unwrap(), marker);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        gate.wait();
        assert_eq!(
            blocked.join().unwrap().unwrap().unwrap().1,
            credential("GATED")
        );
    }

    /// A read racing a rewrite of the same credential is retried against the new record
    /// instead of failing on the generation the rewrite deleted.
    #[test]
    fn a_read_racing_a_rotation_returns_the_new_credential() {
        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(std::sync::Barrier::new(2));
        let vault = Arc::new(ObservedVault {
            gate: Some(gate.clone()),
            ..ObservedVault::default()
        });
        let key = key("remote", URL, "https://as.example.test/");
        let writer = reader(&dir, vault.clone());
        writer.put(&key, &credential("OLD")).unwrap();
        vault.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let racing = {
            let store = reader(&dir, vault.clone());
            std::thread::spawn(move || store.find("remote", URL))
        };
        // The reader has the old record and is about to read its binding.
        gate.wait();
        writer.put(&key, &credential("NEW")).unwrap();
        gate.wait();
        assert_eq!(
            racing.join().unwrap().unwrap().unwrap().1,
            credential("NEW")
        );
    }

    /// A [`MemoryVault`] whose deletions report success while (when `stubborn`) keeping
    /// the entry, as keyring 3.6.3's macOS backend does when the Keychain refuses.
    #[derive(Default)]
    struct StubbornVault {
        inner: MemoryVault,
        stubborn: std::sync::atomic::AtomicBool,
    }

    impl SecretVault for StubbornVault {
        fn put(&self, account: &str, secret: &[u8]) -> Result<(), OAuthError> {
            self.inner.put(account, secret)
        }
        fn get(&self, account: &str) -> Result<Option<Vec<u8>>, OAuthError> {
            self.inner.get(account)
        }
        fn delete(&self, account: &str) -> Result<(), OAuthError> {
            if !self.stubborn.load(std::sync::atomic::Ordering::SeqCst) {
                self.inner.delete(account)?;
            }
            Ok(())
        }
    }

    /// Regression: a deletion the keyring only claimed made logout report the credential
    /// removed and forget it while its secrets stayed in the Keychain.
    #[test]
    fn a_deletion_the_keyring_only_claims_is_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(StubbornVault::default());
        let store = reader(&dir, vault.clone());
        let key = key("remote", URL, "https://as.example.test/");
        store.put(&key, &credential("KEPT-BY-KEYCHAIN")).unwrap();
        vault
            .stubborn
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let error = store.remove_server("remote").unwrap_err();
        assert!(
            matches!(&error, OAuthError::Storage(reason) if reason == DELETE_FAILED),
            "{error}"
        );
        assert!(!store.remove("remote", URL).is_ok_and(|removed| removed));
        // Unusable at once, yet still listed for the next logout to finish.
        assert!(store.find("remote", URL).unwrap().is_none());
        assert!(index_text(&dir).contains("\"remote\""));
        assert!(!vault.inner.entries().is_empty());
        vault
            .stubborn
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(store.remove_server("remote").unwrap(), 1);
        assert!(
            vault.inner.entries().is_empty(),
            "{:?}",
            vault.inner.accounts()
        );
        assert!(!index_text(&dir).contains("\"remote\""));
    }

    /// A [`MemoryVault`] whose next write or deletion, once armed, waits at `gate` twice
    /// (on entry and before acting).
    struct GatedVault {
        inner: MemoryVault,
        armed: std::sync::atomic::AtomicBool,
        gate: Arc<std::sync::Barrier>,
    }

    impl GatedVault {
        fn new(gate: Arc<std::sync::Barrier>) -> Self {
            Self {
                inner: MemoryVault::default(),
                armed: std::sync::atomic::AtomicBool::new(false),
                gate,
            }
        }

        fn arm(&self) {
            self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        fn pass(&self) {
            if self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                self.gate.wait();
                self.gate.wait();
            }
        }
    }

    impl SecretVault for GatedVault {
        fn put(&self, account: &str, secret: &[u8]) -> Result<(), OAuthError> {
            self.pass();
            self.inner.put(account, secret)
        }
        fn get(&self, account: &str) -> Result<Option<Vec<u8>>, OAuthError> {
            self.inner.get(account)
        }
        fn delete(&self, account: &str) -> Result<(), OAuthError> {
            self.pass();
            self.inner.delete(account)
        }
    }

    /// Store operations of another process while one is inside a keyring call: none waits
    /// for the index lock.
    fn assert_index_unlocked(dir: &tempfile::TempDir, vault: Arc<dyn SecretVault>) {
        let started = std::time::Instant::now();
        let other = reader(dir, vault);
        let marker = AuthMarker {
            required_scopes: vec!["mcp.write".into()],
            ..AuthMarker::default()
        };
        other.mark("other", URL, &marker).unwrap();
        assert_eq!(other.marker("other", URL).unwrap(), marker);
        assert_eq!(
            other.find("other", URL).unwrap().unwrap().1,
            credential("OTHER")
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    /// Regression: writes and removals held the index lock across every keyring call, so a
    /// Keychain prompt during one made every other store operation fail as locked.
    #[test]
    fn keyring_writes_and_deletions_never_hold_the_index_lock() {
        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(std::sync::Barrier::new(2));
        let vault = Arc::new(GatedVault::new(gate.clone()));
        let other = key("other", URL, "https://as.example.test/");
        let key = key("remote", URL, "https://as.example.test/");
        reader(&dir, vault.clone())
            .put(&other, &credential("OTHER"))
            .unwrap();
        reader(&dir, vault.clone())
            .put(&key, &credential("FIRST"))
            .unwrap();
        for step in ["rotate", "logout"] {
            vault.arm();
            let blocked = {
                let (store, key) = (reader(&dir, vault.clone()), key.clone());
                std::thread::spawn(move || match step {
                    "rotate" => store.put(&key, &credential("SECOND")).map(|()| 1),
                    _ => store.remove_server("remote"),
                })
            };
            gate.wait();
            assert_index_unlocked(&dir, vault.clone());
            gate.wait();
            assert_eq!(blocked.join().unwrap().unwrap(), 1, "{step}");
        }
        assert!(
            reader(&dir, vault.clone())
                .find("remote", URL)
                .unwrap()
                .is_none()
        );
        let other = credential_digest("other", URL);
        assert!(
            vault
                .inner
                .accounts()
                .iter()
                .all(|account| account.contains(&other))
        );
    }

    /// A logout while a write is putting its entries wins: the write is not committed and
    /// leaves nothing in the keyring.
    #[test]
    fn a_logout_during_a_write_wins_and_leaves_no_entry() {
        let dir = tempfile::tempdir().unwrap();
        let gate = Arc::new(std::sync::Barrier::new(2));
        let vault = Arc::new(GatedVault::new(gate.clone()));
        let key = key("remote", URL, "https://as.example.test/");
        vault.arm();
        let writing = {
            let (store, key) = (reader(&dir, vault.clone()), key.clone());
            std::thread::spawn(move || store.put(&key, &credential(&"w".repeat(6000))))
        };
        gate.wait();
        assert_eq!(
            reader(&dir, vault.clone()).remove_server("remote").unwrap(),
            1
        );
        gate.wait();
        assert!(writing.join().unwrap().is_err());
        let store = reader(&dir, vault.clone());
        assert!(store.find("remote", URL).unwrap().is_none());
        assert!(
            vault.inner.entries().is_empty(),
            "{:?}",
            vault.inner.accounts()
        );
        assert!(!index_text(&dir).contains("\"remote\""));
        // A later sign-in is unaffected.
        store.put(&key, &credential("AGAIN")).unwrap();
        assert_eq!(store.get(&key).unwrap().unwrap(), credential("AGAIN"));
    }

    /// Two data directories (two `CYBER_HOME`s) of one OS user share its keyring but never
    /// each other's entries: a sign-in in one keeps the other's, and a logout in one
    /// leaves the other signed in.
    #[test]
    fn two_data_directories_never_share_keyring_entries() {
        let vault = Arc::new(MemoryVault::default());
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let key = key("remote", URL, "https://as.example.test/");
        reader(&first, vault.clone())
            .put(&key, &credential("FIRST-HOME"))
            .unwrap();
        reader(&second, vault.clone())
            .put(&key, &credential("SECOND-HOME"))
            .unwrap();
        // Fresh handles read from the keyring, not from memory.
        assert_eq!(
            reader(&first, vault.clone()).get(&key).unwrap().unwrap(),
            credential("FIRST-HOME")
        );
        assert_eq!(
            reader(&first, vault.clone())
                .remove_server("remote")
                .unwrap(),
            1
        );
        assert_eq!(
            reader(&second, vault.clone()).get(&key).unwrap().unwrap(),
            credential("SECOND-HOME")
        );
        let namespace = |dir: &tempfile::TempDir| {
            serde_json::from_str::<serde_json::Value>(&index_text(dir)).unwrap()["namespace"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let kept = namespace(&second);
        assert_ne!(namespace(&first), kept);
        assert!(
            vault
                .accounts()
                .iter()
                .all(|account| account.starts_with(&kept))
        );
    }

    /// An index whose namespace was edited into anything but the random hex it creates is
    /// refused rather than used to name keyring entries.
    #[test]
    fn an_index_with_an_invalid_namespace_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let (store, vault) = store(&dir);
        store
            .put(
                &key("remote", URL, "https://as.example.test/"),
                &credential("NS"),
            )
            .unwrap();
        let path = dir.path().join("mcp-auth-index.json");
        let mut edited: serde_json::Value = serde_json::from_str(&index_text(&dir)).unwrap();
        edited["namespace"] = serde_json::json!("other.home");
        std::fs::write(&path, edited.to_string()).unwrap();
        let fresh = reader(&dir, vault.clone());
        assert!(matches!(
            fresh.find("remote", URL),
            Err(OAuthError::Storage(_))
        ));
        assert!(fresh.remove_server("remote").is_err());
        assert!(!vault.entries().is_empty());
    }

    #[test]
    fn the_memory_vault_enforces_the_entry_limit() {
        let vault = MemoryVault::default();
        assert!(vault.put("a", &[0; BLOB_LIMIT]).is_ok());
        assert!(vault.put("b", &[0; BLOB_LIMIT + 1]).is_err());
        assert_eq!(vault.accounts(), ["a"]);
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn os_keyring_errors_carry_no_platform_text() {
        let error = os_failure(
            "read",
            &keyring::Error::Invalid("account".into(), "TOKEN-LEAK".into()),
        );
        assert_eq!(
            error.to_string(),
            "MCP credential storage failed: OS keyring read failed"
        );
    }

    /// Construction only: no keyring or file I/O.
    #[test]
    fn the_backend_is_chosen_at_compile_time() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            data: dir.path().join("data"),
            config: dir.path().join("config"),
            state: dir.path().join("state"),
            cache: dir.path().join("cache"),
            tmp: dir.path().join("tmp"),
        };
        let _store = open_store(&paths);
        #[cfg(any(target_os = "macos", windows))]
        assert_eq!(backend_name(), "keyring");
        #[cfg(not(any(target_os = "macos", windows)))]
        assert_eq!(backend_name(), "file");
        assert_eq!(
            index_file(&paths),
            dir.path().join("data/mcp-auth-index.json")
        );
        assert!(!dir.path().join("data").exists());
    }
}
