//! Run one prompt with the built-in tools against a real model (milestone M0.4 smoke test).
//!
//!     cargo run -p cyber-tools --example agent -- openai/gpt-6-luna /path/to/repo "Add a test" [mode]
//!
//! Commands run under the OS sandbox and every Turn is snapshotted. Permission requests are
//! printed and approved once, standing in for a user; questions are dismissed. The per-message
//! diff is printed at the end.

use std::collections::BTreeMap;
use std::error::Error;
use std::io::Write;
use std::sync::Arc;

use cyber_core::env::ProcessEnv;
use cyber_core::paths::{DatabaseLocation, Paths, home_dir};
use cyber_llm::RetryPolicy;
use cyber_llm::catalog::{SourceOptions, load_source};
use cyber_server::runtime::*;
use cyber_store::{Store, StoreOptions};
use cyber_tools::{BuiltinHost, HostOptions};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [model, directory, prompt, rest @ ..] = &args[..] else {
        return Err("usage: agent <provider/model> <directory> <prompt> [mode]".into());
    };
    let mode = rest.first().cloned().unwrap_or_else(|| "default".into());
    let directory = std::fs::canonicalize(directory)?;

    let env = ProcessEnv;
    let home = home_dir(&env).ok_or("HOME is not set")?;
    let paths = Paths::resolve(&env, &home);
    let loaded = load_source(&SourceOptions::from_env(&env, &paths.cache)).await?;
    let resolver: Arc<dyn ModelResolver> =
        Arc::new(CatalogResolver::new(&loaded.data, json!({}), &env));

    let scratch = tempfile::tempdir()?;
    let store = Arc::new(Store::open(StoreOptions::new(
        DatabaseLocation::File(scratch.path().join("cyber.db")),
        Runtime::registry(),
    ))?);
    let host = BuiltinHost::new(HostOptions {
        store: Arc::clone(&store),
        tool_output_dir: scratch.path().join("tool-output"),
        allowed_dirs: Vec::new(),
        home: home.clone(),
        shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
        config: Arc::new(|_| Ok((json!({}), BTreeMap::new()))),
        global_config_dir: paths.config.clone(),
        env: Arc::new(ProcessEnv),
        models: Some(Arc::clone(&resolver)),
        temp_dir: scratch.path().join("tmp"),
        sandbox_policy: None,
        sandbox_helper: cyber_sandbox::find_helper(),
    });
    let snapshots =
        cyber_snapshot::GitSnapshots::new(scratch.path().join("data"), Arc::new(|_| json!({})));
    let runtime = Runtime::new(RuntimeOptions {
        store,
        resolver,
        tools: Arc::clone(&host) as Arc<dyn ToolHost>,
        global_config_dir: paths.config.clone(),
        shell: std::env::var("SHELL").unwrap_or_else(|_| "sh".into()),
        claude_compat: true,
        compaction: CompactionConfig::default(),
        retry: RetryPolicy::default(),
        max_steps: Some(40),
        today: None,
        interactive: true,
        snapshots,
    });
    host.attach(runtime.clone());

    let session = runtime
        .create_session(CreateSession {
            directory: directory.display().to_string(),
            model: model.clone(),
            mode: Some(mode),
            ..Default::default()
        })
        .await?;
    let mut live = runtime.subscribe();
    let message = runtime
        .admit(
            &session.id,
            Admission::text(prompt.clone(), Delivery::Steer),
        )
        .await?
        .message_id;
    let approver = runtime.clone();
    let printer = tokio::spawn(async move {
        while let Ok(event) = live.recv().await {
            match event {
                LiveEvent::TextDelta { text, .. } => {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
                LiveEvent::Durable { kind, .. }
                    if kind.starts_with("permission.asked")
                        || kind.starts_with("question.asked") =>
                {
                    answer(&approver).await;
                }
                LiveEvent::Durable { kind, .. } if kind.starts_with("session.tool.settled") => {
                    eprintln!("\n  [tool settled]")
                }
                LiveEvent::Error { kind, message, .. } => eprintln!("\n  error {kind}: {message}"),
                LiveEvent::Idle { .. } => break,
                _ => {}
            }
        }
    });
    runtime.wait_idle(&session.id).await;
    let _ = printer.await;

    let state = runtime.state(&session.id).await?;
    println!();
    for call in state.calls.values() {
        let output: String = call
            .output
            .clone()
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect();
        eprintln!(
            "call {} {:?}: {}",
            call.name,
            call.status,
            output.replace('\n', " | ")
        );
    }
    for diff in runtime.diff(&session.id, &message).await? {
        eprintln!(
            "diff {} {} +{} -{}",
            diff.status, diff.file, diff.additions, diff.deletions
        );
    }
    eprintln!(
        "steps: {}  cost: ${:.6}",
        state.totals.steps, state.totals.cost
    );
    Ok(())
}

/// Stand in for a user: approve permission requests once, dismiss questions.
async fn answer(runtime: &Runtime) {
    for request in runtime.pending_requests(None) {
        match &request.kind {
            PendingKind::Permission(ask) => {
                eprintln!("\n  [approve once] {} {:?}", ask.action, ask.resources);
                let _ = runtime
                    .reply_permission(&request.id, PermissionReply::Once)
                    .await;
            }
            PendingKind::Question { .. } => {
                let _ = runtime
                    .answer_question(&request.id, QuestionReply::Dismissed)
                    .await;
            }
        }
    }
}
