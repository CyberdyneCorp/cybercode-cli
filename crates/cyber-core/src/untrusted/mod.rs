//! Untrusted external content (`untrusted-content`): hidden character exposure, the
//! injection signal detector and the `untrusted` record events carry.
//!
//! The detector lives here rather than in `cyber-tools` because both the tool registry
//! (framed results) and the runtime (admitted cross-session messages and Channel events)
//! scan text, and the runtime cannot depend on the tool crate.

mod detect;
mod hidden;
mod notice;
mod rules;
mod settings;
mod taint;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use detect::{Detector, scan};
pub use hidden::{VALUE_LIMIT, expose_hidden, is_hidden, visible_value, without_userinfo};
pub use notice::{BypassSetting, NoticePolicy, TaintSetting, consequence};
pub use rules::{DETECTOR_VERSION, Matcher, RULES, Rule};
pub use settings::{TaintedAllow, UntrustedSettings, restrict_project_layer};
pub use taint::{RunTaint, TaintSource};

/// At most this many hits are recorded per scanned text; all are counted.
pub const MAX_RECORDED: usize = 10;
/// An excerpt holds at most this many characters.
pub const EXCERPT_LIMIT: usize = 120;
/// At most this many bytes of a text are scanned.
pub const SCAN_LIMIT: usize = 256 * 1024;

/// A kind of injection signal, in the order frames and records list them.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    OverrideInstructions,
    RoleMarkup,
    SecretAccess,
    ExfilUrl,
    CommandRequest,
    PermissionChange,
    HiddenText,
}

impl SignalKind {
    pub const ALL: [SignalKind; 7] = [
        Self::OverrideInstructions,
        Self::RoleMarkup,
        Self::SecretAccess,
        Self::ExfilUrl,
        Self::CommandRequest,
        Self::PermissionChange,
        Self::HiddenText,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OverrideInstructions => "override_instructions",
            Self::RoleMarkup => "role_markup",
            Self::SecretAccess => "secret_access",
            Self::ExfilUrl => "exfil_url",
            Self::CommandRequest => "command_request",
            Self::PermissionChange => "permission_change",
            Self::HiddenText => "hidden_text",
        }
    }
}

/// One recorded hit: its kind and a short, redacted excerpt of the matched text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Signal {
    pub kind: SignalKind,
    pub excerpt: String,
}

/// What a scan found: up to [`MAX_RECORDED`] signals, at least one of each kind that
/// hit, and the count of all hits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    pub signals: Vec<Signal>,
    pub count: u32,
}

impl Scan {
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The distinct kinds that hit, in [`SignalKind`] order.
    pub fn kinds(&self) -> Vec<SignalKind> {
        kinds(&self.signals)
    }

    /// Both scans' hits, as if one text held them all.
    pub fn merge(self, other: Scan) -> Scan {
        let mut signals = self.signals;
        signals.extend(other.signals);
        Scan {
            signals: select(signals),
            count: self.count.saturating_add(other.count),
        }
    }
}

/// `signals` cut to [`MAX_RECORDED`] (see [`chosen`]).
fn select(signals: Vec<Signal>) -> Vec<Signal> {
    let picked = chosen(signals.iter().map(|s| s.kind));
    let mut signals: Vec<Option<Signal>> = signals.into_iter().map(Some).collect();
    picked
        .into_iter()
        .filter_map(|index| signals[index].take())
        .collect()
}

/// The indices of the hits to record, out of hits listed in order: the first hit of
/// each kind, then the others in order, up to [`MAX_RECORDED`], returned in kind order
/// (stable within a kind).
pub(crate) fn chosen(kinds: impl Iterator<Item = SignalKind>) -> Vec<usize> {
    let kinds: Vec<SignalKind> = kinds.collect();
    let mut kept = vec![false; kinds.len()];
    let mut room = MAX_RECORDED;
    for kind in SignalKind::ALL {
        if room == 0 {
            break;
        }
        if let Some(first) = kinds.iter().position(|k| *k == kind) {
            kept[first] = true;
            room -= 1;
        }
    }
    for flag in kept.iter_mut().filter(|flag| !**flag).take(room) {
        *flag = true;
    }
    let mut picked: Vec<usize> = (0..kinds.len()).filter(|&i| kept[i]).collect();
    picked.sort_by_key(|&i| kinds[i]);
    picked
}

fn kinds(signals: &[Signal]) -> Vec<SignalKind> {
    let mut kinds: Vec<SignalKind> = signals.iter().map(|s| s.kind).collect();
    kinds.sort();
    kinds.dedup();
    kinds
}

/// Which tool output the detector scans: `permissions.untrusted_content.scan`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanScope {
    /// Framed external results only (the default).
    #[default]
    External,
    /// Also the output of `read`, `grep`, `bash` and `powershell`, without rewriting it.
    All,
}

