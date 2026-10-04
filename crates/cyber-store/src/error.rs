use std::path::PathBuf;

use rusqlite::ErrorCode;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The writer queue stayed full for the admission timeout. Retryable.
    #[error("StorageBusyError: the writer queue is full; retry with the same message ID")]
    Busy,
    /// Disk full, I/O error or corruption. New mutations stop until restart.
    #[error("StorageUnavailableError: {0}")]
    Unavailable(String),
    #[error("DatabaseTooNewError: upgrade cyber to >= the version that wrote this database (unknown migrations: {})", .0.join(", "))]
    TooNew(Vec<String>),
    #[error("ConcurrencyError: aggregate {aggregate} is at seq {actual}, expected {expected}")]
    Concurrency {
        aggregate: String,
        expected: i64,
        actual: i64,
    },
    #[error(
        "unregistered event type {0}: writing an event without a registered schema is a defect"
    )]
    UnregisteredEvent(String),
    #[error("invalid event type {0:?}: expected <domain>.<name>.<version>")]
    InvalidEventType(String),
    #[error("event {kind} failed validation: {reason}")]
    InvalidEvent { kind: String, reason: String },
    #[error("projector failed while applying {kind}: {reason}")]
    Projector { kind: String, reason: String },
    #[error("InvalidLimitError: limit must be between 1 and 500, got {0}")]
    InvalidLimit(u32),
    #[error("the live database cannot be on a network filesystem ({0}); use a local disk")]
    NetworkFilesystem(String),
    #[error("another cyber server holds the writer lock {}", .0.display())]
    LockHeld(PathBuf),
    #[error("corrupt event {id}: {reason}")]
    CorruptEvent { id: String, reason: String },
    #[error("database error: {0}")]
    Sqlite(#[source] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl StoreError {
    /// Fatal errors stop every later mutation (`Durability boundary and storage failure`).
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        let fatal = matches!(
            e.sqlite_error_code(),
            Some(
                ErrorCode::DiskFull
                    | ErrorCode::SystemIoFailure
                    | ErrorCode::DatabaseCorrupt
                    | ErrorCode::NotADatabase
                    | ErrorCode::CannotOpen
            )
        );
        if fatal {
            Self::Unavailable(e.to_string())
        } else {
            Self::Sqlite(e)
        }
    }
}
