//! Wire-level tests of each adapter against a mock server
//! (`provider-catalog` → Native provider adapters, Error classification, Retry policy).

use std::time::Duration;

use cyber_llm::adapters::{ApiKind, Endpoint, ScriptStep, ScriptedAdapter, adapter};
use cyber_llm::catalog::profile::ReasoningCarrier;
use cyber_llm::{
    Content, ErrorKind, FinishReason, LlmEvent, LlmRequest, Message, Opaque, OpaqueKind, Reasoning,
    RetryPolicy, Role, ToolSpec, Usage, collect, open_with_retry,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| {
            if name.is_empty() {
                format!("data: {data}\n\n")
            } else {
                format!("event: {name}\ndata: {data}\n\n")
            }
        })
        .collect()
}

fn ok_stream(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

fn request() -> LlmRequest {
    LlmRequest {
        model: "m-1".into(),
        system: vec!["You are terse.".into()],
        messages: vec![
            Message::user_text("what time is it?"),
            Message {
                role: Role::Assistant,
                content: vec![Content::ToolCall {
                    id: "call_0".into(),
                    name: "clock".into(),
                    input: json!({"tz": "UTC"}),
                    opaque: None,
                }],
            },
            Message {
                role: Role::User,
                content: vec![Content::ToolResult {
                    call_id: "call_0".into(),
                    output: "12:00".into(),
                    is_error: false,
                }],
            },
        ],
        tools: vec![ToolSpec {
            name: "clock".into(),
            description: "Current time".into(),
            input_schema: json!({"type": "object", "properties": {"tz": {"type": "string"}}}),
        }],
        cache_key: Some("ses_1".into()),
        cache: true,
        body: json!({ "api_key": "leak", "extra": 1 }),
        ..LlmRequest::default()
    }
}

async fn run(
    kind: ApiKind,
    server: &MockServer,
    base: &str,
    req: LlmRequest,
) -> (cyber_llm::TurnOutput, Value) {
    let a = adapter(
        kind,
        Endpoint::new(
            format!("{}{base}", server.uri()),
            Some("sk-secret-key".into()),
        ),
    );
    let stream = a.stream(req).await.unwrap();
    let out = collect(stream, |_| {}).await.unwrap();
    let body = server.received_requests().await.unwrap()[0]
        .body_json::<Value>()
        .unwrap();
    (out, body)
}

#[tokio::test]
async fn openai_chat_streams_text_tool_calls_and_usage() {
    let server = MockServer::start().await;
    let body = sse(&[
        ("", json!({"choices": [{"delta": {"content": "Checking"}}]})),
        (
            "",
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": "clock", "arguments": "{\"tz\":"}}]}}]}),
        ),
        (
            "",
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"UTC\"}"}}]}, "finish_reason": "tool_calls"}]}),
        ),
        (
            "",
            json!({"choices": [], "usage": {"prompt_tokens": 100, "completion_tokens": 20,
            "prompt_tokens_details": {"cached_tokens": 60}, "completion_tokens_details": {"reasoning_tokens": 5}}}),
        ),
    ]) + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;

    let (out, sent) = run(ApiKind::OpenaiCompatible, &server, "/v1", request()).await;
    assert_eq!(out.text, "Checking");
    assert_eq!(out.tool_calls.len(), 1);
    assert_eq!(out.tool_calls[0].input, Some(json!({"tz": "UTC"})));
    assert_eq!(
        out.usage,
        Usage {
            input: 40,
            output: 15,
            reasoning: 5,
            cache_read: 60,
            cache_write: 0
        }
    );
    assert_eq!(out.finish, Some(FinishReason::ToolCalls));

    assert_eq!(
        sent["messages"][0],
        json!({"role": "system", "content": "You are terse."})
    );
    assert_eq!(
        sent["messages"][1],
        json!({"role": "user", "content": "what time is it?"})
    );
    assert_eq!(
        sent["messages"][2]["tool_calls"][0]["function"]["arguments"],
        "{\"tz\":\"UTC\"}"
    );
    assert_eq!(
        sent["messages"][3],
        json!({"role": "tool", "tool_call_id": "call_0", "content": "12:00"})
    );
    assert_eq!(sent["tools"][0]["function"]["name"], "clock");
    assert_eq!(sent["stream_options"]["include_usage"], true);
    assert_eq!(sent["extra"], 1);
    assert!(
        sent.get("api_key").is_none(),
        "credentials never travel in the body"
    );
}

