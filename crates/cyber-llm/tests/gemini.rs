//! The native Gemini adapter against a mock server, with SSE and error fixtures written
//! by hand from the documented `streamGenerateContent` shapes (`provider-catalog` →
//! Gemini API adapter, thought signatures, thinking configuration, error and quota
//! classification, finish reasons and safety blocks, usage and implicit caching, tool
//! declarations).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyber_llm::adapters::{Adapter, ApiKind, Endpoint, adapter};
use cyber_llm::catalog::profile::ThinkingParam;
use cyber_llm::{
    Content, ErrorKind, FinishReason, LlmError, LlmEvent, LlmRequest, Message, Opaque, OpaqueKind,
    OverCap, Reasoning, RetryNotice, RetryPolicy, Role, Shaping, Sleeper, ToolSpec, TurnOutput,
    Usage, collect, open_with_retry,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "gm-test-key-123";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/gemini/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn ok_stream(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

fn error_body(name: &str, status: u16) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .set_body_raw(fixture(&format!("errors/{name}")), "application/json")
}

fn stream_path(model: &str) -> String {
    format!("/v1beta/models/{model}:streamGenerateContent")
}

async fn mount(server: &MockServer, model: &str, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(stream_path(model)))
        .and(query_param("alt", "sse"))
        .respond_with(response)
        .mount(server)
        .await;
}

/// The adapter at `<server>/v1beta`, as the catalog's default base resolves.
fn gemini(server: &MockServer) -> Box<dyn Adapter> {
    adapter(
        ApiKind::Gemini,
        Endpoint::new(format!("{}/v1beta", server.uri()), Some(KEY.into())),
    )
}

fn user(text: &str) -> Message {
    Message::user_text(text)
}

fn assistant(content: Vec<Content>) -> Message {
    Message {
        role: Role::Assistant,
        content,
    }
}

fn results(ids: &[(&str, &str)]) -> Message {
    Message {
        role: Role::User,
        content: ids
            .iter()
            .map(|(id, output)| Content::ToolResult {
                call_id: (*id).into(),
                output: (*output).into(),
                is_error: false,
            })
            .collect(),
    }
}

fn signature(data: &str) -> Option<Opaque> {
    let mut opaque = Opaque::new(OpaqueKind::Signature, data);
    opaque.provider = "google".into();
    opaque.model = "gemini-4-carbon".into();
    Some(opaque)
}

fn call(id: &str, name: &str, input: Value, opaque: Option<Opaque>) -> Content {
    Content::ToolCall {
        id: id.into(),
        name: name.into(),
        input,
        opaque,
    }
}

fn tool(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: format!("The {name} tool"),
        input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    }
}

fn request(model: &str, messages: Vec<Message>) -> LlmRequest {
    LlmRequest {
        model: model.into(),
        system: vec!["You are terse.".into()],
        messages,
        cache: true,
        ..LlmRequest::default()
    }
}

/// The body the adapter sends for `req`, against a server that answers a plain text turn.
async fn sent_body(req: LlmRequest) -> Value {
    let server = MockServer::start().await;
    mount(
        &server,
        &req.model.clone(),
        ok_stream(fixture("text_answer.sse")),
    )
    .await;
    let stream = gemini(&server).stream(req).await.unwrap();
    collect(stream, |_| {}).await.unwrap();
    server.received_requests().await.unwrap()[0]
        .body_json::<Value>()
        .unwrap()
}

/// Decode a fixture, keeping every event.
async fn decode(name: &str) -> (Result<TurnOutput, LlmError>, Vec<LlmEvent>) {
    let server = MockServer::start().await;
    mount(&server, "gemini-4-carbon", ok_stream(fixture(name))).await;
    let mut events = Vec::new();
    let stream = gemini(&server)
        .stream(request("gemini-4-carbon", vec![user("go")]))
        .await
        .unwrap();
    let out = collect(stream, |e| events.push(e.clone())).await;
    (out, events)
}

// --- Requests ---------------------------------------------------------------------

/// "Key enables Gemini once the feature is on": the URL, the header and no key in the
/// URL; and the golden body of a text Turn.
#[tokio::test]
async fn a_text_turn_streams_with_the_key_only_in_a_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(stream_path("gemini-4-carbon")))
        .and(query_param("alt", "sse"))
        .and(header("x-goog-api-key", KEY))
        .respond_with(ok_stream(fixture("text_answer.sse")))
        .expect(1)
        .mount(&server)
        .await;
    let stream = gemini(&server)
        .stream(request("gemini-4-carbon", vec![user("hello")]))
        .await
        .unwrap();
    let out = collect(stream, |_| {}).await.unwrap();
    assert_eq!(out.text, "hi");
    assert_eq!(out.finish, Some(FinishReason::Stop));
    let received = &server.received_requests().await.unwrap()[0];
    assert!(!received.url.as_str().contains(KEY), "{}", received.url);
    assert!(received.headers.get("authorization").is_none());
    assert_eq!(
        received.body_json::<Value>().unwrap(),
        json!({
            "systemInstruction": {"parts": [{"text": "You are terse."}]},
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
            "generationConfig": {"maxOutputTokens": 65535},
        })
    );
}

