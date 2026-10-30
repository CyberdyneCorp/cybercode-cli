//! Secret redaction of text bound for a model provider (`tool-registry` → Tool output
//! redaction). Every detection becomes `[REDACTED:<kind>]`, so the model knows a value
//! exists without seeing it; write tools refuse to store a marker over a real value.
//!
//! One scan per detector collects match spans, the spans are ordered and de-overlapped,
//! and the text is rebuilt once; text without a detection is returned borrowed.

mod detectors;
mod known;
mod settings;

use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::LazyLock;

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use regex::Regex;
use serde::{Deserialize, Serialize};

use detectors::{CUSTOM_PRIORITY, DETECTORS, Detector, KNOWN_PRIORITY, Shape};
pub use known::{MIN_KNOWN_LEN, config_secrets, env_secrets, secret_name};
pub use settings::{MAX_ALLOW, MAX_PATTERNS, MAX_SOURCE, RedactSettings};

/// The opening of every marker; the full form is `[REDACTED:<kind>]`.
pub const MARKER_PREFIX: &str = "[REDACTED:";

static MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[REDACTED:[a-z0-9_]{1,32}\]").expect("static marker pattern"));

/// The marker that replaces a secret of `kind`.
pub fn marker(kind: &str) -> String {
    format!("{MARKER_PREFIX}{kind}]")
}

/// Every marker in `text`.
pub fn markers(text: &str) -> impl Iterator<Item = &str> {
    MARKER.find_iter(text).map(|found| found.as_str())
}

/// The first marker `after` has more often than `before`: a write that would store a
/// placeholder where the file holds (or never held) the hidden value. Markers a file
/// already contains literally, such as in documentation, may be kept.
pub fn introduced_marker(before: &str, after: &str) -> Option<String> {
    let mut present: HashMap<&str, usize> = HashMap::new();
    for found in markers(before) {
        *present.entry(found).or_default() += 1;
    }
    markers(after).find_map(|found| match present.get_mut(found) {
        Some(left) if *left > 0 => {
            *left -= 1;
            None
        }
        _ => Some(found.to_string()),
    })
}

/// The markers `redacted` has beyond those `original` already had, as redactions: what
/// a page of a pre-redacted text hides.
pub fn added_markers(original: &str, redacted: &str) -> Redactions {
    let mut present: HashMap<&str, usize> = HashMap::new();
    for found in markers(original) {
        *present.entry(found).or_default() += 1;
    }
    let mut redactions = Redactions::default();
    for found in markers(redacted) {
        match present.get_mut(found) {
            Some(left) if *left > 0 => *left -= 1,
            _ => redactions.add(&found[MARKER_PREFIX.len()..found.len() - 1]),
        }
    }
    redactions
}

/// What a redaction hid: how many values, and their kinds (sorted, distinct).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Redactions {
    pub count: u32,
    pub kinds: Vec<String>,
}

impl Redactions {
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn add(&mut self, kind: &str) {
        self.count = self.count.saturating_add(1);
        if let Err(index) = self
            .kinds
            .binary_search_by(|known| known.as_str().cmp(kind))
        {
            self.kinds.insert(index, kind.to_string());
        }
    }

    pub fn merge(&mut self, other: &Redactions) {
        self.count = self.count.saturating_add(other.count);
        for kind in &other.kinds {
            if let Err(index) = self.kinds.binary_search(kind) {
                self.kinds.insert(index, kind.clone());
            }
        }
    }
}

/// A redacted text: borrowed when nothing was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redacted<'a> {
    pub text: Cow<'a, str>,
    pub redactions: Redactions,
}

/// Which assigned values count: tool output skips code, placeholders and short values;
/// the legacy filter (memory, git text, telemetry) takes any value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    ToolOutput,
    Legacy,
}

/// One detection: `[start, end)` of the secret, the kind that names it, its priority.
#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
    priority: usize,
    kind: &'static str,
}

/// A compiled redaction policy: the built-in detectors, configured patterns, the allow
/// list and the exact values of known secrets.
pub struct Redactor {
    enabled: bool,
    mode: Mode,
    custom: Vec<Regex>,
    allow_literals: Vec<String>,
    allow_patterns: Vec<Regex>,
    known: Option<AhoCorasick>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Known values are secrets: only their presence is shown.
        f.debug_struct("Redactor")
            .field("enabled", &self.enabled)
            .field("custom", &self.custom.len())
            .field(
                "allow",
                &(self.allow_literals.len() + self.allow_patterns.len()),
            )
            .field("known", &self.known.as_ref().map(AhoCorasick::patterns_len))
            .finish()
    }
}

