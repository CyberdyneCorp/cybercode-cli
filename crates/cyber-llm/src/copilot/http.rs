//! What every GitHub and Copilot request shares: the editor headers, a timeout and a
//! bounded JSON body.
use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;

use super::{CopilotError, EDITOR_VERSION, INTEGRATION_ID};

pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// `request` with the headers GitHub's Copilot endpoints expect from an editor.
pub(super) fn editor(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header("accept", "application/json")
        .header("editor-version", EDITOR_VERSION)
        .header("editor-plugin-version", EDITOR_VERSION)
        .header("copilot-integration-id", INTEGRATION_ID)
        .timeout(REQUEST_TIMEOUT)
}

/// Send `request`; the status and the JSON body of at most `limit` bytes. A body that is
/// not JSON reads as `Null`, so error statuses without one are still reported.
pub(super) async fn send(
    request: reqwest::RequestBuilder,
    stage: &'static str,
    limit: usize,
) -> Result<(u16, Value), CopilotError> {
    let response = request
        .send()
        .await
        .map_err(|_| CopilotError::Transport(stage))?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| CopilotError::Transport(stage))?;
        if bytes.len() + chunk.len() > limit {
            return Err(CopilotError::Body("response too large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    ))
}

/// Only visible ASCII is accepted, so a token can never inject into a request header.
pub(super) fn visible(token: &str) -> bool {
    !token.is_empty() && token.len() <= 16 * 1024 && token.bytes().all(|b| b.is_ascii_graphic())
}

/// A string field of `document` that is safe to send as a header value.
pub(super) fn token(document: &Value, name: &str) -> Option<String> {
    document[name]
        .as_str()
        .filter(|value| visible(value))
        .map(str::to_owned)
}