#[tokio::test]
async fn openai_responses_streams_reasoning_and_function_calls() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "response.reasoning_summary_text.delta",
            json!({"delta": "thinking"}),
        ),
        ("response.output_text.delta", json!({"delta": "Hi"})),
        (
            "response.output_item.added",
            json!({"item": {"type": "function_call", "id": "fc_1", "call_id": "call_9", "name": "clock"}}),
        ),
        (
            "response.function_call_arguments.delta",
            json!({"item_id": "fc_1", "delta": "{\"tz\""}),
        ),
        (
            "response.function_call_arguments.delta",
            json!({"item_id": "fc_1", "delta": ":\"UTC\"}"}),
        ),
        (
            "response.output_item.done",
            json!({"item": {"type": "function_call", "id": "fc_1", "call_id": "call_9", "name": "clock", "arguments": "{\"tz\":\"UTC\"}"}}),
        ),
        (
            "response.completed",
            json!({"response": {"usage": {"input_tokens": 50, "output_tokens": 30,
            "input_tokens_details": {"cached_tokens": 10}, "output_tokens_details": {"reasoning_tokens": 12}}}}),
        ),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;

    let mut req = request();
    req.reasoning = Some(Reasoning::Effort("high".into()));
    let (out, sent) = run(ApiKind::OpenaiResponses, &server, "/v1", req).await;
    assert_eq!(out.reasoning, "thinking");
    assert_eq!(out.text, "Hi");
    assert_eq!(out.tool_calls[0].id, "call_9");
    assert_eq!(out.tool_calls[0].input, Some(json!({"tz": "UTC"})));
    assert_eq!(
        out.usage,
        Usage {
            input: 40,
            output: 18,
            reasoning: 12,
            cache_read: 10,
            cache_write: 0
        }
    );
    assert_eq!(out.finish, Some(FinishReason::ToolCalls));

    assert_eq!(sent["instructions"], "You are terse.");
    assert_eq!(
        sent["reasoning"],
        json!({"effort": "high", "summary": "auto"})
    );
    assert_eq!(sent["prompt_cache_key"], "ses_1");
    assert_eq!(sent["store"], false);
    let input = sent["input"].as_array().unwrap();
    assert_eq!(
        input[1],
        json!({"type": "function_call", "call_id": "call_0", "name": "clock", "arguments": "{\"tz\":\"UTC\"}"})
    );
    assert_eq!(
        input[2],
        json!({"type": "function_call_output", "call_id": "call_0", "output": "12:00"})
    );
}

