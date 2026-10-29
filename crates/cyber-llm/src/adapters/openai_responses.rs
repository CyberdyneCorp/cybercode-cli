//! OpenAI Responses API.

use futures::future::BoxFuture;
use serde_json::{Map, Value, json};

use super::{
    Adapter, Decoder, Endpoint, EventStream, ToolCalls, data_url, decode_sse, document_name,
    ensure_resolved, parse_json,
};
use crate::error::{ErrorKind, LlmError, stream_error};
use crate::json::{deep_merge, force_no_tools, str_at, strip_credentials};
use crate::sse::SseEvent;
use crate::types::{
    Content, FinishReason, LlmEvent, LlmRequest, Message, Opaque, OpaqueKind, Reasoning, Role,
    TurnState, Usage,
};

mod reservation;

pub struct OpenAiResponsesAdapter {
    pub(super) endpoint: Endpoint,
    pub(super) response_limit: Option<usize>,
    pub(super) backend: Backend,
}

/// Who serves the Responses protocol, which decides the routing headers
/// (`provider-catalog` → Session routing headers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backend {
    /// The OpenAI API with a key: `session-id` only.
    Api,
    /// The ChatGPT subscription backend: every routing header and the Turn state.
    ChatGpt,
    /// Any other gateway: none.
    Other,
}

impl Backend {
    /// The OpenAI API only at its official URL; a custom gateway speaking Responses gets
    /// no routing headers.
    fn for_url(base_url: &str) -> Self {
        if is_official_openai(base_url) {
            Self::Api
        } else {
            Self::Other
        }
    }

    /// Whether the freeform `apply_patch` tool may be offered: OpenAI's own backends
    /// implement grammar tools, a compatible gateway may not (`builtin-tools` → Freeform
    /// apply_patch).
    fn allows_freeform_patch(self) -> bool {
        self != Self::Other
    }
}

/// Whether `base_url` is the official OpenAI API route: `https://api.openai.com/v1`, on
/// the default port, with no credentials, query or fragment.
pub(super) fn is_official_openai(base_url: &str) -> bool {
    reqwest::Url::parse(base_url).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("api.openai.com")
            && url.port().is_none_or(|port| port == 443)
            && url.username().is_empty()
            && url.password().is_none()
            && url.path().trim_end_matches('/') == "/v1"
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

const TURN_STATE: &str = "x-codex-turn-state";

impl OpenAiResponsesAdapter {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            backend: Backend::for_url(&endpoint.base_url),
            endpoint,
            response_limit: None,
        }
    }

    /// The routing headers of `request` for this backend.
    fn routing_headers(&self, request: &LlmRequest) -> Vec<(String, String)> {
        let Some(routing) = request.routing.as_ref() else {
            return Vec::new();
        };
        let mut headers = match self.backend {
            Backend::Other => return Vec::new(),
            Backend::Api | Backend::ChatGpt => vec![("session-id", routing.root.clone())],
        };
        if self.backend == Backend::ChatGpt {
            headers.push(("thread-id", routing.session.clone()));
            headers.push(("x-client-request-id", routing.session.clone()));
            headers.extend(routing.subagent.clone().map(|s| ("x-openai-subagent", s)));
            let state = routing.turn_state.as_ref().and_then(TurnState::get);
            headers.extend(state.map(|s| (TURN_STATE, s)));
        }
        headers
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect()
    }

    /// Keep the Turn's first `x-codex-turn-state`, from the ChatGPT backend only.
    fn keep_turn_state(&self, request: &LlmRequest, response: &reqwest::Response) {
        let state = request.routing.as_ref().and_then(|r| r.turn_state.as_ref());
        let value = response
            .headers()
            .get(TURN_STATE)
            .and_then(|v| v.to_str().ok());
        if let (Backend::ChatGpt, Some(state), Some(value)) = (self.backend, state, value) {
            state.offer(value);
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
            backend: self.backend,
        }))
    }

    fn stream(&self, request: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>> {
        Box::pin(async move {
            ensure_resolved(&request)?;
            let body = build_body(&request, self.backend);
            let auth = self
                .endpoint
                .api_key
                .iter()
                .map(|k| ("authorization".to_string(), format!("Bearer {k}")))
                .collect();
            let mut headers = self.routing_headers(&request);
            headers.extend(request.headers.iter().cloned());
            let resp = self
                .endpoint
                .post_stream("responses", &body, auth, &headers, self.response_limit)
                .await?;
            self.keep_turn_state(&request, &resp);
            Ok(decode_sse(
                resp,
                ResponsesDecoder::default(),
                &self.endpoint,
                self.response_limit,
            ))
        })
    }
}

