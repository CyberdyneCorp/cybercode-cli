//! Credential storage and a crash-safe cross-process refresh journal. The account
//! lock serializes login, refresh and logout, not just in-process refresh futures.
use crate::{Error, OidcClient, Secret, TokenSet};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroizing;

const MAX_FILE: u64 = 1024 * 1024;
const CHUNK: usize = 2000;

pub trait CredentialVault: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<Secret>, Error>;
    fn put(&self, account: &str, value: &str) -> Result<(), Error>;
    fn delete(&self, account: &str) -> Result<(), Error>;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountKey {
    pub issuer: String,
    pub sub: String,
    pub client_id: String,
}
impl AccountKey {
    fn of(tokens: &TokenSet) -> Self {
        Self {
            issuer: tokens.account.issuer.clone(),
            sub: tokens.account.sub.clone(),
            client_id: tokens.client_id.clone(),
        }
    }
    fn vault_name(&self) -> Result<String, Error> {
        url::Url::parse(&self.issuer).map_err(|_| Error::Storage("invalid issuer"))?;
        let namespace = format!(
            "{:x}",
            Sha256::digest(format!("{}\0{}", self.issuer, self.client_id).as_bytes())
        );
        Ok(format!("account:{namespace}:{}", self.sub))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    Keyring,
    File,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    key: AccountKey,
    kind: StorageKind,
}
#[derive(Default, Serialize, Deserialize)]
struct Index {
    active: Option<AccountKey>,
    entries: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
struct FileRecord {
    tokens: TokenSet,
    pending: bool,
}
#[derive(Default, Serialize, Deserialize)]
struct FileDocument {
    records: Vec<FileRecord>,
}
#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    key: AccountKey,
    generation: String,
    chunks: usize,
    digest: String,
    pending: bool,
    #[serde(default)]
    retired: Vec<Retired>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Retired {
    generation: String,
    chunks: usize,
}

#[derive(Clone)]
pub struct AccountStore {
    directory: PathBuf,
    vault: Option<Arc<dyn CredentialVault>>,
}

impl AccountStore {
    /// Uses native Keychain/Credential Manager where compiled in, file fallback on
    /// Unix where unavailable. Never opens the keyring until an account is used.
    pub fn open(directory: PathBuf) -> Self {
        Self::with_vault(directory, native_vault())
    }
    /// Injection point for embedders/tests. A file fallback is never allowed without
    /// Unix owner-only storage; Windows needs a functioning native vault.
    pub fn with_vault(directory: PathBuf, vault: Option<Arc<dyn CredentialVault>>) -> Self {
        Self { directory, vault }
    }

    pub async fn load(&self) -> Result<Option<TokenSet>, Error> {
        let store = self.clone();
        blocking(move || {
            let _guard = store.lock()?;
            store.load_locked()
        })
        .await
    }
    pub async fn save(&self, tokens: TokenSet) -> Result<StorageKind, Error> {
        let store = self.clone();
        blocking(move || {
            let _guard = store.lock()?;
            store.save_locked(&tokens)
        })
        .await
    }
    pub async fn logout(&self, all: bool) -> Result<(), Error> {
        let store = self.clone();
        blocking(move || {
            let _guard = store.lock()?;
            let mut index = store.index()?;
            let entries: Vec<_> = index
                .entries
                .iter()
                .filter(|e| all || Some(&e.key) == index.active.as_ref())
                .cloned()
                .collect();
            for entry in entries {
                store.delete_entry(&entry)?;
                index.entries.retain(|e| e.key != entry.key);
            }
            index.active = None;
            store.write("account-index.json", &index, false)
        })
        .await
    }

