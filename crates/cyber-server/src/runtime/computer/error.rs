#[derive(Debug, thiserror::Error)]
pub enum ComputerError {
    #[error(transparent)]
    Native(#[from] cyber_computer::Error),
    #[error("ComputerStorageError: durable computer evidence is unavailable")]
    Storage,
    #[error("ComputerInvalidRequestError: {0}")]
    Invalid(&'static str),
    #[error("ComputerConflictError: the operation or reviewed scope changed")]
    Conflict,
    #[error(
        "ComputerUnknownOutcomeError: review and acknowledge prior unknown input before arming again"
    )]
    Unknown,
    #[error("ComputerFrameExpiredError: private frame is unavailable or expired")]
    FrameExpired,
}

impl From<cyber_store::StoreError> for ComputerError {
    fn from(_: cyber_store::StoreError) -> Self {
        Self::Storage
    }
}
impl From<std::io::Error> for ComputerError {
    fn from(_: std::io::Error) -> Self {
        Self::Storage
    }
}
pub type Result<T> = std::result::Result<T, ComputerError>;
