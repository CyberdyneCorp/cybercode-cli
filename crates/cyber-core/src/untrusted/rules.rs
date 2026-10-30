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
pub const DETECTOR_VERSION: u32 = 1;

/// How a rule finds hits in the normalized text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matcher {
    /// Every match is a hit.
    Pattern(&'static str),
    /// A match of `first` within `within` characters (either side) of a match of
    /// `second` is a hit.
    Near {
        first: &'static str,
        second: &'static str,
        within: usize,
    },
    /// A `url` match whose path or query (everything after the host) holds a
    /// `placeholder` match is a hit.
    UrlWith {
        url: &'static str,
        placeholder: &'static str,
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
const SECRET: &str = r"\.ssh\b|\bid_rsa|\bid_ed25519|(?:^|[\s/~`'(=:])\.env\b|\.aws/credentials|\.npmrc\b|\.netrc\b|\.git-credentials\b|\bapi[ _-]?key|\baccess[ _]token|\bprivate[ _]key|\bpassword";
/// Verbs that move data somewhere.
const TRANSFER: &str = r"\b(?:send(?:s|ing)?|sent|upload(?:s|ed|ing)?|post(?:s|ed|ing)?|curl|fetch(?:es|ed|ing)?|e-?mail(?:s|ed|ing)?|paste[sd]?|pasting|include[sd]?|including|append(?: \S+){0,3} to the url)\b";

/// The v1 table, one or more rules per kind. `hidden_text` runs on the original text.
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
        },
    },
    pattern(
        SignalKind::CommandRequest,
        r"\b(?:run|execute|paste)\s(?:(?:this|the following)\s)?(?:commands?\b|in (?:your|the|a) (?:terminal|shell)\b)|\bcurl\b[^\n|]{0,300}\|\s?(?:sudo\s)?(?:ba|z)?sh\b|\|\s?(?:sudo\s)?bash\b|\bbase64\s(?:-d|--decode)\b[^\n|]{0,60}\||\bpowershell(?:\.exe)?\b[^\n]{0,60}\s-e(?:nc(?:odedcommand)?)?\s|\biex\s?\(|\brm\s-(?:rf|fr)\b",
    ),
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

    #[test]
    fn every_kind_has_a_rule_and_every_pattern_compiles() {
        for kind in SignalKind::ALL {
            assert!(RULES.iter().any(|rule| rule.kind == kind), "{kind:?}");
        }
        for rule in RULES {
            let patterns: Vec<&str> = match rule.matcher {
                Matcher::Pattern(p) => vec![p],
                Matcher::Near { first, second, .. } => vec![first, second],
                Matcher::UrlWith { url, placeholder } => vec![url, placeholder],
                Matcher::Hidden { .. } => Vec::new(),
            };
            for p in patterns {
                regex::Regex::new(p).unwrap_or_else(|e| panic!("{p}: {e}"));
            }
        }
    }
}