/// Golden tool loop: declarations with `parametersJsonSchema`, the call with its ID and
/// signature, and its response with the same ID and name.
#[tokio::test]
async fn golden_tool_loop() {
    let mut schema_tool = tool("read");
    schema_tool.input_schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object", "properties": {"path": {"type": "string", "examples": ["a.rs"]}},
        "required": ["path"],
    });
    let mut req = request(
        "gemini-4-carbon",
        vec![
            user("read a.rs"),
            assistant(vec![
                Content::Text {
                    text: "Reading.".into(),
                },
                call("fc_1", "read", json!({"path": "a.rs"}), signature("S1")),
            ]),
            results(&[("fc_1", "fn main() {}")]),
        ],
    );
    req.tools = vec![schema_tool, tool("grep")];
    req.max_output_tokens = Some(32_000);
    assert_eq!(
        sent_body(req).await,
        json!({
            "systemInstruction": {"parts": [{"text": "You are terse."}]},
            "contents": [
                {"role": "user", "parts": [{"text": "read a.rs"}]},
                {"role": "model", "parts": [
                    {"text": "Reading."},
                    {"functionCall": {"id": "fc_1", "name": "read", "args": {"path": "a.rs"}},
                        "thoughtSignature": "S1"},
                ]},
                {"role": "user", "parts": [
                    {"functionResponse": {"id": "fc_1", "name": "read",
                        "response": {"output": "fn main() {}"}}},
                ]},
            ],
            "tools": [{"functionDeclarations": [
                {"name": "read", "description": "The read tool", "parametersJsonSchema": {
                    "type": "object", "properties": {"path": {"type": "string"}},
                    "required": ["path"]}},
                {"name": "grep", "description": "The grep tool", "parametersJsonSchema": {
                    "type": "object", "properties": {"path": {"type": "string"}}}},
            ]}],
            "toolConfig": {"functionCallingConfig": {"mode": "AUTO"}},
            "generationConfig": {"maxOutputTokens": 32000},
        })
    );
}

/// Images and PDFs go as `inlineData`; a failed result goes back as `error`.
#[tokio::test]
async fn golden_images_pdf_and_an_error_result() {
    let req = request(
        "gemini-4-carbon",
        vec![
            Message {
                role: Role::User,
                content: vec![
                    Content::Text {
                        text: "compare".into(),
                    },
                    Content::Image {
                        media_type: "image/png".into(),
                        data: "iVBORw0=".into(),
                    },
                    Content::Document {
                        media_type: "application/pdf".into(),
                        data: "JVBERi0=".into(),
                        name: Some("spec.pdf".into()),
                    },
                ],
            },
            assistant(vec![call("fc_1", "read", json!({}), signature("S1"))]),
            Message {
                role: Role::User,
                content: vec![Content::ToolResult {
                    call_id: "fc_1".into(),
                    output: "no such file".into(),
                    is_error: true,
                }],
            },
        ],
    );
    let body = sent_body(req).await;
    assert_eq!(
        body["contents"][0]["parts"],
        json!([
            {"text": "compare"},
            {"inlineData": {"mimeType": "image/png", "data": "iVBORw0="}},
            {"inlineData": {"mimeType": "application/pdf", "data": "JVBERi0="}},
        ])
    );
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"]["response"],
        json!({"error": "no such file"})
    );
}

/// "Parallel calls keep one signature": decoded as received, and replayed as one model
/// content `[read + S1, grep]` and one user content with both responses in order.
#[tokio::test]
async fn parallel_calls_keep_one_signature() {
    let (out, _) = decode("parallel_calls.sse").await;
    let out = out.unwrap();
    let decoded: Vec<_> = out
        .tool_calls
        .iter()
        .map(|c| {
            (
                c.id.as_str(),
                c.opaque.as_ref().map(|o| (o.kind, o.data.as_str())),
            )
        })
        .collect();
    assert_eq!(
        decoded,
        [
            ("fc_read", Some((OpaqueKind::Signature, "S1sig=="))),
            ("fc_grep", None)
        ]
    );
    assert_eq!(out.finish, Some(FinishReason::ToolCalls));

    let mut stamped = out.tool_calls[0].opaque.clone().unwrap();
    stamped.provider = "google".into();
    stamped.model = "gemini-4-carbon".into();
    let body = sent_body(request(
        "gemini-4-carbon",
        vec![
            user("look"),
            assistant(vec![
                call(
                    "fc_read",
                    "read",
                    json!({"path": "src/main.rs"}),
                    Some(stamped),
                ),
                call("fc_grep", "grep", json!({"pattern": "main"}), None),
            ]),
            results(&[("fc_read", "fn main"), ("fc_grep", "1 match")]),
        ],
    ))
    .await;
    assert_eq!(
        body["contents"][1],
        json!({"role": "model", "parts": [
            {"functionCall": {"id": "fc_read", "name": "read", "args": {"path": "src/main.rs"}},
                "thoughtSignature": "S1sig=="},
            {"functionCall": {"id": "fc_grep", "name": "grep", "args": {"pattern": "main"}}},
        ]})
    );
    assert_eq!(
        body["contents"][2],
        json!({"role": "user", "parts": [
            {"functionResponse": {"id": "fc_read", "name": "read", "response": {"output": "fn main"}}},
            {"functionResponse": {"id": "fc_grep", "name": "grep", "response": {"output": "1 match"}}},
        ]})
    );
}

