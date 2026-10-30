//! The text of injection signal notices (`untrusted-content` → Injection signal notices):
//! the warning line the TUI and web dashboard show under a result or message, its
//! consequence sentence (`permissions-modes` → Taint-aware approvals), and the
//! `cyber exec` stderr warning. Clients share these so the wording cannot drift.

use super::{UntrustedRecord, visible_value};

/// `permissions.untrusted_content.taint`: which framed results taint the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaintSetting {
    /// No run becomes tainted; notices are still shown.
    Off,
    /// Results and messages with at least one signal taint the run (the default).
    #[default]
    Signal,
    /// Every framed result taints the run.
    Any,
}

/// `permissions.untrusted_content.bypass`: what taint does in `bypass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BypassSetting {
    /// Risky actions run as they would without taint (the default).
    #[default]
    Allow,
    /// Risky actions are denied.
    Deny,
}

/// The effective settings a notice's consequence depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NoticePolicy {
    pub taint: TaintSetting,
    pub bypass: BypassSetting,
}

/// What taint does to risky actions in a Session in `mode`, as the second sentence of a
/// notice; `None` when taint is off. `attached` is false for a Session with no
/// interactive client (`cyber exec`, routines, workflow agents). An unknown Mode reads as
/// one that asks, which is what the permission engine does with it.
pub fn consequence(mode: &str, attached: bool, policy: NoticePolicy) -> Option<&'static str> {
    if policy.taint == TaintSetting::Off {
        return None;
    }
    Some(match (mode, policy.bypass) {
        ("bypass", BypassSetting::Allow) => "Taint is off in bypass.",
        ("bypass", BypassSetting::Deny) => "Risky actions in this run will be denied (bypass).",
        ("dont-ask", _) => "Risky actions in this run will be denied.",
        _ if !attached => "Risky actions in this run will be denied.",
        _ => "Risky actions in this run will ask.",
    })
}

impl UntrustedRecord {
    /// The distinct kinds that hit, comma separated in kind order (`exfil_url, …`).
    pub fn kind_list(&self) -> String {
        self.kinds()
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// What the content was: a tool's `result`, or an admitted `message` for the
    /// `session` and `channel` sources.
    fn noun(&self) -> &'static str {
        match self.source.as_str() {
            "session" | "channel" => "message",
            _ => "result",
        }
    }

    /// The warning line under a result or message with signals, followed by
    /// `consequence`; `None` without signals.
    pub fn notice(&self, consequence: Option<&str>) -> Option<String> {
        if !self.has_signals() {
            return None;
        }
        let line = format!(
            "! Possible prompt injection in {} {} ({}).",
            visible_value(&self.source),
            self.noun(),
            self.kind_list()
        );
        Some(match consequence {
            Some(consequence) => format!("{line} {consequence}"),
            None => line,
        })
    }

    /// The `cyber exec` text-mode stderr line; `None` without signals. The record comes
    /// from an event, so its strings are shown without control characters again before
    /// they reach a terminal.
    pub fn exec_warning(&self) -> Option<String> {
        self.has_signals().then(|| {
            format!(
                "warning: possible prompt injection in {} {} from {}: {}",
                visible_value(&self.source),
                self.noun(),
                visible_value(&self.origin),
                self.kind_list()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::untrusted::{Scan, Signal, SignalKind};

    fn record(source: &str, kinds: &[SignalKind]) -> UntrustedRecord {
        let signals: Vec<Signal> = kinds
            .iter()
            .map(|&kind| Signal {
                kind,
                excerpt: "x".into(),
            })
            .collect();
        let count = signals.len() as u32;
        UntrustedRecord::new(source, "https://x.example/a", Scan { signals, count })
    }

    fn policy(taint: TaintSetting, bypass: BypassSetting) -> NoticePolicy {
        NoticePolicy { taint, bypass }
    }

    /// Every row of design D5's consequence list.
    #[test]
    fn consequence_follows_mode_attachment_and_settings() {
        let default = NoticePolicy::default();
        let ask = Some("Risky actions in this run will ask.");
        let denied = Some("Risky actions in this run will be denied.");
        for mode in ["default", "plan", "accept-edits", "auto", "custom"] {
            assert_eq!(consequence(mode, true, default), ask, "{mode}");
            assert_eq!(consequence(mode, false, default), denied, "{mode}");
        }
        assert_eq!(consequence("dont-ask", true, default), denied);
        assert_eq!(
            consequence("bypass", true, default),
            Some("Taint is off in bypass.")
        );
        assert_eq!(
            consequence("bypass", false, default),
            Some("Taint is off in bypass.")
        );
        let deny = policy(TaintSetting::Signal, BypassSetting::Deny);
        assert_eq!(
            consequence("bypass", true, deny),
            Some("Risky actions in this run will be denied (bypass).")
        );
        assert_eq!(consequence("default", true, deny), ask);
        let any = policy(TaintSetting::Any, BypassSetting::Allow);
        assert_eq!(consequence("default", true, any), ask);
        for mode in ["default", "dont-ask", "bypass"] {
            let off = policy(TaintSetting::Off, BypassSetting::Deny);
            assert_eq!(consequence(mode, true, off), None, "{mode}");
        }
    }

    /// Scenario "TUI notice under a fetch"'s line, kinds in kind order, and no line for a
    /// framed result without signals.
    #[test]
    fn notice_lines_name_source_kinds_and_consequence() {
        let fetch = record("webfetch", &[SignalKind::ExfilUrl]);
        assert_eq!(
            fetch
                .notice(Some("Risky actions in this run will ask."))
                .unwrap(),
            "! Possible prompt injection in webfetch result (exfil_url). Risky actions in this run will ask."
        );
        let both = record(
            "mcp",
            &[SignalKind::ExfilUrl, SignalKind::OverrideInstructions],
        );
        assert_eq!(
            both.notice(None).unwrap(),
            "! Possible prompt injection in mcp result (override_instructions, exfil_url)."
        );
        let message = record("session", &[SignalKind::PermissionChange]);
        assert_eq!(
            message.notice(None).unwrap(),
            "! Possible prompt injection in session message (permission_change)."
        );
        let clean = record("webfetch", &[]);
        assert_eq!(clean.notice(Some("x")), None);
        assert_eq!(clean.exec_warning(), None);
    }

    /// The stderr line names the origin; an origin with a terminal escape (an event
    /// written by an older or tampered store) reaches stderr as a marker, not as ESC.
    #[test]
    fn exec_warning_names_the_origin_without_escapes() {
        let mut fetch = record("webfetch", &[SignalKind::OverrideInstructions]);
        assert_eq!(
            fetch.exec_warning().unwrap(),
            "warning: possible prompt injection in webfetch result from https://x.example/a: override_instructions"
        );
        fetch.origin = "https://x.example/\u{1b}[2J\u{202e}".into();
        let line = fetch.exec_warning().unwrap();
        assert!(!line.contains(['\u{1b}', '\u{202e}']), "{line}");
        assert!(line.contains("https://x.example/[2J[U+202E]"), "{line}");
    }
}
