//! Typed provider failures (`provider-catalog` → Error classification).

use std::time::Duration;

use reqwest::header::HeaderMap;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Authentication,
    RateLimit,
    QuotaExceeded,
    ContextOverflow,
    ContentPolicy,
    InvalidRequest,
    ProviderInternal,
    Transport,
}

impl ErrorKind {
    /// Kinds the retry policy may retry before any output.
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimit | Self::ProviderInternal | Self::Transport
        )
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct LlmError {
    pub kind: ErrorKind,
    pub status: Option<u16>,
    /// Redacted of credentials.
    pub message: String,
    /// The server-advised delay before a retry.
    pub retry_after: Option<Duration>,
    /// When a subscription usage limit resets, in Unix seconds (`usage_limit_reached`).
    pub resets_at: Option<i64>,
    /// The subscription plan whose usage limit was reached.
    pub plan_type: Option<String>,
}

impl LlmError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            status: None,
            message: message.into(),
            retry_after: None,
            resets_at: None,
            plan_type: None,
        }
    }

    pub fn transport(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Transport, message)
    }

    /// A subscription usage limit that resets at `resets_at` (Unix seconds), as
    /// `usage_limit_reached` maps it.
    pub fn usage_limit(resets_at: Option<i64>) -> Self {
        let mut e = Self::new(ErrorKind::QuotaExceeded, usage_limit_message(resets_at));
        e.resets_at = resets_at;
        e
    }

    pub(crate) fn from_http(
        status: u16,
        headers: &HeaderMap,
        body: &str,
        secrets: &[&str],
    ) -> Self {
        let error = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.get("error").cloned())
            .unwrap_or(Value::Null);
        let kind = known_code(&error).unwrap_or_else(|| classify(status, body));
        let mut e = Self::new(kind, redact(&summarize(body), secrets));
        e.status = Some(status);
        e.retry_after = retry_after(headers).or_else(|| advised_delay(&e.message));
        e.explain(&error);
        e
    }

    /// Apply what a known code adds: the usage limit's reset time and plan, and the
    /// message for a plan without the model.
    fn explain(&mut self, error: &Value) {
        match error_code(error) {
            "usage_limit_reached" => {
                self.resets_at = error.get("resets_at").and_then(Value::as_i64);
                self.plan_type = error["plan_type"].as_str().map(str::to_owned);
                self.message = usage_limit_message(self.resets_at);
            }
            "usage_not_included" => self.message = "plan does not include this model".into(),
            _ => {}
        }
    }

    pub(crate) fn from_reqwest(e: &reqwest::Error, secrets: &[&str]) -> Self {
        Self::transport(redact(&e.to_string(), secrets))
    }
}

const OVERFLOW: &[&str] = &[
    "context length",
    "context_length",
    "maximum context",
    "too many tokens",
    "prompt is too long",
    "context window",
    "input is too long",
    "exceeds the context",
];
const POLICY: &[&str] = &[
    "content_policy",
    "content policy",
    "safety system",
    "content_filter",
];

/// Map an HTTP status and body to an error kind.
pub fn classify(status: u16, body: &str) -> ErrorKind {
    let lower = body.to_ascii_lowercase();
    let mentions = |patterns: &[&str]| patterns.iter().any(|p| lower.contains(p));
    match status {
        401 | 403 => ErrorKind::Authentication,
        429 if lower.contains("quota") => ErrorKind::QuotaExceeded,
        429 => ErrorKind::RateLimit,
        400 | 413 | 422 if mentions(OVERFLOW) => ErrorKind::ContextOverflow,
        400..=499 if mentions(POLICY) => ErrorKind::ContentPolicy,
        400..=499 => ErrorKind::InvalidRequest,
        _ => ErrorKind::ProviderInternal,
    }
}

