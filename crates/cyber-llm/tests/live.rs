//! Live provider check, skipped unless `CYBER_LIVE_TESTS=1`
//! (`harness-evaluation` → Live quality release baseline is separate from fixtures).
//!
//!     CYBER_LIVE_TESTS=1 cargo test -p cyber-llm --test live -- --nocapture
//!
//! For every provider whose key is present, runs a two-step exchange: the model must call
//! the `clock` tool, receive its result, and finish with text.

use cyber_core::env::{EnvSource, ProcessEnv};
use cyber_llm::catalog::{BuildInputs, Catalog, ModelRef, SourceOptions, load_source};
use cyber_llm::{
    Content, FinishReason, Message, RetryPolicy, Role, ToolSpec, collect, open_with_retry,
};
use serde_json::json;

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
        open_with_retry(
            adapter.as_ref(),
            &request,
            &RetryPolicy::default(),
            |_, _, _| {},
        )
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
        open_with_retry(
            adapter.as_ref(),
            &request,
            &RetryPolicy::default(),
            |_, _, _| {},
        )
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
