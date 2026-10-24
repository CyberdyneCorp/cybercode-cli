//! Small private host secrets (agent capabilities, hub MAC keys), sharing account
//! storage's bounded private files and cross-process lock. Never put these in events.
use super::{AccountStore, CredentialVault, StorageKind, blocking, decode};
use crate::{Error, Secret};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub struct HostSecrets {
    store: AccountStore,
    file_name: String,
    namespace: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Document {
    #[serde(default)]
    namespace: Option<String>,
    entries: BTreeMap<String, Record>,
}
#[derive(Serialize, Deserialize)]
struct Record {
    kind: StorageKind,
    value: Option<Secret>,
}
impl HostSecrets {
    pub fn new(
        directory: PathBuf,
        file_name: &str,
        namespace: &str,
        vault: Option<Arc<dyn CredentialVault>>,
    ) -> Result<Self, Error> {
        let components: Vec<_> = Path::new(file_name).components().collect();
        if !matches!(components.as_slice(), [Component::Normal(_)])
            || !file_name.ends_with(".json")
            || file_name.len() > 128
            || namespace.is_empty()
            || namespace.len() > 256
        {
            return Err(Error::Storage("invalid host secret namespace"));
        }
        Ok(Self {
            store: AccountStore::with_vault(directory, vault),
            file_name: file_name.to_owned(),
            namespace: namespace.to_owned(),
        })
    }
    pub fn file_path(&self) -> PathBuf {
        self.store.path(&self.file_name)
    }
    pub async fn get(&self, key: &str) -> Result<Option<Secret>, Error> {
        let this = self.clone();
        let key = key.to_owned();
        blocking(move || {
            let _guard = this.store.lock()?;
            this.get_locked(&key)
        })
        .await
    }
    pub async fn put(&self, key: &str, value: Secret) -> Result<StorageKind, Error> {
        let this = self.clone();
        let key = key.to_owned();
        blocking(move || {
            let _guard = this.store.lock()?;
            this.put_locked(&key, value)
        })
        .await
    }
    pub async fn delete(&self, key: &str) -> Result<(), Error> {
        let this = self.clone();
        let key = key.to_owned();
        blocking(move || {
            let _guard = this.store.lock()?;
            let name = this.vault_name(&key)?;
            let mut doc = this.document()?;
            if let Some(vault) = &this.store.vault {
                // A known file fallback has never been committed to the vault.
                // Keep it removable when the native service remains unavailable.
                if doc
                    .entries
                    .get(&key)
                    .is_some_and(|r| r.kind == StorageKind::File)
                    && vault.get(&name).is_err()
                {
                    doc.entries.remove(&key);
                    return this.store.write(&this.file_name, &doc, false);
                }
                vault.delete(&name)?;
                if vault.get(&name)?.is_some() {
                    return Err(Error::Storage("keyring delete verification failed"));
                }
            }
            doc.entries.remove(&key);
            this.store.write(&this.file_name, &doc, false)
        })
        .await
    }
    fn vault_name(&self, key: &str) -> Result<String, Error> {
        if key.is_empty() || key.len() > 256 {
            return Err(Error::Storage("invalid host secret key"));
        }
        let directory = std::fs::canonicalize(&self.store.directory)
            .map_err(|_| Error::Storage("host secret directory unavailable"))?;
        let binding = serde_json::to_vec(&(directory, &self.file_name, &self.namespace, key))
            .map_err(|_| Error::Storage("invalid host secret directory"))?;
        Ok(format!("host-secret:{:x}", Sha256::digest(binding)))
    }
    fn document(&self) -> Result<Document, Error> {
        let mut document: Document = self
            .store
            .read(&self.file_name, false)?
            .map_or(Ok(Document::default()), |bytes| decode(&bytes))?;
        match document.namespace.as_deref() {
            Some(namespace) if namespace != self.namespace => {
                return Err(Error::Storage("host secret namespace mismatch"));
            }
            None if !document.entries.is_empty() => {
                return Err(Error::Storage("host secret namespace missing"));
            }
            _ => {}
        }
        document.namespace = Some(self.namespace.clone());
        Ok(document)
    }
    fn get_locked(&self, key: &str) -> Result<Option<Secret>, Error> {
        let name = self.vault_name(key)?;
        let mut doc = self.document()?;
        if let Some(vault) = &self.store.vault {
            match vault.get(&name) {
                Ok(Some(value)) => {
                    if value.expose().len() > 2000 {
                        return Err(Error::Storage("host secret too large"));
                    }
                    // Recover a native commit interrupted before the index write,
                    // removing any obsolete plaintext fallback at the same time.
                    if !doc
                        .entries
                        .get(key)
                        .is_some_and(|r| r.kind == StorageKind::Keyring && r.value.is_none())
                    {
                        doc.entries.insert(
                            key.to_owned(),
                            Record {
                                kind: StorageKind::Keyring,
                                value: None,
                            },
                        );
                        this_write(self, &doc)?;
                    }
                    return Ok(Some(value));
                }
                Err(error)
                    if !doc
                        .entries
                        .get(key)
                        .is_some_and(|r| r.kind == StorageKind::File) =>
                {
                    return Err(error);
                }
                _ => {}
            }
        }
        match doc.entries.get(key) {
            Some(record) if record.kind == StorageKind::File && cfg!(unix) => {
                Ok(record.value.clone())
            }
            Some(record) if record.kind == StorageKind::File => {
                Err(Error::Storage("owner-only file fallback unavailable"))
            }
            Some(_) => Err(Error::Storage(
                "native host secret missing or commit incomplete",
            )),
            _ => Ok(None),
        }
    }
    fn put_locked(&self, key: &str, value: Secret) -> Result<StorageKind, Error> {
        let name = self.vault_name(key)?;
        if value.expose().is_empty() || value.expose().len() > 2000 {
            return Err(Error::Storage("host secret must contain 1 to 2000 bytes"));
        }
        let mut doc = self.document()?;
        let kind = if let Some(vault) = &self.store.vault {
            match vault.get(&name) {
                Err(_)
                    if cfg!(unix)
                        && !doc
                            .entries
                            .get(key)
                            .is_some_and(|r| r.kind == StorageKind::Keyring) =>
                {
                    StorageKind::File
                }
                Err(error) => return Err(error),
                Ok(_) => {
                    // Commit the no-fallback decision before any native write.
                    // An uncertain write must never revive a previous file secret.
                    doc.entries.insert(
                        key.to_owned(),
                        Record {
                            kind: StorageKind::Keyring,
                            value: None,
                        },
                    );
                    this_write(self, &doc)?;
                    vault.put(&name, value.expose())?;
                    if vault.get(&name)?.as_ref().map(Secret::expose) != Some(value.expose()) {
                        return Err(Error::Storage("keyring write verification failed"));
                    }
                    StorageKind::Keyring
                }
            }
        } else if cfg!(unix) {
            StorageKind::File
        } else {
            return Err(Error::Storage("owner-only file fallback unavailable"));
        };
        doc.entries.insert(
            key.to_owned(),
            Record {
                kind,
                value: if kind == StorageKind::File {
                    Some(value)
                } else {
                    None
                },
            },
        );
        this_write(self, &doc)?;
        Ok(kind)
    }
}
fn this_write(this: &HostSecrets, doc: &Document) -> Result<(), Error> {
    this.store.write(&this.file_name, doc, false)
}
