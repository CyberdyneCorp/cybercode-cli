//! Failures (`provider-catalog` → Gemini error and quota classification).
//!
//! Gemini answers every quota problem with 429 `RESOURCE_EXHAUSTED` and a message that
//! says "quota". The `google.rpc` details tell a per-minute limit, which clears on its
//! own, from a daily or weekly quota, which does not: `ErrorInfo.reason`,
//! `QuotaFailure.violations[]` and `RetryInfo.retryDelay`.

use std::time::Duration;

use reqwest::header::HeaderMap;
use serde_json::Value;

use crate::error::{ErrorKind, LlmError, OVERFLOW, header_delay, redact, summarize};
use crate::types::ToolSpec;

const RPC: &str = "type.googleapis.com/google.rpc.";
const PREPAID: &str = "Gemini prepaid credit balance is exhausted; add credits in AI Studio";
const SPEND_CAP: &str = "Gemini project spend cap reached";
const MISSING_SIGNATURE: &str = "Gemini rejected the history: missing thought signature";
const REGION: &str = "Gemini API not available for this project or region";
const DECLARATION: &str = "Gemini rejected a tool declaration";
const DECLARATION_HINT: &str =
    "turn on MCP tool search (deferred tool loading) or disable large MCP servers for this agent";
const SPEND_MARKERS: &[&str] = &["spend cap", "spending cap", "spend limit", "spending limit"];
const SCHEMA_MARKERS: &[&str] = &[
    "function_declarations",
    "function declaration",
    "parameters_json_schema",
    "schema",
];
const TOKEN_MARKERS: &[&str] = &["input token count", "maximum number of tokens"];

/// What the error body says, from its `google.rpc` details.
#[derive(Default)]
struct Rpc {
    status: String,
    reason: String,
    /// Quota IDs and metrics of every violation.
    quotas: Vec<String>,
    /// The model a quota violation names.
    model: Option<String>,
    retry: Option<Duration>,
}

/// The `error` object of a body, bare or wrapped in a one-element array as the
/// OpenAI-compatible endpoint sends it.
fn error_object(body: &Value) -> Option<&Value> {
    body.get("error")
        .or_else(|| body.get(0).and_then(|item| item.get("error")))
}

/// Whether `body` is a `google.rpc` error (`status` is the canonical code name), so the
/// Gemini rules classify it whichever protocol received it.
pub(crate) fn is_rpc_body(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .as_ref()
        .and_then(error_object)
        .and_then(|error| error.get("status"))
        .is_some_and(Value::is_string)
}

/// A non-2xx response as a typed error.
pub(crate) fn http_error(
    status: u16,
    headers: &HeaderMap,
    body: &str,
    secrets: &[&str],
) -> LlmError {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let rpc = error_object(&parsed).map(rpc).unwrap_or_default();
    let message = redact(&summarize_rpc(&parsed, body), secrets);
    let mut e = classified(status, &rpc, &message);
    e.status = Some(status);
    e.retry_after = rpc.retry.or_else(|| header_delay(headers));
    e
}

/// An error chunk inside a stream: `{error: {code, message, status, details}}`.
pub(super) fn stream_error(chunk: &Value) -> LlmError {
    let error = &chunk["error"];
    let code = error["code"]
        .as_u64()
        .and_then(|code| u16::try_from(code).ok())
        .unwrap_or(500);
    let message = error["message"].as_str().unwrap_or_default();
    let rpc = rpc(error);
    let mut e = classified(code, &rpc, message);
    e.retry_after = rpc.retry;
    e
}

pub(super) fn missing_signature() -> LlmError {
    LlmError::new(ErrorKind::InvalidRequest, MISSING_SIGNATURE)
}

/// Name the tool a rejected declaration belongs to, when the provider's message names
/// one of the request's tools.
pub(super) fn name_tool(mut e: LlmError, tools: &[ToolSpec]) -> LlmError {
    if !e.message.starts_with(DECLARATION) {
        return e;
    }
    let named = tools
        .iter()
        .filter(|tool| e.message.contains(tool.name.as_str()))
        .max_by_key(|tool| tool.name.len());
    if let Some(tool) = named {
        e.message = e
            .message
            .replacen(DECLARATION, &format!("{DECLARATION} for {}", tool.name), 1);
    }
    e
}

fn summarize_rpc(parsed: &Value, body: &str) -> String {
    match error_object(parsed)
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
    {
        Some(message) => message.chars().take(500).collect(),
        None => summarize(body),
    }
}

fn rpc(error: &Value) -> Rpc {
    let mut out = Rpc {
        status: error["status"].as_str().unwrap_or_default().into(),
        ..Rpc::default()
    };
    let details = error.get("details").and_then(Value::as_array);
    for detail in details.into_iter().flatten() {
        let kind = detail["@type"].as_str().unwrap_or_default();
        match kind.strip_prefix(RPC).unwrap_or_default() {
            "ErrorInfo" => out.reason = detail["reason"].as_str().unwrap_or_default().into(),
            "QuotaFailure" => violations(detail, &mut out),
            "RetryInfo" => out.retry = detail["retryDelay"].as_str().and_then(parse_delay),
            _ => {}
        }
    }
    out
}

