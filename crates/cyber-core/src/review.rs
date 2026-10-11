//! Review target grammar and the canonical structured findings contract.
//!
//! The target grammar is deliberately strict: user text is validated here and
//! later resolved to commit object IDs by a library engine, so it never
//! reaches a process argument.

pub mod resolve;

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Maximum size of one revision expression.
pub const MAX_REF_BYTES: usize = 256;

/// Typed reasons a review target cannot be resolved; reported before admission.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ReviewTargetCode")]
pub enum ErrorCode {
    InvalidTarget,
    InvalidRef,
    UnresolvedRef,
    NotARepository,
    EmptyDiff,
    // The target has changes, but read policy withholds every one of them.
    AllWithheld,
    GhUnavailable,
    PrUnresolved,
    PrCommitsMissing,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[serde(deny_unknown_fields)]
#[error("{message}")]
pub struct TargetError {
    pub code: ErrorCode,
    pub message: String,
}

impl TargetError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// A parsed review target. It (de)serializes as its normalized text, so every
/// decoded value has passed [`Target::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Target {
    Uncommitted,
    Range { base: String, head: String },
    Commit(String),
    Pr(u32),
}

impl Target {
    pub fn parse(arguments: &str) -> Result<Self, TargetError> {
        let text = arguments.trim();
        let mut words = text.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (None, _, _) => Ok(Self::Uncommitted),
            (Some("pr"), number, None) => parse_pr(number),
            (Some("pr"), Some(_), Some(_)) => Err(invalid_target("`pr` takes one number")),
            _ => parse_revisions(text),
        }
    }

    pub fn normalized(&self) -> String {
        match self {
            Self::Uncommitted => String::new(),
            Self::Range { base, head } => format!("{base}..{head}"),
            Self::Commit(revision) => revision.clone(),
            Self::Pr(number) => format!("pr {number}"),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Uncommitted => "uncommitted",
            Self::Range { .. } => "range",
            Self::Commit(_) => "commit",
            Self::Pr(_) => "pr",
        }
    }
}

impl TryFrom<String> for Target {
    type Error = TargetError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<Target> for String {
    fn from(value: Target) -> Self {
        value.normalized()
    }
}

fn invalid_target(message: &str) -> TargetError {
    TargetError::new(ErrorCode::InvalidTarget, message)
}

fn parse_pr(number: Option<&str>) -> Result<Target, TargetError> {
    let number = number.ok_or_else(|| invalid_target("`pr` requires a pull request number"))?;
    number
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| number.parse::<u32>().ok())
        .flatten()
        .filter(|number| *number >= 1)
        .map(Target::Pr)
        .ok_or_else(|| invalid_target("pull request number must be a positive decimal integer"))
}

fn parse_revisions(text: &str) -> Result<Target, TargetError> {
    if text.contains("...") {
        return Err(invalid_target(
            "three-dot ranges are not supported; use `<base>..<head>`",
        ));
    }
    let Some((base, head)) = text.split_once("..") else {
        return validate_ref(text).map(|()| Target::Commit(text.into()));
    };
    if base.is_empty() || head.is_empty() {
        return Err(invalid_target(
            "a range requires both `<base>` and `<head>`",
        ));
    }
    validate_ref(base)?;
    validate_ref(head)?;
    Ok(Target::Range {
        base: base.into(),
        head: head.into(),
    })
}