#[tokio::test]
async fn anthropic_streams_thinking_tools_and_cache_usage() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "message_start",
            json!({"message": {"usage": {"input_tokens": 12, "output_tokens": 1, "cache_read_input_tokens": 900, "cache_creation_input_tokens": 100}}}),
        ),
        (
            "content_block_start",
            json!({"index": 0, "content_block": {"type": "thinking"}}),
        ),
        (
            "content_block_delta",
            json!({"index": 0, "delta": {"type": "thinking_delta", "thinking": "hmm"}}),
        ),
        (
            "content_block_delta",
            json!({"index": 0, "delta": {"type": "signature_delta", "signature": "sig=="}}),
        ),
        ("content_block_stop", json!({"index": 0})),
        (
            "content_block_start",
            json!({"index": 1, "content_block": {"type": "text"}}),
        ),
        (
            "content_block_delta",
            json!({"index": 1, "delta": {"type": "text_delta", "text": "Let me check."}}),
        ),
        ("content_block_stop", json!({"index": 1})),
        (
            "content_block_start",
            json!({"index": 2, "content_block": {"type": "tool_use", "id": "toolu_1", "name": "clock"}}),
        ),
        (
            "content_block_delta",
            json!({"index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"tz\": \"UTC\"}"}}),
        ),
        ("content_block_stop", json!({"index": 2})),
        ("ping", json!({})),
        (
            "message_delta",
            json!({"delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 42}}),
        ),
        ("message_stop", json!({})),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;

    let mut req = request();
    req.reasoning = Some(Reasoning::BudgetTokens(4096));
    req.temperature = Some(0.3);
    let (out, sent) = run(ApiKind::Anthropic, &server, "/v1", req).await;
    assert_eq!(out.reasoning, "hmm");
    assert_eq!(out.reasoning_signature.as_deref(), Some("sig=="));
    assert_eq!(out.text, "Let me check.");
    assert_eq!(out.tool_calls[0].id, "toolu_1");
    assert_eq!(
        out.usage,
        Usage {
            input: 12,
            output: 42,
            reasoning: 0,
            cache_read: 900,
            cache_write: 100
        }
    );
    assert_eq!(out.finish, Some(FinishReason::ToolCalls));

    assert_eq!(
        sent["system"][0]["cache_control"],
        json!({"type": "ephemeral"})
    );
    assert_eq!(
        sent["tools"][0]["cache_control"],
        json!({"type": "ephemeral"})
    );
    // Regression: only system and tools were cached, so long histories never hit the cache.
    let last = sent["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(
        last["content"].as_array().unwrap().last().unwrap()["cache_control"],
        json!({"type": "ephemeral"})
    );
    let breakpoints = sent.to_string().matches("cache_control").count();
    assert!(
        breakpoints <= 4,
        "Anthropic allows at most four breakpoints: {breakpoints}"
    );
    assert_eq!(
        sent["thinking"],
        json!({"type": "enabled", "budget_tokens": 4096})
    );
    assert!(
        sent.get("temperature").is_none(),
        "temperature is dropped with thinking"
    );
    assert!(sent["max_tokens"].as_u64().unwrap() > 4096);
    assert_eq!(sent["messages"][1]["content"][0]["type"], "tool_use");
    let mut result = sent["messages"][2]["content"][0].clone();
    result.as_object_mut().unwrap().remove("cache_control");
    assert_eq!(
        result,
        json!({"type": "tool_result", "tool_use_id": "call_0", "content": "12:00", "is_error": false})
    );
    let headers = &server.received_requests().await.unwrap()[0].headers;
    assert_eq!(headers.get("x-api-key").unwrap(), "sk-secret-key");
    assert_eq!(headers.get("anthropic-version").unwrap(), "2023-06-01");
}

fn block_start(index: u32, block: Value) -> (&'static str, Value) {
    (
        "content_block_start",
        json!({"index": index, "content_block": block}),
    )
}

fn block_delta(index: u32, delta: Value) -> (&'static str, Value) {
    (
        "content_block_delta",
        json!({"index": index, "delta": delta}),
    )
}

/// Regression (`session-runtime` → Reasoning items): every signed thinking block and every
/// `redacted_thinking` block is decoded as its own item, in stream order between the calls.
#[tokio::test]
async fn anthropic_decodes_interleaved_and_redacted_thinking_in_order() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "message_start",
            json!({"message": {"usage": {"input_tokens": 1, "output_tokens": 1}}}),
        ),
        block_start(0, json!({"type": "thinking"})),
        block_delta(0, json!({"type": "thinking_delta", "thinking": "A"})),
        block_delta(0, json!({"type": "signature_delta", "signature": "sigA"})),
        ("content_block_stop", json!({"index": 0})),
        block_start(1, json!({"type": "tool_use", "id": "c1", "name": "clock"})),
        block_delta(1, json!({"type": "input_json_delta", "partial_json": "{}"})),
        ("content_block_stop", json!({"index": 1})),
        block_start(2, json!({"type": "redacted_thinking", "data": "blob"})),
        ("content_block_stop", json!({"index": 2})),
        block_start(3, json!({"type": "tool_use", "id": "c2", "name": "clock"})),
        block_delta(3, json!({"type": "input_json_delta", "partial_json": "{}"})),
        ("content_block_stop", json!({"index": 3})),
        (
            "message_delta",
            json!({"delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 9}}),
        ),
        ("message_stop", json!({})),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;
    let a = adapter(
        ApiKind::Anthropic,
        Endpoint::new(format!("{}/v1", server.uri()), Some("k".into())),
    );
    let mut seen = Vec::new();
    let stream = a.stream(request()).await.unwrap();
    collect(stream, |event| match event {
        LlmEvent::ReasoningSignature { signature } => seen.push(format!("sig:{signature}")),
        LlmEvent::ReasoningItem { index, opaque, .. } => {
            let opaque = opaque.as_ref().unwrap();
            assert_eq!(opaque.kind, OpaqueKind::Redacted);
            seen.push(format!("redacted{index}:{}", opaque.data));
        }
        LlmEvent::ToolCallDone(call) => seen.push(format!("call:{}", call.id)),
        _ => {}
    })
    .await
    .unwrap();
    assert_eq!(seen, ["sig:sigA", "call:c1", "redacted1:blob", "call:c2"]);
}

/// Regression (`session-runtime` → Interleaved thinking keeps every signed block): thinking
/// was hoisted ahead of every `tool_use`, so signed blocks went out in an order the model
/// never produced. Only tool results lead a message; thinking keeps its place.
#[tokio::test]
async fn anthropic_replays_thinking_blocks_in_block_order() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "message_start",
            json!({"message": {"usage": {"input_tokens": 1, "output_tokens": 1}}}),
        ),
        (
            "message_delta",
            json!({"delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
        ),
        ("message_stop", json!({})),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;
    let call = |id: &str| Content::ToolCall {
        id: id.into(),
        name: "clock".into(),
        input: json!({}),
        opaque: None,
    };
    let thinking = |text: &str, opaque: Opaque| Content::Reasoning {
        text: text.into(),
        opaque: Some(opaque),
    };
    let mut req = request();
    req.reasoning = Some(Reasoning::BudgetTokens(4096));
    req.messages = vec![
        Message::user_text("time?"),
        Message {
            role: Role::Assistant,
            content: vec![
                thinking("A", Opaque::new(OpaqueKind::Signature, "sigA")),
                Content::Text {
                    text: "Checking.".into(),
                },
                call("c1"),
                thinking("", Opaque::new(OpaqueKind::Redacted, "blob")),
                thinking("B", Opaque::new(OpaqueKind::Signature, "sigB")),
                call("c2"),
                // Its state was dropped (over 1 MiB): it replays as text.
                Content::Reasoning {
                    text: "C".into(),
                    opaque: None,
                },
            ],
        },
    ];
    let (_, sent) = run(ApiKind::Anthropic, &server, "/v1", req).await;
    let mut blocks = sent["messages"][1]["content"].as_array().unwrap().clone();
    for block in &mut blocks {
        block.as_object_mut().unwrap().remove("cache_control");
    }
    assert_eq!(
        blocks,
        [
            json!({"type": "thinking", "thinking": "A", "signature": "sigA"}),
            json!({"type": "text", "text": "Checking."}),
            json!({"type": "tool_use", "id": "c1", "name": "clock", "input": {}}),
            json!({"type": "redacted_thinking", "data": "blob"}),
            json!({"type": "thinking", "thinking": "B", "signature": "sigB"}),
            json!({"type": "tool_use", "id": "c2", "name": "clock", "input": {}}),
            json!({"type": "text", "text": "C"}),
        ]
    );
}

/// Regression: effort-only models (Sonnet 5.5) reject `thinking.type: enabled`; an effort
/// level must be sent as adaptive thinking with `output_config.effort`.
#[tokio::test]
async fn anthropic_effort_uses_adaptive_thinking() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "message_start",
            json!({"message": {"usage": {"input_tokens": 1, "output_tokens": 1}}}),
        ),
        (
            "message_delta",
            json!({"delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
        ),
        ("message_stop", json!({})),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;

    let mut req = request();
    req.reasoning = Some(Reasoning::Effort("medium".into()));
    req.temperature = Some(0.3);
    let (_, sent) = run(ApiKind::Anthropic, &server, "/v1", req).await;
    assert_eq!(sent["thinking"], json!({"type": "adaptive"}));
    assert_eq!(sent["output_config"], json!({"effort": "medium"}));
    assert!(sent.get("temperature").is_none());
    assert!(sent["max_tokens"].as_u64().unwrap() > 16_384);
}

#[tokio::test]
async fn http_errors_are_classified_and_redacted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": {"message": "invalid key sk-secret-key"}})),
        )
        .mount(&server)
        .await;
    let a = adapter(
        ApiKind::OpenaiCompatible,
        Endpoint::new(server.uri(), Some("sk-secret-key".into())),
    );
    let err = a.stream(request()).await.err().unwrap();
    assert_eq!(err.kind, ErrorKind::Authentication);
    assert_eq!(err.status, Some(401));
    assert_eq!(err.message, "invalid key ***");
}

