//! Platform-neutral capability report (design F1, F3, F12). Version 1 is the
//! macOS-only shape; this module defines version 2, which keeps every version 1
//! field with its macOS meaning and adds the platform-neutral facts.

use serde::{Deserialize, Serialize};

use crate::ReasonCode;

/// The newest computer schema version; the server always emits it.
pub const SCHEMA_VERSION: u32 = 2;
/// The version of rows and responses written before the platform-neutral contract.
pub const LEGACY_SCHEMA_VERSION: u32 = 1;
/// The platform the original contract shipped on; later platforms are evidence-gated.
pub const FOUNDATION_PLATFORM: &str = "macos";

pub fn legacy_schema_version() -> u32 {
    LEGACY_SCHEMA_VERSION
}

/// Derived from capture and input availability; never set independently.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ComputerMode")]
pub enum Mode {
    #[default]
    Unsupported,
    CaptureOnly,
    InputEnabled,
}

impl Mode {
    pub fn derive(capture: bool, input: bool) -> Self {
        match (capture, input) {
            (false, _) => Self::Unsupported,
            (true, false) => Self::CaptureOnly,
            (true, true) => Self::InputEnabled,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerCapability")]
pub struct Capability {
    pub available: bool,
    /// Every code that applies, most actionable first; empty when available.
    pub reasons: Vec<ReasonCode>,
}

/// Whether the platform can prove the pre-dispatch target checks.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ComputerInputAssurance")]
pub enum Assurance {
    Verified,
    Unavailable,
    #[default]
    NotApplicable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerInputCapability")]
pub struct InputCapability {
    pub available: bool,
    pub reasons: Vec<ReasonCode>,
    pub assurance: Assurance,
}

/// What makes capture legitimate besides Cyber's owner review, which is always required.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ComputerAuthorityKind")]
pub enum AuthorityKind {
    OsPermission,
    PlatformPicker,
    #[default]
    OwnerReviewOnly,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerAuthority")]
pub struct Authority {
    pub kind: AuthorityKind,
    /// Whether an explicit owner `setup` request can ask the OS for anything.
    pub setup_available: bool,
}

/// Interactive-session facts; `null` means the platform cannot tell and is never "safe".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerSessionFacts")]
pub struct SessionFacts {
    pub interactive: Option<bool>,
    pub locked: Option<bool>,
    pub remote: Option<bool>,
    pub kind: Option<String>,
    pub reasons: Vec<ReasonCode>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerOsInfo")]
pub struct OsInfo {
    pub family: String,
    pub version: Option<String>,
    pub build: Option<String>,
    pub architecture: String,
}

/// Selector kinds a platform may honor (design F4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ComputerSelectorKind")]
pub enum SelectorKind {
    App,
    Display,
    Window,
    Pick,
}

impl SelectorKind {
    /// Re-resolvable selectors resolve deterministically without a human choice.
    pub fn re_resolvable(self) -> bool {
        matches!(self, Self::App | Self::Display)
    }
}

/// Support evidence of the running configuration (design F12).
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ComputerEvidence")]
pub enum Evidence {
    #[default]
    Unsupported,
    Implemented,
    FixtureTested,
    Accepted,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerDiagnosticInterface")]
pub struct Interface {
    pub name: String,
    pub version: String,
}

/// Diagnostic values are never used for decisions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerDiagnostics")]
pub struct Diagnostics {
    pub interfaces: Vec<Interface>,
    pub desktop: Option<String>,
}

/// One row of the compiled support-evidence table, mirrored in docs/computer-use.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EvidenceRow {
    pub platform: &'static str,
    pub configuration: &'static str,
    pub level: Evidence,
}

/// `accepted` requires a recorded run on a real interactive desktop with a disposable
/// fixture; compilation, hosted CI and fake adapters never count. macOS stays
/// `implemented` until its own acceptance run is recorded.
pub const EVIDENCE: &[EvidenceRow] = &[
    EvidenceRow {
        platform: "macos",
        configuration: "macOS 14 or newer, console session",
        level: Evidence::Implemented,
    },
    EvidenceRow {
        platform: "linux",
        configuration: "any desktop",
        level: Evidence::Unsupported,
    },
    EvidenceRow {
        platform: "windows",
        configuration: "any edition",
        level: Evidence::Unsupported,
    },
];

/// The compiled evidence level of a platform; an unlisted platform is unsupported.
pub fn evidence_for(platform: &str) -> Evidence {
    EVIDENCE
        .iter()
        .find(|row| row.platform == platform)
        .map_or(Evidence::Unsupported, |row| row.level)
}

/// Platforms added after macOS may enable input only for `accepted` configurations.
pub fn input_gated(platform: &str, evidence: Evidence) -> bool {
    platform != FOUNDATION_PLATFORM && evidence != Evidence::Accepted
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerHealth")]
pub struct Health {
    pub supported: bool,
    pub platform: String,
    pub minimum_version: String,
    pub screen_capture: bool,
    pub accessibility: bool,
    pub binary: String,
    /// 1 for the macOS-only shape, 2 for the platform-neutral report.
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub capture: Capability,
    #[serde(default)]
    pub input: InputCapability,
    #[serde(default)]
    pub authority: Authority,
    #[serde(default)]
    pub session: SessionFacts,
    #[serde(default)]
    pub os: OsInfo,
    #[serde(default)]
    pub selectors: Vec<SelectorKind>,
    #[serde(default)]
    pub evidence: Evidence,
    #[serde(default)]
    pub diagnostics: Diagnostics,
}

impl Health {
    /// The report of a host without any native computer-use adapter.
    pub fn unsupported(platform: &str, binary: String) -> Self {
        let reasons = vec![ReasonCode::UnsupportedPlatform];
        Self {
            supported: false,
            platform: platform.into(),
            minimum_version: "macOS 14".into(),
            binary,
            schema_version: SCHEMA_VERSION,
            backend: "unsupported".into(),
            capture: Capability {
                available: false,
                reasons: reasons.clone(),
            },
            input: InputCapability {
                available: false,
                reasons,
                assurance: Assurance::NotApplicable,
            },
            authority: Authority {
                kind: AuthorityKind::OwnerReviewOnly,
                setup_available: false,
            },
            os: os(platform),
            evidence: evidence_for(platform),
            ..Self::default()
        }
        .finish()
    }

    /// Completes a backend report: a version 1 report (only the macOS fields) is
    /// mapped field by field as design §12 states, then the evidence gate runs and
    /// the mode is derived. The result is always the newest schema version.
    pub fn normalize(mut self) -> Self {
        if self.schema_version < SCHEMA_VERSION {
            self.upgrade_legacy();
        }
        self.finish()
    }

    /// Marks both capabilities unavailable because the owner disabled the feature.
    /// Legacy fields keep the backend's values.
    pub fn disabled(mut self) -> Self {
        for reasons in [&mut self.capture.reasons, &mut self.input.reasons] {
            reasons.retain(|code| *code != ReasonCode::FeatureDisabled);
            reasons.insert(0, ReasonCode::FeatureDisabled);
        }
        self.capture.available = false;
        self.input.available = false;
        self.mode = Mode::derive(false, false);
        self
    }

    fn upgrade_legacy(&mut self) {
        let screen = self.supported && self.screen_capture;
        let input = screen && self.accessibility;
        let host = if self.platform == FOUNDATION_PLATFORM {
            ReasonCode::UnsupportedOsVersion
        } else {
            ReasonCode::UnsupportedPlatform
        };
        let missing = |granted: bool| {
            if !self.supported {
                vec![host]
            } else if granted {
                Vec::new()
            } else {
                vec![ReasonCode::OsPermissionMissing]
            }
        };
        self.capture = Capability {
            available: screen,
            reasons: missing(screen),
        };
        self.input = InputCapability {
            available: input,
            reasons: missing(input),
            assurance: match (self.supported, self.accessibility) {
                (false, _) => Assurance::NotApplicable,
                (true, true) => Assurance::Verified,
                (true, false) => Assurance::Unavailable,
            },
        };
        self.authority = Authority {
            kind: AuthorityKind::OsPermission,
            setup_available: self.supported,
        };
        self.session = SessionFacts::default();
        self.os = os(&self.platform);
        self.selectors = if self.supported {
            vec![SelectorKind::App, SelectorKind::Display]
        } else {
            Vec::new()
        };
        self.backend = match (self.platform.as_str(), self.supported) {
            (FOUNDATION_PLATFORM, _) => "macos-screencapturekit",
            (_, false) => "unsupported",
            (_, true) => "legacy",
        }
        .into();
        self.evidence = evidence_for(&self.platform);
    }

    fn finish(mut self) -> Self {
        if self.input.available && input_gated(&self.platform, self.evidence) {
            self.input.available = false;
            self.input.reasons.push(ReasonCode::EvidencePending);
        }
        if self.platform != FOUNDATION_PLATFORM {
            // No legacy field reports an OS permission that does not exist.
            self.screen_capture = self.capture.available;
            self.accessibility = self.input.available;
        }
        self.schema_version = SCHEMA_VERSION;
        self.mode = Mode::derive(self.capture.available, self.input.available);
        self
    }
}

fn os(platform: &str) -> OsInfo {
    OsInfo {
        family: platform.into(),
        version: None,
        build: None,
        architecture: std::env::consts::ARCH.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(platform: &str, supported: bool, screen: bool, access: bool) -> Health {
        Health {
            supported,
            platform: platform.into(),
            minimum_version: "macOS 14".into(),
            screen_capture: screen,
            accessibility: access,
            binary: "/fixture/cyber".into(),
            ..Health::default()
        }
    }

    #[test]
    fn mode_is_derived_from_capture_and_input_availability() {
        assert_eq!(Mode::derive(false, false), Mode::Unsupported);
        assert_eq!(Mode::derive(false, true), Mode::Unsupported);
        assert_eq!(Mode::derive(true, false), Mode::CaptureOnly);
        assert_eq!(Mode::derive(true, true), Mode::InputEnabled);
    }

    #[test]
    fn macos_legacy_report_maps_field_by_field_and_keeps_legacy_values() {
        let health = legacy("macos", true, true, true).normalize();
        assert_eq!(health.schema_version, 2);
        assert_eq!(health.mode, Mode::InputEnabled);
        assert!(health.capture.available && health.capture.reasons.is_empty());
        assert!(health.input.available && health.input.reasons.is_empty());
        assert_eq!(health.input.assurance, Assurance::Verified);
        assert_eq!(health.authority.kind, AuthorityKind::OsPermission);
        assert!(health.authority.setup_available);
        assert_eq!(health.session, SessionFacts::default());
        assert_eq!(
            health.selectors,
            vec![SelectorKind::App, SelectorKind::Display]
        );
        assert_eq!(health.backend, "macos-screencapturekit");
        assert_eq!(health.evidence, Evidence::Implemented);
        assert_eq!(health.os.family, "macos");
        assert!(health.supported && health.screen_capture && health.accessibility);
        assert_eq!(health.minimum_version, "macOS 14");
    }

    #[test]
    fn macos_missing_permissions_and_old_versions_report_codes_but_keep_legacy_values() {
        let capture_only = legacy("macos", true, true, false).normalize();
        assert_eq!(capture_only.mode, Mode::CaptureOnly);
        assert_eq!(
            capture_only.input.reasons,
            vec![ReasonCode::OsPermissionMissing]
        );
        assert_eq!(capture_only.input.assurance, Assurance::Unavailable);
        assert!(!capture_only.accessibility, "legacy value unchanged");

        let no_screen = legacy("macos", true, false, true).normalize();
        assert_eq!(no_screen.mode, Mode::Unsupported);
        assert_eq!(
            no_screen.capture.reasons,
            vec![ReasonCode::OsPermissionMissing]
        );
        assert!(no_screen.accessibility, "legacy value unchanged on macOS");

        let old = legacy("macos", false, false, false).normalize();
        assert_eq!(old.capture.reasons, vec![ReasonCode::UnsupportedOsVersion]);
        assert_eq!(old.input.assurance, Assurance::NotApplicable);
        assert!(old.selectors.is_empty());
    }

    #[test]
    fn version_one_json_without_new_fields_still_deserializes() {
        let value = serde_json::json!({"supported":true,"platform":"macos","minimum_version":"macOS 14","screen_capture":true,"accessibility":false,"binary":""});
        let health: Health = serde_json::from_value(value).unwrap();
        assert_eq!(health.schema_version, 0);
        let health = health.normalize();
        assert_eq!(health.mode, Mode::CaptureOnly);
        let json = serde_json::to_value(&health).unwrap();
        for legacy in [
            "supported",
            "platform",
            "minimum_version",
            "screen_capture",
            "accessibility",
            "binary",
        ] {
            assert!(json.get(legacy).is_some(), "{legacy} kept");
        }
    }

    #[test]
    fn unsupported_host_reports_no_fictional_permission() {
        let health = Health::unsupported("linux", String::new());
        assert_eq!(health.mode, Mode::Unsupported);
        assert_eq!(health.authority.kind, AuthorityKind::OwnerReviewOnly);
        assert!(!health.authority.setup_available);
        assert_eq!(
            health.capture.reasons,
            vec![ReasonCode::UnsupportedPlatform]
        );
        assert_eq!(health.input.assurance, Assurance::NotApplicable);
        assert!(!health.screen_capture && !health.accessibility);
        assert!(health.selectors.is_empty());
        assert_eq!(health.evidence, Evidence::Unsupported);
    }

    #[test]
    fn evidence_table_lists_each_platform_once_and_macos_is_not_yet_accepted() {
        let mut platforms = std::collections::BTreeSet::new();
        for row in EVIDENCE {
            assert!(platforms.insert(row.platform), "{}", row.platform);
        }
        assert_eq!(evidence_for("macos"), Evidence::Implemented);
        assert_eq!(evidence_for("linux"), Evidence::Unsupported);
        assert_eq!(evidence_for("windows"), Evidence::Unsupported);
        assert_eq!(evidence_for("plan9"), Evidence::Unsupported);
    }

    #[test]
    fn later_platforms_enable_input_only_for_accepted_configurations() {
        for level in [
            Evidence::Unsupported,
            Evidence::Implemented,
            Evidence::FixtureTested,
        ] {
            assert!(input_gated("linux", level));
            assert!(input_gated("windows", level));
            assert!(!input_gated("macos", level), "macOS keeps today's behavior");
            let health = Health {
                platform: "windows".into(),
                schema_version: SCHEMA_VERSION,
                capture: Capability {
                    available: true,
                    reasons: Vec::new(),
                },
                input: InputCapability {
                    available: true,
                    reasons: Vec::new(),
                    assurance: Assurance::Verified,
                },
                evidence: level,
                ..Health::default()
            }
            .normalize();
            assert_eq!(health.mode, Mode::CaptureOnly, "{level:?}");
            assert_eq!(health.input.reasons, vec![ReasonCode::EvidencePending]);
            assert!(health.screen_capture && !health.accessibility);
        }
        assert!(!input_gated("linux", Evidence::Accepted));
        let accepted = Health {
            platform: "linux".into(),
            schema_version: SCHEMA_VERSION,
            capture: Capability {
                available: true,
                reasons: Vec::new(),
            },
            input: InputCapability {
                available: true,
                reasons: Vec::new(),
                assurance: Assurance::Verified,
            },
            evidence: Evidence::Accepted,
            ..Health::default()
        }
        .normalize();
        assert_eq!(accepted.mode, Mode::InputEnabled);
    }

    #[test]
    fn a_legacy_report_from_a_later_platform_cannot_bypass_the_evidence_gate() {
        let health = legacy("linux", true, true, true).normalize();
        assert_eq!(health.evidence, Evidence::Unsupported);
        assert_eq!(health.mode, Mode::CaptureOnly);
        assert!(health.input.reasons.contains(&ReasonCode::EvidencePending));
        assert!(!health.accessibility);
    }

    #[test]
    fn disabled_feature_is_reported_first_without_changing_legacy_fields() {
        let health = legacy("macos", true, true, true).normalize().disabled();
        assert_eq!(health.mode, Mode::Unsupported);
        assert_eq!(health.capture.reasons, vec![ReasonCode::FeatureDisabled]);
        assert_eq!(health.input.reasons[0], ReasonCode::FeatureDisabled);
        assert!(health.screen_capture && health.accessibility);
        let twice = health.disabled();
        assert_eq!(twice.capture.reasons, vec![ReasonCode::FeatureDisabled]);
    }
}
