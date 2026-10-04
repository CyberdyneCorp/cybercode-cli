//! Run one real streaming exchange with a tool call (milestone M0.2 smoke test).
//!
//!     cargo run -p cyber-llm --example turn -- openai/gpt-6-luna "What time is it in UTC?"
//!
//! Resolves the model through the catalog (cache, network or bundled snapshot) and the
//! user's configuration and credentials, streams the reply, executes the `clock` tool when
//! the model calls it, and sends the result back for the final answer.

use std::error::Error;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use cyber_core::config::{self, LoadRequest};
use cyber_core::env::ProcessEnv;
use cyber_core::paths::{Paths, home_dir};
use cyber_llm::catalog::{
    BuildInputs, Catalog, ModelRef, SourceOptions, compute_cost, load_source,
};
use cyber_llm::{
    Content, LlmEvent, Message, RetryPolicy, Role, ToolSpec, Usage, collect, open_with_retry,
};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .ok_or("usage: turn <provider/model[#variant]> [prompt]")?;
    let prompt = args
        .next()
        .unwrap_or_else(|| "What time is it in UTC? Use the clock tool.".into());

    let env = ProcessEnv;
    let home = home_dir(&env).ok_or("HOME is not set")?;
    let paths = Paths::resolve(&env, &home);
    let cwd = std::env::current_dir()?;
    let resolved_config = config::load(&LoadRequest {
        location: &cwd,
        paths: &paths,
        env: &env,
        home: &home,
        profile: None,
        overrides: &[],
        flags: json!({}),
    })?;
    let loaded = load_source(&SourceOptions::from_env(&env, &paths.cache)).await?;
    eprintln!("catalog: {:?}", loaded.origin);
    let catalog = Catalog::build(&BuildInputs {
        data: &loaded.data,
        config: &resolved_config.value,
        env: &env,
    });
    let model_ref = ModelRef::parse(&model)?;
    let target = catalog.resolve(&model_ref, None)?;
    let cost_table = catalog.find(&model_ref)?.1.cost.clone();
    let adapter = target.adapter();

    let mut request = target.request.clone();
    request.system = vec![
        "You are a terse assistant. Call the clock tool whenever the user asks about time.".into(),
    ];
    request.tools = vec![ToolSpec {
        name: "clock".into(),
        description: "Return the current UTC time as an ISO-8601 string.".into(),
        input_schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
    }];
    request.cache_key = Some(cyber_core::ids::new_id("ses"));
    request.messages.push(Message::user_text(prompt));

    let mut total = Usage::default();
    for turn in 1..=4 {
        eprintln!("--- turn {turn}");
        let stream = open_with_retry(
            adapter.as_ref(),
            &request,
            &RetryPolicy::default(),
            |n, d, e| {
                eprintln!("retry {n} in {d:?}: {e}");
            },
        )
        .await?;
        let out = collect(stream, |event| {
            if let LlmEvent::TextDelta { text } = event {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
        })
        .await?;
        println!();
        total.add(&out.usage);
        let mut assistant: Vec<Content> = Vec::new();
        if !out.text.is_empty() {
            assistant.push(Content::Text {
                text: out.text.clone(),
            });
        }
        let mut results = Vec::new();
        for call in &out.tool_calls {
            eprintln!("tool call: {}({})", call.name, call.arguments);
            assistant.push(Content::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.input.clone().unwrap_or(json!({})),
            });
            results.push(Content::ToolResult {
                call_id: call.id.clone(),
                output: utc_now(),
                is_error: false,
            });
        }
        request.messages.push(Message {
            role: Role::Assistant,
            content: assistant,
        });
        if results.is_empty() {
            eprintln!("finish: {:?}", out.finish);
            break;
        }
        request.messages.push(Message {
            role: Role::User,
            content: results,
        });
    }
    let cost = compute_cost(cost_table.as_ref(), &total)
        .map_or("unpriced".to_string(), |c| format!("${c:.6}"));
    eprintln!("usage: {total:?}  cost: {cost}");
    Ok(())
}

fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}