fn violations(detail: &Value, out: &mut Rpc) {
    let violations = detail.get("violations").and_then(Value::as_array);
    for violation in violations.into_iter().flatten() {
        for key in ["quotaId", "quotaMetric"] {
            out.quotas
                .extend(violation[key].as_str().map(str::to_owned));
        }
        if let Some(model) = violation
            .pointer("/quotaDimensions/model")
            .and_then(Value::as_str)
        {
            out.model = Some(model.into());
        }
    }
}

/// A protobuf JSON duration: `"37s"`, `"1.5s"`.
fn parse_delay(text: &str) -> Option<Duration> {
    let seconds: f64 = text.trim().strip_suffix('s')?.parse().ok()?;
    Duration::try_from_secs_f64(seconds).ok()
}

/// Classification, first match wins (design D5).
fn classified(status: u16, rpc: &Rpc, message: &str) -> LlmError {
    let lower = message.to_ascii_lowercase();
    let (kind, text) = if status == 402 || lower.contains("prepaid") {
        (ErrorKind::QuotaExceeded, Some(PREPAID.to_string()))
    } else {
        match status {
            429 => exhausted(rpc, &lower),
            400 => bad_request(rpc, &lower, message),
            401 | 403 => (ErrorKind::Authentication, None),
            404..=499 => (ErrorKind::InvalidRequest, None),
            _ => (ErrorKind::ProviderInternal, None),
        }
    };
    LlmError::new(kind, text.unwrap_or_else(|| message.to_string()))
}

/// A 429: a daily or weekly quota and a spend cap stop at once; anything else (per-minute
/// requests or tokens, capacity, no details) waits and retries.
fn exhausted(rpc: &Rpc, lower: &str) -> (ErrorKind, Option<String>) {
    if let Some(period) = long_period(&rpc.quotas) {
        let model = rpc
            .model
            .as_deref()
            .map(|model| format!(" for {model}"))
            .unwrap_or_default();
        let id = rpc.quotas.first().map(String::as_str).unwrap_or_default();
        return (
            ErrorKind::QuotaExceeded,
            Some(format!("Gemini {period} quota exhausted{model} ({id})")),
        );
    }
    if SPEND_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return (ErrorKind::QuotaExceeded, Some(SPEND_CAP.into()));
    }
    (ErrorKind::RateLimit, None)
}

fn long_period(quotas: &[String]) -> Option<&'static str> {
    let lower: Vec<String> = quotas.iter().map(|q| q.to_ascii_lowercase()).collect();
    let any = |markers: &[&str]| {
        lower
            .iter()
            .any(|quota| markers.iter().any(|marker| quota.contains(marker)))
    };
    if any(&["perweek", "per_week"]) {
        Some("weekly")
    } else if any(&["perday", "per_day", "rpd"]) {
        Some("daily")
    } else {
        None
    }
}

fn bad_request(rpc: &Rpc, lower: &str, message: &str) -> (ErrorKind, Option<String>) {
    let mentions = |markers: &[&str]| markers.iter().any(|marker| lower.contains(marker));
    if rpc.reason == "API_KEY_INVALID" {
        (ErrorKind::Authentication, None)
    } else if mentions(&["thought signature", "thought_signature"]) {
        (ErrorKind::InvalidRequest, Some(MISSING_SIGNATURE.into()))
    } else if rpc.status == "FAILED_PRECONDITION" {
        (ErrorKind::Authentication, Some(REGION.into()))
    } else if mentions(TOKEN_MARKERS) || mentions(OVERFLOW) {
        (ErrorKind::ContextOverflow, None)
    } else if mentions(SCHEMA_MARKERS) {
        let text = format!("{DECLARATION}: {message}; {DECLARATION_HINT}");
        (ErrorKind::InvalidRequest, Some(text))
    } else {
        (ErrorKind::InvalidRequest, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_parse_protobuf_durations() {
        assert_eq!(parse_delay("37s"), Some(Duration::from_secs(37)));
        assert_eq!(parse_delay("1.5s"), Some(Duration::from_millis(1500)));
        assert_eq!(parse_delay("37"), None);
    }

    #[test]
    fn quota_periods_come_from_ids_and_metrics() {
        let ids = |list: &[&str]| list.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(
            long_period(&ids(&["GenerateRequestsPerDayPerProjectPerModel-FreeTier"])),
            Some("daily")
        );
        assert_eq!(
            long_period(&ids(&["TokensPerWeekPerProject"])),
            Some("weekly")
        );
        assert_eq!(
            long_period(&ids(&["GenerateRequestsPerMinutePerProjectPerModel"])),
            None
        );
        assert_eq!(long_period(&[]), None);
    }
}