impl ScanScope {
    /// The effective scope in a merged config. Scanning more is the safe side, so any
    /// value other than an absent key or `"external"` scans everything.
    pub fn from_config(config: &serde_json::Value) -> Self {
        match config.pointer("/permissions/untrusted_content/scan") {
            None => Self::External,
            Some(value) if value.as_str() == Some("external") => Self::External,
            Some(_) => Self::All,
        }
    }
}

/// The `untrusted` record of `session.tool.settled.1` and `session.prompt.admitted.1`
/// (`untrusted-content` → Injection signal notices). Events without it read as having
/// no signals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UntrustedRecord {
    /// The frame's `source` (`webfetch`, `mcp`, …), the workspace tool for a
    /// `scan: "all"` hit (`read`, `grep`, `bash`, `powershell`), or `session` /
    /// `channel` for an admitted message.
    pub source: String,
    /// Where the content came from, without userinfo and cut to 512 characters.
    pub origin: String,
    pub detector_version: u32,
    #[serde(default)]
    pub signals: Vec<Signal>,
    /// All hits, including those past the recorded ten.
    #[serde(default)]
    pub signal_count: u32,
}

impl UntrustedRecord {
    pub fn new(source: &str, origin: &str, scan: Scan) -> Self {
        Self {
            source: source.into(),
            origin: visible_value(&without_userinfo(origin)),
            detector_version: DETECTOR_VERSION,
            signals: scan.signals,
            signal_count: scan.count,
        }
    }

    pub fn has_signals(&self) -> bool {
        self.signal_count > 0 || !self.signals.is_empty()
    }

    /// The distinct kinds recorded, in [`SignalKind`] order.
    pub fn kinds(&self) -> Vec<SignalKind> {
        kinds(&self.signals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(kind: SignalKind, excerpt: &str) -> Signal {
        Signal {
            kind,
            excerpt: excerpt.into(),
        }
    }

    /// The cap keeps one hit of every kind that hit, even when one kind floods.
    #[test]
    fn selection_keeps_every_kind_under_the_cap() {
        let mut signals: Vec<Signal> = (0..30)
            .map(|i| signal(SignalKind::RoleMarkup, &i.to_string()))
            .collect();
        signals.push(signal(SignalKind::ExfilUrl, "late"));
        signals.push(signal(SignalKind::OverrideInstructions, "later"));
        let kept = select(signals);
        assert_eq!(kept.len(), MAX_RECORDED);
        assert_eq!(kept[0], signal(SignalKind::OverrideInstructions, "later"));
        assert_eq!(kept[1], signal(SignalKind::RoleMarkup, "0"));
        assert_eq!(kept[MAX_RECORDED - 1], signal(SignalKind::ExfilUrl, "late"));
    }

    #[test]
    fn scan_scope_reads_the_setting_and_errs_toward_scanning() {
        let scope = |value: serde_json::Value| ScanScope::from_config(&value);
        assert_eq!(scope(serde_json::json!({})), ScanScope::External);
        let set = |scan: serde_json::Value| {
            scope(serde_json::json!({"permissions": {"untrusted_content": {"scan": scan}}}))
        };
        assert_eq!(set("external".into()), ScanScope::External);
        assert_eq!(set("all".into()), ScanScope::All);
        assert_eq!(set("everything".into()), ScanScope::All);
        assert_eq!(set(serde_json::Value::Null), ScanScope::All);
    }

    #[test]
    fn merged_scans_union_kinds_and_add_counts() {
        let a = Scan {
            signals: vec![signal(SignalKind::ExfilUrl, "a")],
            count: 3,
        };
        let b = Scan {
            signals: vec![signal(SignalKind::OverrideInstructions, "b")],
            count: 1,
        };
        let merged = a.merge(b);
        assert_eq!(
            merged.kinds(),
            [SignalKind::OverrideInstructions, SignalKind::ExfilUrl]
        );
        assert_eq!(merged.count, 4);
        assert!(Scan::default().merge(Scan::default()).is_empty());
    }

    /// An event written before this field existed, or a record without signals, reads as
    /// having no signals; a full record round-trips.
    #[test]
    fn records_round_trip_and_default_to_no_signals() {
        let record = UntrustedRecord::new(
            "webfetch",
            "https://me:secret@x.example/a",
            Scan {
                signals: vec![signal(SignalKind::ExfilUrl, "![x](https://x.example/?d=1)")],
                count: 2,
            },
        );
        assert_eq!(record.origin, "https://x.example/a");
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["signals"][0]["kind"], "exfil_url");
        assert_eq!(json["detector_version"], DETECTOR_VERSION);
        let back: UntrustedRecord = serde_json::from_value(json).unwrap();
        assert_eq!(back, record);
        assert_eq!(back.kinds(), [SignalKind::ExfilUrl]);
        let bare: UntrustedRecord = serde_json::from_value(serde_json::json!({
            "source": "mcp", "origin": "mcp:docs/search", "detector_version": 1
        }))
        .unwrap();
        assert!(!bare.has_signals());
    }
}
