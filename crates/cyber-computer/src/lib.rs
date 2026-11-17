//! Native computer-use types. Client coordinates are image pixels, never unchecked
//! desktop coordinates. Runtime grants and native execution are separate boundaries.

pub mod action;
pub mod backend;
pub mod capability;
pub mod chord;
pub mod control;
pub mod geometry;
pub mod reason;

pub use action::{Action, Button, KeyChord, Modifier};
pub use geometry::{DesktopPoint, Geometry, ImagePoint, Rect};
pub use reason::ReasonCode;

/// Consent messages shared by the controller and adapters; `Error::reason` maps them.
pub mod consent {
    pub const PRESENCE: &str = "an attached human is required";
    pub const MODE: &str = "current Mode or permission rules deny this action";
    pub const OS: &str = "native OS consent is required";
    pub const SUPERVISOR: &str = "resolve pending human approvals before native input";
    pub const OS_PERMISSION_MISSING: &str =
        "Screen Recording or Accessibility permission is missing";
    pub const SCREEN_RECORDING: &str = "Screen Recording permission is required";
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error(
        "ComputerUnsupportedPlatformError: native computer use is not available on this platform (unsupported_platform)"
    )]
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
    /// A policy refusal decided before admission, journal intent or any native call.
    #[error("{class}: {1} ({0})", class = refusal_class(*.0))]
    Refused(ReasonCode, &'static str),
    /// An adapter delivered only part of an input action; the outcome is unknown.
    #[error("ComputerNativeError: input was partially delivered ({posted} of {requested} events)")]
    PartialInput { requested: u32, posted: u32 },
}

/// The error class of a pre-admission refusal; class names never change (F2).
pub fn refusal_class(code: ReasonCode) -> &'static str {
    match code {
        ReasonCode::InteractiveReviewRequired => "ComputerConsentError",
        ReasonCode::FrameStale | ReasonCode::TopologyChanged => "ComputerStaleFrameError",
        _ => "ComputerInputError",
    }
}

impl Error {
    /// The registry code for this error, when one applies.
    pub fn reason(&self) -> Option<ReasonCode> {
        Some(match self {
            Self::UnsupportedPlatform => ReasonCode::UnsupportedPlatform,
            Self::Disabled => ReasonCode::FeatureDisabled,
            Self::Busy => ReasonCode::HostLeaseBusy,
            Self::StaleFrame => ReasonCode::FrameStale,
            Self::Limit(_) => ReasonCode::LimitExceeded,
            Self::Refused(code, _) => *code,
            Self::Consent(message) => return consent_reason(message),
            Self::Geometry(_)
            | Self::Input(_)
            | Self::NotArmed
            | Self::Native(_)
            | Self::PartialInput { .. } => return None,
        })
    }
}

fn consent_reason(message: &str) -> Option<ReasonCode> {
    match message {
        consent::PRESENCE => Some(ReasonCode::PresenceLost),
        consent::MODE => Some(ReasonCode::ModeForbidsInput),
        consent::SUPERVISOR => Some(ReasonCode::SupervisorFence),
        consent::OS | consent::OS_PERMISSION_MISSING | consent::SCREEN_RECORDING => {
            Some(ReasonCode::OsPermissionMissing)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_platform_text_is_platform_neutral_and_carries_a_code() {
        let text = Error::UnsupportedPlatform.to_string();
        assert!(text.starts_with("ComputerUnsupportedPlatformError:"));
        assert!(!text.contains("macOS"), "{text}");
        assert!(text.contains("unsupported_platform"));
        assert_eq!(
            Error::UnsupportedPlatform.reason(),
            Some(ReasonCode::UnsupportedPlatform)
        );
    }

    #[test]
    fn refusals_and_consent_failures_map_to_registry_codes() {
        let refused = Error::Refused(ReasonCode::KeyChordRefused, "chord is not allowed");
        assert_eq!(
            refused.to_string(),
            "ComputerInputError: chord is not allowed (key_chord_refused)"
        );
        assert_eq!(refused.reason(), Some(ReasonCode::KeyChordRefused));
        assert!(
            Error::Refused(ReasonCode::InteractiveReviewRequired, "x")
                .to_string()
                .starts_with("ComputerConsentError:")
        );
        for (message, code) in [
            (consent::PRESENCE, ReasonCode::PresenceLost),
            (consent::MODE, ReasonCode::ModeForbidsInput),
            (consent::SUPERVISOR, ReasonCode::SupervisorFence),
            (consent::OS, ReasonCode::OsPermissionMissing),
        ] {
            assert_eq!(Error::Consent(message).reason(), Some(code));
        }
        assert_eq!(Error::Consent("other").reason(), None);
        assert_eq!(Error::StaleFrame.reason(), Some(ReasonCode::FrameStale));
        assert_eq!(Error::NotArmed.reason(), None);
    }
}
