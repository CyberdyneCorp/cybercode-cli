//! The ChatGPT subscription backend (`provider-catalog` → ChatGPT subscription provider):
//! the Responses protocol at the Codex backend, authorized by the stored login, renewed
//! right before each request. The plan's model list is renewed here too, once a day.

use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{Adapter, Endpoint, EventStream, OpenAiResponsesAdapter};
use crate::chatgpt::{ChatGptError, Login, ORIGINATOR, Session, fetch_models};
use crate::error::LlmError;
use crate::types::LlmRequest;

pub struct ChatGptAdapter {
    endpoint: Endpoint,
    response_limit: Option<usize>,
}

impl ChatGptAdapter {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            response_limit: None,
        }
    }

    /// The endpoint for one request: the fresh access token and the account headers.
    async fn authorized(&self, session: &Session) -> Result<Endpoint, LlmError> {
        let login = session
            .file
            .fresh(&self.endpoint.client, &session.issuer)
            .await?;
        self.refresh_models(session, &login).await;
        let mut endpoint = self.endpoint.clone();
        endpoint.api_key = Some(login.access_token);
        endpoint.headers.extend([
            ("chatgpt-account-id".to_string(), login.account_id),
            (
                "OpenAI-Beta".to_string(),
                "responses=experimental".to_string(),
            ),
            ("originator".to_string(), ORIGINATOR.to_string()),
        ]);
        Ok(endpoint)
    }

    /// Renew a missing or day-old model list; the next catalog build lists it. A failure
    /// never fails the request: the old list is kept and the next attempt waits a day.
    async fn refresh_models(&self, session: &Session, login: &Login) {
        let file = session.models();
        if !file.stale() {
            return;
        }
        file.record(fetch_models(&self.endpoint.client, &self.endpoint.base_url, login).await);
    }
}

impl Adapter for ChatGptAdapter {
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
            let session = self
                .endpoint
                .login
                .clone()
                .ok_or_else(|| LlmError::from(ChatGptError::NotSignedIn))?;
            let endpoint = self.authorized(&session).await?;
            let inner = OpenAiResponsesAdapter {
                endpoint,
                response_limit: self.response_limit,
            };
            inner.stream(shaped(request)).await
        })
    }
}

/// The backend rejects `max_output_tokens` and requires `instructions`.
fn shaped(mut request: LlmRequest) -> LlmRequest {
    request.max_output_tokens = None;
    if request.system.is_empty() && request.body.get("instructions").is_none() {
        if !request.body.is_object() {
            request.body = json!({});
        }
        if let Some(map) = request.body.as_object_mut() {
            map.insert("instructions".into(), Value::String(String::new()));
        }
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::openai_responses::build_body;

    #[test]
    fn shaped_requests_drop_the_output_cap_and_keep_instructions() {
        let request = LlmRequest {
            model: "gpt-5.1".into(),
            max_output_tokens: Some(32_000),
            ..LlmRequest::default()
        };
        let body = build_body(&shaped(request));
        assert!(body.get("max_output_tokens").is_none());
        assert_eq!(body["instructions"], "");
        assert_eq!(body["store"], false);

        let request = LlmRequest {
            system: vec!["be brief".into()],
            ..LlmRequest::default()
        };
        assert_eq!(build_body(&shaped(request))["instructions"], "be brief");
    }
}
