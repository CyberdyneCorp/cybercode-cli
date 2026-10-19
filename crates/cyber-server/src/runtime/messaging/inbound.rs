//! Inbound controls (`cross-session-messaging` → Inbound controls, Non-interactive
//! sessions; design section 6). A target decides per message with its Location's
//! `messaging.inbound` and its Session override: a `refuse` pattern matching any of the
//! sender's addresses wins over a `hold` match, which wins over a `deliver` match; without
//! a match the Session override applies, then the default (`deliver` for same-machine
//! senders, `hold` for cross-machine ones). Patterns use the permission wildcard syntax.
//!
//! Only a sender's trusted addresses (see [`SenderAddresses`]) can satisfy a `deliver` or
//! `hold` pattern; its other addresses can only restrict it (`refuse`, or `hold` instead
//! of `deliver`), so a name another Session chose never lets a message in.

use cyber_core::config::{InboundPolicy, InboundSettings};
use serde::{Deserialize, Serialize};

/// The refusal reason of a `refuse` pattern, default or override without its own reason.
pub const INBOUND_RULE: &str = "inbound_rule";

/// Why a Session override refuses; the reason a refused sender sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OverrideReason {
    /// Set by `cyber exec` on the Sessions it creates, unless `--accept-messages`.
    NonInteractive,
}

impl OverrideReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NonInteractive => "non_interactive",
        }
    }
}

/// A Session-level `messaging.inbound` override. It replaces only the default; explicit
/// sender patterns still win.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InboundOverride {
    pub inbound: InboundPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<OverrideReason>,
}

/// What a target does with one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Deliver,
    Hold,
    /// Refused with this reason; the target records metadata only.
    Refuse(&'static str),
}

/// Where a message comes from. Only same-machine senders exist until the direct peers
/// change; the cross-machine default is kept as its extension point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    SameMachine,
    CrossMachine,
}

/// The addresses a target's inbound patterns match for one sender.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SenderAddresses {
    /// Addresses no other Session can claim: a Session's ID, its root title when no other
    /// root Session has it, a client's labels. They match every pattern.
    pub trusted: Vec<String>,
    /// Names a sender may share or have been given by a model (a duplicated title, a
    /// subagent name, the reply address) and every ancestor's ID and names. They match
    /// only the patterns that restrict a message: `refuse`, and `hold` over `deliver`.
    pub other: Vec<String>,
}

impl SenderAddresses {
    /// A sender known only by trusted addresses (a client, a remote peer).
    pub fn trusted(addresses: Vec<String>) -> Self {
        Self {
            trusted: addresses,
            other: Vec::new(),
        }
    }

    /// Add `address` as an untrusted one unless the sender already has it.
    pub fn add_other(&mut self, address: String) {
        if !self.trusted.contains(&address) && !self.other.contains(&address) {
            self.other.push(address);
        }
    }

    fn all(&self) -> impl Iterator<Item = &String> {
        self.trusted.iter().chain(&self.other)
    }
}

fn matches_any<'a>(patterns: &[String], mut addresses: impl Iterator<Item = &'a String>) -> bool {
    addresses.any(|address| {
        patterns
            .iter()
            .any(|pattern| cyber_core::wildcard::matches(pattern, address))
    })
}

/// The inbound decision for a sender known by `addresses`.
pub fn decide(
    settings: &InboundSettings,
    session: Option<&InboundOverride>,
    addresses: &SenderAddresses,
    reach: Reach,
) -> Verdict {
    if matches_any(&settings.refuse, addresses.all()) {
        return Verdict::Refuse(INBOUND_RULE);
    }
    let trusted = || addresses.trusted.iter();
    let decided = if matches_any(&settings.hold, trusted()) {
        Verdict::Hold
    } else if matches_any(&settings.deliver, trusted()) {
        Verdict::Deliver
    } else {
        match (session, reach) {
            (Some(o), _) => verdict(o.inbound, o.reason.map_or(INBOUND_RULE, |r| r.as_str())),
            (None, Reach::CrossMachine) => Verdict::Hold,
            (None, Reach::SameMachine) => verdict(settings.default, INBOUND_RULE),
        }
    };
    if decided == Verdict::Deliver && matches_any(&settings.hold, addresses.all()) {
        return Verdict::Hold;
    }
    decided
}