/// "Signature in an empty final part": the late signature closes a reasoning item and
/// goes back as the last part of that model content, never merged into the text.
#[tokio::test]
async fn a_signature_in_an_empty_final_part_is_kept_and_replayed_last() {
    let (out, events) = decode("empty_signature.sse").await;
    assert_eq!(out.unwrap().text, "The build passes.");
    let items: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::ReasoningItem { opaque, .. } => opaque.as_ref().map(|o| o.data.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(items, ["S2sig=="]);
    // The runtime places the item before the answer's text; it still goes last.
    let body = sent_body(request(
        "gemini-4-carbon",
        vec![
            user("build?"),
            assistant(vec![
                Content::Reasoning {
                    text: String::new(),
                    opaque: signature("S2sig=="),
                },
                Content::Text {
                    text: "The build passes.".into(),
                },
            ]),
            user("thanks"),
        ],
    ))
    .await;
    assert_eq!(
        body["contents"][1]["parts"],
        json!([{"text": "The build passes."}, {"text": "", "thoughtSignature": "S2sig=="}])
    );
}

/// Both steps of a sequential loop keep their own signatures, byte-exact.
#[tokio::test]
async fn a_sequential_loop_returns_every_signature() {
    let body = sent_body(request(
        "gemini-4-carbon",
        vec![
            user("fix it"),
            assistant(vec![call("fc_1", "read", json!({}), signature("A+/="))]),
            results(&[("fc_1", "x")]),
            assistant(vec![call("fc_2", "edit", json!({}), signature("B+/="))]),
            results(&[("fc_2", "ok")]),
        ],
    ))
    .await;
    assert_eq!(body["contents"][1]["parts"][0]["thoughtSignature"], "A+/=");
    assert_eq!(body["contents"][3]["parts"][0]["thoughtSignature"], "B+/=");
}

/// A history where the current turn holds another model's call and result, and an
/// earlier turn holds another of its calls.
fn switched_history(model: &str) -> LlmRequest {
    request(
        model,
        vec![
            user("earlier"),
            assistant(vec![call("toolu_0", "clock", json!({"tz": "UTC"}), None)]),
            results(&[("toolu_0", "11:00")]),
            assistant(vec![Content::Text {
                text: "It was 11.".into(),
            }]),
            user("and now?"),
            assistant(vec![call("toolu_1", "clock", json!({"tz": "UTC"}), None)]),
            results(&[("toolu_1", "12:00")]),
        ],
    )
}

/// "Model switch inside a tool loop": the current turn's foreign call and result go as
/// text, so no unsigned call reaches Gemini 3/4; earlier turns are accepted as they are.
#[tokio::test]
async fn a_model_switch_inside_a_tool_loop_lowers_the_turns_foreign_calls() {
    let body = sent_body(switched_history("gemini-4-carbon")).await;
    let contents = body["contents"].as_array().unwrap();
    assert_eq!(
        contents[1]["parts"],
        json!([{"functionCall": {"id": "toolu_0", "name": "clock", "args": {"tz": "UTC"}}}]),
        "an earlier turn is not validated"
    );
    assert_eq!(
        contents[5]["parts"],
        json!([{"text": "[tool call clock {\"tz\":\"UTC\"}]"}])
    );
    assert_eq!(
        contents[6]["parts"],
        json!([{"text": "[tool result clock] 12:00"}])
    );
    assert!(
        !body
            .to_string()
            .contains("skip_thought_signature_validator")
    );

    // Gemini 2.5 accepts unsigned calls, so nothing is lowered.
    let old = sent_body(switched_history("gemini-2.5-flash")).await;
    assert!(old["contents"][5]["parts"][0].get("functionCall").is_some());
}

/// "Signature bypass enabled": the placeholder instead of text, only when asked.
#[tokio::test]
async fn signature_bypass_sends_the_placeholder_on_foreign_calls() {
    let mut req = switched_history("gemini-4-carbon");
    req.shaping.signature_bypass = true;
    let body = sent_body(req).await;
    assert_eq!(
        body["contents"][5]["parts"],
        json!([{"functionCall": {"id": "toolu_1", "name": "clock", "args": {"tz": "UTC"}},
            "thoughtSignature": "skip_thought_signature_validator"}])
    );
    assert_eq!(
        body["contents"][6]["parts"][0]["functionResponse"]["name"],
        "clock"
    );
    assert!(
        body["contents"][1]["parts"][0]
            .get("thoughtSignature")
            .is_none(),
        "earlier turns get no placeholder"
    );
}

/// A local call ID (a model that issued none) is never sent back.
#[tokio::test]
async fn local_call_ids_are_not_sent() {
    let (out, _) = decode("thoughts_then_call.sse").await;
    let id = out.unwrap().tool_calls[0].id.clone();
    assert!(id.starts_with("gm_"), "{id}");
    let body = sent_body(request(
        "gemini-2.5-flash",
        vec![
            user("read"),
            assistant(vec![call(
                &id,
                "read",
                json!({"path": "a.txt"}),
                signature("S3"),
            )]),
            results(&[(&id, "text")]),
        ],
    ))
    .await;
    assert_eq!(
        body["contents"][1]["parts"][0]["functionCall"],
        json!({"name": "read", "args": {"path": "a.txt"}})
    );
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"],
        json!({"name": "read", "response": {"output": "text"}})
    );
}

