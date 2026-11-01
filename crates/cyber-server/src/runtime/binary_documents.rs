//! Server-lifetime ownership for live binary views. IDs never confer authority.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyber_core::binary::{Document, Error, Metadata};

pub(crate) struct Entry {
    pub session_id: String,
    pub directory: String,
    pub root: PathBuf,
    pub path: PathBuf,
    pub document: Mutex<Document>,
    used: Mutex<std::time::Instant>,
}

#[derive(Default)]
pub(crate) struct Registry(
    Mutex<HashMap<String, Arc<Entry>>>,
    std::sync::OnceLock<Arc<tokio::sync::Semaphore>>,
    std::sync::OnceLock<Arc<cyber_core::binary::storage::StorageRoot>>,
);

impl Registry {
    pub fn worker(&self) -> Result<tokio::sync::OwnedSemaphorePermit, Error> {
        self.1
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(16)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Limit)
    }

    pub fn insert(
        &self,
        session_id: String,
        directory: String,
        root: PathBuf,
        document: Document,
    ) -> Result<(String, Metadata), Error> {
        let mut documents = self.0.lock().map_err(|_| Error::Io)?;
        documents.retain(|_, entry| {
            entry
                .used
                .lock()
                .is_ok_and(|used| used.elapsed() < std::time::Duration::from_secs(24 * 60 * 60))
        });
        if documents.len() >= 16
            || documents
                .values()
                .filter(|d| d.session_id == session_id)
                .count()
                >= 4
        {
            return Err(Error::Limit);
        }
        let id = cyber_core::ids::new_id("bin");
        let metadata = document.metadata();
        documents.insert(
            id.clone(),
            Arc::new(Entry {
                session_id,
                directory,
                root,
                path: document.path(),
                document: Mutex::new(document),
                used: Mutex::new(std::time::Instant::now()),
            }),
        );
        Ok((id, metadata))
    }

    pub fn get(&self, id: &str) -> Option<Arc<Entry>> {
        let entry = self.0.lock().ok()?.get(id).cloned()?;
        *entry.used.lock().ok()? = std::time::Instant::now();
        Some(entry)
    }

    pub fn remove_sessions(&self, ids: &[String]) {
        if let Ok(mut documents) = self.0.lock() {
            documents.retain(|_, entry| !ids.contains(&entry.session_id));
        }
    }

    pub fn remove(&self, id: &str, entry: &Arc<Entry>) {
        if let Ok(mut documents) = self.0.lock()
            && documents
                .get(id)
                .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            documents.remove(id);
        }
    }
}

impl super::Runtime {
    /// Install only after bootstrap has prepared and hidden the existing root from
    /// all child launches. An unconfigured runtime keeps binary editing disabled.
    pub fn configure_binary_storage(
        &self,
        root: Arc<cyber_core::binary::storage::StorageRoot>,
    ) -> Result<(), Error> {
        self.inner
            .binary_documents
            .2
            .set(root)
            .map_err(|_| Error::Changed)
    }

    pub(crate) fn binary_documents(&self) -> &Registry {
        &self.inner.binary_documents
    }
}