    /// A detached owned task keeps the lock through refresh and persistence even
    /// when the caller is cancelled. A durable pending marker is written BEFORE
    /// sending a rotating refresh token; crashes/timeouts never permit its reuse.
    pub async fn refresh(
        &self,
        mut client: OidcClient,
        audience: String,
    ) -> Result<TokenSet, Error> {
        let store = self.clone();
        tokio::spawn(async move {
            let locked = store.clone();
            let issuer = client.provider().issuer().to_owned();
            let client_id = client.client_id().to_owned();
            let (guard, tokens, due) = blocking(move || {
                let guard = locked.lock()?;
                let tokens = locked.load_locked()?.ok_or(Error::NotLoggedIn)?;
                if tokens.account.issuer != issuer || tokens.client_id != client_id {
                    return Err(Error::IssuerMismatch);
                }
                let due = tokens.account.expires_at <= crate::tokens::now().saturating_add(60);
                if due {
                    locked.mark_pending(true)?;
                }
                Ok((guard, tokens, due))
            })
            .await?;
            if !due {
                return Ok(tokens);
            }
            match client.refresh(&tokens, &audience).await {
                Ok(next) => {
                    let persist = store.clone();
                    let stored = next.clone();
                    blocking(move || {
                        let _guard = guard;
                        persist.save_locked(&stored)
                    })
                    .await?;
                    Ok(next)
                }
                Err(error) => {
                    // Even a transport error can follow a committed rotation. Do not
                    // send the old refresh token a second time after an unknown result.
                    let cleanup = store.clone();
                    blocking(move || {
                        let _guard = guard;
                        let mut index = cleanup.index()?;
                        if let Some(active) = index.active.take() {
                            let entry = index
                                .entries
                                .iter()
                                .find(|e| e.key == active)
                                .cloned()
                                .ok_or(Error::Storage("missing account entry"))?;
                            cleanup.delete_entry(&entry)?;
                            index.entries.retain(|e| e.key != active);
                            cleanup.write("account-index.json", &index, false)?;
                        }
                        Ok(())
                    })
                    .await?;
                    Err(error)
                }
            }
        })
        .await
        .map_err(|_| Error::Storage("refresh task failed"))?
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }
    fn lock(&self) -> Result<File, Error> {
        std::fs::create_dir_all(&self.directory)
            .map_err(|_| Error::Storage("account directory unavailable"))?;
        check_directory(&self.directory)?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options
            .open(self.path("account.lock"))
            .map_err(|_| Error::Storage("account lock unavailable"))?;
        if !file
            .metadata()
            .map_err(|_| Error::Storage("account lock unavailable"))?
            .is_file()
        {
            return Err(Error::Storage("account lock is not regular"));
        }
        for _ in 0..200 {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(25))
                }
                Err(_) => return Err(Error::Storage("account lock failed")),
            }
        }
        Err(Error::Busy)
    }
    fn index(&self) -> Result<Index, Error> {
        self.read("account-index.json", false)?
            .map_or(Ok(Index::default()), |b| decode(&b))
    }
    fn file_document(&self) -> Result<FileDocument, Error> {
        self.read("account.json", true)?
            .map_or(Ok(FileDocument::default()), |b| decode(&b))
    }
    fn load_locked(&self) -> Result<Option<TokenSet>, Error> {
        let index = self.index()?;
        let Some(active) = &index.active else {
            return Ok(None);
        };
        let entry = index
            .entries
            .iter()
            .find(|e| &e.key == active)
            .ok_or(Error::Storage("missing account entry"))?;
        let (tokens, pending) = match entry.kind {
            StorageKind::File => {
                let document = self.file_document()?;
                let record = document
                    .records
                    .into_iter()
                    .find(|r| AccountKey::of(&r.tokens) == entry.key)
                    .ok_or(Error::Storage("account credential missing"))?;
                (record.tokens, record.pending)
            }
            StorageKind::Keyring => self.load_vault(&entry.key)?,
        };
        if pending {
            return Err(Error::SessionExpired);
        }
        if AccountKey::of(&tokens) != entry.key {
            return Err(Error::Storage("credential identity mismatch"));
        }
        Ok(Some(tokens))
    }
    fn save_locked(&self, tokens: &TokenSet) -> Result<StorageKind, Error> {
        let key = AccountKey::of(tokens);
        let mut index = self.index()?;
        let kind = if let Some(vault) = &self.vault {
            // A vault failure before any write may use the Unix fallback. Once
            // writes begin, errors are reported; no plaintext downgrade after a
            // partial or uncertain keyring commit.
            match vault.get(&key.vault_name()?) {
                Err(_)
                    if cfg!(unix)
                        && !index
                            .entries
                            .iter()
                            .any(|e| e.key == key && e.kind == StorageKind::Keyring) =>
                {
                    self.save_file(tokens)?;
                    StorageKind::File
                }
                Err(error) => return Err(error),
                Ok(_) => {
                    self.save_vault(vault.as_ref(), tokens)?;
                    StorageKind::Keyring
                }
            }
        } else {
            self.save_file(tokens)?;
            StorageKind::File
        };
        // Remove an old plaintext fallback only after the vault commit succeeds.
        if kind == StorageKind::Keyring && self.path("account.json").exists() {
            let mut document = self.file_document()?;
            document
                .records
                .retain(|r| AccountKey::of(&r.tokens) != key);
            self.write("account.json", &document, true)?;
        }
        index.entries.retain(|e| e.key != key);
        index.entries.push(Entry {
            key: key.clone(),
            kind,
        });
        index.active = Some(key);
        self.write("account-index.json", &index, false)?;
        Ok(kind)
    }
    fn save_file(&self, tokens: &TokenSet) -> Result<(), Error> {
        let mut document = self.file_document()?;
        let key = AccountKey::of(tokens);
        document
            .records
            .retain(|r| AccountKey::of(&r.tokens) != key);
        document.records.push(FileRecord {
            tokens: tokens.clone(),
            pending: false,
        });
        self.write("account.json", &document, true)
    }
    fn mark_pending(&self, pending: bool) -> Result<(), Error> {
        let index = self.index()?;
        let active = index.active.ok_or(Error::NotLoggedIn)?;
        let entry = index
            .entries
            .into_iter()
            .find(|e| e.key == active)
            .ok_or(Error::Storage("missing account entry"))?;
        match entry.kind {
            StorageKind::File => {
                let mut document = self.file_document()?;
                let record = document
                    .records
                    .iter_mut()
                    .find(|r| AccountKey::of(&r.tokens) == active)
                    .ok_or(Error::Storage("account credential missing"))?;
                record.pending = pending;
                self.write("account.json", &document, true)
            }
            StorageKind::Keyring => {
                let vault = self
                    .vault
                    .as_ref()
                    .ok_or(Error::Storage("keyring unavailable"))?;
                let name = active.vault_name()?;
                let mut manifest = self.manifest(&active)?;
                manifest.pending = pending;
                let value = serde_json::to_string(&manifest)
                    .map_err(|_| Error::Storage("manifest encoding failed"))?;
                vault.put(&name, &value)?;
                if vault.get(&name)?.as_ref().map(Secret::expose) != Some(value.as_str()) {
                    return Err(Error::Storage("keyring write verification failed"));
                }
                Ok(())
            }
        }
    }
    fn manifest(&self, key: &AccountKey) -> Result<Manifest, Error> {
        let value = self
            .vault
            .as_ref()
            .ok_or(Error::Storage("keyring unavailable"))?
            .get(&key.vault_name()?)?
            .ok_or(Error::Storage("account credential missing"))?;
        let manifest: Manifest = decode(value.expose().as_bytes())?;
        if manifest.version != 1
            || manifest.key != *key
            || manifest.chunks > MAX_FILE as usize * 2 / CHUNK + 1
            || manifest.chunks == 0
            || manifest.generation.len() != 32
            || !manifest.generation.bytes().all(|b| b.is_ascii_hexdigit())
            || manifest.retired.len() > 32
            || manifest.retired.iter().any(|r| {
                r.generation.len() != 32
                    || !r.generation.bytes().all(|b| b.is_ascii_hexdigit())
                    || r.chunks == 0
                    || r.chunks > MAX_FILE as usize * 2 / CHUNK + 1
            })
        {
            return Err(Error::Storage("invalid account manifest"));
        }
        Ok(manifest)
    }
    fn load_vault(&self, key: &AccountKey) -> Result<(TokenSet, bool), Error> {
        let manifest = self.manifest(key)?;
        let vault = self
            .vault
            .as_ref()
            .ok_or(Error::Storage("keyring unavailable"))?;
        let name = key.vault_name()?;
        let mut encoded = Zeroizing::new(String::new());
        for i in 0..manifest.chunks {
            let chunk = vault
                .get(&format!("{name}.{}.{i}", manifest.generation))?
                .ok_or(Error::Storage("credential chunk missing"))?;
            if chunk.expose().len() > CHUNK {
                return Err(Error::Storage("credential chunk too large"));
            }
            encoded.push_str(chunk.expose());
        }
        let bytes = Zeroizing::new(
            STANDARD
                .decode(encoded.as_bytes())
                .map_err(|_| Error::Storage("credential decoding failed"))?,
        );
        if format!("{:x}", Sha256::digest(&bytes)) != manifest.digest {
            return Err(Error::Storage("credential binding mismatch"));
        }
        Ok((decode(&bytes)?, manifest.pending))
    }
    fn save_vault(&self, vault: &dyn CredentialVault, tokens: &TokenSet) -> Result<(), Error> {
        let key = AccountKey::of(tokens);
        let name = key.vault_name()?;
        let previous = match vault.get(&name)? {
            Some(_) => Some(self.manifest(&key)?),
            None => None,
        };
        let mut retired = previous
            .as_ref()
            .map(|m| m.retired.clone())
            .unwrap_or_default();
        if let Some(old) = &previous {
            retired.push(Retired {
                generation: old.generation.clone(),
                chunks: old.chunks,
            });
        }
        if retired.len() > 32 {
            return Err(Error::Storage("keyring cleanup required"));
        }
        let bytes = Zeroizing::new(
            serde_json::to_vec(tokens).map_err(|_| Error::Storage("credential encoding failed"))?,
        );
        if bytes.len() as u64 > MAX_FILE {
            return Err(Error::Storage("credential too large"));
        }
        let encoded = Zeroizing::new(STANDARD.encode(&bytes));
        let generation = random_generation()?;
        let parts: Vec<_> = encoded.as_bytes().chunks(CHUNK).collect();
        for (i, part) in parts.iter().enumerate() {
            let part = std::str::from_utf8(part)
                .map_err(|_| Error::Storage("credential encoding failed"))?;
            let account = format!("{name}.{generation}.{i}");
            vault.put(&account, part)?;
            if vault.get(&account)?.as_ref().map(Secret::expose) != Some(part) {
                return Err(Error::Storage("keyring write verification failed"));
            }
        }
        let mut manifest = Manifest {
            version: 1,
            key,
            generation,
            chunks: parts.len(),
            digest: format!("{:x}", Sha256::digest(&bytes)),
            pending: false,
            retired,
        };
        let value = serde_json::to_string(&manifest)
            .map_err(|_| Error::Storage("manifest encoding failed"))?;
        vault.put(&name, &value)?;
        if vault.get(&name)?.as_ref().map(Secret::expose) != Some(value.as_str()) {
            return Err(Error::Storage("keyring write verification failed"));
        }
        // Retired generations stay in the protected manifest until every chunk
        // is deleted. Logout can still remove them after a partial cleanup failure.
        let mut complete = true;
        for old in &manifest.retired {
            for i in 0..old.chunks {
                if vault
                    .delete(&format!("{name}.{}.{i}", old.generation))
                    .is_err()
                {
                    complete = false;
                }
            }
        }
        if complete && !manifest.retired.is_empty() {
            manifest.retired.clear();
            if let Ok(value) = serde_json::to_string(&manifest) {
                let _ = vault.put(&name, &value);
            }
        }
        Ok(())
    }
    fn delete_entry(&self, entry: &Entry) -> Result<(), Error> {
        match entry.kind {
            StorageKind::File => {
                let mut document = self.file_document()?;
                document
                    .records
                    .retain(|r| AccountKey::of(&r.tokens) != entry.key);
                self.write("account.json", &document, true)
            }
            StorageKind::Keyring => {
                let vault = self
                    .vault
                    .as_ref()
                    .ok_or(Error::Storage("keyring unavailable"))?;
                let name = entry.key.vault_name()?;
                let manifest = self.manifest(&entry.key)?;
                for old in &manifest.retired {
                    for i in 0..old.chunks {
                        vault.delete(&format!("{name}.{}.{i}", old.generation))?;
                    }
                }
                for i in 0..manifest.chunks {
                    vault.delete(&format!("{name}.{}.{i}", manifest.generation))?;
                }
                vault.delete(&name)?;
                if vault.get(&name)?.is_some() {
                    return Err(Error::Storage("keyring delete verification failed"));
                }
                Ok(())
            }
        }
    }
    fn read(&self, name: &str, secret: bool) -> Result<Option<Zeroizing<Vec<u8>>>, Error> {
        if secret && !cfg!(unix) {
            return Err(Error::Storage("owner-only file fallback unavailable"));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(self.path(name)) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::Storage("account file unavailable")),
        };
        let meta = file
            .metadata()
            .map_err(|_| Error::Storage("account file unavailable"))?;
        if !meta.is_file() || meta.len() > MAX_FILE {
            return Err(Error::Storage("account file is not bounded and regular"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.mode() & 0o077 != 0
                || meta.uid() != rustix::process::geteuid().as_raw()
                || meta.nlink() != 1
            {
                return Err(Error::Storage("account file is accessible to other users"));
            }
        }
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(MAX_FILE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage("account file read failed"))?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(Error::Storage("account file too large"));
        }
        Ok(Some(bytes))
    }
    fn write<T: Serialize>(&self, name: &str, value: &T, secret: bool) -> Result<(), Error> {
        if secret && !cfg!(unix) {
            return Err(Error::Storage("owner-only file fallback unavailable"));
        }
        let bytes = Zeroizing::new(
            serde_json::to_vec(value)
                .map_err(|_| Error::Storage("account file encoding failed"))?,
        );
        if bytes.len() as u64 > MAX_FILE {
            return Err(Error::Storage("account file too large"));
        }
        let mut temp = tempfile::NamedTempFile::new_in(&self.directory)
            .map_err(|_| Error::Storage("account temporary file unavailable"))?;
        temp.write_all(&bytes)
            .and_then(|_| temp.as_file().sync_all())
            .map_err(|_| Error::Storage("account file write failed"))?;
        temp.persist(self.path(name))
            .map_err(|_| Error::Storage("account file replace failed"))?;
        #[cfg(unix)]
        File::open(&self.directory)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::Storage("account directory sync failed"))?;
        Ok(())
    }
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes).map_err(|_| Error::Storage("invalid account format"))
}
fn random_generation() -> Result<String, Error> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::RandomUnavailable)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn check_directory(path: &Path) -> Result<(), Error> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|_| Error::Storage("account directory unavailable"))?;
    if !meta.is_dir() {
        return Err(Error::Storage("account directory is not regular"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o022 != 0 || meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(Error::Storage(
                "account directory is writable by other users",
            ));
        }
    }
    Ok(())
}
async fn blocking<T: Send + 'static>(
    op: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(op)
        .await
        .map_err(|_| Error::Storage("account storage task failed"))?
}