fn thinking_request(model: &str, reasoning: Option<Reasoning>) -> LlmRequest {
    let mut req = request(model, vec![user("think")]);
    req.reasoning = reasoning;
    req.shaping.reasoning = true;
    req
}

/// "Level for Gemini 4".
#[tokio::test]
async fn a_level_for_gemini_4() {
    let req = thinking_request("gemini-4-hydrogen", Some(Reasoning::Effort("high".into())));
    assert_eq!(
        sent_body(req).await["generationConfig"]["thinkingConfig"],
        json!({"thinkingLevel": "high", "includeThoughts": true})
    );
}

/// "Budget for Gemini 2.5".
#[tokio::test]
async fn a_budget_for_gemini_2_5() {
    let req = thinking_request("gemini-2.5-flash", Some(Reasoning::BudgetTokens(4096)));
    assert_eq!(
        sent_body(req).await["generationConfig"]["thinkingConfig"],
        json!({"thinkingBudget": 4096, "includeThoughts": true})
    );
}

/// "Default output limit", the 65,535 ceiling, and temperature only when set.
#[tokio::test]
async fn output_limit_and_temperature() {
    let mut req = request("gemini-4-carbon", vec![user("x")]);
    assert_eq!(
        sent_body(req.clone()).await["generationConfig"],
        json!({"maxOutputTokens": 65535})
    );
    req.max_output_tokens = Some(100_000);
    req.temperature = Some(0.2);
    assert_eq!(
        sent_body(req).await["generationConfig"],
        json!({"maxOutputTokens": 65535, "temperature": 0.2})
    );
}

/// No request ever holds both thinking fields, whatever the model, reasoning or
/// profile override; reasoning off on 2.5 Flash turns thinking (and summaries) off.
#[tokio::test]
async fn no_request_holds_both_thinking_fields() {
    let reasonings = [
        None,
        Some(Reasoning::Off),
        Some(Reasoning::Effort("low".into())),
        Some(Reasoning::Effort("xhigh".into())),
        Some(Reasoning::BudgetTokens(0)),
        Some(Reasoning::BudgetTokens(20_000)),
    ];
    for model in [
        "gemini-2.5-flash",
        "gemini-2.5-pro",
        "gemini-3.1-pro-preview",
        "gemini-4-carbon",
    ] {
        for param in [
            None,
            Some(ThinkingParam::Level),
            Some(ThinkingParam::Budget),
        ] {
            for reasoning in &reasonings {
                let mut req = thinking_request(model, reasoning.clone());
                req.shaping.thinking_param = param;
                let config = &sent_body(req).await["generationConfig"]["thinkingConfig"];
                let both =
                    config.get("thinkingLevel").is_some() && config.get("thinkingBudget").is_some();
                assert!(!both, "{model} {param:?} {reasoning:?}: {config}");
            }
        }
    }
    let off = thinking_request("gemini-2.5-flash", Some(Reasoning::Off));
    assert_eq!(
        sent_body(off).await["generationConfig"]["thinkingConfig"],
        json!({"thinkingBudget": 0})
    );
    let mut forced = thinking_request("gemini-4-carbon", Some(Reasoning::BudgetTokens(2048)));
    forced.shaping.thinking_param = Some(ThinkingParam::Budget);
    assert_eq!(
        sent_body(forced).await["generationConfig"]["thinkingConfig"],
        json!({"thinkingBudget": 2048, "includeThoughts": true}),
        "the profile override wins"
    );
}

/// "Stable prefix": a new user message changes nothing before it.
#[tokio::test]
async fn consecutive_requests_share_a_byte_identical_prefix() {
    let history = vec![
        user("one"),
        assistant(vec![
            call("fc_1", "read", json!({"b": 1, "a": 2}), signature("S1")),
            call("fc_2", "grep", json!({}), None),
        ]),
        results(&[("fc_1", "x"), ("fc_2", "y")]),
        assistant(vec![
            Content::Reasoning {
                text: "summary".into(),
                opaque: signature("S9"),
            },
            Content::Text {
                text: "done".into(),
            },
        ]),
    ];
    let mut first = request("gemini-4-carbon", history.clone());
    first.tools = vec![tool("read"), tool("grep")];
    let mut second = first.clone();
    second.messages.push(user("two"));
    let (a, b) = (sent_body(first).await, sent_body(second).await);
    for key in [
        "systemInstruction",
        "tools",
        "toolConfig",
        "generationConfig",
    ] {
        assert_eq!(a[key].to_string(), b[key].to_string(), "{key}");
    }
    let (a, b) = (
        a["contents"].as_array().unwrap(),
        b["contents"].as_array().unwrap(),
    );
    assert_eq!(b.len(), a.len() + 1);
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.to_string(), y.to_string());
    }
}