#[tokio::test]
async fn context_overflow_is_detected_and_never_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            json!({"error": {"message": "This model's maximum context length is 8192 tokens"}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let a = adapter(ApiKind::OpenaiCompatible, Endpoint::new(server.uri(), None));
    let err = open_with_retry(a.as_ref(), &request(), &fast_policy(), |_, _, _| {})
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::ContextOverflow);
}

fn fast_policy() -> RetryPolicy {
    RetryPolicy {
        base_delay: Duration::from_millis(5),
        max_delay: Duration::from_millis(20),
        ..RetryPolicy::default()
    }
}

#[tokio::test]
async fn overloaded_provider_is_retried_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(529).insert_header("retry-after-ms", "10"))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    let body = sse(&[(
        "",
        json!({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}]}),
    )]) + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;

    let a = adapter(ApiKind::OpenaiCompatible, Endpoint::new(server.uri(), None));
    let mut retries = Vec::new();
    let stream = open_with_retry(a.as_ref(), &request(), &fast_policy(), |n, d, e| {
        retries.push((n, d, e.kind))
    })
    .await
    .unwrap();
    let out = collect(stream, |_| {}).await.unwrap();
    assert_eq!(out.text, "ok");
    assert_eq!(retries.len(), 2);
    assert_eq!(
        retries[0].1,
        Duration::from_millis(10),
        "retry-after-ms is honored"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn failure_after_output_is_not_replayed() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "message_start",
            json!({"message": {"usage": {"input_tokens": 1}}}),
        ),
        (
            "content_block_delta",
            json!({"index": 0, "delta": {"type": "text_delta", "text": "partial"}}),
        ),
        (
            "error",
            json!({"error": {"type": "overloaded_error", "message": "Overloaded"}}),
        ),
    ]);
    Mock::given(method("POST"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;
    let a = adapter(ApiKind::Anthropic, Endpoint::new(server.uri(), None));
    let stream = open_with_retry(a.as_ref(), &request(), &fast_policy(), |_, _, _| {
        panic!("must not retry")
    })
    .await
    .unwrap();
    let mut seen = Vec::new();
    let err = collect(stream, |e| seen.push(e.clone())).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::ProviderInternal);
    assert!(seen.contains(&LlmEvent::TextDelta {
        text: "partial".into()
    }));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn error_before_output_in_stream_is_retried() {
    let adapter = ScriptedAdapter::new(vec![
        vec![
            ScriptStep::Event(LlmEvent::Usage(Usage::default())),
            ScriptStep::Error {
                error: ErrorKind::RateLimit,
                message: "slow".into(),
            },
        ],
        vec![
            ScriptStep::Event(LlmEvent::TextDelta {
                text: "fine".into(),
            }),
            ScriptStep::Event(LlmEvent::Finish {
                reason: FinishReason::Stop,
            }),
        ],
    ]);
    let stream = open_with_retry(&adapter, &request(), &fast_policy(), |_, _, _| {})
        .await
        .unwrap();
    assert_eq!(collect(stream, |_| {}).await.unwrap().text, "fine");
    assert_eq!(adapter.requests().len(), 2);
}

#[tokio::test]
async fn truncated_stream_is_a_transport_error() {
    let server = MockServer::start().await;
    let body = sse(&[("", json!({"choices": [{"delta": {"content": "half"}}]}))]);
    Mock::given(method("POST"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;
    let a = adapter(ApiKind::OpenaiCompatible, Endpoint::new(server.uri(), None));
    let err = collect(a.stream(request()).await.unwrap(), |_| {})
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Transport);
}

/// `tool-registry` → Eager execution during streaming: an OpenAI Chat call completes as
/// soon as the next call or text begins, not when the stream ends; a call whose input is
/// not yet whole JSON stays open.
#[tokio::test]
async fn openai_chat_completes_each_call_when_the_next_one_or_text_starts() {
    let server = MockServer::start().await;
    let call = |index: u32, id: &str, arguments: &str| json!({"choices": [{"delta": {"tool_calls": [{"index": index, "id": id, "function": {"name": "clock", "arguments": arguments}}]}}]});
    let more = |index: u32, arguments: &str| json!({"choices": [{"delta": {"tool_calls": [{"index": index, "function": {"arguments": arguments}}]}}]});
    let body = sse(&[
        ("", call(0, "call_a", "{\"tz\":")),
        ("", more(0, "\"UTC\"}")),
        ("", call(1, "call_b", "{\"tz\":\"C")),
        ("", call(2, "call_c", "{}")),
        ("", more(1, "ET\"}")),
        ("", json!({"choices": [{"delta": {"content": "Done"}}]})),
        (
            "",
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ),
    ]) + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;
    let a = adapter(ApiKind::OpenaiCompatible, Endpoint::new(server.uri(), None));
    let mut seen = Vec::new();
    let out = collect(a.stream(request()).await.unwrap(), |e| seen.push(e.clone()))
        .await
        .unwrap();
    let order: Vec<String> = seen
        .iter()
        .filter_map(|event| match event {
            LlmEvent::ToolCallDone(call) => Some(format!("done {}", call.id)),
            LlmEvent::ToolCallDelta { id, .. } => Some(format!("delta {id}")),
            LlmEvent::TextDelta { text } => Some(format!("text {text}")),
            LlmEvent::Finish { .. } => Some("finish".into()),
            _ => None,
        })
        .collect();
    assert_eq!(
        order,
        [
            "delta call_a",
            "delta call_a",
            // call_b starts: call_a is whole and ends there.
            "done call_a",
            "delta call_b",
            // call_c starts while call_b is not whole JSON yet: call_b stays open.
            "delta call_c",
            "delta call_b",
            // Text ends both remaining calls, in start order.
            "done call_b",
            "done call_c",
            "text Done",
            "finish",
        ]
    );
    let inputs: Vec<_> = out.tool_calls.iter().map(|c| c.input.clone()).collect();
    assert_eq!(
        inputs,
        [
            Some(json!({"tz": "UTC"})),
            Some(json!({"tz": "CET"})),
            Some(json!({}))
        ]
    );
}

#[tokio::test]
async fn malformed_tool_arguments_have_no_input() {
    let server = MockServer::start().await;
    let body = sse(&[(
        "",
        json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c", "function": {"name": "write", "arguments": "{\"path\": \"a"}}]}, "finish_reason": "tool_calls"}]}),
    )]) + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;
    let a = adapter(ApiKind::OpenaiCompatible, Endpoint::new(server.uri(), None));
    let out = collect(a.stream(request()).await.unwrap(), |_| {})
        .await
        .unwrap();
    assert_eq!(out.tool_calls[0].input, None);
    assert_eq!(out.tool_calls[0].arguments, "{\"path\": \"a");
}

