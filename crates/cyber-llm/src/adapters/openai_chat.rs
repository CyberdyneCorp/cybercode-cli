//! OpenAI-compatible Chat Completions (also Ollama, llama.cpp server, vLLM, OpenRouter).

use futures::future::BoxFuture;
use serde_json::{Map, Value, json};

use super::{
    Adapter, Decoder, Endpoint, EventStream, ToolCalls, data_url, decode_sse, document_name,
    ensure_resolved, parse_json,
};
use crate::catalog::profile::ReasoningCarrier;
use crate::error::{LlmError, stream_error};
use crate::json::{deep_merge, force_no_tools, str_at, strip_credentials, u64_at};
use crate::sse::SseEvent;
use crate::types::{
    Content, FinishReason, LlmEvent, LlmRequest, Message, Reasoning, Role, ToolCall, Usage,
};

mod reasoning;

pub struct OpenAiChatAdapter {
    pub(super) endpoint: Endpoint,
    pub(super) response_limit: Option<usize>,
}

impl OpenAiChatAdapter {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            response_limit: None,
        }
    }
}

impl Adapter for OpenAiChatAdapter {
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
            let body = build_body(&request);
            let auth = self
                .endpoint
                .api_key
                .iter()
                .map(|k| ("authorization".to_string(), format!("Bearer {k}")))
                .collect();
            let resp = self
                .endpoint
                .post_stream(
                    "chat/completions",
                    &body,
                    auth,
                    &request.headers,
                    self.response_limit,
                )
                .await?;
            Ok(decode_sse(
                resp,
                ChatDecoder::default(),
                &self.endpoint,
                self.response_limit,
            ))
        })
    }
}

pub(crate) fn build_body(request: &LlmRequest) -> Value {
    let mut messages = Vec::new();
    if !request.system.is_empty() {
        messages.push(json!({ "role": "system", "content": request.system.join("\n\n") }));
    }
    let loop_start = tool_loop_start(&request.messages);
    for (index, message) in request.messages.iter().enumerate() {
        match message.role {
            Role::User => push_user(message, &mut messages),
            Role::Assistant => {
                let mut value = assistant(message, request.reasoning_carrier);
                if index > loop_start {
                    reasoning::hold_tool_loop(&mut value, request.reasoning_carrier);
                }
                messages.push(value);
            }
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

/// The index of the latest prompt: a user message that is more than tool results. The
/// messages after it are the current tool loop.
fn tool_loop_start(messages: &[Message]) -> usize {
    messages
        .iter()
        .rposition(|message| {
            message.role == Role::User
                && message
                    .content
                    .iter()
                    .any(|part| !matches!(part, Content::ToolResult { .. }))
        })
        .unwrap_or(0)
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
            Content::Document {
                media_type,
                data,
                name,
            } => {
                parts.push(json!({ "type": "file", "file": { "filename": document_name(name), "file_data": data_url(media_type, data) } }));
            }
            // References are resolved before a request (`ensure_resolved`).
            Content::Reasoning { .. } | Content::ToolCall { .. } | Content::Attachment(_) => {}
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

fn assistant(message: &Message, carrier: ReasoningCarrier) -> Value {
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut gathered = reasoning::Gathered::default();
    for content in &message.content {
        match content {
            Content::Text { text: t } => text.push_str(t),
            Content::ToolCall {
                id,
                name,
                input,
                opaque,
            } => {
                let mut call = json!({ "id": id, "type": "function", "function": { "name": name, "arguments": input.to_string() } });
                if let Some(extra) = reasoning::call_extra(opaque.as_ref()) {
                    call["extra_content"] = extra;
                }
                calls.push(call);
            }
            Content::Reasoning { text, opaque } => gathered.add(text, opaque.as_ref()),
            _ => {}
        }
    }
    let mut value = json!({ "role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) } });
    if !calls.is_empty() {
        value["tool_calls"] = Value::Array(calls);
    }
    reasoning::carry(&mut value, carrier, &gathered);
    value
}

#[derive(Default)]
struct ChatDecoder {
    tools: ToolCalls,
    reasoning: reasoning::Streamed,
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
            out.push(Err(stream_error(error)));
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
            ("/reasoning_text", true),
        ] {
            if let Some(text) = delta
                .pointer(pointer)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                let event = if reasoning {
                    LlmEvent::ReasoningDelta { text: text.into() }
                } else {
                    // Text after a call ends that call's input (`tool-registry` →
                    // Eager execution during streaming).
                    self.finish_whole(None, out);
                    LlmEvent::TextDelta { text: text.into() }
                };
                out.push(Ok(event));
            }
        }
        self.reasoning.absorb(delta);
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
            // Calls stream one after another: a new one completes those before it, so
            // each is recorded as soon as its input is whole, not at the end of the stream.
            if !self.tools.has(&key) {
                self.finish_whole(Some(&key), out);
            }
            self.tools.start(&key, id, name);
        }
        if let Some(signature) = reasoning::thought_signature(call) {
            self.tools.set_opaque(&key, signature);
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

    fn finish_whole(&mut self, except: Option<&str>, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        let calls = self.tools.finish_whole(except);
        self.push_calls(calls, out);
    }

    /// Completed calls, after the reasoning state that came before them.
    fn push_calls(&mut self, calls: Vec<ToolCall>, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        if !calls.is_empty() {
            out.extend(self.reasoning.take_items().into_iter().map(Ok));
        }
        out.extend(
            calls
                .into_iter()
                .map(|call| Ok(LlmEvent::ToolCallDone(call))),
        );
    }

    fn complete(&mut self, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        self.done = true;
        let calls = self.tools.finish_all();
        self.push_calls(calls, out);
        out.extend(self.reasoning.take_items().into_iter().map(Ok));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_become_file_parts_with_a_data_url() {
        let request = LlmRequest {
            messages: vec![Message {
                role: Role::User,
                content: vec![
                    Content::Text {
                        text: "read this".into(),
                    },
                    Content::Document {
                        media_type: "application/pdf".into(),
                        data: "JVBERi0=".into(),
                        name: None,
                    },
                ],
            }],
            ..LlmRequest::default()
        };
        assert_eq!(
            build_body(&request)["messages"][0]["content"][1],
            json!({ "type": "file", "file": { "filename": "document.pdf", "file_data": "data:application/pdf;base64,JVBERi0=" } })
        );
    }
}
