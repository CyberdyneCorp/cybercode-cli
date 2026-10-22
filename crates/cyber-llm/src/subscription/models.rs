//! A subscription's cached model list (`{fetched_at, models}`), written at login and
//! refreshed at most once a day, so the synchronous catalog build never waits on the
//! network. A failed refresh keeps the old list and waits another day.
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{StoreError, now};

/// A cached list older than this is refreshed before the next request.
pub const MODEL_MAX_AGE: u64 = 24 * 60 * 60;

#[derive(Serialize, Deserialize)]
struct Document<M> {
    fetched_at: u64,
    models: Vec<M>,
}

/// One provider's model-list cache at `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCache<M> {
    path: PathBuf,
    model: PhantomData<fn() -> M>,
}

impl<M: Serialize + DeserializeOwned> ModelCache<M> {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            model: PhantomData,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The cached models; empty when nothing usable is cached.
    pub fn load(&self) -> Vec<M> {
        self.document().map(|d| d.models).unwrap_or_default()
    }

    /// Whether the cache is missing or older than [`MODEL_MAX_AGE`].
    pub fn stale(&self) -> bool {
        self.document()
            .is_none_or(|d| d.fetched_at.saturating_add(MODEL_MAX_AGE) <= now())
    }

    pub fn save(&self, models: Vec<M>) -> Result<(), StoreError> {
        let document = Document {
            fetched_at: now(),
            models,
        };
        let bytes = serde_json::to_vec_pretty(&document)
            .map_err(|_| StoreError::Storage("model list encoding failed"))?;
        cyber_core::trust::write_private_atomic(&self.path, &bytes)
            .map_err(|_| StoreError::Storage("model list write failed"))
    }

    /// Cache a refresh's result. A failure re-saves the old list, so the next attempt
    /// waits [`MODEL_MAX_AGE`]; a failure to write is ignored, as the list is only a cache.
    pub fn record<E>(&self, fetched: Result<Vec<M>, E>) {
        let _ = self.save(fetched.unwrap_or_else(|_| self.load()));
    }

    pub fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }

    fn document(&self) -> Option<Document<M>> {
        let bytes = std::fs::read(&self.path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(dir: &Path) -> ModelCache<String> {
        ModelCache::new(dir.join("test-models.json"))
    }

    #[test]
    fn the_cache_round_trips_and_ages() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache(dir.path());
        assert!(cache.stale());
        assert!(cache.load().is_empty());
        cache.save(vec!["m".into()]).unwrap();
        assert!(!cache.stale());
        assert_eq!(cache.load(), ["m"]);
        let old = format!(
            r#"{{"fetched_at":{},"models":["m"]}}"#,
            now() - MODEL_MAX_AGE
        );
        std::fs::write(cache.path(), old).unwrap();
        assert!(cache.stale());
        assert_eq!(cache.load(), ["m"]);
        cache.remove();
        assert!(!cache.path().exists());
    }

    #[test]
    fn a_failed_refresh_keeps_the_old_list_for_another_day() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache(dir.path());
        let old = format!(
            r#"{{"fetched_at":{},"models":["m"]}}"#,
            now() - MODEL_MAX_AGE
        );
        std::fs::write(cache.path(), old).unwrap();
        cache.record(Err::<Vec<String>, _>("offline"));
        assert!(!cache.stale());
        assert_eq!(cache.load(), ["m"]);
        cache.record(Ok::<_, ()>(vec!["n".into()]));
        assert_eq!(cache.load(), ["n"]);
    }
}