/// Map an in-stream provider error type (Anthropic `error.type`, OpenAI `error.code`).
pub(crate) fn classify_stream_error(kind: &str, message: &str) -> ErrorKind {
    if let Some(known) = code_kind(kind) {
        return known;
    }
    match kind {
        "overloaded_error" | "api_error" | "server_error" => ErrorKind::ProviderInternal,
        "rate_limit_error" => ErrorKind::RateLimit,
        "authentication_error" | "permission_error" => ErrorKind::Authentication,
        _ => classify(400, &format!("{kind} {message}")),
    }
}

/// An in-stream error object (`{code | type, message, ...}`) as a typed error, with the
/// delay its message advises.
pub(crate) fn stream_error(error: &Value) -> LlmError {
    let message = error["message"].as_str().unwrap_or_default();
    let mut e = LlmError::new(classify_stream_error(error_code(error), message), message);
    e.retry_after = advised_delay(message);
    e.explain(error);
    e
}

/// The known provider error codes (`provider-catalog` → Provider-specific error codes),
/// matched against `code` and then `type`.
fn code_kind(code: &str) -> Option<ErrorKind> {
    Some(match code {
        "context_length_exceeded" => ErrorKind::ContextOverflow,
        "insufficient_quota" | "credit_balance_exhausted" | "usage_limit_reached" => {
            ErrorKind::QuotaExceeded
        }
        c if c.ends_with("_spend_limit_exceeded") => ErrorKind::QuotaExceeded,
        "usage_not_included" => ErrorKind::Authentication,
        "server_is_overloaded" | "slow_down" | "policy_capacity_unavailable" => {
            ErrorKind::ProviderInternal
        }
        "rate_limit_exceeded" => ErrorKind::RateLimit,
        "invalid_prompt" => ErrorKind::ContentPolicy,
        c if c.ends_with("_policy") => ErrorKind::ContentPolicy,
        "previous_response_not_found" => ErrorKind::InvalidRequest,
        _ => return None,
    })
}

/// The code of an error object: a known `code` or `type`, else whichever is present.
fn error_code(error: &Value) -> &str {
    let codes = || {
        [&error["code"], &error["type"]]
            .into_iter()
            .filter_map(Value::as_str)
    };
    codes()
        .find(|code| code_kind(code).is_some())
        .or_else(|| codes().next())
        .unwrap_or_default()
}

fn known_code(error: &Value) -> Option<ErrorKind> {
    code_kind(error_code(error))
}

/// A delay a message advises, as in "Please try again in 2.5s" (`ms`, `s` or `seconds`).
fn advised_delay(message: &str) -> Option<Duration> {
    const MARKER: &str = "try again in ";
    let lower = message.to_ascii_lowercase();
    let rest = &lower[lower.find(MARKER)? + MARKER.len()..];
    let digits = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    let value: f64 = rest[..digits].parse().ok()?;
    let unit = rest[digits..].trim_start();
    let seconds = if unit.starts_with("ms") {
        value / 1000.0
    } else if unit.starts_with('s') {
        value
    } else {
        return None;
    };
    Duration::try_from_secs_f64(seconds).ok()
}

/// "usage limit reached; resets at 14:20", in local time; a reset on another day names
/// the date too.
fn usage_limit_message(resets_at: Option<i64>) -> String {
    let at = resets_at.and_then(|seconds| jiff::Timestamp::from_second(seconds).ok());
    match at {
        Some(at) => reset_message(at, jiff::Timestamp::now(), &jiff::tz::TimeZone::system()),
        None => "usage limit reached".into(),
    }
}

/// [`usage_limit_message`] for a reset at `at`, seen at `now` in `zone`.
fn reset_message(at: jiff::Timestamp, now: jiff::Timestamp, zone: &jiff::tz::TimeZone) -> String {
    format!("usage limit reached; resets at {}", clock_at(at, now, zone))
}

/// `14:20` in local time, or `2026-10-29 14:20` on another day, for a Unix time in ms.
pub fn local_clock(ms: i64) -> String {
    match jiff::Timestamp::from_millisecond(ms) {
        Ok(at) => clock_at(at, jiff::Timestamp::now(), &jiff::tz::TimeZone::system()),
        Err(_) => ms.to_string(),
    }
}

