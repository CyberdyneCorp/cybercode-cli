//! Native computer-use types. Client coordinates are image pixels, never unchecked
//! desktop coordinates. Runtime grants and native execution are separate boundaries.

pub mod action;
pub mod backend;
pub mod control;
pub mod geometry;

pub use action::{Action, Button, KeyChord, Modifier};
pub use geometry::{DesktopPoint, Geometry, ImagePoint, Rect};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("ComputerUnsupportedPlatformError: native computer use requires macOS 14 or newer")]
    UnsupportedPlatform,
    #[error("ComputerGeometryError: {0}")]
    Geometry(&'static str),
    #[error("ComputerInputError: {0}")]
    Input(&'static str),
    #[error("ComputerDisabledError: native computer use is disabled")]
    Disabled,
    #[error("ComputerNotArmedError: the Session has no active owner control grant")]
    NotArmed,
    #[error("ComputerConsentError: {0}")]
    Consent(&'static str),
    #[error("ComputerBusyError: another grant or native operation is active")]
    Busy,
    #[error("ComputerStaleFrameError: capture a fresh matching target frame")]
    StaleFrame,
    #[error("ComputerLimitError: {0}")]
    Limit(&'static str),
    #[error("ComputerNativeError: {0}")]
    Native(&'static str),
}
