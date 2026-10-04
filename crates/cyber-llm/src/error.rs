//! Typed provider failures (`provider-catalog` → Error classification).

use std::time::Duration;

use reqwest::header::HeaderMap;

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
    pub retry_after: Option<Duration>,
}

impl LlmError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            status: None,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn transport(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Transport, message)
    }

    pub(crate) fn from_http(
        status: u16,
        headers: &HeaderMap,
        body: &str,
        secrets: &[&str],
    ) -> Self {
        Self {
            kind: classify(status, body),
            status: Some(status),
            message: redact(&summarize(body), secrets),
            retry_after: retry_after(headers),
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
    match kind {
        "overloaded_error" | "api_error" | "server_error" => ErrorKind::ProviderInternal,
        "rate_limit_error" | "rate_limit_exceeded" => ErrorKind::RateLimit,
        "insufficient_quota" => ErrorKind::QuotaExceeded,
        "authentication_error" | "permission_error" => ErrorKind::Authentication,
        _ => classify(400, &format!("{kind} {message}")),
    }
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
}
