//! OpenAI-compatible Chat Completions (also Ollama, llama.cpp server, vLLM, OpenRouter).

use futures::future::BoxFuture;
use serde_json::{Map, Value, json};

use super::{Adapter, Decoder, Endpoint, EventStream, ToolCalls, data_url, decode_sse, parse_json};
use crate::error::{LlmError, classify_stream_error};
use crate::json::{deep_merge, force_no_tools, str_at, strip_credentials, u64_at};
use crate::sse::SseEvent;
use crate::types::{Content, FinishReason, LlmEvent, LlmRequest, Message, Reasoning, Role, Usage};

pub struct OpenAiChatAdapter {
    endpoint: Endpoint,
}

impl OpenAiChatAdapter {
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }
}

impl Adapter for OpenAiChatAdapter {
    fn stream(&self, request: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>> {
        Box::pin(async move {
            let body = build_body(&request);
            let auth = self
                .endpoint
                .api_key
                .iter()
                .map(|k| ("authorization".to_string(), format!("Bearer {k}")))
                .collect();
            let resp = self
                .endpoint
                .post_stream("chat/completions", &body, auth, &request.headers)
                .await?;
            Ok(decode_sse(resp, ChatDecoder::default(), &self.endpoint))
        })
    }
}

pub(crate) fn build_body(request: &LlmRequest) -> Value {
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(json!({ "role": "system", "content": request.system.join("\n\n") }));
    }
    for message in &request.messages {
        match message.role {
            Role::User => push_user(message, &mut messages),
            Role::Assistant => messages.push(assistant(message)),
        }
    }
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    let map = body.as_object_mut().expect("object literal");
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.input_schema } }))
            .collect();
        map.insert("tools".into(), Value::Array(tools));
    }
    optional_fields(request, map);
    deep_merge(&mut body, &request.body);
    force_no_tools(&mut body, request.tools_disabled, json!("none"));
    strip_credentials(&mut body);
    body
}

fn optional_fields(request: &LlmRequest, map: &mut Map<String, Value>) {
    if let Some(max) = request.max_output_tokens {
        map.insert("max_tokens".into(), json!(max));
    }
    if let Some(t) = request.temperature {
        map.insert("temperature".into(), json!(t));
    }
    if let Some(Reasoning::Effort(effort)) = &request.reasoning {
        map.insert("reasoning_effort".into(), json!(effort));
    }
    if let (Some(key), true) = (&request.cache_key, request.cache) {
        map.insert("prompt_cache_key".into(), json!(key));
    }
}

fn push_user(message: &Message, out: &mut Vec<Value>) {
    let mut parts = Vec::new();
    for content in &message.content {
        match content {
            Content::ToolResult {
                call_id, output, ..
            } => {
                out.push(json!({ "role": "tool", "tool_call_id": call_id, "content": output }));
            }
            Content::Text { text } => parts.push(json!({ "type": "text", "text": text })),
            Content::Image { media_type, data } => {
                parts.push(json!({ "type": "image_url", "image_url": { "url": data_url(media_type, data) } }));
            }
            Content::Reasoning { .. } | Content::ToolCall { .. } => {}
        }
    }
    match parts.as_slice() {
        [] => {}
        [only] if only["type"] == "text" => {
            out.push(json!({ "role": "user", "content": only["text"] }))
        }
        _ => out.push(json!({ "role": "user", "content": parts })),
    }
}

fn assistant(message: &Message) -> Value {
    let mut text = String::new();
    let mut calls = Vec::new();
    for content in &message.content {
        match content {
            Content::Text { text: t } => text.push_str(t),
            Content::ToolCall { id, name, input } => calls.push(
                json!({ "id": id, "type": "function", "function": { "name": name, "arguments": input.to_string() } }),
            ),
            _ => {}
        }
    }
    let mut value = json!({ "role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) } });
    if !calls.is_empty() {
        value["tool_calls"] = Value::Array(calls);
    }
    value
}

#[derive(Default)]
struct ChatDecoder {
    tools: ToolCalls,
    finish: Option<String>,
    usage: Option<Usage>,
    done: bool,
}

impl Decoder for ChatDecoder {
    fn on_event(&mut self, event: SseEvent, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        if self.done {
            return;
        }
        if event.data.trim() == "[DONE]" {
            self.complete(out);
            return;
        }
        match parse_json(&event) {
            Ok(chunk) => self.chunk(&chunk, out),
            Err(e) => out.push(Err(e)),
        }
    }

    fn on_end(&mut self, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        if self.done {
            return;
        }
        if self.finish.is_some() {
            self.complete(out);
        } else {
            out.push(Err(LlmError::transport(
                "stream ended before the response completed",
            )));
        }
    }
}

impl ChatDecoder {
    fn chunk(&mut self, chunk: &Value, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        if let Some(error) = chunk.get("error") {
            let kind = classify_stream_error(str_at(error, "/code"), str_at(error, "/message"));
            out.push(Err(LlmError::new(kind, str_at(error, "/message"))));
            return;
        }
        if let Some(usage) = chunk.get("usage").filter(|u| u.is_object()) {
            self.usage = Some(usage_from(usage));
        }
        let Some(choice) = chunk.pointer("/choices/0") else {
            return;
        };
        let delta = &choice["delta"];
        for (pointer, reasoning) in [
            ("/content", false),
            ("/reasoning_content", true),
            ("/reasoning", true),
        ] {
            if let Some(text) = delta
                .pointer(pointer)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                let event = if reasoning {
                    LlmEvent::ReasoningDelta { text: text.into() }
                } else {
                    LlmEvent::TextDelta { text: text.into() }
                };
                out.push(Ok(event));
            }
        }
        for call in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.tool_delta(call, out);
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish = Some(reason.to_string());
        }
    }

    fn tool_delta(&mut self, call: &Value, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        let key = call
            .get("index")
            .map(Value::to_string)
            .unwrap_or_else(|| "0".into());
        let id = str_at(call, "/id");
        let name = str_at(call, "/function/name");
        if !id.is_empty() || !name.is_empty() {
            self.tools.start(&key, id, name);
        }
        let fragment = str_at(call, "/function/arguments");
        if let Some(event) = self
            .tools
            .append(&key, fragment)
            .filter(|_| !fragment.is_empty())
        {
            out.push(Ok(event));
        }
    }

    fn complete(&mut self, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        self.done = true;
        for call in self.tools.finish_all() {
            out.push(Ok(LlmEvent::ToolCallDone(call)));
        }
        if let Some(usage) = self.usage.take() {
            out.push(Ok(LlmEvent::Usage(usage)));
        }
        let reason = match self.finish.as_deref() {
            Some("tool_calls" | "function_call") => FinishReason::ToolCalls,
            Some("length") => FinishReason::Length,
            Some("content_filter") => FinishReason::ContentFilter,
            _ if self.tools.any() => FinishReason::ToolCalls,
            Some("stop") | None => FinishReason::Stop,
            Some(other) => FinishReason::Other(other.into()),
        };
        out.push(Ok(LlmEvent::Finish { reason }));
    }
}

fn usage_from(u: &Value) -> Usage {
    let cached = u64_at(u, "/prompt_tokens_details/cached_tokens");
    let reasoning = u64_at(u, "/completion_tokens_details/reasoning_tokens");
    Usage {
        input: u64_at(u, "/prompt_tokens").saturating_sub(cached),
        output: u64_at(u, "/completion_tokens").saturating_sub(reasoning),
        reasoning,
        cache_read: cached,
        cache_write: 0,
    }
}