static BUILTIN: LazyLock<Redactor> =
    LazyLock::new(|| Redactor::new(&RedactSettings::default(), []));
static LEGACY: LazyLock<Redactor> = LazyLock::new(|| Redactor {
    mode: Mode::Legacy,
    ..Redactor::new(&RedactSettings::default(), [])
});

impl Redactor {
    /// The built-in detectors only, enabled: the default for tool output.
    pub fn builtin() -> &'static Redactor {
        &BUILTIN
    }

    /// A policy from validated settings and the exact `known` secret values (shorter
    /// than [`MIN_KNOWN_LEN`] are ignored). An invalid configured regex is skipped,
    /// since the config loader already refused it.
    pub fn new(settings: &RedactSettings, known: impl IntoIterator<Item = String>) -> Self {
        let mut known: Vec<String> = known
            .into_iter()
            .filter(|value| value.chars().count() >= MIN_KNOWN_LEN)
            .collect();
        known.sort();
        known.dedup();
        let known = (!known.is_empty()).then(|| {
            AhoCorasickBuilder::new()
                .match_kind(MatchKind::Standard)
                .build(&known)
                .expect("literal secret automaton")
        });
        let (patterns, literals): (Vec<&String>, Vec<&String>) = settings
            .allow
            .iter()
            .partition(|entry| settings::allow_regex(entry).is_some());
        Self {
            enabled: settings.enabled,
            mode: Mode::ToolOutput,
            custom: settings
                .patterns
                .iter()
                .filter_map(|pattern| settings::compile(pattern).ok())
                .collect(),
            allow_literals: literals.into_iter().cloned().collect(),
            allow_patterns: patterns
                .into_iter()
                .filter_map(|entry| settings::allow_regex(entry))
                .filter_map(|pattern| settings::compile(&format!("^(?:{pattern})$")).ok())
                .collect(),
            known,
        }
    }

    /// `text` with every detected secret replaced by its marker. Existing markers are
    /// never matched again, so redacting twice changes nothing.
    pub fn redact<'a>(&self, text: &'a str) -> Redacted<'a> {
        self.apply(text, None)
    }

    /// [`Redactor::redact`] of an owned text, which is returned as is when nothing was
    /// found.
    pub fn redact_owned(&self, text: String) -> (String, Redactions) {
        let Redacted {
            text: redacted,
            redactions,
        } = self.redact(&text);
        match redacted {
            Cow::Borrowed(_) => (text, redactions),
            Cow::Owned(owned) => (owned, redactions),
        }
    }

    /// [`Redactor::redact`], with `fixed` in place of every kinded marker when given.
    fn apply<'a>(&self, text: &'a str, fixed: Option<&str>) -> Redacted<'a> {
        let unchanged = || Redacted {
            text: Cow::Borrowed(text),
            redactions: Redactions::default(),
        };
        if !self.enabled || text.is_empty() {
            return unchanged();
        }
        let existing: Vec<(usize, usize)> = MARKER
            .find_iter(text)
            .map(|found| (found.start(), found.end()))
            .collect();
        let mut spans = self.detect(text, &existing);
        if spans.is_empty() {
            return unchanged();
        }
        spans.sort_unstable_by_key(|span| (span.start, Reverse(span.end), span.priority));
        let mut covered = Vec::new();
        for span in spans {
            cover(&mut covered, span);
        }
        let mut out = String::with_capacity(text.len());
        let mut redactions = Redactions::default();
        let mut last = 0;
        for span in covered {
            out.push_str(&text[last..span.start]);
            match fixed {
                Some(fixed) => out.push_str(fixed),
                None => {
                    out.push_str(MARKER_PREFIX);
                    out.push_str(span.kind);
                    out.push(']');
                }
            }
            // A multi-line secret (a private key) keeps its line breaks, so line numbers
            // and paging stay those of the raw text.
            let breaks = text[span.start..span.end].matches('\n').count();
            out.extend(std::iter::repeat_n('\n', breaks));
            last = span.end;
            redactions.add(span.kind);
        }
        if redactions.is_empty() {
            return unchanged();
        }
        out.push_str(&text[last..]);
        Redacted {
            text: Cow::Owned(out),
            redactions,
        }
    }

    fn detect(&self, text: &str, existing: &[(usize, usize)]) -> Vec<Span> {
        let mut spans = Vec::new();
        for (index, detector) in DETECTORS.iter().enumerate() {
            self.scan(detector, detectors::priority(index), text, &mut spans);
        }
        for pattern in &self.custom {
            spans.extend(pattern.find_iter(text).map(|found| Span {
                start: found.start(),
                end: found.end(),
                priority: CUSTOM_PRIORITY,
                kind: "custom",
            }));
        }
        spans.retain(|span| {
            !overlaps(existing, *span) && !self.allowed(&text[span.start..span.end])
        });
        if let Some(known) = &self.known {
            // Standard overlapping matches arrive in end order. Coalesce as they
            // arrive instead of retaining one span per pattern at every position.
            let mut covered = Vec::new();
            for found in known.find_overlapping_iter(text) {
                let span = Span {
                    start: found.start(),
                    end: found.end(),
                    priority: KNOWN_PRIORITY,
                    kind: "known_secret",
                };
                if !overlaps(existing, span) && !self.allowed(&text[span.start..span.end]) {
                    cover(&mut covered, span);
                }
            }
            spans.extend(covered);
        }
        spans
    }

    fn scan(&self, detector: &Detector, priority: usize, text: &str, spans: &mut Vec<Span>) {
        let span = |start, end| Span {
            start,
            end,
            priority,
            kind: detector.kind,
        };
        match detector.shape {
            Shape::Token => spans.extend(detector.regex.find_iter(text).map(|found| {
                span(
                    found.start() + detectors::escape_len(found.as_str()),
                    found.end(),
                )
            })),
            Shape::Value { min } => {
                for captures in detector.regex.captures_iter(text) {
                    if let Some(value) = captures.name("v")
                        && self.accepts_value(value.as_str(), min)
                    {
                        spans.push(span(value.start(), value.end()));
                    }
                }
            }
            Shape::Assignment => {
                for found in detector.regex.find_iter(text) {
                    // The value follows the name; an escape before the name is irrelevant.
                    let assigned = detectors::assigned(found.as_str());
                    if self.accepts_assignment(
                        &found.as_str()[assigned.value.clone()],
                        assigned.strict,
                    ) {
                        let start = found.start();
                        spans.push(span(
                            start + assigned.value.start,
                            start + assigned.value.end,
                        ));
                    }
                }
            }
        }
    }

    fn accepts_value(&self, value: &str, min: usize) -> bool {
        let placeholder = value.starts_with(['$', '{', '<', '[', '%', '*']);
        match self.mode {
            Mode::ToolOutput => value.chars().count() >= min && !placeholder,
            Mode::Legacy => !value.is_empty() && !value.starts_with(MARKER_PREFIX),
        }
    }

    fn accepts_assignment(&self, value: &str, strict: bool) -> bool {
        match self.mode {
            Mode::ToolOutput => detectors::plausible_value(value, strict),
            Mode::Legacy => !value.is_empty() && !value.starts_with(MARKER_PREFIX),
        }
    }

    fn allowed(&self, secret: &str) -> bool {
        self.allow_literals.iter().any(|literal| literal == secret)
            || self
                .allow_patterns
                .iter()
                .any(|pattern| pattern.is_match(secret))
    }
}

