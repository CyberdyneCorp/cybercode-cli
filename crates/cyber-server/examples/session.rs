//! Drive a durable Session against a real model (milestone M0.3 smoke test).
//!
//!     cargo run -p cyber-server --example session -- openai/gpt-6-luna "What time is it in UTC?"
//!
//! The store lives in a temporary directory. A `clock` tool shows the tool loop; the reply
//! streams through the live bus; the final state shows totals, epoch and title.

use std::error::Error;
use std::io::Write;
use std::sync::Arc;

use cyber_core::env::ProcessEnv;
use cyber_core::paths::{DatabaseLocation, Paths, home_dir};
use cyber_llm::catalog::{SourceOptions, load_source};
use cyber_llm::{RetryPolicy, ToolSpec};
use cyber_server::runtime::*;
use cyber_store::{Store, StoreOptions};
use futures::future::BoxFuture;
use serde_json::json;
use tokio_util::sync::CancellationToken;

struct Clock;

impl ToolHost for Clock {
    fn definitions(&self, _turn: &TurnContext) -> Vec<ToolDef> {
        vec![ToolDef {
            scope: cyber_server::runtime::ToolScope::Builtin,
            registration: None,
            spec: ToolSpec {
                name: "clock".into(),
                description: "Return the current UTC time as an ISO-8601 string.".into(),
                input_schema: json!({ "type": "object", "properties": {} }),
            },
            retry_safety: RetrySafety::ReadOnly,
            concurrency_safe: true,
        }]
    }

    fn execute(&self, _call: Invocation, _cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        Box::pin(async {
            ToolOutcome::Ok(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let model = args
        .next()
        .ok_or("usage: session <provider/model> [prompt]")?;
    let prompt = args.next().unwrap_or_else(|| {
        "What time is it in UTC? Use the clock tool, then answer in one sentence.".into()
    });

    let env = ProcessEnv;
    let home = home_dir(&env).ok_or("HOME is not set")?;
    let paths = Paths::resolve(&env, &home);
    let loaded = load_source(&SourceOptions::from_env(&env, &paths.cache)).await?;
    let resolver: Arc<dyn ModelResolver> =
        Arc::new(CatalogResolver::new(&loaded.data, json!({}), &env));

    let dir = tempfile::tempdir()?;
    let store = Arc::new(Store::open(StoreOptions::new(
        DatabaseLocation::File(dir.path().join("cyber.db")),
        Runtime::registry(),
    ))?);
    let runtime = Runtime::new(RuntimeOptions {
        store,
        resolver,
        tools: Arc::new(Clock),
        global_config_dir: paths.config.clone(),
        shell: std::env::var("SHELL").unwrap_or_else(|_| "sh".into()),
        claude_compat: true,
        compaction: CompactionConfig::default(),
        retry: RetryPolicy::default(),
        max_steps: None,
        today: None,
        interactive: true,
        snapshots: Arc::new(NoSnapshots),
    });
    let mut live = runtime.subscribe();
    let cwd = std::env::current_dir()?;
    let session = runtime
        .create_session(CreateSession {
            directory: cwd.display().to_string(),
            model,
            ..Default::default()
        })
        .await?;
    runtime
        .admit(&session.id, Admission::text(prompt, Delivery::Steer))
        .await?;

    let printer = tokio::spawn(async move {
        while let Ok(event) = live.recv().await {
            match event {
                LiveEvent::TextDelta { text, .. } => {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
                LiveEvent::Durable { kind, seq, .. } => eprintln!("  [{seq}] {kind}"),
                LiveEvent::Error { kind, message, .. } => eprintln!("  error {kind}: {message}"),
                LiveEvent::Idle { .. } => break,
                _ => {}
            }
        }
    });
    runtime.wait_idle(&session.id).await;
    let _ = printer.await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // let the title arrive
    let state = runtime.state(&session.id).await?;
    println!();
    eprintln!("title: {}", state.info.title);
    eprintln!(
        "epoch: {}  steps: {}  usage: {:?}  cost: ${:.6}",
        state.epoch.map_or(0, |e| e.number),
        state.totals.steps,
        state.totals.usage,
        state.totals.cost
    );
    Ok(())
}
