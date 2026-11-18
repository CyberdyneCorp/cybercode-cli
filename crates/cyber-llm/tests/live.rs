//! Live provider check, skipped unless `CYBER_LIVE_TESTS=1`
//! (`harness-evaluation` → Live quality release baseline is separate from fixtures).
//!
//!     CYBER_LIVE_TESTS=1 cargo test -p cyber-llm --test live -- --nocapture
//!
//! For every provider whose key is present, runs a two-step exchange: the model must call
//! the `clock` tool, receive its result, and finish with text.
//!
//! The reasoning replay loops (`session-runtime` → Reasoning items) are ignored by default
//! and each needs its provider's key; every step must succeed, so a provider that rejects
//! the replayed reasoning fails the test with its 400:
//!
//!     OPENAI_API_KEY=… cargo test -p cyber-llm --test live -- --ignored --nocapture

use cyber_core::env::{EnvSource, ProcessEnv};
use cyber_llm::catalog::{BuildInputs, Catalog, ModelRef, SourceOptions, load_source};
use cyber_llm::{
    Content, FinishReason, LlmEvent, Message, Opaque, OpaqueKind, Reasoning, RetryPolicy, Role,
    ToolSpec, TurnOutput, collect, open_with_retry,
};
use serde_json::{Value, json};

const TARGETS: &[(&str, &str, &str)] = &[
    (
        "OPENAI_API_KEY",
        "CYBER_LIVE_OPENAI_MODEL",
        "openai/gpt-6-luna",
    ),
    (
        "ANTHROPIC_API_KEY",
        "CYBER_LIVE_ANTHROPIC_MODEL",
        "anthropic/claude-haiku-4-5",
    ),
];