pub(crate) fn build_body(request: &LlmRequest, backend: Backend) -> Value {
    let freeform = request.shaping.freeform_patch && backend.allows_freeform_patch();
    let custom = custom_calls(request, freeform);
    let input: Vec<Value> = request
        .messages
        .iter()
        .flat_map(|message| input_items(message, &custom))
        .collect();
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
            .map(|t| tool_entry(t, freeform))
            .collect();
        map.insert("tools".into(), Value::Array(tools));
        map.insert("tool_choice".into(), json!("auto"));
        if let Some(parallel) = request.shaping.parallel_tool_calls {
            map.insert("parallel_tool_calls".into(), json!(parallel));
        }
    }
    optional_fields(request, map);
    deep_merge(&mut body, &request.body);
    force_no_tools(&mut body, request.tools_disabled, json!("none"));
    if request.tools_disabled
        && let Some(map) = body.as_object_mut()
    {
        map.remove("parallel_tool_calls");
    }
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
    if let Some(reasoning) = reasoning_field(request) {
        map.insert("reasoning".into(), reasoning);
    }
    if let Some(verbosity) = request.shaping.verbosity {
        map.insert("text".into(), json!({ "verbosity": verbosity }));
    }
    if let (Some(key), true) = (&request.cache_key, request.cache) {
        map.insert("prompt_cache_key".into(), json!(key));
    }
    // With `store: false` the encrypted reasoning is the only way to keep it across calls;
    // whether it goes back is the replay policy's choice, made later.
    if request.shaping.reasoning {
        map.insert("include".into(), json!(["reasoning.encrypted_content"]));
    }
}

/// `reasoning`: a summary only where the model accepts one, and reasoning off as the
/// `none` effort only where the model lists it; otherwise the field is left out.
fn reasoning_field(request: &LlmRequest) -> Option<Value> {
    match &request.reasoning {
        Some(Reasoning::Effort(effort)) if request.shaping.reasoning_summary => {
            Some(json!({ "effort": effort, "summary": "auto" }))
        }
        Some(Reasoning::Effort(effort)) => Some(json!({ "effort": effort })),
        Some(Reasoning::Off) if request.shaping.effort_none => Some(json!({ "effort": "none" })),
        _ => None,
    }
}

/// The tool the freeform patch replaces (`builtin-tools` → Freeform apply_patch).
const PATCH_TOOL: &str = "apply_patch";
/// Our grammar of the patch envelope; the model's raw input must match it.
const PATCH_GRAMMAR: &str = include_str!("openai_responses/apply_patch.lark");

/// A function tool, or the patch tool as a grammar-constrained custom tool.
fn tool_entry(tool: &crate::types::ToolSpec, freeform_patch: bool) -> Value {
    if freeform_patch && tool.name == PATCH_TOOL {
        return json!({
            "type": "custom",
            "name": tool.name,
            "description": tool.description,
            "format": { "type": "grammar", "syntax": "lark", "definition": PATCH_GRAMMAR },
        });
    }
    json!({ "type": "function", "name": tool.name, "description": tool.description, "parameters": tool.input_schema, "strict": false })
}