/// A runtime no-tools ceiling removes tools, injected ones included.
#[tokio::test]
async fn no_tools_overrides_injected_tools() {
    let mut req = request("gemini-4-carbon", vec![user("x")]);
    req.tools = vec![tool("read")];
    req.tools_disabled = true;
    req.body = json!({"tools": [{"functionDeclarations": [{"name": "injected"}]}]});
    let body = sent_body(req).await;
    assert!(body.get("tools").is_none(), "{body}");
    assert_eq!(
        body["toolConfig"],
        json!({"functionCallingConfig": {"mode": "NONE"}})
    );
}

/// "Base URL with a version suffix", a bare host, and a proxy prefix kept as written.
#[tokio::test]
async fn base_urls_are_normalized() {
    let server = MockServer::start().await;
    for route in [
        stream_path("m"),
        "/proxy/models/m:streamGenerateContent".into(),
    ] {
        Mock::given(path(route))
            .respond_with(ok_stream(fixture("text_answer.sse")))
            .mount(&server)
            .await;
    }
    for base in ["/v1beta/", "", "/", "/proxy"] {
        let a = adapter(
            ApiKind::Gemini,
            Endpoint::new(format!("{}{base}", server.uri()), Some(KEY.into())),
        );
        let stream = a.stream(request("m", vec![user("x")])).await.unwrap();
        assert_eq!(collect(stream, |_| {}).await.unwrap().text, "hi", "{base}");
    }
    let paths: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert_eq!(
        paths,
        [
            stream_path("m"),
            stream_path("m"),
            stream_path("m"),
            "/proxy/models/m:streamGenerateContent".into()
        ]
    );
}

/// "Plain HTTP to a remote host": refused before anything is sent.
#[tokio::test]
async fn plain_http_to_a_remote_host_is_refused() {
    let a = adapter(
        ApiKind::Gemini,
        Endpoint::new("http://gemini.corp.example", Some(KEY.into())),
    );
    let error = a.stream(request("m", vec![user("x")])).await.err().unwrap();
    assert_eq!(error.kind, ErrorKind::InvalidRequest);
    assert!(
        error.message.contains("http://gemini.corp.example/v1beta"),
        "{}",
        error.message
    );
}

// --- Decoding ---------------------------------------------------------------------

/// Thought summaries stream as reasoning; the call carries its own signature.
#[tokio::test]
async fn thought_summaries_then_a_call() {
    let (out, events) = decode("thoughts_then_call.sse").await;
    let out = out.unwrap();
    assert_eq!(
        out.reasoning,
        "**Planning** I should read the file first. Then answer."
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, LlmEvent::ReasoningDelta { .. }))
    );
    assert_eq!(out.tool_calls[0].input, Some(json!({"path": "a.txt"})));
    assert_eq!(out.tool_calls[0].opaque.as_ref().unwrap().data, "S3sig==");
    assert_eq!(out.finish, Some(FinishReason::ToolCalls));
    assert_eq!(out.usage.reasoning, 30);
}

/// Safety blocks: output already streamed is kept and ends `content_filter`; with none,
/// the step fails with `ContentPolicy` naming the reason.
#[tokio::test]
async fn safety_blocks_keep_output_or_fail_with_the_reason() {
    let (out, _) = decode("safety_after_text.sse").await;
    let out = out.unwrap();
    assert_eq!(out.text, "Here is the first part");
    assert_eq!(out.finish, Some(FinishReason::ContentFilter));
    let (out, _) = decode("safety_no_output.sse").await;
    let error = out.unwrap_err();
    assert_eq!(error.kind, ErrorKind::ContentPolicy);
    assert!(
        error.message.contains("PROHIBITED_CONTENT"),
        "{}",
        error.message
    );
}

/// Finish reasons that end the step without an error.
#[tokio::test]
async fn finish_reasons_map_onto_neutral_ones() {
    let (out, _) = decode("max_tokens.sse").await;
    assert_eq!(out.unwrap().finish, Some(FinishReason::Length));
    let (out, _) = decode("too_many_tool_calls.sse").await;
    let out = out.unwrap();
    assert_eq!(
        out.finish,
        Some(FinishReason::Other("too_many_tool_calls".into()))
    );
    assert_eq!(
        out.tool_calls.len(),
        1,
        "the calls received so far are kept"
    );
    let (out, _) = decode("other_finish.sse").await;
    assert_eq!(
        out.unwrap().finish,
        Some(FinishReason::Other("language".into()))
    );
}

