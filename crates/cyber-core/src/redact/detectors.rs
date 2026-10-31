//! The built-in credential detectors, one list for every caller (`tool-registry` → Tool
//! output redaction): tool output bound for a model, memory writes, git text, telemetry
//! and exports. Generic high-entropy text is never redacted, since hashes and IDs are
//! legitimately random.

use regex::Regex;
use std::sync::LazyLock;

/// How a detector's match is checked before it is replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    /// The whole match is the secret.
    Token,
    /// A name such as `password`, a separator and a value; only the value is the secret,
    /// and it must look like one ([`assigned`] splits the match).
    Assignment,
    /// The `v` group is the secret, at least `min` characters long.
    Value { min: usize },
}

pub(super) struct Detector {
    pub kind: &'static str,
    pub regex: Regex,
    pub shape: Shape,
}

/// Names that make `NAME=value` and `"name": "value"` a credential assignment.
const SECRET_WORDS: &str = r"api[_-]?key|access[_-]?key|secret|token|passw(?:or)?d|credential|private[_-]?key|auth[_-]?key|session[_-]?key|client[_-]?key";

/// `(kind, pattern, shape)` in priority order: when two matches cover the same text, the
/// earlier kind names it. Word boundaries are ASCII so the scan stays on the DFA.
fn table() -> Vec<(&'static str, String, Shape)> {
    let token = |kind, pattern: &str| (kind, pattern.to_string(), Shape::Token);
    vec![
        token(
            "private_key",
            r"-----BEGIN[ A-Z0-9]*PRIVATE KEY(?: BLOCK)?-----(?:(?s:.*?)-----END[ A-Z0-9]*PRIVATE KEY(?: BLOCK)?-----|(?:\r?\n(?:[A-Za-z-]+:[^\r\n]*|[A-Za-z0-9+/=]+\r?(?m:$)))*)",
        ),
        token("aws_access_key", r"(?-u:\b)(?:AKIA|ASIA)[0-9A-Z]{16}(?-u:\b)"),
        token(
            "github_token",
            r"(?-u:\b)(?:gh[pousr]_[A-Za-z0-9]{8,}|github_pat_[A-Za-z0-9_]{8,})",
        ),
        token("anthropic_key", r"(?-u:\b)sk-ant-[A-Za-z0-9_-]{8,}"),
        token(
            "openai_key",
            r"(?-u:\b)sk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{8,}",
        ),
        token("slack_token", r"(?-u:\b)xox[abposr]-[A-Za-z0-9-]{10,}"),
        token("stripe_key", r"(?-u:\b)[rs]k_live_[A-Za-z0-9]{10,}"),
        token("google_api_key", r"(?-u:\b)AIza[0-9A-Za-z_-]{35}"),
        token("gitlab_token", r"(?-u:\b)glpat-[0-9A-Za-z_-]{20,}"),
        token("npm_token", r"(?-u:\b)npm_[A-Za-z0-9]{36}(?-u:\b)"),
        // Minted GitHub Copilot API tokens (`tid=<hex>;exp=<unix>;…`).
        token("copilot_token", r#"(?-u:\b)tid=[0-9a-fA-F]{8,};[^\s"']+"#),
        // Peer grant tokens (`cyp_pgr…_<secret>`) of `cross-session-messaging` → Peer grants.
        token("peer_grant", r"(?-u:\b)cyp_[A-Za-z0-9_-]{8,}"),
        token(
            "jwt",
            r"(?-u:\b)eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
        ),
        (
            "auth_header",
            r#"(?i)(?-u:\b)(?:proxy-)?authorization["']?[ \t]*[:=][ \t]*["']?(?:(?:bearer|basic|token|digest)[ \t]+)?(?P<v>[A-Za-z0-9._~+/=-]+)"#.into(),
            Shape::Value { min: 8 },
        ),
        (
            "url_password",
            r#"(?i)(?-u:\b)[a-z][a-z0-9+.-]*://[^\s:/@"'<>]+:(?P<v>[^\s/@"'<>]+)@"#.into(),
            Shape::Value { min: 3 },
        ),
        (
            "assignment",
            format!(
                r#"(?im)(?-u:\b)[a-z0-9_.-]*(?:{SECRET_WORDS})[a-z0-9_.-]*["']?[ \t]*[:=][ \t]*(?:"[^"\s]+"|'[^'\s]+'|[A-Za-z0-9_+/=.~!@#%^*?:-]+(?:[ \t,;&\r"'\\)\]}}]|$))"#
            ),
            Shape::Assignment,
        ),
    ]
}

/// A leading word boundary, or a `\n`, `\r` or `\t` escape: output is often JSON or a
/// quoted string, where the escape's letter would otherwise hide the boundary before a
/// token. [`escape_len`] trims the escape from a match.
const LEADING_BOUNDARY: &str = r"(?:(?-u:\b)|\\[nrt])";

pub(super) static DETECTORS: LazyLock<Vec<Detector>> = LazyLock::new(|| {
    table()
        .into_iter()
        .map(|(kind, pattern, shape)| Detector {
            kind,
            regex: Regex::new(&pattern.replacen(r"(?-u:\b)", LEADING_BOUNDARY, 1))
                .expect("static credential pattern"),
            shape,
        })
        .collect()
});

/// The length of the escape a match starts with, if it does.
pub(super) fn escape_len(found: &str) -> usize {
    match found.as_bytes() {
        [b'\\', b'n' | b'r' | b't', ..] => 2,
        _ => 0,
    }
}

/// The priority of user-configured matches: after every provider format, before the
/// generic header, URL and assignment detectors.
pub(super) const KNOWN_PRIORITY: usize = 13;
pub(super) const CUSTOM_PRIORITY: usize = 14;

/// Priority of the detector at `index` in [`DETECTORS`].
pub(super) fn priority(index: usize) -> usize {
    if index < KNOWN_PRIORITY {
        index
    } else {
        index + 2
    }
}

/// Where an assignment match keeps its value, and whether it is `strict` (unquoted after
/// `:` or a spaced `=`, where code is common).
pub(super) struct Assigned {
    pub value: std::ops::Range<usize>,
    pub strict: bool,
}

/// Split an `assignment` match without the capture engine, which would dominate the scan
/// of text dense with `token` keys: the name, an optional quote, the separator, then a
/// quoted value or an unquoted one and the terminator the pattern consumed.
pub(super) fn assigned(found: &str) -> Assigned {
    let bytes = found.as_bytes();
    let separator = bytes
        .iter()
        .position(|b| matches!(b, b':' | b'='))
        .unwrap_or(0);
    let mut start = separator + 1;
    while bytes.get(start).is_some_and(|b| matches!(b, b' ' | b'\t')) {
        start += 1;
    }
    let spaced =
        start > separator + 1 || matches!(bytes.get(separator.wrapping_sub(1)), Some(b' ' | b'\t'));
    match bytes.get(start) {
        Some(quote @ (b'"' | b'\'')) if bytes.last() == Some(quote) && found.len() > start + 1 => {
            Assigned {
                value: start + 1..found.len() - 1,
                strict: false,
            }
        }
        _ => {
            let end = match bytes.last() {
                Some(
                    b' ' | b'\t' | b',' | b';' | b'&' | b'\r' | b'"' | b'\'' | b'\\' | b')' | b']'
                    | b'}',
                ) => found.len() - 1,
                _ => found.len(),
            };
            Assigned {
                value: start..end,
                strict: spaced || bytes[separator] == b':',
            }
        }
    }
}

/// Whether an assigned value looks like a secret rather than code, a placeholder or a
/// number. `strict` (a value after `:` or a spaced `=`, where code is common) also
/// needs a digit.
pub(super) fn plausible_value(value: &str, strict: bool) -> bool {
    let bytes = value.as_bytes();
    let classes = [
        bytes.iter().any(u8::is_ascii_lowercase),
        bytes.iter().any(u8::is_ascii_uppercase),
        bytes.iter().any(u8::is_ascii_digit),
    ];
    value.len() >= 8
        && !value.starts_with(['$', '{', '<', '%', '[', '*', '(', '#'])
        && !bytes.iter().all(u8::is_ascii_digit)
        && !bytes.iter().all(|b| *b == bytes[0])
        && classes.iter().filter(|class| **class).count() >= 2
        && !identifier_path(value)
        && (!strict || classes[2])
}

/// `a.b.c` or `A::B`: a code path, not a secret.
fn identifier_path(value: &str) -> bool {
    let segments: Vec<&str> = value.split("::").flat_map(|part| part.split('.')).collect();
    segments.len() > 1
        && segments.iter().all(|segment| {
            let mut chars = segment.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}