/// IDs of the patch calls replayed as custom tool calls: with the freeform tool offered,
/// every earlier `apply_patch` call and its result take the custom item shapes.
fn custom_calls(request: &LlmRequest, freeform: bool) -> std::collections::HashSet<&str> {
    if !freeform {
        return Default::default();
    }
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|content| match content {
            Content::ToolCall { id, name, .. } if name == PATCH_TOOL => Some(id.as_str()),
            _ => None,
        })
        .collect()
}

fn input_items(message: &Message, custom: &std::collections::HashSet<&str>) -> Vec<Value> {
    match message.role {
        Role::User => user_items(message, custom),
        Role::Assistant => assistant_items(message, custom),
    }
}

/// Function call outputs answer the previous assistant turn, so they come first.
fn user_items(message: &Message, custom: &std::collections::HashSet<&str>) -> Vec<Value> {
    let mut items = Vec::new();
    let mut parts = Vec::new();
    for content in &message.content {
        match content {
            Content::ToolResult {
                call_id, output, ..
            } => {
                let kind = if custom.contains(call_id.as_str()) {
                    "custom_tool_call_output"
                } else {
                    "function_call_output"
                };
                items.push(json!({ "type": kind, "call_id": call_id, "output": output }));
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
fn assistant_items(message: &Message, custom: &std::collections::HashSet<&str>) -> Vec<Value> {
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
            } if custom.contains(id.as_str()) => Some(
                json!({ "type": "custom_tool_call", "call_id": id, "name": name, "input": str_at(input, "/patch") }),
            ),
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
            "response.output_item.added"
                if matches!(
                    str_at(data, "/item/type"),
                    "function_call" | "custom_tool_call"
                ) =>
            {
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
            "response.output_item.done" if str_at(data, "/item/type") == "custom_tool_call" => {
                out.extend(self.custom_call(&data["item"]).map(Ok));
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

    /// A finished custom tool call: its raw text becomes the `patch` input, so it runs
    /// as the JSON tool would (`builtin-tools` → Freeform apply_patch).
    fn custom_call(&mut self, item: &Value) -> Option<LlmEvent> {
        let key = str_at(item, "/id");
        self.tools
            .start(key, str_at(item, "/call_id"), str_at(item, "/name"));
        let input = json!({ "patch": str_at(item, "/input") }).to_string();
        let call = self.tools.finish(key, Some(&input))?;
        Some(LlmEvent::ToolCallDone(call))
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
        LlmEvent::ReasoningItem { text, opaque }
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
        out.push(finish(response, self.tools.any()));
    }

    fn fail(&mut self, error: &Value, out: &mut Vec<Result<LlmEvent, LlmError>>) {
        self.done = true;
        out.push(Err(stream_error(error)));
    }
}

/// How a completed or incomplete response finishes (`provider-catalog` →
/// Provider-specific error codes): an unknown incomplete reason fails the step rather
/// than passing for a normal stop.
fn finish(response: &Value, tool_calls: bool) -> Result<LlmEvent, LlmError> {
    let reason = match str_at(response, "/incomplete_details/reason") {
        "max_output_tokens" => FinishReason::Length,
        "content_filter" => FinishReason::ContentFilter,
        "interrupted" => FinishReason::Stop,
        "" if tool_calls => FinishReason::ToolCalls,
        "" => FinishReason::Stop,
        other => {
            return Err(LlmError::new(
                ErrorKind::ProviderInternal,
                format!("response incomplete: {other}"),
            ));
        }
    };
    Ok(LlmEvent::Finish { reason })
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
            build_body(&request, Backend::Api)["input"][0]["content"][0],
            json!({ "type": "input_file", "filename": "spec.pdf", "file_data": "data:application/pdf;base64,JVBERi0=" })
        );
    }

    /// `provider-catalog` → Provider-specific error codes: the `response.incomplete`
    /// reasons, and an unknown one failing the step instead of a silent stop.
    #[test]
    fn incomplete_reasons_map_to_finishes_or_a_failure() {
        let finish_of = |reason: &str, tools: bool| {
            let response = json!({"incomplete_details": {"reason": reason}});
            match finish(&response, tools) {
                Ok(LlmEvent::Finish { reason }) => Ok(reason),
                Ok(other) => panic!("{other:?}"),
                Err(e) => Err((e.kind, e.message)),
            }
        };
        assert_eq!(finish_of("interrupted", true), Ok(FinishReason::Stop));
        assert_eq!(
            finish_of("max_output_tokens", false),
            Ok(FinishReason::Length)
        );
        assert_eq!(
            finish_of("content_filter", false),
            Ok(FinishReason::ContentFilter)
        );
        assert_eq!(finish_of("", true), Ok(FinishReason::ToolCalls));
        assert_eq!(finish_of("", false), Ok(FinishReason::Stop));
        assert_eq!(
            finish_of("server_shutdown", false),
            Err((
                ErrorKind::ProviderInternal,
                "response incomplete: server_shutdown".into()
            ))
        );
    }

    fn patch_tool() -> crate::types::ToolSpec {
        crate::types::ToolSpec {
            name: "apply_patch".into(),
            description: "Apply a patch".into(),
            input_schema: json!({"type": "object", "properties": {"patch": {"type": "string"}}}),
        }
    }

    /// `builtin-tools` → Freeform apply_patch: with the profile's freeform patch, the
    /// patch tool is a custom tool carrying our grammar, and its earlier calls and
    /// results replay as custom items; other tools stay functions.
    #[test]
    fn freeform_patch_offers_a_grammar_tool_and_replays_custom_items() {
        let read = crate::types::ToolSpec {
            name: "read".into(),
            description: "Read".into(),
            input_schema: json!({"type": "object"}),
        };
        let patch = "*** Begin Patch\n*** Delete File: a.txt\n*** End Patch\n";
        let mut request = LlmRequest {
            tools: vec![patch_tool(), read],
            messages: vec![
                Message::user_text("remove a.txt"),
                Message {
                    role: Role::Assistant,
                    content: vec![Content::ToolCall {
                        id: "call_1".into(),
                        name: "apply_patch".into(),
                        input: json!({ "patch": patch }),
                        opaque: None,
                    }],
                },
                Message {
                    role: Role::User,
                    content: vec![Content::ToolResult {
                        call_id: "call_1".into(),
                        output: "D a.txt".into(),
                        is_error: false,
                    }],
                },
            ],
            ..LlmRequest::default()
        };
        request.shaping.freeform_patch = true;
        let body = build_body(&request, Backend::Api);
        let tool = &body["tools"][0];
        assert_eq!(tool["type"], "custom");
        assert_eq!(tool["name"], "apply_patch");
        assert_eq!(tool["format"]["type"], "grammar");
        assert_eq!(tool["format"]["syntax"], "lark");
        assert_eq!(tool["format"]["definition"], PATCH_GRAMMAR);
        assert_eq!(body["tools"][1]["type"], "function");
        assert_eq!(
            body["input"][1],
            json!({"type": "custom_tool_call", "call_id": "call_1", "name": "apply_patch", "input": patch})
        );
        assert_eq!(
            body["input"][2],
            json!({"type": "custom_tool_call_output", "call_id": "call_1", "output": "D a.txt"})
        );
        // A compatible gateway may not implement grammar tools, and Copilot's Responses
        // models are served through one: the patch tool stays a function there.
        for (freeform, backend) in [(false, Backend::Api), (true, Backend::Other)] {
            request.shaping.freeform_patch = freeform;
            let body = build_body(&request, backend);
            assert_eq!(body["tools"][0]["type"], "function");
            assert_eq!(body["input"][1]["type"], "function_call");
            assert_eq!(body["input"][2]["type"], "function_call_output");
        }
    }

    /// A streamed custom tool call decodes to `apply_patch { patch }` and finishes the
    /// response as a tool call.
    #[test]
    fn custom_tool_calls_decode_to_the_patch_input() {
        let patch = "*** Begin Patch\n*** Add File: b.txt\n+hi\n*** End Patch";
        let item = json!({"type": "custom_tool_call", "id": "ctc_1", "call_id": "call_9", "name": "apply_patch"});
        let mut done = item.clone();
        done["input"] = json!(patch);
        let mut decoder = ResponsesDecoder::default();
        let mut out = Vec::new();
        decoder.dispatch(
            "response.output_item.added",
            &json!({ "item": item }),
            &mut out,
        );
        decoder.dispatch(
            "response.custom_tool_call_input.delta",
            &json!({"item_id": "ctc_1", "delta": "*** Begin"}),
            &mut out,
        );
        decoder.dispatch(
            "response.output_item.done",
            &json!({ "item": done }),
            &mut out,
        );
        decoder.dispatch("response.completed", &json!({"response": {}}), &mut out);
        let events: Vec<LlmEvent> = out.into_iter().map(Result::unwrap).collect();
        let call = events
            .iter()
            .find_map(|event| match event {
                LlmEvent::ToolCallDone(call) => Some(call),
                _ => None,
            })
            .expect("a finished call");
        assert_eq!(
            (call.id.as_str(), call.name.as_str()),
            ("call_9", "apply_patch")
        );
        assert_eq!(call.input, Some(json!({ "patch": patch })));
        assert!(matches!(
            events.last(),
            Some(LlmEvent::Finish {
                reason: FinishReason::ToolCalls
            })
        ));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, LlmEvent::ToolCallDone(_)))
                .count(),
            1
        );
    }

    /// Regression: every Responses endpoint was treated as the OpenAI API, so a custom
    /// gateway received `session-id`.
    #[test]
    fn only_the_default_openai_url_is_the_api_backend() {
        for (url, backend) in [
            ("https://api.openai.com/v1", Backend::Api),
            ("https://api.openai.com/v1/", Backend::Api),
            ("https://api.openai.com:443/v1", Backend::Api),
            ("https://api.openai.com/v1?x", Backend::Other),
            ("http://api.openai.com/v1", Backend::Other),
            ("https://api.openai.com:8443/v1", Backend::Other),
            ("https://gateway.corp.example/v1", Backend::Other),
            ("http://127.0.0.1:4000/v1", Backend::Other),
        ] {
            let adapter = OpenAiResponsesAdapter::new(Endpoint::new(url, None));
            assert_eq!(adapter.backend, backend, "{url}");
        }
    }

    /// `provider-catalog` → Session routing headers, per backend.
    #[test]
    fn routing_headers_follow_the_backend() {
        let state = TurnState::default();
        state.offer("abc");
        let request = LlmRequest {
            routing: Some(crate::types::Routing {
                root: "ses_root".into(),
                session: "ses_child".into(),
                subagent: Some("explore".into()),
                turn_state: Some(state),
            }),
            ..LlmRequest::default()
        };
        let headers = |backend| {
            let mut adapter =
                OpenAiResponsesAdapter::new(Endpoint::new("https://api.openai.com/v1", None));
            adapter.backend = backend;
            adapter.routing_headers(&request)
        };
        let pairs = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter()
                .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
                .collect()
        };
        assert_eq!(
            headers(Backend::ChatGpt),
            pairs(&[
                ("session-id", "ses_root"),
                ("thread-id", "ses_child"),
                ("x-client-request-id", "ses_child"),
                ("x-openai-subagent", "explore"),
                ("x-codex-turn-state", "abc"),
            ])
        );
        assert_eq!(headers(Backend::Api), pairs(&[("session-id", "ses_root")]));
        assert!(headers(Backend::Other).is_empty());
        let unrouted =
            OpenAiResponsesAdapter::new(Endpoint::new("https://api.openai.com/v1", None));
        assert!(unrouted.routing_headers(&LlmRequest::default()).is_empty());
    }
}