#[tokio::test]
async fn tool_round_trip_on_configured_providers() {
    let env = ProcessEnv;
    if !env.flag("CYBER_LIVE_TESTS") {
        eprintln!("skipped: set CYBER_LIVE_TESTS=1 to call real providers");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let loaded = load_source(&SourceOptions::from_env(&env, dir.path()))
        .await
        .unwrap();
    let catalog = Catalog::build(&BuildInputs {
        data: &loaded.data,
        config: &json!({}),
        env: &env,
    });
    let mut ran = 0;
    for (key, override_var, default) in TARGETS {
        if env.get(key).is_none() {
            eprintln!("skipped {default}: {key} not set");
            continue;
        }
        let model = env.get(override_var).unwrap_or_else(|| default.to_string());
        round_trip(&catalog, &model).await;
        ran += 1;
    }
    assert!(ran > 0, "no provider key present");
}

async fn round_trip(catalog: &Catalog, model: &str) {
    let target = catalog
        .resolve(&ModelRef::parse(model).unwrap(), None)
        .unwrap();
    let adapter = target.adapter();
    let mut request = target.request.clone();
    request.system = vec!["Call the clock tool when asked about time. Be brief.".into()];
    request.tools = vec![ToolSpec {
        name: "clock".into(),
        description: "Return the current UTC time.".into(),
        input_schema: json!({ "type": "object", "properties": {} }),
    }];
    request.messages.push(Message::user_text(
        "What time is it in UTC? Use the clock tool.",
    ));

    let first = collect(
        open_with_retry(adapter.as_ref(), &request, &RetryPolicy::default(), |_| {})
            .await
            .unwrap(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(
        first.finish,
        Some(FinishReason::ToolCalls),
        "{model} did not call the tool"
    );
    let call = &first.tool_calls[0];
    assert_eq!(call.name, "clock");
    request.messages.push(Message {
        role: Role::Assistant,
        content: vec![Content::ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            input: json!({}),
            opaque: None,
        }],
    });
    request.messages.push(Message {
        role: Role::User,
        content: vec![Content::ToolResult {
            call_id: call.id.clone(),
            output: "2026-10-03T12:34:56Z".into(),
            is_error: false,
        }],
    });
    let second = collect(
        open_with_retry(adapter.as_ref(), &request, &RetryPolicy::default(), |_| {})
            .await
            .unwrap(),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(second.finish, Some(FinishReason::Stop));
    assert!(
        second.text.contains("12:34"),
        "{model} answer ignored the tool result: {}",
        second.text
    );
    assert!(first.usage.input + first.usage.cache_read > 0);
    eprintln!("{model}: ok ({:?})", second.usage);
}

/// The most model steps a reasoning loop may take before its final answer.
const MAX_STEPS: usize = 5;

/// What a reasoning tool loop saw.
#[derive(Default)]
struct LoopReport {
    tool_steps: usize,
    /// The kinds of opaque state replayed, on reasoning items and tool calls.
    replayed: Vec<OpaqueKind>,
    /// Whether any step streamed reasoning text.
    reasoned: bool,
    answer: String,
}

/// The key's value, or `None` after printing why the test is skipped.
fn live_key(key: &str) -> Option<String> {
    let value = ProcessEnv.get(key);
    if value.is_none() {
        eprintln!("skipped: {key} not set");
    }
    value
}

async fn live_catalog(config: Value) -> Catalog {
    let env = ProcessEnv;
    let dir = tempfile::tempdir().unwrap();
    let loaded = load_source(&SourceOptions::from_env(&env, dir.path()))
        .await
        .unwrap();
    Catalog::build(&BuildInputs {
        data: &loaded.data,
        config: &config,
        env: &env,
    })
}

/// An OpenAI-compatible provider `id` at `url`, keyed by `key`, whose `model` replays
/// reasoning within a tool loop.
fn compatible_provider(id: &str, url: &str, key: &str, model: &str) -> Value {
    json!({ "providers": { id: {
        "api": { "type": "openai-compatible", "url": url },
        "env": [key],
        "models": { model: { "profile": { "reasoning_replay": "tool_loop" } } },
    }}})
}

fn model_from(var: &str, default: &str) -> String {
    ProcessEnv.get(var).unwrap_or_else(|| default.to_string())
}

/// Ask for two clock readings in sequence, so the model reasons, calls a tool, reasons
/// again over the result and calls again, replaying everything it produced each step.
async fn reasoning_tool_loop(
    catalog: &Catalog,
    model: &str,
    reasoning: Option<Reasoning>,
) -> LoopReport {
    tool_loop(catalog, model, reasoning, SEQUENTIAL).await
}

/// Two dependent clock readings: one call per step.
const SEQUENTIAL: &str = "Read the clock for zone UTC first. Then, depending on that reading, read it for \
     zone UTC+2. Finally tell me both times.";
/// Two independent clock readings, asked for in one step: parallel calls.
const PARALLEL: &str = "Read the clock for zones UTC and UTC+2 at the same time, with two clock calls in \
     the same step. Then tell me both times.";

async fn tool_loop(
    catalog: &Catalog,
    model: &str,
    reasoning: Option<Reasoning>,
    prompt: &str,
) -> LoopReport {
    let model_ref = ModelRef::parse(model).unwrap();
    let target = catalog.resolve(&model_ref, None).unwrap();
    let adapter = target.adapter();
    let mut request = target.request.clone();
    if reasoning.is_some() {
        request.reasoning = reasoning;
    }
    request.system = vec!["Use the clock tool for every time reading, one call per zone.".into()];
    request.tools = vec![ToolSpec {
        name: "clock".into(),
        description: "Return the current time in one zone.".into(),
        input_schema: json!({ "type": "object", "required": ["zone"],
            "properties": { "zone": { "type": "string" } } }),
    }];
    request.messages.push(Message::user_text(prompt));
    let mut report = LoopReport::default();
    for step in 0..MAX_STEPS {
        let mut items = Vec::new();
        let mut streamed = String::new();
        let stream = open_with_retry(adapter.as_ref(), &request, &RetryPolicy::default(), |_| {})
            .await
            .unwrap_or_else(|e| panic!("{model} step {step} rejected: {e:?}"));
        // An item without text closes the reasoning streamed since the last item.
        let out = collect(stream, |event| match event {
            LlmEvent::ReasoningDelta { text } => streamed.push_str(text),
            LlmEvent::ReasoningItem { text, opaque } => {
                let streamed = std::mem::take(&mut streamed);
                let text = if text.is_empty() {
                    streamed
                } else {
                    text.clone()
                };
                items.push((text, opaque.clone()));
            }
            _ => {}
        })
        .await
        .unwrap_or_else(|e| panic!("{model} step {step} failed: {e:?}"));
        report.reasoned |= !out.reasoning.is_empty() || items.iter().any(|(t, _)| !t.is_empty());
        if out.finish != Some(FinishReason::ToolCalls) {
            assert_eq!(out.finish, Some(FinishReason::Stop), "{model} step {step}");
            report.answer = out.text;
            return report;
        }
        let (assistant, results) = replay(&out, items, &model_ref, report.tool_steps);
        report.replayed.extend(
            assistant
                .content
                .iter()
                .filter_map(|c| c.opaque().map(|o| o.kind)),
        );
        request.messages.extend([assistant, results]);
        report.tool_steps += 1;
    }
    panic!("{model} gave no final answer within {MAX_STEPS} steps");
}

/// The assistant message of a tool-calling step, with every reasoning item in block
/// order before its calls, and the user message of clock results for step `step`.
fn replay(
    out: &TurnOutput,
    items: Vec<(String, Option<Opaque>)>,
    model_ref: &ModelRef,
    step: usize,
) -> (Message, Message) {
    let mut content: Vec<Content> = items
        .into_iter()
        .map(|(text, opaque)| Content::Reasoning {
            text,
            opaque: stamped(opaque, model_ref),
        })
        .collect();
    if content.is_empty() && !out.reasoning.is_empty() {
        content.push(Content::Reasoning {
            text: out.reasoning.clone(),
            opaque: None,
        });
    }
    let mut results = Vec::new();
    for call in &out.tool_calls {
        content.push(Content::ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone().unwrap_or_else(|| json!({})),
            opaque: stamped(call.opaque.clone(), model_ref),
        });
        results.push(Content::ToolResult {
            call_id: call.id.clone(),
            output: format!("2026-10-03T{}:34:56", 12 + 2 * step),
            is_error: false,
        });
    }
    let assistant = Message {
        role: Role::Assistant,
        content,
    };
    (
        assistant,
        Message {
            role: Role::User,
            content: results,
        },
    )
}

/// Stamp state the adapter decoded with its producer, as the runtime does.
fn stamped(opaque: Option<Opaque>, model_ref: &ModelRef) -> Option<Opaque> {
    let mut opaque = opaque?;
    if opaque.provider.is_empty() {
        opaque.provider.clone_from(&model_ref.provider);
        opaque.model.clone_from(&model_ref.model);
    }
    Some(opaque)
}

fn assert_loop(model: &str, report: &LoopReport) {
    assert!(
        report.tool_steps >= 2,
        "{model} called the tool {} times",
        report.tool_steps
    );
    assert!(
        report.answer.contains("12:34"),
        "{model}: {}",
        report.answer
    );
    eprintln!("{model}: ok, replayed {:?}", report.replayed);
}

#[tokio::test]
#[ignore = "calls OpenAI; needs OPENAI_API_KEY"]
async fn openai_tool_loop_replays_encrypted_reasoning() {
    if live_key("OPENAI_API_KEY").is_none() {
        return;
    }
    let model = model_from("CYBER_LIVE_OPENAI_MODEL", "openai/gpt-6-luna");
    let catalog = live_catalog(json!({})).await;
    let effort = Some(Reasoning::Effort("low".into()));
    let report = reasoning_tool_loop(&catalog, &model, effort).await;
    assert_loop(&model, &report);
    assert!(
        report.replayed.contains(&OpaqueKind::EncryptedReasoning),
        "{model}"
    );
}

#[tokio::test]
#[ignore = "calls Anthropic; needs ANTHROPIC_API_KEY"]
async fn anthropic_interleaved_thinking_tool_loop() {
    if live_key("ANTHROPIC_API_KEY").is_none() {
        return;
    }
    let model = model_from("CYBER_LIVE_ANTHROPIC_MODEL", "anthropic/claude-haiku-4-5");
    let catalog = live_catalog(json!({})).await;
    let effort = Some(Reasoning::Effort("low".into()));
    let report = reasoning_tool_loop(&catalog, &model, effort).await;
    assert_loop(&model, &report);
    let signed = report
        .replayed
        .iter()
        .any(|kind| matches!(kind, OpaqueKind::Signature | OpaqueKind::Redacted));
    assert!(signed, "{model} replayed no thinking block");
}

#[tokio::test]
#[ignore = "calls DeepSeek; needs DEEPSEEK_API_KEY"]
async fn deepseek_thinking_tool_loop() {
    if live_key("DEEPSEEK_API_KEY").is_none() {
        return;
    }
    let id = model_from("CYBER_LIVE_DEEPSEEK_MODEL", "deepseek-reasoner");
    let config = compatible_provider(
        "live-deepseek",
        "https://api.deepseek.com/v1",
        "DEEPSEEK_API_KEY",
        &id,
    );
    let catalog = live_catalog(config).await;
    let model = format!("live-deepseek/{id}");
    let report = reasoning_tool_loop(&catalog, &model, None).await;
    assert_loop(&model, &report);
    assert!(report.reasoned, "{model} streamed no reasoning_content");
}

#[tokio::test]
#[ignore = "calls Gemini; needs GEMINI_API_KEY"]
async fn gemini_3_tool_loop_through_the_compatible_endpoint() {
    if live_key("GEMINI_API_KEY").is_none() {
        return;
    }
    let id = model_from("CYBER_LIVE_GEMINI_MODEL", "gemini-3-pro-preview");
    let url = "https://generativelanguage.googleapis.com/v1beta/openai";
    let catalog = live_catalog(compatible_provider(
        "live-gemini",
        url,
        "GEMINI_API_KEY",
        &id,
    ))
    .await;
    let model = format!("live-gemini/{id}");
    let report = reasoning_tool_loop(&catalog, &model, None).await;
    assert_loop(&model, &report);
    assert!(
        report.replayed.contains(&OpaqueKind::Signature),
        "{model} sent no thought signature"
    );
}

/// The catalog with the experimental native Gemini protocol on.
async fn gemini_catalog() -> Catalog {
    live_catalog(json!({ "features": { "gemini": true } })).await
}

#[tokio::test]
#[ignore = "calls Gemini; needs GEMINI_API_KEY"]
async fn gemini_4_native_tool_loop_with_parallel_calls() {
    if live_key("GEMINI_API_KEY").is_none() {
        return;
    }
    let model = model_from("CYBER_LIVE_GEMINI_NATIVE_MODEL", "google/gemini-4-carbon");
    let catalog = gemini_catalog().await;
    let report = tool_loop(&catalog, &model, None, PARALLEL).await;
    assert!(report.tool_steps >= 1, "{model} called no tool");
    assert!(
        report.answer.contains("12:34"),
        "{model}: {}",
        report.answer
    );
    assert!(
        report.replayed.contains(&OpaqueKind::Signature),
        "{model} sent no thought signature"
    );
    eprintln!("{model}: ok, replayed {:?}", report.replayed);
}

#[tokio::test]
#[ignore = "calls Gemini; needs GEMINI_API_KEY"]
async fn gemini_2_5_native_tool_loop_with_a_budget() {
    if live_key("GEMINI_API_KEY").is_none() {
        return;
    }
    let model = model_from("CYBER_LIVE_GEMINI_25_MODEL", "google/gemini-2.5-flash");
    let catalog = gemini_catalog().await;
    let report = reasoning_tool_loop(&catalog, &model, Some(Reasoning::BudgetTokens(1024))).await;
    assert_loop(&model, &report);
}

/// Another provider makes the first call; Gemini continues the same tool loop, with the
/// foreign call and its result sent as text.
#[tokio::test]
#[ignore = "calls Gemini and OpenAI or Anthropic; needs GEMINI_API_KEY and one of their keys"]
async fn a_model_switch_to_gemini_inside_a_tool_loop() {
    if live_key("GEMINI_API_KEY").is_none() {
        return;
    }
    let first = if ProcessEnv.get("ANTHROPIC_API_KEY").is_some() {
        model_from("CYBER_LIVE_ANTHROPIC_MODEL", "anthropic/claude-haiku-4-5")
    } else if live_key("OPENAI_API_KEY").is_some() {
        model_from("CYBER_LIVE_OPENAI_MODEL", "openai/gpt-6-luna")
    } else {
        return;
    };
    let catalog = gemini_catalog().await;
    let target = catalog
        .resolve(&ModelRef::parse(&first).unwrap(), None)
        .unwrap();
    let mut request = target.request.clone();
    request.system = vec!["Use the clock tool for every time reading, one call per zone.".into()];
    request.tools = vec![ToolSpec {
        name: "clock".into(),
        description: "Return the current time in one zone.".into(),
        input_schema: json!({ "type": "object", "required": ["zone"],
            "properties": { "zone": { "type": "string" } } }),
    }];
    request.messages.push(Message::user_text(SEQUENTIAL));
    let adapter = target.adapter();
    let stream = open_with_retry(adapter.as_ref(), &request, &RetryPolicy::default(), |_| {})
        .await
        .unwrap();
    let out = collect(stream, |_| {}).await.unwrap();
    assert_eq!(
        out.finish,
        Some(FinishReason::ToolCalls),
        "{first} called no tool"
    );
    // The runtime drops another model's state; the call carries none.
    let mut foreign = out.clone();
    for call in &mut foreign.tool_calls {
        call.opaque = None;
    }
    let (assistant, results) = replay(&foreign, Vec::new(), &ModelRef::parse(&first).unwrap(), 0);
    let gemini = model_from("CYBER_LIVE_GEMINI_NATIVE_MODEL", "google/gemini-4-carbon");
    let target = catalog
        .resolve(&ModelRef::parse(&gemini).unwrap(), None)
        .unwrap();
    let mut next = target.request.clone();
    next.system = request.system.clone();
    next.tools = request.tools.clone();
    next.messages = request.messages.clone();
    next.messages.extend([assistant, results]);
    let adapter = target.adapter();
    let stream = open_with_retry(adapter.as_ref(), &next, &RetryPolicy::default(), |_| {})
        .await
        .unwrap_or_else(|e| panic!("{gemini} rejected the switched history: {e:?}"));
    let out = collect(stream, |_| {})
        .await
        .unwrap_or_else(|e| panic!("{gemini}: {e:?}"));
    assert!(out.finish.is_some(), "{gemini} gave no answer");
    eprintln!("{first} -> {gemini}: ok ({:?})", out.finish);
}