/// Finish reasons that are errors.
#[tokio::test]
async fn finish_reasons_that_fail_the_step() {
    for (name, kind, message) in [
        (
            "unexpected_tool_call.sse",
            ErrorKind::InvalidRequest,
            "Gemini called a tool that was not offered",
        ),
        (
            "missing_signature.sse",
            ErrorKind::InvalidRequest,
            "Gemini rejected the history: missing thought signature",
        ),
        (
            "malformed_call.sse",
            ErrorKind::ProviderInternal,
            "Gemini produced a malformed function call",
        ),
        (
            "stream_error.sse",
            ErrorKind::ProviderInternal,
            "The model is overloaded. Please try again later.",
        ),
    ] {
        let error = decode(name).await.0.unwrap_err();
        assert_eq!(
            (error.kind, error.message.as_str()),
            (kind, message),
            "{name}"
        );
    }
}

/// "Cached tokens reported", priced at the model's cache-read rate.
#[tokio::test]
async fn cached_and_thought_tokens_are_reported_from_the_last_usage() {
    let (out, _) = decode("cached_usage.sse").await;
    let usage = out.unwrap().usage;
    assert_eq!(
        usage,
        Usage {
            input: 20_000,
            output: 300,
            reasoning: 800,
            cache_read: 100_000,
            cache_write: 0
        }
    );
    // gemini-4-carbon: $2 input, $10 output, $0.10 cache read per million.
    let cost = cyber_llm::catalog::Cost {
        input: 2.0,
        output: 10.0,
        cache_read: Some(0.1),
        ..cyber_llm::catalog::Cost::default()
    };
    let usd = cyber_llm::catalog::compute_cost(Some(&cost), &usage).unwrap();
    assert!((usd - (0.04 + 0.003 + 0.008 + 0.01)).abs() < 1e-12, "{usd}");
}

// --- Errors and retries -------------------------------------------------------------

/// A sleeper that records each wait and returns at once.
fn recording() -> (Sleeper, Arc<Mutex<Vec<Duration>>>) {
    let slept = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&slept);
    let sleeper = Sleeper::new(move |duration| {
        log.lock().unwrap().push(duration);
        Box::pin(async {})
    });
    (sleeper, slept)
}

fn instant_policy(cap: Duration) -> (RetryPolicy, Arc<Mutex<Vec<Duration>>>) {
    let (sleeper, slept) = recording();
    let policy = RetryPolicy {
        retry_after_cap: cap,
        sleeper,
        ..RetryPolicy::default()
    };
    (policy, slept)
}

/// Run one request through the retry policy; a first failure answers `first`, later
/// attempts answer `then`.
async fn with_retry(
    first: ResponseTemplate,
    then: ResponseTemplate,
    policy: &RetryPolicy,
    req: LlmRequest,
) -> (Result<TurnOutput, LlmError>, Vec<(u32, Duration)>, usize) {
    let server = MockServer::start().await;
    Mock::given(path(stream_path(&req.model)))
        .respond_with(first)
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path(stream_path(&req.model)))
        .respond_with(then)
        .with_priority(2)
        .mount(&server)
        .await;
    let a = gemini(&server);
    let mut notices = Vec::new();
    let note = |n: &RetryNotice<'_>| notices.push((n.attempt, n.delay));
    let out = match open_with_retry(a.as_ref(), &req, policy, note).await {
        Ok(stream) => collect(stream, |_| {}).await,
        Err(e) => Err(e),
    };
    let sent = server.received_requests().await.unwrap().len();
    (out, notices, sent)
}

fn turn() -> LlmRequest {
    request("gemini-4-carbon", vec![user("go")])
}