fn validate_ref(revision: &str) -> Result<(), TargetError> {
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || b"._/~^-".contains(&byte);
    let reason = if revision.len() > MAX_REF_BYTES {
        Some("is too long")
    } else if revision.starts_with(['-', '/']) {
        Some("must not start with `-` or `/`")
    } else if revision.contains("@{") || revision.contains("..") {
        Some("must not contain `@{` or `..`")
    } else if revision.ends_with(".lock") {
        Some("must not end with `.lock`")
    } else if !revision.bytes().all(allowed) {
        Some("may contain only letters, digits and `._/~^-`")
    } else {
        None
    };
    match reason {
        None => Ok(()),
        Some(reason) => Err(TargetError::new(
            ErrorCode::InvalidRef,
            format!("revision {} {reason}", escape(revision)),
        )),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub file: String,
    #[serde(deserialize_with = "integral_line")]
    pub line: u64,
    pub severity: Severity,
    pub summary: String,
    pub failure_scenario: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Counts {
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
}

/// Validated findings in deterministic order: severity, file, line, summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Findings {
    pub findings: Vec<Finding>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid review findings: {0}")]
pub struct InvalidFindings(String);

/// Characters a finding's file may not contain: what [`escape`] rewrites.
const UNSAFE_CLASS: &str = "\\u0000-\\u001f\\u007f-\\u009f\\u202a-\\u202e\\u2066-\\u2069";

/// JSON Schema given to the bundled review child as its structured-result
/// schema. It states every rule [`Findings::from_result`] enforces, so a result
/// the consumer would reject fails `return_result` and earns the corrective retry.
pub static FINDINGS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
    let text = json!({"type": "string", "minLength": 1, "pattern": "\\S"});
    json!({
        "type": "object",
        "properties": {
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "file": {
                            "type": "string",
                            "minLength": 1,
                            "pattern": format!("^[^{UNSAFE_CLASS}]+$"),
                            "not": {"anyOf": [
                                {"pattern": "^[/\\\\]"},
                                {"pattern": "^.:"},
                                {"pattern": "(^|[/\\\\])\\.\\.([/\\\\]|$)"}
                            ]},
                            "description": "Repository-relative path: no leading `/` or drive, no `..` segment"
                        },
                        "line": {"type": "integer", "minimum": 1},
                        "severity": {"type": "string", "enum": ["critical", "high", "medium", "low"]},
                        "summary": text,
                        "failure_scenario": text
                    },
                    "required": ["file", "line", "severity", "summary", "failure_scenario"],
                    "additionalProperties": false
                }
            },
            "limitations": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["findings", "limitations"],
        "additionalProperties": false
    })
});

/// A line number; JSON Schema's `integer` also admits integral floats like `12.0`.
fn integral_line<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let number = serde_json::Number::deserialize(deserializer)?;
    number
        .as_u64()
        .or_else(|| {
            number
                .as_f64()
                .filter(|f| f.fract() == 0.0 && (0.0..u64::MAX as f64).contains(f))
                .map(|f| f as u64)
        })
        .ok_or_else(|| serde::de::Error::custom("line must be a non-negative integer"))
}

impl Findings {
    /// Re-validates a structured result independently of the model and sorts it.
    pub fn from_result(value: &Value) -> Result<Self, InvalidFindings> {
        let mut result = Self::deserialize(value).map_err(|e| InvalidFindings(e.to_string()))?;
        for finding in &result.findings {
            finding.validate()?;
        }
        result.findings.sort_by(|a, b| {
            (a.severity, &a.file, a.line, &a.summary)
                .cmp(&(b.severity, &b.file, b.line, &b.summary))
        });
        Ok(result)
    }

    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for finding in &self.findings {
            *match finding.severity {
                Severity::Critical => &mut counts.critical,
                Severity::High => &mut counts.high,
                Severity::Medium => &mut counts.medium,
                Severity::Low => &mut counts.low,
            } += 1;
        }
        counts
    }

    /// Terminal-safe text: every control or bidirectional character is escaped.
    pub fn render_text(&self) -> String {
        let mut text = String::new();
        if self.findings.is_empty() {
            text.push_str("No findings.\n");
        }
        for finding in &self.findings {
            text.push_str(&format!(
                "[{}] {}:{} {}\n    {}\n",
                finding.severity.as_str(),
                escape(&finding.file),
                finding.line,
                escape(&finding.summary),
                escape(&finding.failure_scenario),
            ));
        }
        for limitation in &self.limitations {
            text.push_str(&format!("Limitation: {}\n", escape(limitation)));
        }
        text
    }
}