#[tokio::test]
async fn every_adapter_emits_the_same_event_types() {
    // Same scenario through two different providers yields identical provider-neutral events.
    let server = MockServer::start().await;
    let chat = sse(&[(
        "",
        json!({"choices": [{"delta": {"content": "hi"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 1}}),
    )]) + "data: [DONE]\n\n";
    let anthropic = sse(&[
        (
            "message_start",
            json!({"message": {"usage": {"input_tokens": 3}}}),
        ),
        (
            "content_block_delta",
            json!({"index": 0, "delta": {"type": "text_delta", "text": "hi"}}),
        ),
        (
            "message_delta",
            json!({"delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
        ),
        ("message_stop", json!({})),
    ]);
    Mock::given(path("/chat/completions"))
        .respond_with(ok_stream(chat))
        .mount(&server)
        .await;
    Mock::given(path("/messages"))
        .respond_with(ok_stream(anthropic))
        .mount(&server)
        .await;
    let mut events = Vec::new();
    for kind in [ApiKind::OpenaiCompatible, ApiKind::Anthropic] {
        let a = adapter(kind, Endpoint::new(server.uri(), None));
        let mut seen = Vec::new();
        collect(a.stream(request()).await.unwrap(), |e| seen.push(e.clone()))
            .await
            .unwrap();
        events.push(seen);
    }
    assert_eq!(events[0], events[1]);
}

#[tokio::test]
async fn runtime_no_tools_choice_overrides_injected_tools_on_every_adapter() {
    for kind in [
        ApiKind::OpenaiResponses,
        ApiKind::OpenaiCompatible,
        ApiKind::Anthropic,
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(json!({"error":{"message":"recorded test request"}})),
            )
            .mount(&server)
            .await;
        let a = adapter(kind, Endpoint::new(server.uri(), Some("test-key".into())));
        let mut req = request();
        req.tools_disabled = true;
        req.body = json!({"tools":[{"name":"injected"}],"tool_choice":"required"});
        assert!(a.stream(req).await.is_err());
        let body = server.received_requests().await.unwrap()[0]
            .body_json::<Value>()
            .unwrap();
        assert!(body.get("tools").is_none(), "{kind:?}: {body}");
        assert!(body.get("tools_disabled").is_none());
        let expected = if kind == ApiKind::Anthropic {
            json!({"type":"none"})
        } else {
            json!("none")
        };
        assert_eq!(body["tool_choice"], expected, "{kind:?}");
    }
}

/// Every adapter records the usage windows of its responses, including a rate-limited
/// one, and a provider that sends none records nothing (`observability-costs` →
/// Provider-reported limits).
#[tokio::test]
async fn adapters_record_reported_usage_windows_and_nothing_else() {
    use cyber_llm::limits::{LimitStore, WindowPeriod};
    let server = MockServer::start().await;
    let chat = sse(&[(
        "",
        json!({"choices": [{"delta": {"content": "hi"}, "finish_reason": "stop"}]}),
    )]) + "data: [DONE]\n\n";
    Mock::given(path("/plan/chat/completions"))
        .respond_with(
            ok_stream(chat.clone())
                .insert_header("x-codex-primary-used-percent", "40")
                .insert_header("x-codex-primary-window-minutes", "300"),
        )
        .mount(&server)
        .await;
    Mock::given(path("/plain/chat/completions"))
        .respond_with(ok_stream(chat))
        .mount(&server)
        .await;
    Mock::given(path("/limited/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("anthropic-ratelimit-unified-7d-utilization", "1.0")
                .set_body_json(json!({"error": {"type": "rate_limit_error", "message": "x"}})),
        )
        .mount(&server)
        .await;
    let store = LimitStore::default();
    let target = |provider: &str, base: &str| {
        let mut endpoint = Endpoint::new(format!("{}/{base}", server.uri()), None);
        endpoint.limits = Some(store.sink(provider));
        endpoint
    };
    let plan = adapter(ApiKind::OpenaiCompatible, target("plan", "plan"));
    collect(plan.stream(request()).await.unwrap(), |_| {})
        .await
        .unwrap();
    let plain = adapter(ApiKind::OpenaiCompatible, target("local", "plain"));
    collect(plain.stream(request()).await.unwrap(), |_| {})
        .await
        .unwrap();
    let limited = adapter(ApiKind::Anthropic, target("claude", "limited"));
    let err = limited.stream(request()).await.err().unwrap();
    assert_eq!(err.kind, ErrorKind::RateLimit);
    let readings = store.readings();
    let names: Vec<(&str, &str)> = readings
        .iter()
        .map(|r| (r.provider.as_str(), r.window.label.as_str()))
        .collect();
    assert_eq!(
        names,
        [("claude", "Weekly limit"), ("plan", "5-hour limit")]
    );
    assert_eq!(readings[0].window.period, WindowPeriod::Weekly);
    assert_eq!(readings[0].window.used_percent, 100.0);
    assert_eq!(readings[1].window.used_percent, 40.0);
    assert!(readings.iter().all(|r| r.observed_ms > 0));
}

#[tokio::test]
async fn every_adapter_refuses_an_unresolved_attachment_before_sending() {
    // A stored reference reaching an adapter is a runtime bug: it fails closed and the
    // provider never sees the request.
    let server = MockServer::start().await;
    let mut req = request();
    req.messages.push(Message {
        role: Role::User,
        content: vec![Content::Attachment(cyber_llm::AttachmentRef {
            sha256: "ab".repeat(32),
            media_type: "image/png".into(),
            name: Some("shot.png".into()),
            size: 10,
            width: None,
            height: None,
            pages: None,
            original: None,
        })],
    });
    for kind in [
        ApiKind::OpenaiCompatible,
        ApiKind::OpenaiResponses,
        ApiKind::Anthropic,
        ApiKind::ChatgptCodex,
        ApiKind::CopilotChat,
    ] {
        let a = adapter(kind, Endpoint::new(server.uri(), None));
        let error = a.stream(req.clone()).await.err().unwrap();
        assert_eq!(error.kind, ErrorKind::InvalidRequest, "{kind:?}");
        assert_eq!(error.message, "unresolved attachment", "{kind:?}");
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

// Golden request fixtures, one per reasoning carrier (`session-runtime` → Reasoning items,
// design D2): each pins the exact JSON body of a tool-loop follow-up request.

fn chat_done() -> String {
    sse(&[(
        "",
        json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
    )]) + "data: [DONE]\n\n"
}

fn responses_done() -> String {
    sse(&[(
        "response.completed",
        json!({"response": {"usage": {"input_tokens": 1, "output_tokens": 1}}}),
    )])
}

/// The body one request sends, with the mock answering `stream` at `route`.
async fn sent_body(kind: ApiKind, route: &str, stream: String, req: LlmRequest) -> Value {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(ok_stream(stream))
        .mount(&server)
        .await;
    run(kind, &server, "/v1", req).await.1
}

fn reasoning(text: &str, opaque: Option<Opaque>) -> Content {
    Content::Reasoning {
        text: text.into(),
        opaque,
    }
}

fn clock_call(id: &str, opaque: Option<Opaque>) -> Content {
    Content::ToolCall {
        id: id.into(),
        name: "clock".into(),
        input: json!({}),
        opaque,
    }
}

fn opaque(kind: OpaqueKind, data: &str, id: Option<&str>) -> Option<Opaque> {
    let mut opaque = Opaque::new(kind, data);
    opaque.id = id.map(str::to_owned);
    Some(opaque)
}

/// A tool loop: the user asks, the answer is `answer`, and both calls returned.
fn tool_loop(carrier: ReasoningCarrier, answer: Vec<Content>) -> LlmRequest {
    let result = |id: &str| Content::ToolResult {
        call_id: id.into(),
        output: "12:00".into(),
        is_error: false,
    };
    LlmRequest {
        model: "m-1".into(),
        messages: vec![
            Message::user_text("time?"),
            Message {
                role: Role::Assistant,
                content: answer,
            },
            Message {
                role: Role::User,
                content: vec![result("c1"), result("c2")],
            },
        ],
        cache: false,
        reasoning_carrier: carrier,
        ..LlmRequest::default()
    }
}

/// `provider-catalog` → Responses request shaping: encrypted reasoning is requested and
/// replayed in position, and an ID the API did not issue is left out.
#[tokio::test]
async fn golden_responses_replays_encrypted_reasoning_in_position() {
    let answer = vec![
        reasoning(
            "plan",
            opaque(OpaqueKind::EncryptedReasoning, "gAAA1", Some("rs_1")),
        ),
        Content::Text {
            text: "Checking.".into(),
        },
        clock_call("c1", None),
        reasoning(
            "",
            opaque(OpaqueKind::EncryptedReasoning, "gAAA2", Some("fork_rs_2")),
        ),
        clock_call("c2", None),
        // No encrypted content (dropped as oversized): nothing to replay statelessly.
        reasoning("lost", None),
    ];
    let req = tool_loop(ReasoningCarrier::Responses, answer);
    let sent = sent_body(
        ApiKind::OpenaiResponses,
        "/v1/responses",
        responses_done(),
        req,
    )
    .await;
    assert_eq!(
        sent,
        json!({
            "model": "m-1",
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
            "input": [
                {"role": "user", "content": [{"type": "input_text", "text": "time?"}]},
                {"type": "reasoning", "id": "rs_1", "encrypted_content": "gAAA1",
                    "summary": [{"type": "summary_text", "text": "plan"}]},
                {"role": "assistant", "content": [{"type": "output_text", "text": "Checking."}]},
                {"type": "function_call", "call_id": "c1", "name": "clock", "arguments": "{}"},
                {"type": "reasoning", "encrypted_content": "gAAA2", "summary": []},
                {"type": "function_call", "call_id": "c2", "name": "clock", "arguments": "{}"},
                {"type": "function_call_output", "call_id": "c1", "output": "12:00"},
                {"type": "function_call_output", "call_id": "c2", "output": "12:00"},
            ],
        })
    );
}

/// A recorded Responses stream: each finished reasoning item becomes one item with its
/// summary text, encrypted content and ID, ahead of the call it preceded.
#[tokio::test]
async fn responses_decodes_reasoning_items_with_encrypted_content() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "response.output_item.added",
            json!({"item": {"type": "reasoning", "id": "rs_1", "summary": []}}),
        ),
        (
            "response.reasoning_summary_text.delta",
            json!({"item_id": "rs_1", "delta": "First"}),
        ),
        (
            "response.reasoning_summary_text.delta",
            json!({"item_id": "rs_1", "delta": "Second"}),
        ),
        (
            "response.output_item.done",
            json!({"item": {"type": "reasoning", "id": "rs_1", "encrypted_content": "gAAA1",
                "summary": [{"type": "summary_text", "text": "First"},
                    {"type": "summary_text", "text": "Second"}]}}),
        ),
        (
            "response.output_item.added",
            json!({"item": {"type": "function_call", "id": "fc_1", "call_id": "c1", "name": "clock"}}),
        ),
        (
            "response.output_item.done",
            json!({"item": {"type": "function_call", "id": "fc_1", "call_id": "c1", "name": "clock", "arguments": "{}"}}),
        ),
        (
            "response.completed",
            json!({"response": {"usage": {"input_tokens": 5, "output_tokens": 5}}}),
        ),
    ]);
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;
    let a = adapter(
        ApiKind::OpenaiResponses,
        Endpoint::new(format!("{}/v1", server.uri()), None),
    );
    let mut seen = Vec::new();
    collect(a.stream(request()).await.unwrap(), |event| match event {
        LlmEvent::ReasoningItem {
            index,
            text,
            opaque,
        } => {
            let opaque = opaque.as_ref().unwrap();
            assert_eq!(opaque.kind, OpaqueKind::EncryptedReasoning);
            seen.push(format!(
                "item{index}:{text}:{}:{}",
                opaque.data,
                opaque.id.as_deref().unwrap()
            ));
        }
        LlmEvent::ToolCallDone(call) => seen.push(format!("call:{}", call.id)),
        _ => {}
    })
    .await
    .unwrap();
    assert_eq!(seen, ["item0:First\n\nSecond:gAAA1:rs_1", "call:c1"]);
}

/// The Chat Completions body of a tool loop whose answer reasoned before its one call.
async fn chat_loop_body(carrier: ReasoningCarrier, answer: Vec<Content>) -> Value {
    sent_body(
        ApiKind::OpenaiCompatible,
        "/v1/chat/completions",
        chat_done(),
        tool_loop(carrier, answer),
    )
    .await
}

fn chat_golden(assistant: Value) -> Value {
    json!({
        "model": "m-1",
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": [
            {"role": "user", "content": "time?"},
            assistant,
            {"role": "tool", "tool_call_id": "c1", "content": "12:00"},
            {"role": "tool", "tool_call_id": "c2", "content": "12:00"},
        ],
    })
}

fn chat_call(id: &str) -> Value {
    json!({"id": id, "type": "function", "function": {"name": "clock", "arguments": "{}"}})
}

/// DeepSeek, Kimi, GLM, Qwen and vLLM take the reasoning back as `reasoning_content`; an
/// empty one is a single space, since some servers reject an empty string.
#[tokio::test]
async fn golden_chat_reasoning_content() {
    let answer = vec![
        reasoning("look", None),
        clock_call("c1", None),
        clock_call("c2", None),
    ];
    let sent = chat_loop_body(ReasoningCarrier::ReasoningContent, answer).await;
    assert_eq!(
        sent,
        chat_golden(json!({"role": "assistant", "content": null,
            "tool_calls": [chat_call("c1"), chat_call("c2")], "reasoning_content": "look"}))
    );
    let empty = vec![
        reasoning("", None),
        clock_call("c1", None),
        clock_call("c2", None),
    ];
    let sent = chat_loop_body(ReasoningCarrier::ReasoningContent, empty).await;
    assert_eq!(sent["messages"][1]["reasoning_content"], " ");
    // Without a carrier the reasoning is not sent.
    let plain = vec![
        reasoning("look", None),
        clock_call("c1", None),
        clock_call("c2", None),
    ];
    let sent = chat_loop_body(ReasoningCarrier::None, plain).await;
    assert!(sent["messages"][1].get("reasoning_content").is_none());
}

/// Ollama and LM Studio take it back as `reasoning`.
#[tokio::test]
async fn golden_chat_reasoning() {
    let answer = vec![
        reasoning("look", None),
        clock_call("c1", None),
        clock_call("c2", None),
    ];
    let sent = chat_loop_body(ReasoningCarrier::Reasoning, answer).await;
    assert_eq!(
        sent,
        chat_golden(json!({"role": "assistant", "content": null,
            "tool_calls": [chat_call("c1"), chat_call("c2")], "reasoning": "look"}))
    );
}

/// OpenRouter streams `reasoning_details` in pieces: they are concatenated by index into
/// one item ahead of the call, and replayed unchanged.
#[tokio::test]
async fn golden_chat_reasoning_details_round_trip() {
    let server = MockServer::start().await;
    let body = sse(&[
        (
            "",
            json!({"choices": [{"delta": {"reasoning": "Lo", "reasoning_details": [
                {"type": "reasoning.text", "index": 0, "text": "Lo", "format": "anthropic-claude-v1"}]}}]}),
        ),
        (
            "",
            json!({"choices": [{"delta": {"reasoning": "ok", "reasoning_details": [
                {"type": "reasoning.text", "index": 0, "text": "ok", "signature": "sig"}]}}]}),
        ),
        (
            "",
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1",
                "function": {"name": "clock", "arguments": "{}"}}]}, "finish_reason": "tool_calls"}]}),
        ),
    ]) + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;
    let a = adapter(
        ApiKind::OpenaiCompatible,
        Endpoint::new(format!("{}/v1", server.uri()), None),
    );
    let mut order = Vec::new();
    let mut details = None;
    collect(a.stream(request()).await.unwrap(), |event| match event {
        LlmEvent::ReasoningItem { opaque, .. } => {
            let opaque = opaque.clone().unwrap();
            assert_eq!(opaque.kind, OpaqueKind::Details);
            order.push("details");
            details = Some(opaque);
        }
        LlmEvent::ToolCallDone(_) => order.push("call"),
        _ => {}
    })
    .await
    .unwrap();
    assert_eq!(order, ["details", "call"]);
    let details = details.unwrap();
    let merged = json!([{"type": "reasoning.text", "index": 0, "text": "Look",
        "format": "anthropic-claude-v1", "signature": "sig"}]);
    assert_eq!(
        serde_json::from_str::<Value>(&details.data).unwrap(),
        merged
    );

    let answer = vec![
        reasoning("Look", Some(details)),
        clock_call("c1", None),
        clock_call("c2", None),
    ];
    let sent = chat_loop_body(ReasoningCarrier::ReasoningDetails, answer).await;
    assert_eq!(
        sent,
        chat_golden(json!({"role": "assistant", "content": null,
            "tool_calls": [chat_call("c1"), chat_call("c2")], "reasoning_details": merged}))
    );
}

