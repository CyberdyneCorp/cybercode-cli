//! `cyber eval run` (`harness-evaluation` → Coding quality and cost measures, Live quality
//! release baseline): run coding tasks in disposable fixtures, grade final workspace state
//! with hidden graders, and write a versioned report.

mod report;
mod repositories;
mod solve;
mod trial;
mod verification;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Args, Subcommand, ValueEnum};
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
    /// Download verified pinned source trees into a new isolated evaluation root.
    Prepare(repositories::PrepareArgs),
}

#[derive(Debug, Clone, Copy, Default, ValueEnum, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    #[default]
    Single,
    Solve,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Evaluate one Session or the separately graded bundled solve workflow.
    #[arg(long, value_enum, default_value = "single")]
    pub strategy: Strategy,
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
    /// First trial identity, for predetermined interleaving across strategies.
    #[arg(long, default_value_t = 1)]
    pub trial_start: u32,
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
    let args = match cmd {
        EvalCmd::Run(args) => args,
        EvalCmd::Prepare(args) => return repositories::prepare(args),
    };
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
    if args.trials == 0
        || args.trial_start == 0
        || args.trial_start.checked_add(args.trials).is_none()
    {
        return Err(CliError::usage(
            "trial count and first identity must be positive and fit u32",
        ));
    }
    if matches!(args.strategy, Strategy::Solve) && tasks.iter().any(|t| t.checks.is_empty()) {
        return Err(CliError::usage(
            "solve evaluation requires public literal-argv checks on every task",
        ));
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
    let repository_run = tasks
        .iter()
        .any(|t| eval::task_fixture(manifest, t).is_some_and(|f| f.repository.is_some()));
    if repository_run {
        let (value, sources) =
            cyber_app::config_loader(ctx.paths.clone(), ctx.home.clone())(&ctx.location)
                .map_err(CliError::runtime)?;
        let sandbox = cyber_sandbox::SandboxConfig::resolve(
            &value,
            &sources,
            Some("workspace-write"),
            &ctx.home,
        );
        if sandbox.network != cyber_sandbox::NetworkMode::Off {
            return Err(CliError::usage(
                "real-repository trials require an isolated global configuration with sandbox.network = off",
            ));
        }
        let output = std::fs::canonicalize(&args.output)?;
        if output.starts_with(root) {
            return Err(CliError::usage(
                "real-repository trial output must be outside the private evaluation input root",
            ));
        }
        // Model file tools and native commands share this non-overridable read ceiling.
        // Host-side import and independent grading do not use model tool permissions.
        cyber_sandbox::hide_credential_file(&ctx.home, root.to_path_buf());
        for name in ["trials", "report.json", "report.md"] {
            cyber_sandbox::hide_credential_file(&ctx.home, output.join(name));
        }
    }
    let app = App::build(AppOptions {
        paths: ctx.paths.clone(),
        home: ctx.home.clone(),
        database: cyber_core::paths::DatabaseLocation::Memory,
        default_directory: ctx.location.clone(),
        sandbox_policy: repository_run.then(|| "workspace-write".into()),
        snapshots: false,
        interactive: false,
        password: None,
        mcp_credentials: None,
        remote_skill_fetcher: None,
        workflow_cores: None,
        isolation: None,
    })
    .await
    .map_err(CliError::runtime)?;
    let client = Client::embedded(app.embedded());
    let model_info = model_info(&client, &model).await?;
    let started = report::now();
    let plan: Vec<(Task, u32)> = tasks
        .iter()
        .flat_map(|t| {
            (args.trial_start..args.trial_start + args.trials).map(move |n| (t.clone(), n))
        })
        .collect();
    let total = plan.len();
    let env = Arc::new(trial::TrialEnv {
        client,
        root: root.to_path_buf(),
        manifest: manifest.clone(),
        model: model.clone(),
        output: args.output.clone(),
        keep: args.keep_workspaces,
        strategy: args.strategy,
        home: ctx.home.clone(),
        private_roots: vec![ctx.paths.data.clone(), ctx.paths.config.clone()],
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
    let mut report = report::build(
        manifest,
        &args.manifest,
        &model_info,
        args.trials,
        started,
        &tasks,
        results,
    );
    report["strategy"] = serde_json::to_value(args.strategy).expect("strategy serializes");
    if repository_run {
        report["options"]["command_network"] = "off".into();
        report["options"]["private_inputs_hidden"] = true.into();
    }
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
