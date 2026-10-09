//! Anthropic Messages API.

use futures::future::BoxFuture;
use serde_json::{Map, Value, json};

use super::{Adapter, Decoder, Endpoint, EventStream, ToolCalls, decode_sse, parse_json};
use crate::error::{LlmError, classify_stream_error};
use crate::json::{deep_merge, force_no_tools, str_at, strip_credentials, u64_at};
use crate::sse::SseEvent;
use crate::types::{Content, FinishReason, LlmEvent, LlmRequest, Message, Reasoning, Role, Usage};

const API_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8192;

pub struct AnthropicAdapter {
    endpoint: Endpoint,
    response_limit: Option<usize>,
}

impl AnthropicAdapter {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            response_limit: None,
        }
    }
}

impl Adapter for AnthropicAdapter {
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
            let body = build_body(&request);
            let mut auth = vec![("anthropic-version".to_string(), API_VERSION.to_string())];
            auth.extend(
                self.endpoint
                    .api_key
                    .iter()
                    .map(|k| ("x-api-key".to_string(), k.clone())),
            );
            let resp = self
                .endpoint
                .post_stream(
                    "messages",
                    &body,
                    auth,
                    &request.headers,
                    self.response_limit,
                )
                .await?;
            Ok(decode_sse(
                resp,
                AnthropicDecoder::default(),
                &self.endpoint,
                self.response_limit,
            ))
        })
    }
}

/// Output headroom for thinking at an effort level; adaptive thinking counts against
/// `max_tokens`.
fn effort_headroom(effort: &str) -> u32 {
    match effort {
        "minimal" | "low" => 4096,
        "medium" => 16_384,
        "high" => 32_768,
        "xhigh" => 49_152,
        _ => 63_999,
    }
}

/// The `thinking` object and the output headroom it needs.
///
/// Models whose catalog entry offers only effort levels reject `budget_tokens` and take
/// adaptive thinking with `output_config.effort`; models that accept budgets resolve to
/// [`Reasoning::BudgetTokens`] in the catalog.
fn thinking(reasoning: Option<&Reasoning>) -> Option<(Value, u32)> {
    match reasoning? {
        Reasoning::BudgetTokens(b) => Some((json!({ "type": "enabled", "budget_tokens": b }), *b)),
        Reasoning::Effort(e) => Some((json!({ "type": "adaptive" }), effort_headroom(e))),
        Reasoning::Off => None,
    }
}

/// Anthropic effort levels; `minimal` has no Anthropic equivalent.
fn anthropic_effort(effort: &str) -> &str {
    if effort == "minimal" { "low" } else { effort }
}

pub(crate) fn build_body(request: &LlmRequest) -> Value {
    let cache = request.cache.then(|| json!({ "type": "ephemeral" }));
    let thinking = thinking(request.reasoning.as_ref());
    let mut max_tokens = request.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
    if let Some((_, headroom)) = &thinking {
        max_tokens = max_tokens.max(headroom + 1024);
    }
    let mut body = json!({
        "model": request.model,
        "max_tokens": max_tokens,
                "messages": with_history_breakpoints(messages(&request.messages), &cache),
        "stream": true,
    });
    let map = body.as_object_mut().expect("object literal");
    if !request.system.is_empty() {
        map.insert(
            "system".into(),
            with_breakpoint(
                request
                    .system
                    .iter()
                    .map(|t| json!({ "type": "text", "text": t }))
                    .collect(),
                &cache,
            ),
        );
    }
    if !request.tools.is_empty() {
        let tools = request
            .tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.input_schema }))
            .collect();
        map.insert("tools".into(), with_breakpoint(tools, &cache));
    }
    optional_fields(request, thinking.map(|(t, _)| t), map);
    deep_merge(&mut body, &request.body);
    force_no_tools(&mut body, request.tools_disabled, json!({"type":"none"}));
    strip_credentials(&mut body);
    body
}

fn optional_fields(request: &LlmRequest, thinking: Option<Value>, map: &mut Map<String, Value>) {
    if let Some(Reasoning::Effort(e)) = &request.reasoning {
        map.insert(
            "output_config".into(),
            json!({ "effort": anthropic_effort(e) }),
        );
    }
    match thinking {
        // Temperature cannot be combined with extended thinking.
        Some(t) => {
            map.insert("thinking".into(), t);
        }
        None => {
            if let Some(t) = request.temperature {
                map.insert("temperature".into(), json!(t));
            }
        }
    }
}

/// Mark the last block as a cache breakpoint.
fn with_breakpoint(mut blocks: Vec<Value>, cache: &Option<Value>) -> Value {
    if let (Some(last), Some(control)) = (blocks.last_mut(), cache) {
        last["cache_control"] = control.clone();
    }
    Value::Array(blocks)
}

/// Mark the end of the conversation (and of the message before it) as cache breakpoints,
/// so each Turn reads the history the previous Turn wrote. With the system and tool
/// breakpoints that is Anthropic's limit of four. Thinking blocks cannot carry one.
fn with_history_breakpoints(mut messages: Vec<Value>, cache: &Option<Value>) -> Vec<Value> {
    let Some(control) = cache else {
        return messages;
    };
    let n = messages.len();
    for message in messages.iter_mut().skip(n.saturating_sub(2)) {
        let cacheable = message["content"].as_array_mut().and_then(|blocks| {
            blocks
                .iter_mut()
                .rev()
                .find(|b| !matches!(b["type"].as_str(), Some("thinking" | "redacted_thinking")))
        });
        if let Some(block) = cacheable {
            block["cache_control"] = control.clone();
        }
    }
    messages
}