/// Each documented failure, classified (design D5).
#[tokio::test]
async fn http_errors_are_classified_from_the_rpc_details() {
    let server = MockServer::start().await;
    let cases: &[(&str, u16, ErrorKind, &str, Option<u64>)] = &[
        (
            "429_per_minute.json",
            429,
            ErrorKind::RateLimit,
            "You exceeded your current quota",
            Some(37_000),
        ),
        (
            "429_no_details.json",
            429,
            ErrorKind::RateLimit,
            "Resource has been exhausted",
            None,
        ),
        (
            "429_per_day.json",
            429,
            ErrorKind::QuotaExceeded,
            "Gemini daily quota exhausted for gemini-4-carbon (GenerateRequestsPerDayPerProjectPerModel-FreeTier)",
            Some(14_522_000),
        ),
        (
            "429_spend_cap.json",
            429,
            ErrorKind::QuotaExceeded,
            "Gemini project spend cap reached",
            None,
        ),
        (
            "402_prepaid.json",
            402,
            ErrorKind::QuotaExceeded,
            "Gemini prepaid credit balance is exhausted",
            None,
        ),
        (
            "400_missing_signature.json",
            400,
            ErrorKind::InvalidRequest,
            "Gemini rejected the history: missing thought signature",
            None,
        ),
        (
            "400_token_overflow.json",
            400,
            ErrorKind::ContextOverflow,
            "The input token count",
            None,
        ),
        (
            "400_schema.json",
            400,
            ErrorKind::InvalidRequest,
            "Gemini rejected a tool declaration",
            None,
        ),
        (
            "400_failed_precondition.json",
            400,
            ErrorKind::Authentication,
            "Gemini API not available for this project or region",
            None,
        ),
        (
            "400_key_invalid.json",
            400,
            ErrorKind::Authentication,
            "API key not valid",
            None,
        ),
        (
            "403_denied.json",
            403,
            ErrorKind::Authentication,
            "Method doesn't allow",
            None,
        ),
        (
            "503_unavailable.json",
            503,
            ErrorKind::ProviderInternal,
            "The model is overloaded",
            None,
        ),
    ];
    for (name, status, kind, message, retry_ms) in cases {
        let model = name.trim_end_matches(".json");
        mount(&server, model, error_body(name, *status)).await;
        let error = gemini(&server)
            .stream(request(model, vec![user("x")]))
            .await
            .err()
            .unwrap();
        assert_eq!((error.kind, error.status), (*kind, Some(*status)), "{name}");
        assert!(
            error.message.starts_with(message),
            "{name}: {}",
            error.message
        );
        assert_eq!(
            error.retry_after,
            retry_ms.map(Duration::from_millis),
            "{name}"
        );
    }
}

/// "Per-minute limit waits and recovers": one retry after the advised 37 seconds.
#[tokio::test]
async fn a_per_minute_limit_waits_and_recovers() {
    let (policy, slept) = instant_policy(Duration::from_secs(240));
    let (out, notices, sent) = with_retry(
        error_body("429_per_minute.json", 429),
        ok_stream(fixture("text_answer.sse")),
        &policy,
        turn(),
    )
    .await;
    assert_eq!(out.unwrap().text, "hi");
    assert_eq!(notices, [(1, Duration::from_secs(37))]);
    assert_eq!(*slept.lock().unwrap(), [Duration::from_secs(37)]);
    assert_eq!(sent, 2);
}

/// "Daily quota stops at once" and "Prepaid balance exhausted": no retry.
#[tokio::test]
async fn daily_quota_and_prepaid_balance_stop_at_once() {
    for (name, status, message) in [
        ("429_per_day.json", 429, "Gemini daily quota exhausted"),
        (
            "402_prepaid.json",
            402,
            "Gemini prepaid credit balance is exhausted",
        ),
        (
            "429_spend_cap.json",
            429,
            "Gemini project spend cap reached",
        ),
    ] {
        let (policy, _) = instant_policy(Duration::from_secs(240));
        let ok = ok_stream(fixture("text_answer.sse"));
        let (out, notices, sent) = with_retry(error_body(name, status), ok, &policy, turn()).await;
        let error = out.unwrap_err();
        assert_eq!(error.kind, ErrorKind::QuotaExceeded, "{name}");
        assert!(error.message.starts_with(message), "{}", error.message);
        assert!(notices.is_empty() && sent == 1, "{name}: never retried");
    }
}

/// A per-minute body advising 10 minutes.
fn ten_minutes() -> ResponseTemplate {
    let body = fixture("errors/429_per_minute.json").replace("\"37s\"", "\"600s\"");
    ResponseTemplate::new(429).set_body_raw(body, "application/json")
}

/// "Advised delay beyond the cap in an interactive Session": fail at once, naming the
/// delay; never a shortened wait.
#[tokio::test]
async fn an_advised_delay_beyond_the_cap_fails_at_once_when_attended() {
    let (policy, slept) = instant_policy(Duration::from_secs(240));
    let ok = ok_stream(fixture("text_answer.sse"));
    let (out, notices, sent) = with_retry(ten_minutes(), ok, &policy, turn()).await;
    let error = out.unwrap_err();
    assert_eq!(error.kind, ErrorKind::RateLimit);
    assert!(
        error
            .message
            .contains("advised waiting 10m, beyond the 4m retry wait cap"),
        "{}",
        error.message
    );
    assert!(notices.is_empty() && slept.lock().unwrap().is_empty());
    assert_eq!(sent, 1);
}

/// "Advised delay beyond the cap in exec": the whole delay, announced once, then the
/// request again.
#[tokio::test]
async fn an_unattended_run_waits_the_whole_advised_delay() {
    let (mut policy, slept) = instant_policy(Duration::from_secs(240));
    let (long, waited) = recording();
    policy.over_cap = OverCap::Wait {
        up_to: Duration::from_secs(6 * 3600),
        sleeper: long,
    };
    let ok = ok_stream(fixture("text_answer.sse"));
    let (out, notices, sent) = with_retry(ten_minutes(), ok, &policy, turn()).await;
    assert_eq!(out.unwrap().text, "hi");
    assert_eq!(notices, [(1, Duration::from_secs(600))]);
    assert_eq!(*waited.lock().unwrap(), [Duration::from_secs(600)]);
    assert!(slept.lock().unwrap().is_empty());
    assert_eq!(sent, 2);
}

