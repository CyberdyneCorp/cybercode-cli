//! The storage every subscription login shares (`provider-credentials`): an owner-only
//! login document with a cross-process refresh lock ([`LoginFile`], [`refresh_locked`])
//! and the cached model list the synchronous catalog build reads ([`ModelCache`]).
//!
//! Provider modules (`chatgpt`, `copilot`) own their login type, their requests and their
//! error messages; this module owns only the files. Its errors never carry a token.
mod login;
mod models;

pub use login::{Credential, LoginFile, refresh_locked};
pub use models::{MODEL_MAX_AGE, ModelCache};

/// Why a stored-login or model-cache operation failed. Never carries a token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("not signed in")]
    NotSignedIn,
    #[error("login storage: {0}")]
    Storage(&'static str),
    #[error("login is being refreshed by another process; try again")]
    Busy,
}

/// Unix seconds now; `0` when the clock is before the epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Run a file operation on the blocking pool.
async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<T, StoreError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| StoreError::Storage("login file task failed"))?
}
