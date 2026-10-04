//! Wire-level tests of each adapter against a mock server
//! (`provider-catalog` → Native provider adapters, Error classification, Retry policy).

use std::time::Duration;

use cyber_llm::adapters::{ApiKind, Endpoint, ScriptStep, ScriptedAdapter, adapter};
use cyber_llm::{
    Content, ErrorKind, FinishReason, LlmEvent, LlmRequest, Message, Reasoning, RetryPolicy, Role,
    ToolSpec, Usage, collect, open_with_retry,
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
    assert_eq!(result, json!({"type": "tool_result", "tool_use_id": "call_0", "content": "12:00", "is_error": false}));
    let headers = &server.received_requests().await.unwrap()[0].headers;
    assert_eq!(headers.get("x-api-key").unwrap(), "sk-secret-key");
    assert_eq!(headers.get("anthropic-version").unwrap(), "2023-06-01");
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