/// Convert history, merging consecutive messages of the same role.
fn messages(history: &[Message]) -> Vec<Value> {
    let mut out: Vec<(Role, Vec<Value>)> = Vec::new();
    for message in history {
        let blocks = blocks(message);
        if blocks.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some((role, existing)) if *role == message.role => existing.extend(blocks),
            _ => out.push((message.role, blocks)),
        }
    }
    out.into_iter()
        .map(|(role, content)| {
            let role = if role == Role::User {
                "user"
            } else {
                "assistant"
            };
            json!({ "role": role, "content": content })
        })
        .collect()
}

fn blocks(message: &Message) -> Vec<Value> {
    let mut first = Vec::new();
    let mut rest = Vec::new();
    for content in &message.content {
        match content {
            // Tool results and signed thinking must lead their message.
            Content::ToolResult { call_id, output, is_error } => {
                first.push(json!({ "type": "tool_result", "tool_use_id": call_id, "content": output, "is_error": is_error }));
            }
            Content::Reasoning { text, signature: Some(sig) } => {
                first.push(json!({ "type": "thinking", "thinking": text, "signature": sig }));
            }
            Content::Text { text } if !text.is_empty() => rest.push(json!({ "type": "text", "text": text })),
            Content::Image { media_type, data } => rest.push(
                json!({ "type": "image", "source": { "type": "base64", "media_type": media_type, "data": data } }),
            ),
            Content::ToolCall { id, name, input } => rest.push(json!({ "type": "tool_use", "id": id, "name": name, "input": input })),
            Content::Text { .. } | Content::Reasoning { signature: None, .. } => {}
        }
    }
    first.extend(rest);
    first
}

#[derive(Default)]
struct AnthropicDecoder {
    tools: ToolCalls,
    usage: Usage,
    stop_reason: Option<String>,
    done: bool,
}

impl Decoder for AnthropicDecoder {
    fn on_event(&mut self, event: SseEvent, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        if self.done {
            return;
        }
        let data = match parse_json(&event) {
            Ok(data) => data,
            Err(e) => return out.push(Err(e)),
        };
        let kind = event
            .event
            .as_deref()
            .unwrap_or_else(|| str_at(&data, "/type"))
            .to_string();
        self.dispatch(&kind, &data, out);
    }

    fn on_end(&mut self, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        if !self.done {
            out.push(Err(LlmError::transport("stream ended before message_stop")));
        }
    }
}

impl AnthropicDecoder {
    fn dispatch(&mut self, kind: &str, data: &Value, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        match kind {
            "message_start" => self.start_usage(&data["message"]["usage"]),
            "content_block_start" if str_at(data, "/content_block/type") == "tool_use" => {
                let key = data["index"].to_string();
                self.tools.start(
                    &key,
                    str_at(data, "/content_block/id"),
                    str_at(data, "/content_block/name"),
                );
            }
            "content_block_delta" => self.delta(data, out),
            "content_block_stop" => {
                if let Some(call) = self.tools.finish(&data["index"].to_string(), None) {
                    out.push(Ok(LlmEvent::ToolCallDone(call)));
                }
            }
            "message_delta" => {
                if let Some(reason) = data.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(reason.into());
                }
                if let Some(n) = data.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                    self.usage.output = n;
                }
            }
            "message_stop" => self.complete(out),
            "error" => {
                self.done = true;
                let message = str_at(data, "/error/message");
                out.push(Err(LlmError::new(
                    classify_stream_error(str_at(data, "/error/type"), message),
                    message,
                )));
            }
            _ => {}
        }
    }

    fn start_usage(&mut self, u: &Value) {
        self.usage = Usage {
            input: u64_at(u, "/input_tokens"),
            output: u64_at(u, "/output_tokens"),
            reasoning: 0,
            cache_read: u64_at(u, "/cache_read_input_tokens"),
            cache_write: u64_at(u, "/cache_creation_input_tokens"),
        };
    }

    fn delta(&mut self, data: &Value, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        let delta = &data["delta"];
        let event = match str_at(delta, "/type") {
            "text_delta" => Some(LlmEvent::TextDelta {
                text: str_at(delta, "/text").into(),
            }),
            "thinking_delta" => Some(LlmEvent::ReasoningDelta {
                text: str_at(delta, "/thinking").into(),
            }),
            "signature_delta" => Some(LlmEvent::ReasoningSignature {
                signature: str_at(delta, "/signature").into(),
            }),
            "input_json_delta" => self
                .tools
                .append(&data["index"].to_string(), str_at(delta, "/partial_json")),
            _ => None,
        };
        out.extend(event.map(Ok));
    }

    fn complete(&mut self, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        self.done = true;
        for call in self.tools.finish_all() {
            out.push(Ok(LlmEvent::ToolCallDone(call)));
        }
        out.push(Ok(LlmEvent::Usage(self.usage)));
        let reason = match self.stop_reason.as_deref() {
            Some("tool_use") => FinishReason::ToolCalls,
            Some("max_tokens") => FinishReason::Length,
            Some("refusal") => FinishReason::ContentFilter,
            Some("end_turn" | "stop_sequence") | None => FinishReason::Stop,
            Some(other) => FinishReason::Other(other.into()),
        };
        out.push(Ok(LlmEvent::Finish { reason }));
    }
}
