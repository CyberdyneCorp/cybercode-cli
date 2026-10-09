//! Local synthetic tests; no application startup, Session owners or model calls.
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use cyber_core::env::{EnvSource, ProcessEnv};
use cyber_core::hooks::{HookEvent, HookLocation};
use cyber_server::runtime::Runtime;
use cyber_store::{Store, StoreOptions};
use cyber_tools::hook_commands::{HookCommandRunner, HookTestRun};
use cyber_tools::{BuiltinHost, HostOptions};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::{CliError, EXIT_RUNTIME};
use crate::output;

const PAYLOAD_LIMIT: u64 = 1024 * 1024;

pub(super) fn run(
    event: &str,
    path: Option<&Path>,
    ctx: &Context,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let payload = payload(path)?;
    let resolved = ctx.config()?;
    let timestamp = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| CliError::runtime(error.to_string()))?
            .as_millis(),
    )
    .map_err(|error| CliError::runtime(error.to_string()))?;
    let event = HookEvent::synthetic(
        event,
        HookLocation {
            directory: ctx.location.clone(),
            workspace: None,
        },
        "global".into(),
        resolved.value["default_agent"]
            .as_str()
            .unwrap_or("build")
            .into(),
        resolved.value["mode"].as_str().unwrap_or("default").into(),
        timestamp,
        payload,
    )
    .map_err(CliError::usage)?;
    let store = Arc::new(Store::open(StoreOptions::new(
        ctx.database(),
        Runtime::registry(),
    ))?);
    let shell = resolved.value["shell"]
        .as_str()
        .map(str::to_string)
        .or_else(|| ctx.env.get("SHELL"))
        .unwrap_or_else(|| "/bin/sh".into());
    let value = resolved.value.clone();
    let sources = resolved.sources.clone();
    let helper = cyber_sandbox::find_helper();
    let temp = ctx.paths.tmp.join("hook-tests");
    let host = BuiltinHost::new(HostOptions {
        store,
        tool_output_dir: ctx.paths.data.join("tool-output"),
        allowed_dirs: Vec::new(),
        home: ctx.home.clone(),
        shell: shell.clone(),
        config: Arc::new(move |_| Ok((value.clone(), sources.clone()))),
        global_config_dir: ctx.paths.config.clone(),
        env: Arc::new(ProcessEnv),
        models: None,
        temp_dir: temp.clone(),
        sandbox_policy: None,
        sandbox_helper: helper.clone(),
    });
    let trust = cyber_core::trust::TrustStore::new(ctx.paths.trust_file());
    let runner = HookCommandRunner {
        resolved: &resolved,
        trust: &trust,
        invocation_trust: None,
        home: &ctx.home,
        temp_dir: &temp,
        shell: &shell,
        helper: helper.as_deref(),
        credential_env_names: &[],
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime
        .block_on(async {
            let cancel = CancellationToken::new();
            let notice = |id: &str, message: &str| {
                eprintln!("hook {}: {}", id.escape_debug(), message.escape_debug())
            };
            let execution = host.test_hooks(&runner, event, cancel.clone(), &notice);
            tokio::pin!(execution);
            tokio::select! {
                biased;
                _ = tokio::signal::ctrl_c() => {
                    cancel.cancel();
                    execution.await
                }
                result = &mut execution => result,
            }
        })
        .map_err(CliError::runtime)?;
    print(&result, global)?;
    if !result.complete {
        return Err(CliError::silent(EXIT_RUNTIME));
    }
    Ok(())
}

fn payload(path: Option<&Path>) -> Result<Value, CliError> {
    let Some(path) = path else {
        return Ok(json!({}));
    };
    if !std::fs::metadata(path)?.is_file() {
        return Err(CliError::usage("hook payload must be a regular JSON file"));
    }
    let file = std::fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(CliError::usage("hook payload must be a regular JSON file"));
    }
    let mut bytes = Vec::new();
    file.take(PAYLOAD_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > PAYLOAD_LIMIT {
        return Err(CliError::usage("hook payload exceeds the 1 MiB limit"));
    }
    let payload: Value = serde_json::from_slice(&bytes)
        .map_err(|error| CliError::usage(format!("invalid hook payload JSON: {error}")))?;
    if !payload.is_object() {
        return Err(CliError::usage("hook payload must be a JSON object"));
    }
    Ok(payload)
}

fn print(run: &HookTestRun, global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(run);
    }
    println!(
        "Synthetic {} invocation {}",
        run.event.escape_debug(),
        run.invocation_id.escape_debug()
    );
    for result in &run.results {
        println!(
            "{} {:?} {:?}: {:?}",
            result.hook_id.escape_debug(),
            result.scope,
            result.kind,
            result.report.outcome
        );
        println!(
            "  decision: {}",
            serde_json::to_string(&result.report.decision)
                .unwrap()
                .escape_debug()
        );
        if let Some(diagnostic) = &result.report.diagnostic {
            println!("  {}", diagnostic.escape_debug());
        }
        println!(
            "  acknowledged={} must_stop={}",
            result.report.acknowledged, result.report.must_stop
        );
    }
    for path in &run.withheld_definitions {
        println!("withheld checkout configuration: {}", path.escape_debug());
    }
    println!(
        "Merged decision: {}",
        serde_json::to_string(&run.decision).unwrap().escape_debug()
    );
    Ok(())
}