/// Gemini's OpenAI-compatible endpoint signs the first of parallel calls in
/// `extra_content.google`; the signature stays on that call and goes back on it.
#[tokio::test]
async fn golden_gemini_thought_signature_on_the_first_parallel_call() {
    let server = MockServer::start().await;
    let body = sse(&[(
        "",
        json!({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": "c1", "function": {"name": "clock", "arguments": "{}"},
                "extra_content": {"google": {"thought_signature": "ts=="}}},
            {"index": 1, "id": "c2", "function": {"name": "clock", "arguments": "{}"}}]},
            "finish_reason": "tool_calls"}]}),
    )]) + "data: [DONE]\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ok_stream(body))
        .mount(&server)
        .await;
    let (out, _) = run(ApiKind::OpenaiCompatible, &server, "/v1", request()).await;
    let signatures: Vec<_> = out
        .tool_calls
        .iter()
        .map(|call| call.opaque.as_ref().map(|o| (o.kind, o.data.as_str())))
        .collect();
    assert_eq!(signatures, [Some((OpaqueKind::Signature, "ts==")), None]);

    let answer = vec![
        clock_call("c1", out.tool_calls[0].opaque.clone()),
        clock_call("c2", None),
    ];
    let sent = chat_loop_body(ReasoningCarrier::GeminiSignature, answer).await;
    let mut signed = chat_call("c1");
    signed["extra_content"] = json!({"google": {"thought_signature": "ts=="}});
    assert_eq!(
        sent,
        chat_golden(json!({"role": "assistant", "content": null,
            "tool_calls": [signed, chat_call("c2")]}))
    );
}