impl Finding {
    fn validate(&self) -> Result<(), InvalidFindings> {
        let invalid = |reason: &str| {
            Err(InvalidFindings(format!(
                "finding {}:{} {reason}",
                escape(&self.file),
                self.line
            )))
        };
        if self.line < 1 {
            return invalid("has a line below 1");
        }
        if !is_repository_relative(&self.file) {
            return invalid("is not a repository-relative path");
        }
        if self.summary.trim().is_empty() || self.failure_scenario.trim().is_empty() {
            return invalid("has an empty summary or failure scenario");
        }
        Ok(())
    }
}

fn is_unsafe_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn is_repository_relative(file: &str) -> bool {
    let drive = file.as_bytes().get(1) == Some(&b':');
    !file.is_empty()
        && !drive
        && !file.starts_with(['/', '\\'])
        && !file.chars().any(is_unsafe_char)
        && file.split(['/', '\\']).all(|part| part != "..")
}

/// Escapes control and bidirectional characters for terminal display.
pub fn escape(text: &str) -> String {
    text.chars()
        .map(|c| {
            if is_unsafe_char(c) {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(arguments: &str) -> ErrorCode {
        Target::parse(arguments).unwrap_err().code
    }

    #[test]
    fn parses_supported_targets() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(Target::parse("").unwrap(), Target::Uncommitted);
        assert_eq!(Target::parse("  \t").unwrap(), Target::Uncommitted);
        assert_eq!(
            Target::parse("main..HEAD").unwrap(),
            Target::Range {
                base: "main".into(),
                head: "HEAD".into()
            }
        );
        assert_eq!(
            Target::parse("HEAD~1").unwrap(),
            Target::Commit("HEAD~1".into())
        );
        assert_eq!(Target::parse(sha).unwrap(), Target::Commit(sha.into()));
        assert_eq!(Target::parse("pr 42").unwrap(), Target::Pr(42));
        assert_eq!(Target::parse("  pr \t 42 ").unwrap().normalized(), "pr 42");
        assert_eq!(
            Target::parse(" feature/x-1..v1.2^ ").unwrap().normalized(),
            "feature/x-1..v1.2^"
        );
    }

    #[test]
    fn rejects_unsafe_or_unsupported_targets() {
        for (arguments, expected) in [
            ("--output=/tmp/x", ErrorCode::InvalidRef),
            ("-c core.pager=x", ErrorCode::InvalidRef),
            ("HEAD --ext-diff", ErrorCode::InvalidRef),
            ("main...HEAD", ErrorCode::InvalidTarget),
            ("..HEAD", ErrorCode::InvalidTarget),
            ("main..", ErrorCode::InvalidTarget),
            ("a..b..c", ErrorCode::InvalidRef),
            (":/fix", ErrorCode::InvalidRef),
            ("HEAD:src", ErrorCode::InvalidRef),
            ("HEAD@{1}", ErrorCode::InvalidRef),
            ("/etc/passwd", ErrorCode::InvalidRef),
            ("main.lock", ErrorCode::InvalidRef),
            ("pr", ErrorCode::InvalidTarget),
            ("pr 0", ErrorCode::InvalidTarget),
            ("pr -1", ErrorCode::InvalidTarget),
            ("pr 1x", ErrorCode::InvalidTarget),
            ("pr +1", ErrorCode::InvalidTarget),
            ("pr 1 2", ErrorCode::InvalidTarget),
            ("pr 99999999999", ErrorCode::InvalidTarget),
            ("ma\u{1b}[31min", ErrorCode::InvalidRef),
            ("main\u{0}", ErrorCode::InvalidRef),
            ("HEAD\nmain", ErrorCode::InvalidRef),
            ("ma\u{202e}in", ErrorCode::InvalidRef),
        ] {
            assert_eq!(code(arguments), expected, "{arguments:?}");
        }
        assert_eq!(code(&"a".repeat(MAX_REF_BYTES + 1)), ErrorCode::InvalidRef);
        assert!(Target::parse(&"a".repeat(MAX_REF_BYTES)).is_ok());
        let message = Target::parse("ma\u{1b}in").unwrap_err().message;
        assert!(!message.contains('\u{1b}'), "{message}");
    }

    #[test]
    fn target_round_trips_only_through_the_grammar() {
        let target = Target::parse("main..HEAD").unwrap();
        let text = serde_json::to_string(&target).unwrap();
        assert_eq!(text, "\"main..HEAD\"");
        assert_eq!(serde_json::from_str::<Target>(&text).unwrap(), target);
        assert!(serde_json::from_str::<Target>("\"--output=x\"").is_err());
        assert_eq!(Target::Pr(7).kind(), "pr");
    }

    fn finding(severity: &str, file: &str, line: u64, summary: &str) -> Value {
        json!({"file": file, "line": line, "severity": severity, "summary": summary, "failure_scenario": "x"})
    }

    fn order(findings: &Findings) -> Vec<String> {
        findings
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{} {}:{} {}",
                    f.severity.as_str(),
                    f.file,
                    f.line,
                    f.summary
                )
            })
            .collect()
    }

    #[test]
    fn findings_sort_by_severity_file_line_summary() {
        let items = vec![
            finding("low", "b.rs", 3, "s"),
            finding("critical", "z.rs", 9, "s"),
            finding("high", "a.rs", 20, "s"),
            finding("high", "a.rs", 4, "s"),
            finding("medium", "a.rs", 4, "b"),
            finding("medium", "a.rs", 4, "a"),
        ];
        let expected = [
            "critical z.rs:9 s",
            "high a.rs:4 s",
            "high a.rs:20 s",
            "medium a.rs:4 a",
            "medium a.rs:4 b",
            "low b.rs:3 s",
        ];
        for rotation in 0..items.len() {
            let mut permuted = items.clone();
            permuted.rotate_left(rotation);
            permuted.swap(0, items.len() - 1 - rotation);
            let findings =
                Findings::from_result(&json!({"findings": permuted, "limitations": []})).unwrap();
            assert_eq!(order(&findings), expected);
        }
        let findings =
            Findings::from_result(&json!({"findings": items, "limitations": []})).unwrap();
        assert_eq!(
            findings.counts(),
            Counts {
                critical: 1,
                high: 2,
                medium: 2,
                low: 1
            }
        );
    }

    #[test]
    fn findings_reject_invalid_entries() {
        let mut extra = finding("low", "a.rs", 1, "s");
        extra["extra"] = json!(true);
        for item in [
            finding("low", "a.rs", 0, "s"),
            finding("blocker", "a.rs", 1, "s"),
            finding("low", "/etc/passwd", 1, "s"),
            finding("low", "C:\\a.rs", 1, "s"),
            finding("low", "../a.rs", 1, "s"),
            finding("low", "src/../../a.rs", 1, "s"),
            finding("low", "src\\..\\a.rs", 1, "s"),
            finding("low", "a\u{1b}[2J.rs", 1, "s"),
            finding("low", "", 1, "s"),
            finding("low", "a.rs", 1, " "),
            json!({"file": "a.rs", "line": -1, "severity": "low", "summary": "s", "failure_scenario": "x"}),
            extra,
        ] {
            let result = json!({"findings": [item], "limitations": []});
            assert!(Findings::from_result(&result).is_err(), "{result}");
        }
        assert!(Findings::from_result(&json!({"findings": []})).is_err());
        assert!(
            Findings::from_result(&json!({"findings": [], "limitations": [], "dropped": 0}))
                .is_err()
        );
    }

    #[test]
    fn render_text_escapes_terminal_controls() {
        let result = json!({
            "findings": [{"file": "a.rs", "line": 2, "severity": "high",
                "summary": "bad\u{1b}[2Jthing\u{202e}", "failure_scenario": "line\nbreak\u{7}"}],
            "limitations": ["skipped\u{1b}]0;title\u{7}"]
        });
        let text = Findings::from_result(&result).unwrap().render_text();
        assert!(
            !text
                .chars()
                .any(|c| c == '\u{1b}' || c == '\u{7}' || c == '\u{202e}')
        );
        assert!(
            text.contains("[high] a.rs:2 bad\\u{1b}[2Jthing\\u{202e}"),
            "{text}"
        );
        assert!(text.contains("line\\nbreak\\u{7}"), "{text}");
        let empty = json!({"findings": [], "limitations": []});
        assert_eq!(
            Findings::from_result(&empty).unwrap().render_text(),
            "No findings.\n"
        );
    }

    #[test]
    fn findings_schema_compiles_and_matches_the_contract() {
        let validator = jsonschema::validator_for(&FINDINGS_SCHEMA).unwrap();
        assert!(validator.is_valid(&json!({"findings": [], "limitations": []})));
        let valid =
            json!({"findings": [finding("high", "src/a.rs", 12, "s")], "limitations": ["l"]});
        assert!(validator.is_valid(&valid));
        for invalid in [
            json!({"findings": []}),
            json!({"findings": [], "limitations": [], "extra": 1}),
            json!({"findings": [finding("blocker", "a.rs", 1, "s")], "limitations": []}),
            json!({"findings": [finding("low", "a.rs", 0, "s")], "limitations": []}),
            json!({"findings": [finding("low", "a\u{1b}.rs", 1, "s")], "limitations": []}),
            json!({"findings": [finding("low", "a.rs", 1, "")], "limitations": []}),
        ] {
            assert!(!validator.is_valid(&invalid), "{invalid}");
        }
    }

    /// Every result `return_result` accepts must also pass the consumer, or a
    /// rejection would skip the corrective retry and drop all findings.
    #[test]
    fn findings_schema_accepts_only_what_the_consumer_accepts() {
        let validator = jsonschema::validator_for(&FINDINGS_SCHEMA).unwrap();
        let mut float_line = finding("low", "a.rs", 1, "s");
        float_line["line"] = json!(12.0);
        let mut blank_scenario = finding("low", "a.rs", 1, "s");
        blank_scenario["failure_scenario"] = json!("\t \n");
        let candidates = [
            finding("low", "/repo/src/a.rs", 1, "s"),
            finding("low", "\\repo\\a.rs", 1, "s"),
            finding("low", "C:\\a.rs", 1, "s"),
            finding("low", "c:/a.rs", 1, "s"),
            finding("low", "../x", 1, "s"),
            finding("low", "src/../x", 1, "s"),
            finding("low", "src\\..", 1, "s"),
            finding("low", "..", 1, "s"),
            finding("low", "a\u{202e}.rs", 1, "s"),
            finding("low", "a\u{2066}.rs", 1, "s"),
            finding("low", "a\u{85}.rs", 1, "s"),
            finding("low", "a.rs", 1, " "),
            finding("low", "a.rs", 1, "\u{3000}"),
            blank_scenario,
            float_line.clone(),
            finding("low", "src/..hidden/a.rs", 1, "s"),
            finding("low", "src/a..b.rs", 1, "s"),
            finding("low", "src/ünï.rs", 1, "s"),
        ];
        for item in candidates {
            let result = json!({"findings": [item], "limitations": []});
            if validator.is_valid(&result) {
                assert!(Findings::from_result(&result).is_ok(), "{result}");
            }
        }
        for rejected in [
            finding("low", "/repo/src/a.rs", 1, "s"),
            finding("low", "../x", 1, "s"),
            finding("low", "a\u{202e}.rs", 1, "s"),
            finding("low", "a.rs", 1, " "),
        ] {
            let result = json!({"findings": [rejected], "limitations": []});
            assert!(!validator.is_valid(&result), "{result}");
        }
        for accepted in [float_line, finding("low", "src/a..b.rs", 1, "s")] {
            let result = json!({"findings": [accepted], "limitations": []});
            assert!(validator.is_valid(&result), "{result}");
        }
        let result = json!({"findings": [finding("low", "a.rs", 12, "s")], "limitations": []});
        let mut float = result.clone();
        float["findings"][0]["line"] = json!(12.0);
        assert_eq!(
            Findings::from_result(&float).unwrap(),
            Findings::from_result(&result).unwrap()
        );
    }
}
