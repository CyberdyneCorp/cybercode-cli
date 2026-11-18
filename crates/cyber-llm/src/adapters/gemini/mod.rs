//! The Gemini API, `streamGenerateContent` (`provider-catalog` → Gemini API adapter).
//!
//! Stateless: every request carries the whole history, with each thought signature on
//! the part it came on. Request building, decoding and error parsing do not depend on the
//! URL or the auth header, so a Vertex AI protocol could reuse them.

mod decode;
mod errors;
mod request;

use std::time::Duration;

use futures::future::BoxFuture;

use super::{Adapter, Dialect, Endpoint, EventStream, decode_sse, ensure_resolved};
use crate::error::{ErrorKind, LlmError};
use crate::types::LlmRequest;

pub(crate) use errors::{http_error, is_rpc_body};

pub(crate) const DEFAULT_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// The stream idle window of the `gemini` protocol: the API sends nothing while a model
/// thinks, and high-effort answers can be silent for more than five minutes.
pub const GEMINI_IDLE_TIMEOUT: Duration = Duration::from_secs(600);

pub struct GeminiAdapter {
    endpoint: Endpoint,
    response_limit: Option<usize>,
}

impl GeminiAdapter {
    pub fn new(mut endpoint: Endpoint) -> Self {
        endpoint.base_url = gemini_base_url(&endpoint.base_url);
        endpoint.dialect = Dialect::Gemini;
        Self {
            endpoint,
            response_limit: None,
        }
    }
}

impl Adapter for GeminiAdapter {
    fn with_http_client(
        &self,
        client: reqwest::Client,
        response_limit: usize,
    ) -> Option<Box<dyn Adapter>> {
        let mut endpoint = self.endpoint.clone();
        endpoint.client = client;
        Some(Box::new(Self {
            endpoint,
            response_limit: Some(response_limit),
        }))
    }

    fn stream(&self, request: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>> {
        Box::pin(async move {
            ensure_resolved(&request)?;
            if !gemini_url_allowed(&self.endpoint.base_url) {
                return Err(insecure(&self.endpoint.base_url));
            }
            let body = request::build_body(&request)?;
            // The key travels only in a header, so it never appears in a logged URL.
            let auth = self
                .endpoint
                .api_key
                .iter()
                .map(|key| ("x-goog-api-key".to_string(), key.clone()))
                .collect();
            let path = format!("models/{}:streamGenerateContent?alt=sse", request.model);
            let resp = self
                .endpoint
                .post_stream(&path, &body, auth, &request.headers, self.response_limit)
                .await
                .map_err(|e| errors::name_tool(e, &request.tools))?;
            Ok(decode_sse(
                resp,
                decode::GeminiDecoder::default(),
                &self.endpoint,
                self.response_limit,
            ))
        })
    }
}

/// A configured base URL as requests use it: without a trailing `/`, and with `/v1beta`
/// when it names no path. Any other path (a proxy prefix) is kept as written.
pub fn gemini_base_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let rest = trimmed.split_once("://").map_or(trimmed, |(_, rest)| rest);
    if rest.contains('/') {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1beta")
    }
}

/// Requests go over HTTPS, or plain HTTP to a loopback address only.
pub fn gemini_url_allowed(url: &str) -> bool {
    url.starts_with("https://")
        || (url.starts_with("http://") && crate::catalog::is_loopback_url(url))
}

fn insecure(url: &str) -> LlmError {
    LlmError::new(
        ErrorKind::InvalidRequest,
        format!("Gemini base URL {url} must use https unless it is a loopback address"),
    )
}

/// Whether sending failed before any response header on a pooled connection the server
/// had already closed or reset (HTTP/2 GOAWAY, connection reset, broken pipe), so a
/// resend on a new connection is safe.
pub(crate) fn stale_connection(error: &reqwest::Error) -> bool {
    if error.is_timeout() || error.is_connect() || error.is_builder() || error.status().is_some() {
        return false;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
            )
        {
            return true;
        }
        let text = current.to_string().to_ascii_lowercase();
        if STALE.iter().any(|marker| text.contains(marker)) {
            return true;
        }
        source = current.source();
    }
    false
}

const STALE: &[&str] = &[
    "connection closed before message completed",
    "connection reset",
    "broken pipe",
    "goaway",
    "go away",
];

#[cfg(test)]
mod tests {
    use super::*;

    use crate::adapters::ApiKind;

    #[test]
    fn the_protocol_parses_its_aliases_and_package() {
        for alias in ["gemini", "google", "google-genai"] {
            assert_eq!(ApiKind::parse(alias), Some(ApiKind::Gemini), "{alias}");
        }
        assert_eq!(
            ApiKind::parse(ApiKind::Gemini.as_str()),
            Some(ApiKind::Gemini)
        );
        assert_eq!(ApiKind::from_npm("@ai-sdk/google"), Some(ApiKind::Gemini));
        assert_eq!(ApiKind::from_npm("@ai-sdk/google-vertex"), None);
        assert_eq!(ApiKind::Gemini.default_url(), Some(DEFAULT_URL));
        assert_eq!(serde_json::to_value(ApiKind::Gemini).unwrap(), "gemini");
    }

    #[test]
    fn base_urls_are_normalized() {
        assert_eq!(gemini_base_url(DEFAULT_URL), DEFAULT_URL);
        assert_eq!(
            gemini_base_url("https://generativelanguage.googleapis.com/v1beta/"),
            DEFAULT_URL
        );
        assert_eq!(
            gemini_base_url("https://generativelanguage.googleapis.com"),
            DEFAULT_URL
        );
        assert_eq!(
            gemini_base_url("https://generativelanguage.googleapis.com/"),
            DEFAULT_URL
        );
        assert_eq!(
            gemini_base_url("https://proxy.corp/gemini/"),
            "https://proxy.corp/gemini",
            "a proxy prefix is kept as written"
        );
    }

    #[test]
    fn plain_http_is_allowed_only_for_loopback() {
        assert!(gemini_url_allowed(DEFAULT_URL));
        assert!(gemini_url_allowed("http://127.0.0.1:8080/v1beta"));
        assert!(gemini_url_allowed("http://localhost:9/v1beta"));
        assert!(!gemini_url_allowed("http://gemini.corp.example"));
        assert!(!gemini_url_allowed("ftp://gemini.corp.example"));
    }
}
