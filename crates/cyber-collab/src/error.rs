use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, thiserror::Error, Serialize, Deserialize, JsonSchema)]
#[error("{tag}: {message}")]
pub struct Error {
    #[serde(rename = "_tag")]
    pub tag: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}
impl Error {
    pub fn new(tag: &str, message: &str) -> Self {
        Self {
            tag: tag.into(),
            message: message.into(),
            current_revision: None,
            retry_after_ms: None,
        }
    }
    pub fn invalid(message: &str) -> Self {
        Self::new("InvalidRequestError", message)
    }
    pub fn forbidden() -> Self {
        Self::new(
            "ForbiddenError",
            "project role does not permit this operation",
        )
    }
    pub fn missing() -> Self {
        Self::new("ProjectNotFoundError", "project or resource not found")
    }
    pub fn conflict(revision: u64) -> Self {
        Self {
            current_revision: Some(revision),
            ..Self::new(
                "RevisionConflictError",
                "state changed; inspect the current revision",
            )
        }
    }
    pub fn storage() -> Self {
        Self::new(
            "StorageUnavailableError",
            "collaboration storage is unavailable",
        )
    }
}
impl From<cyber_store::StoreError> for Error {
    fn from(error: cyber_store::StoreError) -> Self {
        match error {
            cyber_store::StoreError::Busy => {
                Self::new("StorageBusyError", "retry with the same operation ID")
            }
            _ => Self::storage(),
        }
    }
}
impl From<crate::repository::InvalidRepository> for Error {
    fn from(_: crate::repository::InvalidRepository) -> Self {
        Self::new(
            "RepositoryMismatchError",
            "repository does not match the confirmed project",
        )
    }
}
impl From<cyber_account::Error> for Error {
    fn from(error: cyber_account::Error) -> Self {
        let message = error.to_string();
        let tag = message
            .split(':')
            .next()
            .unwrap_or("AccountError")
            .to_owned();
        Self {
            tag,
            message,
            current_revision: None,
            retry_after_ms: None,
        }
    }
}