#[cfg(any(target_os = "macos", windows))]
struct NativeVault;
#[cfg(any(target_os = "macos", windows))]
impl CredentialVault for NativeVault {
    fn get(&self, account: &str) -> Result<Option<Secret>, Error> {
        let entry = keyring::Entry::new("cyber-code", account)
            .map_err(|_| Error::Storage("keyring unavailable"))?;
        match entry.get_password() {
            Ok(s) => Ok(Some(Secret::new(s))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(Error::Storage("keyring read failed")),
        }
    }
    fn put(&self, account: &str, value: &str) -> Result<(), Error> {
        keyring::Entry::new("cyber-code", account)
            .and_then(|e| e.set_password(value))
            .map_err(|_| Error::Storage("keyring write failed"))
    }
    fn delete(&self, account: &str) -> Result<(), Error> {
        let entry = keyring::Entry::new("cyber-code", account)
            .map_err(|_| Error::Storage("keyring unavailable"))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(_) => return Err(Error::Storage("keyring delete failed")),
        }
        if self.get(account)?.is_some() {
            return Err(Error::Storage("keyring delete verification failed"));
        }
        Ok(())
    }
}
fn native_vault() -> Option<Arc<dyn CredentialVault>> {
    #[cfg(any(target_os = "macos", windows))]
    {
        Some(Arc::new(NativeVault))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}
