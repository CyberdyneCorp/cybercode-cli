//! OpenAI Responses API.

use futures::future::BoxFuture;
use serde_json::{Map, Value, json};

use super::{Adapter, Decoder, Endpoint, EventStream, ToolCalls, data_url, decode_sse, parse_json};
use crate::error::{LlmError, classify_stream_error};
use crate::json::{deep_merge, force_no_tools, str_at, strip_credentials, u64_at};
use crate::sse::SseEvent;
use crate::types::{Content, FinishReason, LlmEvent, LlmRequest, Message, Reasoning, Role, Usage};

pub struct OpenAiResponsesAdapter {
    endpoint: Endpoint,
}

impl OpenAiResponsesAdapter {
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }
}

impl Adapter for OpenAiResponsesAdapter {
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
                .post_stream("responses", &body, auth, &request.headers)
                .await?;
            Ok(decode_sse(
                resp,
                ResponsesDecoder::default(),
                &self.endpoint,
            ))
        })
    }
}

pub(crate) fn build_body(request: &LlmRequest) -> Value {
    let input: Vec<Value> = request.messages.iter().flat_map(input_items).collect();
    let mut body =
        json!({ "model": request.model, "input": input, "stream": true, "store": false });
    let map = body.as_object_mut().expect("object literal");
    if !request.system.is_empty() {
        map.insert("instructions".into(), json!(request.system.join("\n\n")));
    }
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| json!({ "type": "function", "name": t.name, "description": t.description, "parameters": t.input_schema, "strict": false }))
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
        map.insert("max_output_tokens".into(), json!(max));
    }
    if let Some(t) = request.temperature {
        map.insert("temperature".into(), json!(t));
    }
    match &request.reasoning {
        Some(Reasoning::Effort(effort)) => {
            map.insert(
                "reasoning".into(),
                json!({ "effort": effort, "summary": "auto" }),
            );
        }
        Some(Reasoning::Off) => {
            map.insert("reasoning".into(), json!({ "effort": "none" }));
        }
        _ => {}
    }
    if let (Some(key), true) = (&request.cache_key, request.cache) {
        map.insert("prompt_cache_key".into(), json!(key));
    }
}

fn input_items(message: &Message) -> Vec<Value> {
    let mut items = Vec::new();
    let mut parts = Vec::new();
    let user = message.role == Role::User;
    for content in &message.content {
        match content {
            Content::Text { text } if user => {
                parts.push(json!({ "type": "input_text", "text": text }))
            }
            Content::Text { text } => parts.push(json!({ "type": "output_text", "text": text })),
            Content::Image { media_type, data } => {
                parts.push(
                    json!({ "type": "input_image", "image_url": data_url(media_type, data) }),
                );
            }
            Content::ToolCall { id, name, input } => {
                items.push(json!({ "type": "function_call", "call_id": id, "name": name, "arguments": input.to_string() }));
            }
            Content::ToolResult {
                call_id, output, ..
            } => {
                items.push(
                    json!({ "type": "function_call_output", "call_id": call_id, "output": output }),
                );
            }
            Content::Reasoning { .. } => {}
        }
    }
    if !parts.is_empty() {
        let role = if user { "user" } else { "assistant" };
        // Function call outputs answer the previous assistant turn, so they come first.
        let message = json!({ "role": role, "content": parts });
        if user {
            items.push(message)
        } else {
            items.insert(0, message)
        }
    }
    items
}

#[derive(Default)]
struct ResponsesDecoder {
    tools: ToolCalls,
    done: bool,
}

impl Decoder for ResponsesDecoder {
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
            out.push(Err(LlmError::transport(
                "stream ended before response.completed",
            )));
        }
    }
}

impl ResponsesDecoder {
    fn dispatch(&mut self, kind: &str, data: &Value, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        match kind {
            "response.output_text.delta" => out.push(Ok(LlmEvent::TextDelta {
                text: str_at(data, "/delta").into(),
            })),
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                out.push(Ok(LlmEvent::ReasoningDelta {
                    text: str_at(data, "/delta").into(),
                }));
            }
            "response.output_item.added" if str_at(data, "/item/type") == "function_call" => {
                self.tools.start(
                    str_at(data, "/item/id"),
                    str_at(data, "/item/call_id"),
                    str_at(data, "/item/name"),
                );
            }
            "response.function_call_arguments.delta" => {
                if let Some(event) = self
                    .tools
                    .append(str_at(data, "/item_id"), str_at(data, "/delta"))
                {
                    out.push(Ok(event));
                }
            }
            "response.output_item.done" if str_at(data, "/item/type") == "function_call" => {
                let arguments = data.pointer("/item/arguments").and_then(Value::as_str);
                if let Some(call) = self.tools.finish(str_at(data, "/item/id"), arguments) {
                    out.push(Ok(LlmEvent::ToolCallDone(call)));
                }
            }
            "response.completed" | "response.incomplete" => self.complete(&data["response"], out),
            "response.failed" => self.fail(&data["response"]["error"], out),
            "error" => self.fail(data, out),
            _ => {}
        }
    }

    fn complete(&mut self, response: &Value, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        self.done = true;
        for call in self.tools.finish_all() {
            out.push(Ok(LlmEvent::ToolCallDone(call)));
        }
        out.push(Ok(LlmEvent::Usage(usage_from(&response["usage"]))));
        let reason = match str_at(response, "/incomplete_details/reason") {
            "max_output_tokens" => FinishReason::Length,
            "content_filter" => FinishReason::ContentFilter,
            _ if self.tools.any() => FinishReason::ToolCalls,
            _ => FinishReason::Stop,
        };
        out.push(Ok(LlmEvent::Finish { reason }));
    }

    fn fail(&mut self, error: &Value, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        self.done = true;
        let message = str_at(error, "/message");
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .or_else(|| error.get("type").and_then(Value::as_str))
            .unwrap_or_default();
        out.push(Err(LlmError::new(
            classify_stream_error(code, message),
            message,
        )));
    }
}

fn usage_from(u: &Value) -> Usage {
    let cached = u64_at(u, "/input_tokens_details/cached_tokens");
    let reasoning = u64_at(u, "/output_tokens_details/reasoning_tokens");
    Usage {
        input: u64_at(u, "/input_tokens").saturating_sub(cached),
        output: u64_at(u, "/output_tokens").saturating_sub(reasoning),
        reasoning,
        cache_read: cached,
        cache_write: 0,
    }
}
