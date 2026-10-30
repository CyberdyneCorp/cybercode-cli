//! The injection signal rule table (`untrusted-content` → Injection signal detection).
//!
//! Patterns run on normalized text: NFKC, lower case, hidden characters removed, each
//! whitespace run collapsed to one space (or one line break when it held one, so rules
//! can still anchor at a line start). Phrase rules are English; markup, URL and hidden
//! text rules are language-neutral. Every pattern is a `regex` (linear-time) pattern,
//! and `\b` in it is an ASCII word boundary: the detector compiles it as `(?-u:\b)`,
//! since a Unicode boundary would push matching off the fast engines on non-ASCII text.
//!
//! Any change to a rule raises [`DETECTOR_VERSION`].

use super::SignalKind;

/// The version of this rule table, recorded on every `untrusted` record.
pub const DETECTOR_VERSION: u32 = 2;

/// How a rule finds hits in the normalized text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matcher {
    /// Every match is a hit.
    Pattern(&'static str),
    /// Every match is a hit, but only when a rule that is not a companion also hit in
    /// the same text: a pattern common in honest pages (`curl … | sh` in an install
    /// guide) that marks an injection only beside another signal.
    Companion(&'static str),
    /// A match of `first` within `within` characters (either side) of a match of
    /// `second` is a hit. A `second` match where `unless` matches (anchored at the match's
    /// start) does not count.
    Near {
        first: &'static str,
        second: &'static str,
        within: usize,
        unless: Option<&'static str>,
    },
    /// A `url` match whose query or fragment holds a `placeholder` match, or whose path
    /// holds a `path_placeholder` match, is a hit. A path placeholder must name data
    /// (`{conversation}`, `[output]`): REST routes such as `/users/{id}` are not hits.
    UrlWith {
        url: &'static str,
        placeholder: &'static str,
        path_placeholder: &'static str,
    },
    /// At least one bidi control, tag character, ESC or other control, or at least
    /// `zero_width` zero-width characters outside emoji sequences, in the original text.
    Hidden { zero_width: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub kind: SignalKind,
    pub matcher: Matcher,
}

const fn pattern(kind: SignalKind, pattern: &'static str) -> Rule {
    Rule {
        kind,
        matcher: Matcher::Pattern(pattern),
    }
}

/// Paths and names of secrets.
const SECRET: &str = r"\.ssh\b|\bid_rsa|\bid_ed25519|(?:^|[\s/~`'(=:@])\.env\b|\.aws/credentials|\.npmrc\b|\.netrc\b|\.git-credentials\b|\bapi[ _-]?keys?\b|\baccess[ _]tokens?\b|\bprivate[ _]keys?\b|\bpasswords?\b";
/// A verb that moves data, with where it goes: a URL, an e-mail address, a reply or a
/// public place (`to the issue`, `in the comments`), or `curl` with a data flag. A verb
/// without a destination (`an API key sent in the Authorization header`) is no hit.
const TRANSFER: &str = r"\b(?:send|sent|upload|post|e-?mail|paste|forward|transmit|leak|exfiltrate|submit|share|attach|append|fetch)\w*\b[^\n]{0,80}?(?:https?://|\b[\w.+-]+@[\w-]+\.[a-z]{2,}\b|\b(?:to|in|into) (?:the|this|that|our|my|your|a) (?:next )?(?:issue|ticket|url|link|form|chat|thread|comments?|reply|response|answer|address|inbox|webhook|pastebin|gist|build server)\b)|\bcurl\b[^\n]{0,80}?\s(?:-d|--data\S*|-f|--form|-t|--upload-file)\s";
/// A transfer of requests or telemetry rather than of a secret (`send a POST request to
/// https://…`), which API documentation says beside its key and token names.
const TRANSFER_UNLESS: &str = r"^\w+\s(?:(?:a|an|the|your|our|all|any|these|those)\s)?(?:\w+\s)?(?:requests?|calls?|queries|traffic|events|logs|metrics|webhooks?|payloads?|messages?)\b";

/// The v2 table, one or more rules per kind. `hidden_text` runs on the original text.
pub const RULES: &[Rule] = &[
    pattern(
        SignalKind::OverrideInstructions,
        r"\b(?:ignore|disregard|forget|override)\s(?:(?:all|any|the|of)\s){0,3}(?:(?:previous|prior|above|earlier|your|system)\s){1,2}(?:instructions?|rules|prompts?|guidelines)\b|\bnew instructions\s?:|\byou are now\b|\bfrom now on,?\syou\b|\b(?:do not|don't|don’t|dont)\stell\sthe\suser\b",
    ),
    pattern(
        SignalKind::RoleMarkup,
        r"(?m)</?system>|<\|im_start\|>|\[inst\]|<\s*/?\s*(?:untrusted_content|cross_session_message|mcp_instructions)|^(?:system|assistant)\s?:|^###\s?system\b",
    ),
    Rule {
        kind: SignalKind::SecretAccess,
        matcher: Matcher::Near {
            first: SECRET,
            second: TRANSFER,
            within: 200,
            unless: Some(TRANSFER_UNLESS),
        },
    },
    pattern(
        SignalKind::ExfilUrl,
        r"!\[[^\]\n]*\]\(\s?https?://[^\s)]*\?[^\s)]*\)",
    ),
    Rule {
        kind: SignalKind::ExfilUrl,
        matcher: Matcher::UrlWith {
            url: r#"https?://[^\s"'`]+"#,
            placeholder: r"\$\{[^}\s]*\}|\{[^{}\s]+\}|\$[a-z_][a-z0-9_]*|<[^<>\s/][^<>\s]*>|\[[^\[\]\s]+\]",
            path_placeholder: r"[{<\[$][^{}<>\[\]\s/]*(?:data|secret|token|conversation|history|env|password|credential|context|chat|prompt|output|response|answer|summary|clipboard|contents|home|pwd|hostname)[^{}<>\[\]\s/]*[}>\]]?",
        },
    },
    // Requests aimed at the agent, or to act at once without asking. A bare "run the
    // following command" is how install guides talk to people.
    pattern(
        SignalKind::CommandRequest,
        r"\b(?:assistants?|agents?|ai|bots?|models?|llms?|copilots?)\b[^.\n]{0,40}\b(?:run|execute|paste)\s(?:(?:this|these|the following)\s)?(?:commands?\b|in (?:your|the|a) (?:terminal|shell)\b)|\b(?:run|execute|paste|do)\s(?:this|these|it|that|the following)(?:\s\w+){0,2}?\s(?:immediately|right away|silently|without (?:asking|telling|confirm\w*|review\w*))\b|\bbase64\s(?:-d|--decode)\b[^\n|]{0,60}\||\bpowershell(?:\.exe)?\b[^\n]{0,60}\s-e(?:nc(?:odedcommand)?)?\s|\biex\s?\(",
    ),
    // Install guides pipe downloads to a shell and clean build trees: an injection
    // only beside another signal.
    Rule {
        kind: SignalKind::CommandRequest,
        matcher: Matcher::Companion(
            r"\b(?:curl|wget)\b[^\n|]{0,300}\|\s?(?:sudo\s)?(?:ba|z)?sh\b|\|\s?(?:sudo\s)?(?:ba|z)sh\b|\brm\s-(?:rf|fr)\b",
        ),
    },
    pattern(
        SignalKind::PermissionChange,
        r"\bbypass mode\b|\bmode (?:to )?bypass\b|--dangerously-|(?:^|\s)/mode\s|(?:^|\s)/approve\b|\bapprove the pending\b|\bdisable the sandbox\b",
    ),
    Rule {
        kind: SignalKind::PermissionChange,
        matcher: Matcher::Near {
            first: r#"\bcyber\.jsonc?\b|\bpermissions?["']?\s?[:=]"#,
            second: r#"["']?\ballow\b["']?"#,
            within: 80,
            unless: None,
        },
    },
    Rule {
        kind: SignalKind::HiddenText,
        matcher: Matcher::Hidden { zero_width: 3 },
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind has a rule, and the table compiles through the detector's own path
    /// (with `\b` as an ASCII boundary), not a separate one.
    #[test]
    fn every_kind_has_a_rule_and_the_table_compiles() {
        for kind in SignalKind::ALL {
            assert!(RULES.iter().any(|rule| rule.kind == kind), "{kind:?}");
        }
        super::super::Detector::new(RULES).unwrap_or_else(|e| panic!("{e}"));
    }
}