/// Anthropic thinking and redacted thinking go back as their own blocks, in block order
/// between the calls (`session-runtime` → Interleaved thinking keeps every signed block).
#[tokio::test]
async fn golden_anthropic_thinking_blocks() {
    let answer = vec![
        reasoning("A", opaque(OpaqueKind::Signature, "sigA", None)),
        clock_call("c1", None),
        reasoning("", opaque(OpaqueKind::Redacted, "blob", None)),
        clock_call("c2", None),
    ];
    let mut req = tool_loop(ReasoningCarrier::Anthropic, answer);
    req.reasoning = Some(Reasoning::BudgetTokens(1024));
    let done = sse(&[
        (
            "message_start",
            json!({"message": {"usage": {"input_tokens": 1, "output_tokens": 1}}}),
        ),
        ("message_stop", json!({})),
    ]);
    let sent = sent_body(ApiKind::Anthropic, "/v1/messages", done, req).await;
    assert_eq!(
        sent,
        json!({
            "model": "m-1",
            "max_tokens": 8192,
            "stream": true,
            "thinking": {"type": "enabled", "budget_tokens": 1024},
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "time?"}]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "A", "signature": "sigA"},
                    {"type": "tool_use", "id": "c1", "name": "clock", "input": {}},
                    {"type": "redacted_thinking", "data": "blob"},
                    {"type": "tool_use", "id": "c2", "name": "clock", "input": {}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "c1", "content": "12:00", "is_error": false},
                    {"type": "tool_result", "tool_use_id": "c2", "content": "12:00", "is_error": false},
                ]},
            ],
        })
    );
}
