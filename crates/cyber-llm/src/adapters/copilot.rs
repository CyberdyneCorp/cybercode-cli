//! The GitHub Copilot subscription API (`provider-catalog` → GitHub Copilot subscription
//! provider): Chat Completions, or Responses for models Copilot lists with `/responses`,
//! at the API endpoint the minted Copilot token names. This adapter only picks the
//! protocol, attaches the token and adds Copilot's headers; the protocol adapters do the
//! rest. The plan's model list is renewed here too, once a day.

use futures::future::BoxFuture;

use super::{Adapter, Endpoint, EventStream, OpenAiChatAdapter, OpenAiResponsesAdapter};
use crate::copilot::{CopilotError, EDITOR_VERSION, INTEGRATION_ID, Protocol, Route};
use crate::error::LlmError;
use crate::types::{Content, Initiator, LlmRequest};

pub struct CopilotAdapter {
    endpoint: Endpoint,
    response_limit: Option<usize>,
}

impl CopilotAdapter {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            response_limit: None,
        }
    }

    /// The endpoint for one request: a usable Copilot token, its API and the headers.
    async fn authorized(&self, route: &Route, request: &LlmRequest) -> Result<Endpoint, LlmError> {
        let client = &self.endpoint.client;
        let token = route.session.token(client).await?;
        route.session.refresh_models(client).await;
        let mut endpoint = self.endpoint.clone();
        endpoint.base_url = token.api.to_string();
        endpoint.api_key = Some(token.token);
        endpoint.headers.extend(headers(request));
        Ok(endpoint)
    }
}

/// Copilot's integration headers. `X-Initiator` is `user` unless the Turn loop says the
/// call continues a Turn, so an unattributed call is never under-counted.
fn headers(request: &LlmRequest) -> Vec<(String, String)> {
    let initiator = request.initiator.unwrap_or(Initiator::User);
    let mut headers: Vec<(String, String)> = [
        ("Copilot-Integration-Id", INTEGRATION_ID),
        ("Editor-Version", EDITOR_VERSION),
        ("Editor-Plugin-Version", EDITOR_VERSION),
        ("Openai-Intent", "conversation-edits"),
        ("X-Initiator", initiator.as_str()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect();
    if has_images(request) {
        headers.push(("Copilot-Vision-Request".into(), "true".into()));
    }
    headers
}

fn has_images(request: &LlmRequest) -> bool {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .any(|content| matches!(content, Content::Image { .. }))
}

impl Adapter for CopilotAdapter {
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
            super::ensure_resolved(&request)?;
            let route = self
                .endpoint
                .copilot
                .clone()
                .ok_or_else(|| LlmError::from(CopilotError::NotSignedIn))?;
            let endpoint = self.authorized(&route, &request).await?;
            let response_limit = self.response_limit;
            match route.protocol {
                Protocol::Responses => {
                    let inner = OpenAiResponsesAdapter {
                        endpoint,
                        response_limit,
                    };
                    inner.stream(request).await
                }
                Protocol::Chat => {
                    let inner = OpenAiChatAdapter {
                        endpoint,
                        response_limit,
                    };
                    inner.stream(request).await
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, Role};

    fn value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn unattributed_calls_count_as_the_users_and_images_add_the_vision_header() {
        let plain = headers(&LlmRequest::default());
        assert_eq!(value(&plain, "X-Initiator"), Some("user"));
        assert_eq!(value(&plain, "Copilot-Vision-Request"), None);
        let request = LlmRequest {
            initiator: Some(Initiator::Agent),
            messages: vec![Message {
                role: Role::User,
                content: vec![Content::Image {
                    media_type: "image/png".into(),
                    data: "AA==".into(),
                }],
            }],
            ..LlmRequest::default()
        };
        let with_image = headers(&request);
        assert_eq!(value(&with_image, "X-Initiator"), Some("agent"));
        assert_eq!(value(&with_image, "Copilot-Vision-Request"), Some("true"));
        assert_eq!(
            value(&with_image, "Openai-Intent"),
            Some("conversation-edits")
        );
    }

    #[test]
    fn the_vision_header_covers_images_only() {
        let request = LlmRequest {
            messages: vec![Message {
                role: Role::User,
                content: vec![Content::Document {
                    media_type: "application/pdf".into(),
                    data: "JVBERi0=".into(),
                    name: None,
                }],
            }],
            ..LlmRequest::default()
        };
        assert_eq!(value(&headers(&request), "Copilot-Vision-Request"), None);
    }
}
