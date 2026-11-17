//! The one registry of computer reason codes (design F2). A code has one meaning
//! on every platform; a platform change may add platform-only codes in one of the
//! categories below but never reuses a shared code with a different meaning.
//! Clients treat a code they do not know as "unavailable, see text".

use serde::{Deserialize, Serialize};

/// Registry category of a reason code; used for documentation and grouping only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Host,
    Session,
    Authority,
    Backend,
    Lease,
    Target,
    Frame,
    Policy,
    Revocation,
}

macro_rules! registry {
    ($($variant:ident => ($code:literal, $category:ident)),+ $(,)?) => {
        /// Stable snake_case code for every computer refusal, unavailability,
        /// revocation and receipt reason.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
        #[serde(rename_all = "snake_case")]
        #[schemars(rename = "ComputerReasonCode", extend("x-extensible" = true))]
        pub enum ReasonCode {
            $($variant,)+
        }

        impl ReasonCode {
            /// Every registered code, in registry order.
            pub const ALL: &'static [ReasonCode] = &[$(ReasonCode::$variant,)+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(ReasonCode::$variant => $code,)+
                }
            }

            pub const fn category(self) -> Category {
                match self {
                    $(ReasonCode::$variant => Category::$category,)+
                }
            }

            /// Parses a registered code; an unknown code is `None` (treat as unavailable).
            pub fn parse(code: &str) -> Option<Self> {
                match code {
                    $($code => Some(ReasonCode::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

registry! {
    FeatureDisabled => ("feature_disabled", Host),
    UnsupportedPlatform => ("unsupported_platform", Host),
    UnsupportedOsVersion => ("unsupported_os_version", Host),
    UnsupportedArchitecture => ("unsupported_architecture", Host),
    UnsupportedBuild => ("unsupported_build", Host),
    EvidencePending => ("evidence_pending", Host),
    NoGraphicalSession => ("no_graphical_session", Session),
    NonInteractiveSession => ("non_interactive_session", Session),
    RemoteSession => ("remote_session", Session),
    SessionLocked => ("session_locked", Session),
    SessionInactive => ("session_inactive", Session),
    SessionWatchUnavailable => ("session_watch_unavailable", Session),
    OsPermissionMissing => ("os_permission_missing", Authority),
    CapturePolicyDenied => ("capture_policy_denied", Authority),
    ControllerPrivilegeUnsupported => ("controller_privilege_unsupported", Authority),
    PickerCancelled => ("picker_cancelled", Authority),
    InteractiveReviewRequired => ("interactive_review_required", Authority),
    CaptureUnavailable => ("capture_unavailable", Backend),
    InputUnavailable => ("input_unavailable", Backend),
    TargetAssuranceUnavailable => ("target_assurance_unavailable", Backend),
    SelectorUnsupportedOnPlatform => ("selector_unsupported_on_platform", Backend),
    ActionUnsupportedOnPlatform => ("action_unsupported_on_platform", Backend),
    HostLeaseBusy => ("host_lease_busy", Lease),
    HostLeaseUnavailable => ("host_lease_unavailable", Lease),
    PreviousHolderAbandoned => ("previous_holder_abandoned", Lease),
    TargetGone => ("target_gone", Target),
    TargetChanged => ("target_changed", Target),
    TargetProtected => ("target_protected", Target),
    TargetElevated => ("target_elevated", Target),
    TargetOtherUser => ("target_other_user", Target),
    TargetCaptureExcluded => ("target_capture_excluded", Target),
    TargetNotForeground => ("target_not_foreground", Target),
    SecureField => ("secure_field", Target),
    FrameStale => ("frame_stale", Frame),
    TopologyChanged => ("topology_changed", Frame),
    KeyChordRefused => ("key_chord_refused", Policy),
    ModeForbidsInput => ("mode_forbids_input", Policy),
    SupervisorFence => ("supervisor_fence", Policy),
    LimitExceeded => ("limit_exceeded", Policy),
    OwnerStop => ("owner_stop", Revocation),
    SessionInterrupt => ("session_interrupt", Revocation),
    PresenceLost => ("presence_lost", Revocation),
    GrantExpired => ("grant_expired", Revocation),
    OsPermissionRevoked => ("os_permission_revoked", Revocation),
    ServerRestart => ("server_restart", Revocation),
}

impl std::fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_snake_case_and_round_trip_through_json() {
        let mut seen = std::collections::BTreeSet::new();
        for code in ReasonCode::ALL {
            let text = code.as_str();
            assert!(seen.insert(text), "duplicate code {text}");
            assert!(
                text.bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "{text} is not snake_case"
            );
            let json = serde_json::to_string(code).unwrap();
            assert_eq!(json, format!("\"{text}\""));
            assert_eq!(serde_json::from_str::<ReasonCode>(&json).unwrap(), *code);
            assert_eq!(ReasonCode::parse(text), Some(*code));
            assert_eq!(code.to_string(), text);
        }
        assert_eq!(seen.len(), 45);
    }

    #[test]
    fn unknown_codes_are_not_parsed_as_available_or_as_another_code() {
        assert_eq!(ReasonCode::parse("x11_session_unsupported"), None);
        assert!(serde_json::from_str::<ReasonCode>("\"portal_unavailable\"").is_err());
    }

    #[test]
    fn reconciled_draft_names_map_to_one_registry_name() {
        // Linux and Windows draft spellings that the registry renamed (design §4).
        for retired in [
            "unsupported_selector",
            "unsupported_os_build",
            "source_changed",
            "portal_selection_cancelled",
        ] {
            assert_eq!(ReasonCode::parse(retired), None, "{retired}");
        }
        assert_eq!(
            ReasonCode::SelectorUnsupportedOnPlatform.category(),
            Category::Backend
        );
        assert_eq!(ReasonCode::RemoteSession.category(), Category::Session);
        assert_eq!(ReasonCode::KeyChordRefused.category(), Category::Policy);
    }
}
