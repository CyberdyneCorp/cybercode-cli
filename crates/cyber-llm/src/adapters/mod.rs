//! Native adapters behind one trait (`provider-catalog` → Native provider adapters).

mod anthropic;
mod openai_chat;
mod openai_responses;
mod scripted;

use std::collections::VecDeque;
use std::pin::Pin;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::{self, BoxStream, Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::LlmError;
use crate::sse::{SseEvent, SseParser};
use crate::types::{LlmEvent, LlmRequest, ToolCall};

pub use anthropic::AnthropicAdapter;
pub use openai_chat::OpenAiChatAdapter;
pub use openai_responses::OpenAiResponsesAdapter;
pub use scripted::{ScriptStep, ScriptedAdapter};

pub type EventStream = Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send>>;

/// The wire protocol a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiKind {
    OpenaiResponses,
    OpenaiCompatible,
    Anthropic,
}

impl ApiKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "openai-responses" | "openai" => Some(Self::OpenaiResponses),
            "openai-compatible" | "openai-chat" => Some(Self::OpenaiCompatible),
            "anthropic" => Some(Self::Anthropic),
            _ => None,
        }
    }

    /// The SDK package named by models.dev, mapped to a P0 adapter.
    pub fn from_npm(npm: &str) -> Option<Self> {
        match npm {
            "@ai-sdk/openai" => Some(Self::OpenaiResponses),
            "@ai-sdk/anthropic" => Some(Self::Anthropic),
            "@ai-sdk/openai-compatible" | "@openrouter/ai-sdk-provider" => {
                Some(Self::OpenaiCompatible)
            }
            _ => None,
        }
    }

    pub fn default_url(self) -> Option<&'static str> {
        match self {
            Self::OpenaiResponses => Some("https://api.openai.com/v1"),
            Self::Anthropic => Some("https://api.anthropic.com/v1"),
            Self::OpenaiCompatible => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiResponses => "openai-responses",
            Self::OpenaiCompatible => "openai-compatible",
            Self::Anthropic => "anthropic",
        }
    }
}

/// One provider stream per call.
pub trait Adapter: Send + Sync {
    fn stream(&self, request: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>>;
}

/// Build the adapter for a protocol.
pub fn adapter(kind: ApiKind, endpoint: Endpoint) -> Box<dyn Adapter> {
    match kind {
        ApiKind::OpenaiResponses => Box::new(OpenAiResponsesAdapter::new(endpoint)),
        ApiKind::OpenaiCompatible => Box::new(OpenAiChatAdapter::new(endpoint)),
        ApiKind::Anthropic => Box::new(AnthropicAdapter::new(endpoint)),
    }
}

/// Where and how to reach a provider.
#[derive(Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: Vec<(String, String)>,
    pub client: reqwest::Client,
}

impl Endpoint {
    pub fn new(base_url: impl Into<String>, api_key: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .user_agent(format!(
                "cyber/{} ({}; {}; cli)",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
            .build()
            .unwrap_or_default();
        Self {
            base_url: base_url.into(),
            api_key,
            headers: Vec::new(),
            client,
        }
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        )
    }

    fn secrets(&self) -> Vec<&str> {
        self.api_key.iter().map(String::as_str).collect()
    }

    /// POST a streaming request; non-2xx responses become classified errors.
    pub(crate) async fn post_stream(
        &self,
        path: &str,
        body: &Value,
        auth: Vec<(String, String)>,
        extra: &[(String, String)],
    ) -> Result<reqwest::Response, LlmError> {
        let mut req = self
            .client
            .post(self.url(path))
            .header("accept", "text/event-stream")
            .json(body);
        for (name, value) in auth.iter().chain(&self.headers).chain(extra) {
            req = req.header(name.as_str(), value.as_str());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| LlmError::from_reqwest(&e, &self.secrets()))?;
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(resp);
        }
        let headers = resp.headers().clone();
        let text = resp.text().await.unwrap_or_default();
        Err(LlmError::from_http(
            status,
            &headers,
            &text,
            &self.secrets(),
        ))
    }
}

/// Protocol-specific translation of SSE events into provider-neutral events.
pub(crate) trait Decoder: Send + 'static {
    fn on_event(&mut self, event: SseEvent, out: &mut Vec<Result<LlmEvent, LlmError>>);
    /// Called when the body ends; must emit `Finish` or an error if it has not already.
    fn on_end(&mut self, out: &mut Vec<Result<LlmEvent, LlmError>>);
}