fn clock_at(at: jiff::Timestamp, now: jiff::Timestamp, zone: &jiff::tz::TimeZone) -> String {
    let (at, now) = (at.to_zoned(zone.clone()), now.to_zoned(zone.clone()));
    let format = if at.date() == now.date() {
        "%H:%M"
    } else {
        "%Y-%m-%d %H:%M"
    };
    at.strftime(format).to_string()
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if let Some(ms) = header("retry-after-ms").and_then(|v| v.trim().parse::<f64>().ok()) {
        return Some(Duration::from_secs_f64(ms.max(0.0) / 1000.0));
    }
    header("retry-after")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(|s| Duration::from_secs_f64(s.max(0.0)))
}

fn summarize(body: &str) -> String {
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_string)
        });
    let text = message.unwrap_or_else(|| body.trim().to_string());
    text.chars().take(500).collect()
}

pub(crate) fn redact(text: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|s| s.len() >= 4)
        .fold(text.to_string(), |acc, s| acc.replace(s, "***"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_table() {
        assert_eq!(classify(401, ""), ErrorKind::Authentication);
        assert_eq!(classify(429, "slow down"), ErrorKind::RateLimit);
        assert_eq!(
            classify(429, r#"{"error":{"code":"insufficient_quota"}}"#),
            ErrorKind::QuotaExceeded
        );
        assert_eq!(
            classify(400, "This model's maximum context length is 128000"),
            ErrorKind::ContextOverflow
        );
        assert_eq!(
            classify(413, "prompt is too long"),
            ErrorKind::ContextOverflow
        );
        assert_eq!(
            classify(400, "rejected by content_policy"),
            ErrorKind::ContentPolicy
        );
        assert_eq!(classify(404, "model not found"), ErrorKind::InvalidRequest);
        assert_eq!(classify(529, "overloaded"), ErrorKind::ProviderInternal);
        assert_eq!(classify(503, ""), ErrorKind::ProviderInternal);
    }

    #[test]
    fn credentials_are_redacted() {
        assert_eq!(redact("bad key sk-abcdef", &["sk-abcdef"]), "bad key ***");
    }

    /// `provider-catalog` → Provider-specific error codes: each code, read from `code`
    /// or `type`, maps onto its reason.
    #[test]
    fn known_codes_map_to_their_reasons() {
        use ErrorKind::*;
        for (code, kind) in [
            ("context_length_exceeded", ContextOverflow),
            ("insufficient_quota", QuotaExceeded),
            ("credit_balance_exhausted", QuotaExceeded),
            ("monthly_spend_limit_exceeded", QuotaExceeded),
            ("usage_limit_reached", QuotaExceeded),
            ("usage_not_included", Authentication),
            ("server_is_overloaded", ProviderInternal),
            ("slow_down", ProviderInternal),
            ("policy_capacity_unavailable", ProviderInternal),
            ("rate_limit_exceeded", RateLimit),
            ("invalid_prompt", ContentPolicy),
            ("cyber_abuse_policy", ContentPolicy),
            ("previous_response_not_found", InvalidRequest),
        ] {
            let in_code = stream_error(&serde_json::json!({"code": code, "message": "m"}));
            assert_eq!(in_code.kind, kind, "{code} as code");
            let in_type = stream_error(&serde_json::json!({"type": code, "message": "m"}));
            assert_eq!(in_type.kind, kind, "{code} as type");
            let body = serde_json::json!({"error": {"code": code, "message": "m"}}).to_string();
            let http = LlmError::from_http(400, &HeaderMap::new(), &body, &[]);
            assert_eq!(http.kind, kind, "{code} in an HTTP body");
        }
        // A known `type` wins over an unknown `code`; unknown codes keep the old rules.
        let both = serde_json::json!({"code": "x_1", "type": "rate_limit_exceeded"});
        assert_eq!(stream_error(&both).kind, RateLimit);
        let old = serde_json::json!({"type": "overloaded_error", "message": "busy"});
        assert_eq!(stream_error(&old).kind, ProviderInternal);
        let message = stream_error(&serde_json::json!({"code": "usage_not_included"})).message;
        assert_eq!(message, "plan does not include this model");
    }

    /// The advised delay formats a message may carry.
    #[test]
    fn advised_delays_parse_each_unit() {
        let delay = |m: &str| advised_delay(m);
        assert_eq!(
            delay("Please try again in 2.5s."),
            Some(Duration::from_millis(2500))
        );
        assert_eq!(
            delay("Rate limited. Try again in 250ms"),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            delay("try again in 3 seconds"),
            Some(Duration::from_secs(3))
        );
        assert_eq!(delay("try again in 2 minutes"), None);
        assert_eq!(delay("try again in a moment"), None);
        assert_eq!(delay("overloaded"), None);
        let overloaded = serde_json::json!({
            "code": "server_is_overloaded", "message": "Please try again in 2.5s"
        });
        let e = stream_error(&overloaded);
        assert_eq!(
            (e.kind, e.retry_after),
            (
                ErrorKind::ProviderInternal,
                Some(Duration::from_millis(2500))
            )
        );
    }

    /// `retry-after-ms`, then `Retry-After`, then the message.
    #[test]
    fn header_delays_win_over_the_message() {
        let body = r#"{"error":{"code":"rate_limit_exceeded","message":"try again in 9s"}}"#;
        let delay = |pairs: &[(&'static str, &'static str)]| {
            let mut headers = HeaderMap::new();
            for (name, value) in pairs {
                headers.insert(*name, value.parse().unwrap());
            }
            LlmError::from_http(429, &headers, body, &[]).retry_after
        };
        let both = [("retry-after-ms", "1500"), ("retry-after", "4")];
        assert_eq!(delay(&both), Some(Duration::from_millis(1500)));
        assert_eq!(delay(&[("retry-after", "4")]), Some(Duration::from_secs(4)));
        assert_eq!(delay(&[]), Some(Duration::from_secs(9)));
    }

    /// `provider-catalog` → "Subscription limit reached": the reset time and plan.
    #[test]
    fn usage_limit_carries_its_reset_and_plan() {
        let at = jiff::Timestamp::now().as_second() + 40 * 60;
        let body = serde_json::json!({"error": {
            "type": "usage_limit_reached", "message": "The usage limit has been reached",
            "plan_type": "plus", "resets_at": at
        }})
        .to_string();
        let e = LlmError::from_http(429, &HeaderMap::new(), &body, &[]);
        assert_eq!(e.kind, ErrorKind::QuotaExceeded);
        assert_eq!(
            (e.resets_at, e.plan_type.as_deref()),
            (Some(at), Some("plus"))
        );
        assert!(
            e.message.starts_with("usage limit reached; resets at "),
            "{}",
            e.message
        );
        let bare = serde_json::json!({"type": "usage_limit_reached", "message": "limit"});
        let e = stream_error(&bare);
        assert_eq!(
            (e.resets_at, e.message.as_str()),
            (None, "usage limit reached")
        );
    }

    /// The reset shows the time alone today, and the date too on another day.
    #[test]
    fn reset_time_is_local_and_names_another_day() {
        let zone = jiff::tz::TimeZone::fixed(jiff::tz::offset(-3));
        let now: jiff::Timestamp = "2026-10-28T12:00:00Z".parse().unwrap();
        let later: jiff::Timestamp = "2026-10-28T17:20:00Z".parse().unwrap();
        assert_eq!(
            reset_message(later, now, &zone),
            "usage limit reached; resets at 14:20"
        );
        let tomorrow: jiff::Timestamp = "2026-10-29T03:05:00Z".parse().unwrap();
        assert_eq!(
            reset_message(tomorrow, now, &zone),
            "usage limit reached; resets at 2026-10-29 00:05"
        );
    }
}