fn verdict(policy: InboundPolicy, reason: &'static str) -> Verdict {
    match policy {
        InboundPolicy::Deliver => Verdict::Deliver,
        InboundPolicy::Hold => Verdict::Hold,
        InboundPolicy::Refuse => Verdict::Refuse(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(deliver: &[&str], hold: &[&str], refuse: &[&str]) -> InboundSettings {
        let list = |l: &[&str]| l.iter().map(|p| (*p).to_owned()).collect();
        InboundSettings {
            default: InboundPolicy::Deliver,
            deliver: list(deliver),
            hold: list(hold),
            refuse: list(refuse),
        }
    }

    fn from(addresses: &[&str]) -> SenderAddresses {
        SenderAddresses::trusted(addresses.iter().map(|a| (*a).to_owned()).collect())
    }

    const NON_INTERACTIVE: InboundOverride = InboundOverride {
        inbound: InboundPolicy::Refuse,
        reason: Some(OverrideReason::NonInteractive),
    };

    #[test]
    fn refuse_wins_over_hold_and_hold_over_deliver() {
        let frontend = from(&["ses_01JF", "frontend"]);
        let same = Reach::SameMachine;
        // Scenario "Refuse wins over deliver".
        let settings = rules(&["front*"], &[], &["frontend"]);
        assert_eq!(
            decide(&settings, None, &frontend, same),
            Verdict::Refuse(INBOUND_RULE)
        );
        let settings = rules(&["front*"], &["ses_01J*"], &[]);
        assert_eq!(decide(&settings, None, &frontend, same), Verdict::Hold);
        let settings = rules(&["front*"], &["back*"], &["noisy-*"]);
        assert_eq!(decide(&settings, None, &frontend, same), Verdict::Deliver);
        // Any of the sender's addresses matches: its ID as well as its name.
        let settings = rules(&[], &[], &["ses_01JF"]);
        assert_eq!(
            decide(&settings, None, &frontend, same),
            Verdict::Refuse(INBOUND_RULE)
        );
    }

    /// Regression: a name another Session could claim (a duplicated title, a subagent
    /// name, an ancestor's) never satisfies `deliver` or `hold`, but still refuses.
    #[test]
    fn untrusted_addresses_only_restrict() {
        let same = Reach::SameMachine;
        let mut spoof = from(&["ses_01SPOOF"]);
        spoof.add_other("lead".into());
        let mut allowlist = rules(&["lead"], &[], &[]);
        allowlist.default = InboundPolicy::Refuse;
        assert_eq!(
            decide(&allowlist, None, &spoof, same),
            Verdict::Refuse(INBOUND_RULE)
        );
        let mut held = rules(&[], &["lead"], &[]);
        held.default = InboundPolicy::Refuse;
        assert_eq!(
            decide(&held, None, &spoof, same),
            Verdict::Refuse(INBOUND_RULE)
        );
        // Restricting patterns match every address.
        let refuse = rules(&[], &[], &["lead"]);
        assert_eq!(
            decide(&refuse, None, &spoof, same),
            Verdict::Refuse(INBOUND_RULE)
        );
        let hold = rules(&[], &["lead"], &[]);
        assert_eq!(decide(&hold, None, &spoof, same), Verdict::Hold);
        // A trusted address still delivers.
        assert_eq!(
            decide(&allowlist, None, &from(&["ses_01J", "lead"]), same),
            Verdict::Deliver
        );
    }

    #[test]
    fn the_session_override_replaces_only_the_default() {
        let client = from(&["client:api", "client"]);
        let same = Reach::SameMachine;
        let none = rules(&[], &[], &[]);
        assert_eq!(
            decide(&none, Some(&NON_INTERACTIVE), &client, same),
            Verdict::Refuse("non_interactive")
        );
        let hold = InboundOverride {
            inbound: InboundPolicy::Hold,
            reason: None,
        };
        assert_eq!(decide(&none, Some(&hold), &client, same), Verdict::Hold);
        let plain_refuse = InboundOverride {
            inbound: InboundPolicy::Refuse,
            reason: None,
        };
        assert_eq!(
            decide(&none, Some(&plain_refuse), &client, same),
            Verdict::Refuse(INBOUND_RULE)
        );
        // An explicit sender pattern still wins over the override.
        let delivers_clients = rules(&["client"], &[], &[]);
        assert_eq!(
            decide(&delivers_clients, Some(&NON_INTERACTIVE), &client, same),
            Verdict::Deliver
        );
    }

    #[test]
    fn defaults_deliver_same_machine_and_hold_cross_machine_senders() {
        let peer = from(&["build@desk"]);
        let mut settings = rules(&[], &[], &[]);
        assert_eq!(
            decide(&settings, None, &peer, Reach::SameMachine),
            Verdict::Deliver
        );
        assert_eq!(
            decide(&settings, None, &peer, Reach::CrossMachine),
            Verdict::Hold
        );
        // `default` is the same-machine default only.
        settings.default = InboundPolicy::Refuse;
        assert_eq!(
            decide(&settings, None, &peer, Reach::SameMachine),
            Verdict::Refuse(INBOUND_RULE)
        );
        assert_eq!(
            decide(&settings, None, &peer, Reach::CrossMachine),
            Verdict::Hold
        );
        settings.deliver = vec!["*@desk".into()];
        assert_eq!(
            decide(&settings, None, &peer, Reach::CrossMachine),
            Verdict::Deliver
        );
    }

    #[test]
    fn the_override_round_trips_and_rejects_unknown_fields() {
        let value = serde_json::to_value(NON_INTERACTIVE).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"inbound": "refuse", "reason": "non_interactive"})
        );
        assert_eq!(
            serde_json::from_value::<InboundOverride>(value).unwrap(),
            NON_INTERACTIVE
        );
        for bad in [
            serde_json::json!({"inbound": "drop"}),
            serde_json::json!({"inbound": "refuse", "reason": "busy"}),
            serde_json::json!({"inbound": "refuse", "extra": 1}),
        ] {
            assert!(serde_json::from_value::<InboundOverride>(bad).is_err());
        }
    }
}
