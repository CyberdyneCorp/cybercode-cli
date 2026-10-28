//! OpenAI Responses API.

use futures::future::BoxFuture;
use serde_json::{Map, Value, json};

use super::{
    Adapter, Decoder, Endpoint, EventStream, ToolCalls, data_url, decode_sse, document_name,
    ensure_resolved, parse_json,
};
use crate::catalog::profile::ReasoningCarrier;
use crate::error::{LlmError, classify_stream_error};
use crate::json::{deep_merge, force_no_tools, str_at, strip_credentials};
use crate::sse::SseEvent;
use crate::types::{
    Content, FinishReason, LlmEvent, LlmRequest, Message, Opaque, OpaqueKind, Reasoning, Role,
    Usage,
};

mod reservation;

pub struct OpenAiResponsesAdapter {
    pub(super) endpoint: Endpoint,
    pub(super) response_limit: Option<usize>,
}

impl OpenAiResponsesAdapter {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            response_limit: None,
        }
    }
}

impl Adapter for OpenAiResponsesAdapter {
    fn prepare_reserved(
        &self,
        request: LlmRequest,
        context_limit: u64,
    ) -> Result<crate::reservations::PreparedReservation, crate::reservations::BoundUnavailable>
    {
        reservation::prepare(&self.endpoint, request, context_limit)
    }
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
                    "responses",
                    &body,
                    auth,
                    &request.headers,
                    self.response_limit,
                )
                .await?;
            Ok(decode_sse(
                resp,
                ResponsesDecoder::default(),
                &self.endpoint,
                self.response_limit,
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
    // With `store: false` the encrypted reasoning is the only way to keep it across calls.
    if request.reasoning_carrier == ReasoningCarrier::Responses {
        map.insert("include".into(), json!(["reasoning.encrypted_content"]));
    }
}

fn input_items(message: &Message) -> Vec<Value> {
    match message.role {
        Role::User => user_items(message),
        Role::Assistant => assistant_items(message),
    }
}

/// Function call outputs answer the previous assistant turn, so they come first.
fn user_items(message: &Message) -> Vec<Value> {
    let mut items = Vec::new();
    let mut parts = Vec::new();
    for content in &message.content {
        match content {
            Content::ToolResult {
                call_id, output, ..
            } => {
                items.push(
                    json!({ "type": "function_call_output", "call_id": call_id, "output": output }),
                );
            }
            Content::Text { text } => parts.push(json!({ "type": "input_text", "text": text })),
            other => parts.extend(media_part(other)),
        }
    }
    if !parts.is_empty() {
        items.push(json!({ "role": "user", "content": parts }));
    }
    items
}

/// An answer's items in the order the model produced them: reasoning, its message, and
/// the function calls with the reasoning that came between them
/// (`provider-catalog` → Responses request shaping).
fn assistant_items(message: &Message) -> Vec<Value> {
    let mut items = Vec::new();
    let mut parts = Vec::new();
    for content in &message.content {
        let item = match content {
            Content::Text { text } => {
                parts.push(json!({ "type": "output_text", "text": text }));
                continue;
            }
            Content::ToolCall {
                id, name, input, ..
            } => Some(
                json!({ "type": "function_call", "call_id": id, "name": name, "arguments": input.to_string() }),
            ),
            Content::Reasoning { text, opaque } => reasoning_item(text, opaque.as_ref()),
            other => {
                parts.extend(media_part(other));
                continue;
            }
        };
        flush_message(&mut parts, &mut items);
        items.extend(item);
    }
    flush_message(&mut parts, &mut items);
    items
}

fn flush_message(parts: &mut Vec<Value>, items: &mut Vec<Value>) {
    if !parts.is_empty() {
        items.push(json!({ "role": "assistant", "content": std::mem::take(parts) }));
    }
}

fn media_part(content: &Content) -> Option<Value> {
    match content {
        Content::Image { media_type, data } => {
            Some(json!({ "type": "input_image", "image_url": data_url(media_type, data) }))
        }
        Content::Document {
            media_type,
            data,
            name,
        } => Some(
            json!({ "type": "input_file", "filename": document_name(name), "file_data": data_url(media_type, data) }),
        ),
        // References are resolved before a request (`ensure_resolved`).
        _ => None,
    }
}

/// The prefix of the reasoning item IDs the Responses API issues.
const REASONING_ID_PREFIX: &str = "rs_";

/// A reasoning item with its encrypted content. Reasoning without it cannot be replayed
/// statelessly and is left out. An ID the API did not issue (a forked or imported item)
/// is left out too, since the API rejects it.
fn reasoning_item(text: &str, opaque: Option<&Opaque>) -> Option<Value> {
    let opaque = opaque.filter(|opaque| opaque.kind == OpaqueKind::EncryptedReasoning)?;
    let summary: Vec<Value> = (!text.is_empty())
        .then(|| json!({ "type": "summary_text", "text": text }))
        .into_iter()
        .collect();
    let mut item =
        json!({ "type": "reasoning", "summary": summary, "encrypted_content": opaque.data });
    if let Some(id) = opaque
        .id
        .as_deref()
        .filter(|id| id.starts_with(REASONING_ID_PREFIX))
    {
        item["id"] = json!(id);
    }
    Some(item)
}

#[derive(Default)]
struct ResponsesDecoder {
    tools: ToolCalls,
    /// Reasoning items decoded so far, for [`LlmEvent::ReasoningItem`] indexes.
    reasoning_items: u32,
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
            "response.output_item.done" if str_at(data, "/item/type") == "reasoning" => {
                out.push(Ok(self.reasoning_item(&data["item"])));
            }
            "response.completed" | "response.incomplete" => self.complete(&data["response"], out),
            "response.failed" => self.fail(&data["response"]["error"], out),
            "error" => self.fail(data, out),
            _ => {}
        }
    }

    /// A finished reasoning item: its summary parts as the item text, and its encrypted
    /// content with the item ID as the state to replay.
    fn reasoning_item(&mut self, item: &Value) -> LlmEvent {
        let text = item["summary"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let opaque = item["encrypted_content"].as_str().map(|data| {
            let mut opaque = Opaque::new(OpaqueKind::EncryptedReasoning, data);
            opaque.id = item["id"].as_str().map(str::to_owned);
            opaque
        });
        self.reasoning_items += 1;
        LlmEvent::ReasoningItem {
            index: self.reasoning_items - 1,
            text,
            opaque,
        }
    }

    fn complete(&mut self, response: &Value, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        self.done = true;
        for call in self.tools.finish_all() {
            out.push(Ok(LlmEvent::ToolCallDone(call)));
        }
        // Missing or malformed billing evidence is unknown, never a free receipt.
        if let Some(usage) = usage_from(&response["usage"]) {
            out.push(Ok(LlmEvent::Usage(usage)));
        }
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

fn usage_from(u: &Value) -> Option<Usage> {
    let input = u.get("input_tokens")?.as_u64()?;
    let output = u.get("output_tokens")?.as_u64()?;
    let detail = |group: &str, key: &str| match u.get(group) {
        None => Some(0),
        Some(value) => match value.as_object()?.get(key) {
            None => Some(0),
            Some(value) => value.as_u64(),
        },
    };
    let cached = detail("input_tokens_details", "cached_tokens")?;
    let reasoning = detail("output_tokens_details", "reasoning_tokens")?;
    Some(Usage {
        input: input.checked_sub(cached)?,
        output: output.checked_sub(reasoning)?,
        reasoning,
        cache_read: cached,
        cache_write: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_missing_or_malformed_usage_never_emits_a_zero_receipt() {
        for usage in [
            Value::Null,
            json!({}),
            json!({"input_tokens":10}),
            json!({"input_tokens":"10","output_tokens":2}),
            json!({"input_tokens":10,"output_tokens":2,"input_tokens_details":{"cached_tokens":11}}),
            json!({"input_tokens":10,"output_tokens":2,"output_tokens_details":{"reasoning_tokens":"2"}}),
        ] {
            let mut decoder = ResponsesDecoder::default();
            let mut out = Vec::new();
            decoder.complete(&json!({"usage":usage}), &mut out);
            assert!(!out.iter().any(|e| matches!(e, Ok(LlmEvent::Usage(_)))));
            assert!(out.iter().any(|e| matches!(e, Ok(LlmEvent::Finish { .. }))));
        }
        assert_eq!(
            usage_from(&json!({"input_tokens":10,"output_tokens":2}))
                .unwrap()
                .input,
            10
        );
    }
    #[test]
    fn documents_become_input_files_with_a_data_url() {
        let request = LlmRequest {
            messages: vec![Message {
                role: Role::User,
                content: vec![Content::Document {
                    media_type: "application/pdf".into(),
                    data: "JVBERi0=".into(),
                    name: Some("spec.pdf".into()),
                }],
            }],
            ..LlmRequest::default()
        };
        assert_eq!(
            build_body(&request)["input"][0]["content"][0],
            json!({ "type": "input_file", "filename": "spec.pdf", "file_data": "data:application/pdf;base64,JVBERi0=" })
        );
    }
}