/// Add a span in start order or end order, coalescing intersecting coverage.
/// Keep the most specific detector kind; adjacent secrets remain separate.
fn cover(covered: &mut Vec<Span>, mut span: Span) {
    while covered
        .last()
        .is_some_and(|previous| span.start < previous.end)
    {
        let previous = covered.pop().expect("intersecting span");
        span.start = span.start.min(previous.start);
        span.end = span.end.max(previous.end);
        if previous.priority < span.priority {
            span.priority = previous.priority;
            span.kind = previous.kind;
        }
    }
    covered.push(span);
}

/// Whether `span` overlaps one of the sorted, disjoint `existing` marker ranges.
fn overlaps(existing: &[(usize, usize)], span: Span) -> bool {
    let after = existing.partition_point(|(_, end)| *end <= span.start);
    existing
        .get(after)
        .is_some_and(|(start, _)| *start < span.end)
}

/// Every built-in detection replaced by `marker`, with the permissive legacy filter:
/// memory, git text, telemetry and exports.
pub fn redact_credentials_with(text: &str, marker: &str) -> String {
    LEGACY.apply(text, Some(marker)).text.into_owned()
}

/// Whether any built-in detector (legacy filter) matches `text`.
pub fn contains_credential(text: &str) -> bool {
    !LEGACY.redact(text).redactions.is_empty()
}

#[cfg(test)]
mod tests;