struct DecodeState<D> {
    body: BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>,
    parser: SseParser,
    decoder: D,
    queue: VecDeque<Result<LlmEvent, LlmError>>,
    finished: bool,
    secrets: Vec<String>,
}

impl<D: Decoder> DecodeState<D> {
    fn feed(&mut self, chunk: Option<Result<bytes::Bytes, reqwest::Error>>) {
        let mut out = Vec::new();
        match chunk {
            Some(Ok(bytes)) => {
                for event in self.parser.push(&bytes) {
                    self.decoder.on_event(event, &mut out);
                }
            }
            Some(Err(e)) => {
                let secrets: Vec<&str> = self.secrets.iter().map(String::as_str).collect();
                out.push(Err(LlmError::from_reqwest(&e, &secrets)));
            }
            None => {
                if let Some(event) = self.parser.finish() {
                    self.decoder.on_event(event, &mut out);
                }
                self.decoder.on_end(&mut out);
                self.finished = true;
            }
        }
        self.queue.extend(out);
    }

    fn pop(&mut self) -> Option<Result<LlmEvent, LlmError>> {
        let item = self.queue.pop_front()?;
        if item.is_err() {
            // Nothing follows an error.
            self.queue.clear();
            self.finished = true;
        }
        Some(item)
    }
}

pub(crate) fn decode_sse<D: Decoder>(
    resp: reqwest::Response,
    decoder: D,
    endpoint: &Endpoint,
) -> EventStream {
    let state = DecodeState {
        body: resp.bytes_stream().boxed(),
        parser: SseParser::default(),
        decoder,
        queue: VecDeque::new(),
        finished: false,
        secrets: endpoint.api_key.iter().cloned().collect(),
    };
    stream::unfold(state, |mut s| async move {
        loop {
            if let Some(item) = s.pop() {
                return Some((item, s));
            }
            if s.finished {
                return None;
            }
            let chunk = s.body.next().await;
            s.feed(chunk);
        }
    })
    .boxed()
}

/// Accumulates streamed tool calls keyed by the protocol's index or item ID.
#[derive(Debug, Default)]
pub(crate) struct ToolCalls {
    pending: Vec<Pending>,
    completed: usize,
}

#[derive(Debug)]
struct Pending {
    key: String,
    id: String,
    name: String,
    arguments: String,
    done: bool,
}

impl ToolCalls {
    pub fn start(&mut self, key: &str, id: &str, name: &str) {
        if self.pending.iter().any(|p| p.key == key) {
            return;
        }
        self.pending.push(Pending {
            key: key.into(),
            id: id.into(),
            name: name.into(),
            arguments: String::new(),
            done: false,
        });
    }

    pub fn append(&mut self, key: &str, fragment: &str) -> Option<LlmEvent> {
        let p = self.pending.iter_mut().find(|p| p.key == key && !p.done)?;
        p.arguments.push_str(fragment);
        Some(LlmEvent::ToolCallDelta {
            id: p.id.clone(),
            name: p.name.clone(),
            arguments: fragment.into(),
        })
    }

    /// Complete one call; `arguments` replaces the accumulated text when given.
    pub fn finish(&mut self, key: &str, arguments: Option<&str>) -> Option<ToolCall> {
        let p = self.pending.iter_mut().find(|p| p.key == key && !p.done)?;
        if let Some(full) = arguments {
            p.arguments = full.to_string();
        }
        p.done = true;
        self.completed += 1;
        Some(to_call(p))
    }

    /// Complete every open call, in start order.
    pub fn finish_all(&mut self) -> Vec<ToolCall> {
        let open: Vec<String> = self
            .pending
            .iter()
            .filter(|p| !p.done)
            .map(|p| p.key.clone())
            .collect();
        open.iter().filter_map(|k| self.finish(k, None)).collect()
    }

    pub fn any(&self) -> bool {
        !self.pending.is_empty()
    }
}

fn to_call(p: &Pending) -> ToolCall {
    let trimmed = p.arguments.trim();
    let input = if trimmed.is_empty() {
        Some(Value::Object(Default::default()))
    } else {
        serde_json::from_str(trimmed).ok()
    };
    ToolCall {
        id: p.id.clone(),
        name: p.name.clone(),
        arguments: p.arguments.clone(),
        input,
    }
}

pub(crate) fn data_url(media_type: &str, data: &str) -> String {
    format!("data:{media_type};base64,{data}")
}

pub(crate) fn parse_json(event: &SseEvent) -> Result<Value, LlmError> {
    serde_json::from_str(&event.data)
        .map_err(|e| LlmError::transport(format!("malformed stream event: {e}")))
}
