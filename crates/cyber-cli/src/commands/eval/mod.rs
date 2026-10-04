//! `cyber eval run` (`harness-evaluation` → Coding quality and cost measures, Live quality
//! release baseline): run coding tasks in disposable fixtures, grade final workspace state
//! with hidden graders, and write a versioned report.

mod report;
mod trial;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Args, Subcommand};
use cyber_app::{App, AppOptions};
use cyber_client::Client;
use cyber_core::eval::{self, Manifest, ManifestKind, Task};
use futures::StreamExt;
use serde_json::Value;

use crate::context::Context;
use crate::error::CliError;

#[derive(Debug, Subcommand)]
pub enum EvalCmd {
    /// Run a coding manifest and write a report.
    Run(RunArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Manifest path, e.g. eval/manifests/suite-coding-v1.json.
    pub manifest: PathBuf,
    /// Directory for report.json, report.md and per-trial logs.
    #[arg(long)]
    pub output: PathBuf,
    /// Model as provider/model[#variant]; required for suites, overrides live manifests.
    #[arg(short = 'm', long)]
    pub model: Option<String>,
    /// Trials per task (the release baseline uses 3).
    #[arg(long, default_value_t = 3)]
    pub trials: u32,
    /// Only these task IDs (repeatable).
    #[arg(long = "task")]
    pub tasks: Vec<String>,
    /// Trials run at the same time.
    #[arg(long, default_value_t = 4)]
    pub jobs: usize,
    /// Keep trial workspaces under <output>/workspaces for inspection.
    #[arg(long)]
    pub keep_workspaces: bool,
}

pub fn run(cmd: EvalCmd, ctx: &Context) -> Result<(), CliError> {
    let EvalCmd::Run(args) = cmd;
    let root = repo_root(&args.manifest)?;
    let manifest = eval::load_manifest(&args.manifest).map_err(CliError::usage)?;
    let issues = eval::validate(&manifest, &root);
    if !issues.is_empty() {
        return Err(CliError::usage(format!(
            "invalid manifest:\n  {}",
            issues.join("\n  ")
        )));
    }
    if manifest.kind == ManifestKind::Recovery {
        return Err(CliError::usage(
            "recovery manifests run as tests: cargo test (see eval/README.md)",
        ));
    }
    let model = args
        .model
        .clone()
        .or_else(|| {
            manifest
                .model
                .as_ref()
                .map(|m| format!("{}/{}", m.provider, m.id))
        })
        .ok_or_else(|| CliError::usage("suites need --model provider/model"))?;
    let tasks: Vec<Task> = manifest
        .tasks
        .iter()
        .filter(|t| args.tasks.is_empty() || args.tasks.contains(&t.id))
        .cloned()
        .collect();
    if tasks.is_empty() {
        return Err(CliError::usage("no tasks match --task"));
    }
    std::fs::create_dir_all(args.output.join("trials"))?;
    let code =
        super::serve::runtime()?.block_on(execute(ctx, &args, &manifest, &root, tasks, model))?;
    if code == 0 {
        Ok(())
    } else {
        Err(CliError::silent(code))
    }
}

/// The repository root: the closest ancestor of the manifest holding `eval/`.
fn repo_root(manifest: &Path) -> Result<PathBuf, CliError> {
    let abs = std::fs::canonicalize(manifest)
        .map_err(|_| CliError::usage(format!("{}: no such manifest", manifest.display())))?;
    abs.ancestors()
        .find(|d| d.join("eval").is_dir() && d.join("eval/manifests").is_dir())
        .map(Path::to_path_buf)
        .ok_or_else(|| CliError::usage("the manifest must live under <repo>/eval/manifests"))
}

async fn execute(
    ctx: &Context,
    args: &RunArgs,
    manifest: &Manifest,
    root: &Path,
    tasks: Vec<Task>,
    model: String,
) -> Result<u8, CliError> {
    let app = App::build(AppOptions {
        paths: ctx.paths.clone(),
        home: ctx.home.clone(),
        database: cyber_core::paths::DatabaseLocation::Memory,
        default_directory: ctx.location.clone(),
        sandbox_policy: None,
        snapshots: false,
        interactive: false,
        password: None,
    })
    .await
    .map_err(CliError::runtime)?;
    let client = Client::embedded(app.embedded());
    let model_info = model_info(&client, &model).await?;
    let started = report::now();
    let plan: Vec<(Task, u32)> = tasks
        .iter()
        .flat_map(|t| (1..=args.trials).map(move |n| (t.clone(), n)))
        .collect();
    let total = plan.len();
    let env = Arc::new(trial::TrialEnv {
        client,
        root: root.to_path_buf(),
        manifest: manifest.clone(),
        model: model.clone(),
        output: args.output.clone(),
        keep: args.keep_workspaces,
    });
    let results: Vec<trial::TrialResult> = futures::stream::iter(plan.into_iter().enumerate())
        .map(|(i, (task, n))| {
            let env = Arc::clone(&env);
            async move {
                let result = trial::run(&env, &task, n).await;
                eprintln!("[{}/{total}] {} #{n}: {}", i + 1, task.id, result.verdict());
                result
            }
        })
        .buffer_unordered(args.jobs.max(1))
        .collect()
        .await;
    let report = report::build(
        manifest,
        &args.manifest,
        &model_info,
        args.trials,
        started,
        &tasks,
        results,
    );
    report::write(&args.output, &report)?;
    eprintln!("{}", report::headline(&report));
    eprintln!("report: {}", args.output.join("report.json").display());
    Ok(0)
}

/// Catalog metadata for the report; the model must be available.
async fn model_info(client: &Client, model: &str) -> Result<Value, CliError> {
    let base = model.split('#').next().unwrap_or(model);
    let list = client
        .get("/models")
        .await
        .map_err(|e| CliError::runtime(e.to_string()))?;
    let found = list["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|m| m["id"] == base)
        .cloned();
    match found {
        Some(m) if m["available"] == true => Ok(
            serde_json::json!({ "ref": model, "provider": m["provider"], "name": m["name"], "context_limit": m["context_limit"] }),
        ),
        Some(_) => Err(CliError::usage(format!(
            "{base} is not available: missing credentials?"
        ))),
        None => Err(CliError::usage(format!("unknown model {base}"))),
    }
}