/// "Malformed function call resampled": retried, and only the retried attempt counts.
#[tokio::test]
async fn a_malformed_function_call_is_resampled() {
    let (policy, _) = instant_policy(Duration::from_secs(240));
    let (out, notices, sent) = with_retry(
        ok_stream(fixture("malformed_call.sse")),
        ok_stream(fixture("parallel_calls.sse")),
        &policy,
        turn(),
    )
    .await;
    let out = out.unwrap();
    assert_eq!(out.tool_calls.len(), 2, "the valid calls, once");
    assert_eq!(notices.len(), 1);
    assert_eq!(sent, 2);
}

/// "Prompt blocked": `ContentPolicy` naming the reason, never retried.
#[tokio::test]
async fn a_blocked_prompt_is_never_retried() {
    let (policy, _) = instant_policy(Duration::from_secs(240));
    let ok = ok_stream(fixture("text_answer.sse"));
    let (out, notices, sent) = with_retry(
        ok_stream(fixture("prompt_blocked.sse")),
        ok,
        &policy,
        turn(),
    )
    .await;
    let error = out.unwrap_err();
    assert_eq!(error.kind, ErrorKind::ContentPolicy);
    assert!(error.message.contains("PROHIBITED_CONTENT"));
    assert!(notices.is_empty() && sent == 1);
}

/// "Oversized MCP schema": `InvalidRequest` naming the tool and the mitigation.
#[tokio::test]
async fn a_rejected_schema_names_the_tool_and_the_mitigation() {
    let (policy, _) = instant_policy(Duration::from_secs(240));
    let mut req = turn();
    req.tools = vec![tool("read"), tool("big_tool")];
    let ok = ok_stream(fixture("text_answer.sse"));
    let (out, notices, sent) =
        with_retry(error_body("400_schema.json", 400), ok, &policy, req).await;
    let error = out.unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidRequest);
    assert!(
        error
            .message
            .starts_with("Gemini rejected a tool declaration for big_tool"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("MCP tool search"),
        "{}",
        error.message
    );
    assert!(notices.is_empty() && sent == 1);
}

/// The key never appears in an error.
#[tokio::test]
async fn the_key_is_redacted_from_errors() {
    let server = MockServer::start().await;
    let body = json!({"error": {"code": 400, "message": format!("bad key {KEY}"), "status": "INVALID_ARGUMENT"}});
    mount(&server, "m", ResponseTemplate::new(400).set_body_json(body)).await;
    let error = gemini(&server)
        .stream(request("m", vec![user("x")]))
        .await
        .err()
        .unwrap();
    assert_eq!(error.message, "bad key ***");
}

/// Regression: a Gemini 429 through the OpenAI-compatible endpoint was `QuotaExceeded`
/// because its body says "quota", so a per-minute limit ended the Turn unretried.
#[tokio::test]
async fn a_per_minute_gemini_limit_on_the_compatible_path_is_a_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(path("/v1beta/openai/chat/completions"))
        .respond_with(error_body("compat_429_per_minute.json", 429))
        .mount(&server)
        .await;
    let a = adapter(
        ApiKind::OpenaiCompatible,
        Endpoint::new(format!("{}/v1beta/openai", server.uri()), Some(KEY.into())),
    );
    let error = a.stream(turn()).await.err().unwrap();
    assert_eq!(error.kind, ErrorKind::RateLimit);
    assert_eq!(error.retry_after, Some(Duration::from_secs(12)));
    let daily = fixture("errors/429_per_day.json");
    let server = MockServer::start().await;
    Mock::given(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429).set_body_raw(format!("[{daily}]"), "application/json"),
        )
        .mount(&server)
        .await;
    let a = adapter(ApiKind::OpenaiCompatible, Endpoint::new(server.uri(), None));
    assert_eq!(
        a.stream(turn()).await.err().unwrap().kind,
        ErrorKind::QuotaExceeded
    );
}

/// The neutral event stream matches the other adapters' for the same answer.
#[tokio::test]
async fn a_text_answer_is_the_same_neutral_stream() {
    let (out, events) = decode("text_answer.sse").await;
    out.unwrap();
    assert_eq!(
        events,
        [
            LlmEvent::TextDelta { text: "hi".into() },
            LlmEvent::Usage(Usage {
                input: 3,
                output: 1,
                ..Usage::default()
            }),
            LlmEvent::Finish {
                reason: FinishReason::Stop
            },
        ]
    );
}

/// Interruption can verify transport termination: the adapter moves onto a caller's
/// client.
#[test]
fn the_adapter_moves_onto_a_caller_owned_client() {
    let a = adapter(ApiKind::Gemini, Endpoint::new("https://x.example", None));
    assert!(
        a.with_http_client(reqwest::Client::new(), 1 << 20)
            .is_some()
    );
    let shaping = Shaping::default();
    assert!(!shaping.signature_bypass && shaping.thinking_param.is_none());
}
