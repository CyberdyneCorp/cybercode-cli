//! Synthetic tests without application startup or Session ownership.
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
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let trust = cyber_core::trust::TrustStore::new(ctx.paths.trust_file());
    let needs_models = cyber_core::hooks::HookCatalog::from_config(&resolved)
        .map_err(CliError::runtime)?
        .definitions
        .iter()
        .any(|definition| {
            definition.event == event.event()
                && definition.kind() == cyber_core::config::HookKind::Prompt
                && !definition.handler.asynchronous
                && definition
                    .is_trusted(&resolved.trust.checkout_root, &trust, None)
                    .unwrap_or(false)
        });
    let models: Option<Arc<dyn cyber_server::runtime::ModelResolver>> = if needs_models {
        let mut source = cyber_llm::catalog::SourceOptions::from_env(&ctx.env, &ctx.paths.cache);
        source.allow_fetch = false;
        runtime
            .block_on(cyber_llm::catalog::load_source(&source))
            .ok()
            .map(|loaded| {
                Arc::new(cyber_server::runtime::CatalogResolver::new(
                    &loaded.data,
                    resolved.value.clone(),
                    &ctx.env,
                )) as Arc<dyn cyber_server::runtime::ModelResolver>
            })
    } else {
        None
    };
    let credential_env_names = models
        .as_ref()
        .map_or_else(Vec::new, |models| models.credential_env_names());
    let host = BuiltinHost::new(HostOptions {
        store,
        tool_output_dir: ctx.paths.data.join("tool-output"),
        allowed_dirs: Vec::new(),
        home: ctx.home.clone(),
        shell: shell.clone(),
        config: Arc::new(move |_| Ok((value.clone(), sources.clone()))),
        global_config_dir: ctx.paths.config.clone(),
        env: Arc::new(ProcessEnv),
        models,
        temp_dir: temp.clone(),
        sandbox_policy: None,
        sandbox_helper: helper.clone(),
    });
    host.attach_hook_config(ctx.hook_resolver(), trust.clone())
        .map_err(CliError::runtime)?;
    let runner = HookCommandRunner {
        resolved: &resolved,
        trust: &trust,
        invocation_trust: None,
        home: &ctx.home,
        temp_dir: &temp,
        shell: &shell,
        helper: helper.as_deref(),
        credential_env_names: &credential_env_names,
    };
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
