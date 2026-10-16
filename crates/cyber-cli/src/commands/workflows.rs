//! `cyber workflows` (`workflows` → Workflow CLI, Run monitor, Remote execution on runners).
//! `list` discovers saved Workflows of the current Location; the run commands talk to the
//! shared server through `cyber-client`, so runs execute in the server and outlive the
//! command. `run --wait` follows the run and exits 0 (completed), 1 (failed or stopped),
//! 4 (budget exceeded) or 130 (interrupted; the run is stopped).

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use clap::{Args, Subcommand};
use cyber_client::Client;
use cyber_core::env::EnvSource;
use cyber_workflow::{Listing, Origin, Scope, WorkflowModule};
use serde_json::{Map, Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

/// `run --wait` when the run exceeded its budget.
pub const EXIT_BUDGET: u8 = 4;
/// Milestone that ships remote runners (`ROADMAP.md` → P3).
const RUNNER_MILESTONE: &str = "P3 (remote runners)";
const POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Subcommand)]
pub enum WorkflowsCmd {
    /// List saved Workflows (project overrides global) and modules that fail to load.
    List,
    /// Start a Workflow Run from a saved name or a script file.
    Run(RunArgs),
    /// List recent Workflow Runs of this Location.
    Runs,
    /// Show one Workflow Run and its agents.
    Show { run_id: String },
    /// Pause a Workflow Run: in-flight agents finish, no new agent starts.
    Pause { run_id: String },
    /// Resume a paused, failed or stopped Workflow Run.
    Resume {
        run_id: String,
        /// Accept a changed script or configuration as a new revision.
        #[arg(long)]
        revision: bool,
    },
    /// Stop a Workflow Run and cancel its in-flight agents.
    Stop { run_id: String },
    /// Print a Workflow Run's log.
    Logs { run_id: String },
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// A saved Workflow's name, or the path of a `.js` module.
    pub target: String,
    /// The script's `args` as a JSON object.
    #[arg(long, value_name = "JSON")]
    pub args: Option<String>,
    /// Stop starting agents once the run cost this much (USD).
    #[arg(long, value_name = "USD")]
    pub max_cost: Option<f64>,
    #[arg(long, value_name = "N")]
    pub max_turns: Option<u64>,
    #[arg(long, value_name = "N")]
    pub max_tokens: Option<u64>,
    /// Wall-clock limit of the run, e.g. 30m or 90s.
    #[arg(long, value_name = "DURATION")]
    pub timeout: Option<String>,
    #[arg(long, value_name = "N")]
    pub max_agents: Option<u64>,
    /// Run on a remote runner (not available yet).
    #[arg(long, value_name = "ID")]
    pub runner: Option<String>,
    /// Follow the run until it ends; the exit code reports how it ended.
    #[arg(long)]
    pub wait: bool,
}

pub fn run(cmd: WorkflowsCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    if let WorkflowsCmd::List = cmd {
        return list(&discover(ctx), global);
    }
    if let WorkflowsCmd::Run(args) = &cmd {
        // Refused before any server starts.
        if args.runner.is_some() {
            return Err(CliError::unavailable(
                "`cyber workflows run --runner`",
                RUNNER_MILESTONE,
            ));
        }
    }
    let rt = super::serve::runtime()?;
    rt.block_on(async {
        let info = super::serve::start(ctx).await?;
        let client = Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string());
        remote(cmd, &client, ctx, global).await
    })
}

async fn remote(
    cmd: WorkflowsCmd,
    client: &Client,
    ctx: &Context,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    match cmd {
        WorkflowsCmd::List => unreachable!("listed locally"),
        WorkflowsCmd::Run(args) => start(client, ctx, global, args).await,
        WorkflowsCmd::Runs => runs(client, ctx, global).await,
        WorkflowsCmd::Show { run_id } => show(client, global, &run_id).await,
        WorkflowsCmd::Pause { run_id } => control(client, global, &run_id, "pause", None).await,
        WorkflowsCmd::Resume { run_id, revision } => {
            let body = json!({"revision": revision});
            control(client, global, &run_id, "resume", Some(body)).await
        }
        WorkflowsCmd::Stop { run_id } => control(client, global, &run_id, "stop", None).await,
        WorkflowsCmd::Logs { run_id } => logs(client, global, &run_id).await,
    }
}

fn api(error: cyber_client::ClientError) -> CliError {
    match error.tag() {
        Some("InvalidRequestError") => CliError::usage(error.to_string()),
        _ => CliError::runtime(error.to_string()),
    }
}

/// The run budget fields of the flags.
fn budget(args: &RunArgs) -> Result<Value, CliError> {
    let mut budget = Map::new();
    if let Some(cost) = args.max_cost {
        budget.insert("max_cost_usd".into(), json!(cost));
    }
    if let Some(turns) = args.max_turns {
        budget.insert("max_turns".into(), json!(turns));
    }
    if let Some(tokens) = args.max_tokens {
        budget.insert("max_tokens".into(), json!(tokens));
    }
    if let Some(agents) = args.max_agents {
        budget.insert("max_agents".into(), json!(agents));
    }
    if let Some(timeout) = &args.timeout {
        let limit = super::exec::parse_duration(timeout)?;
        budget.insert("max_wall_seconds".into(), json!(limit.as_secs_f64()));
    }
    Ok(Value::Object(budget))
}

/// The start request body: a saved name, or a module file's source as an inline script.
fn start_body(ctx: &Context, args: &RunArgs, session: &str) -> Result<Value, CliError> {
    let script_args = match &args.args {
        None => json!({}),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(value @ Value::Object(_)) => value,
            _ => return Err(CliError::usage("--args must be a JSON object")),
        },
    };
    let mut body = json!({"session_id": session, "args": script_args, "budget": budget(args)?});
    let path = ctx.location.join(&args.target);
    if looks_like_file(&args.target) || path.is_file() {
        let source = std::fs::read_to_string(&path).map_err(|error| {
            CliError::usage(format!("could not read {}: {error}", path.display()))
        })?;
        body["script"] = source.into();
    } else {
        body["name"] = args.target.clone().into();
    }
    Ok(body)
}

fn looks_like_file(target: &str) -> bool {
    let path = Path::new(target);
    path.extension().is_some() || path.components().count() > 1
}

async fn start(
    client: &Client,
    ctx: &Context,
    global: &GlobalArgs,
    args: RunArgs,
) -> Result<(), CliError> {
    let session = client
        .post(
            "/sessions",
            json!({
                "title": format!("Workflow: {}", args.target),
                "model": global.model,
                "mode": global.mode.clone().unwrap_or_else(|| "default".into()),
            }),
        )
        .await
        .map_err(api)?;
    let session = session["data"]["id"]
        .as_str()
        .ok_or_else(|| CliError::runtime("the server returned no Session"))?
        .to_owned();
    let body = start_body(ctx, &args, &session)?;
    let started = client.post("/workflows/runs", body).await.map_err(api)?;
    let run = &started["data"];
    let id = run["id"].as_str().unwrap_or_default().to_owned();
    if !args.wait {
        if output::is_json(global.format) {
            return output::json(run);
        }
        println!("{id}");
        return Ok(());
    }
    if !output::is_json(global.format) {
        eprintln!(
            "started {} ({id})",
            run["name"].as_str().unwrap_or("workflow")
        );
    }
    wait(client, global, &id).await
}

/// Follow the run until it ends, reporting phase and agent changes on stderr. Ctrl-C
/// stops the run, whether it arrives during a poll or between polls.
async fn wait(client: &Client, global: &GlobalArgs, id: &str) -> Result<(), CliError> {
    let path = format!("/workflows/runs/{id}");
    let mut progress = Progress::default();
    // One listener for the whole wait: a signal between two polls is not lost.
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    loop {
        let detail = tokio::select! {
            biased;
            _ = &mut interrupt => return interrupted(client, &path, id).await,
            detail = client.get(&path) => detail.map_err(api)?,
        };
        let run = &detail["data"];
        if !output::is_json(global.format) {
            progress.report(run);
        }
        if let Some(code) = ended(run) {
            report(global, run)?;
            return match code {
                0 => Ok(()),
                code => Err(CliError::silent(code)),
            };
        }
        tokio::select! {
            biased;
            _ = &mut interrupt => return interrupted(client, &path, id).await,
            () = tokio::time::sleep(POLL) => {}
        }
    }
}

/// Stop the run after Ctrl-C and exit 130.
async fn interrupted(client: &Client, path: &str, id: &str) -> Result<(), CliError> {
    let _ = client.post(&format!("{path}/stop"), json!({})).await;
    eprintln!("interrupted; stopped {id}");
    Err(CliError::silent(super::exec::EXIT_INTERRUPTED))
}

/// The exit code of an ended run; `None` while it may still run.
fn ended(run: &Value) -> Option<u8> {
    match run["status"].as_str()? {
        "completed" => Some(0),
        "budget_exceeded" => Some(EXIT_BUDGET),
        "failed" | "stopped" | "paused" => Some(crate::error::EXIT_RUNTIME),
        _ => None,
    }
}

/// What `--wait` already printed.
#[derive(Default)]
struct Progress {
    phase: Option<String>,
    /// Agents whose end was printed, by ID: agents can settle in any order.
    settled: HashSet<String>,
}

impl Progress {
    fn report(&mut self, run: &Value) {
        for line in self.changes(run) {
            eprintln!("{line}");
        }
    }

    /// New progress lines since the last poll. A phase change is printed after the agents
    /// of earlier phases that settled in the same poll and before those of the new phase.
    fn changes(&mut self, run: &Value) -> Vec<String> {
        let phase = run["phase"].as_str().map(str::to_owned);
        let agents = run["agents"].as_array().map_or(&[][..], Vec::as_slice);
        let (current, earlier): (Vec<&Value>, Vec<&Value>) = agents
            .iter()
            .filter(|agent| agent["status"] != "running")
            .filter(|agent| {
                let id = agent["id"].as_str().unwrap_or_default();
                self.settled.insert(id.to_owned())
            })
            .partition(|agent| phase.is_some() && agent["phase"].as_str() == phase.as_deref());
        let mut lines: Vec<String> = earlier.into_iter().map(settled_line).collect();
        if phase.is_some() && phase != self.phase {
            lines.push(format!("phase: {}", phase.as_deref().unwrap_or_default()));
            self.phase = phase;
        }
        lines.extend(current.into_iter().map(settled_line));
        lines
    }
}

fn settled_line(agent: &Value) -> String {
    let label = agent["label"].as_str().or(agent["id"].as_str());
    let status = agent["status"].as_str().unwrap_or("?");
    format!("agent {} {status}", label.unwrap_or("?"))
}

/// `<kind>: <message>`; messages that already name their kind (`BudgetExceeded: max_tokens`)
/// are not prefixed twice.
fn error_text(error: &Value) -> String {
    let kind = error["kind"].as_str().unwrap_or("?");
    let message = error["message"].as_str().unwrap_or("?");
    match message.strip_prefix(kind) {
        Some(rest) if rest.starts_with(": ") => message.to_owned(),
        _ => format!("{kind}: {message}"),
    }
}

fn report(global: &GlobalArgs, run: &Value) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(run);
    }
    let status = run["status"].as_str().unwrap_or("?");
    eprintln!("{} {status}", run["id"].as_str().unwrap_or("?"));
    if let Some(error) = run.get("error").filter(|e| !e.is_null()) {
        eprintln!("error: {}", error_text(error));
    }
    if let Some(reason) = run["reason"].as_str() {
        eprintln!("reason: {reason}");
    }
    if let Some(result) = run.get("result").filter(|r| !r.is_null()) {
        println!(
            "{}",
            serde_json::to_string_pretty(result).unwrap_or_default()
        );
    } else if let Some(file) = run["result_file"].as_str() {
        println!("result: {file}");
    }
    Ok(())
}

async fn runs(client: &Client, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let listed = client.get("/workflows/runs").await.map_err(api)?;
    let location = ctx.location.display().to_string();
    let runs: Vec<&Value> = listed["data"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(|run| same_location(run["directory"].as_str(), &location))
        .collect();
    if output::is_json(global.format) {
        return output::json(&runs);
    }
    if runs.is_empty() {
        println!("No Workflow Runs in this Location.");
    }
    for run in runs {
        println!(
            "{}  {:16}  {:15}  {}",
            run["id"].as_str().unwrap_or("?"),
            run["name"].as_str().unwrap_or("?"),
            run["status"].as_str().unwrap_or("?"),
            run["phase"].as_str().unwrap_or("-"),
        );
    }
    Ok(())
}

/// Locations compare canonically, so `\\?\` and symlinked spellings match.
fn same_location(directory: Option<&str>, location: &str) -> bool {
    let canonical = |path: &str| std::fs::canonicalize(path).unwrap_or_else(|_| path.into());
    directory.is_some_and(|directory| canonical(directory) == canonical(location))
}

async fn show(client: &Client, global: &GlobalArgs, id: &str) -> Result<(), CliError> {
    let detail = client
        .get(&format!("/workflows/runs/{id}"))
        .await
        .map_err(api)?;
    let run = &detail["data"];
    if output::is_json(global.format) {
        return output::json(run);
    }
    let agents = run["agents"].as_array().map_or(&[][..], Vec::as_slice);
    let count = |status: &str| agents.iter().filter(|a| a["status"] == status).count();
    output::rows(&[
        ("run", text(&run["id"])),
        ("name", text(&run["name"])),
        ("status", text(&run["status"])),
        ("phase", text(&run["phase"])),
        ("session", text(&run["session_id"])),
        (
            "agents",
            format!(
                "{} done, {} running, {} failed",
                count("completed"),
                count("running"),
                count("failed")
            ),
        ),
        (
            "cost",
            format!(
                "{:.4} USD",
                agents
                    .iter()
                    .filter_map(|a| a["cost"].as_f64())
                    .sum::<f64>()
            ),
        ),
        ("script", text(&run["script_file"])),
    ]);
    for agent in agents {
        println!(
            "  {}  {:10}  {}",
            text(&agent["id"]),
            text(&agent["status"]),
            agent["label"]
                .as_str()
                .or(agent["branch"].as_str())
                .unwrap_or("")
        );
    }
    Ok(())
}

fn text(value: &Value) -> String {
    value.as_str().map_or_else(|| "-".into(), str::to_owned)
}

async fn control(
    client: &Client,
    global: &GlobalArgs,
    id: &str,
    action: &str,
    body: Option<Value>,
) -> Result<(), CliError> {
    let run = client
        .post(
            &format!("/workflows/runs/{id}/{action}"),
            body.unwrap_or_else(|| json!({})),
        )
        .await
        .map_err(api)?;
    let run = &run["data"];
    if output::is_json(global.format) {
        return output::json(run);
    }
    println!("{} {}", text(&run["id"]), text(&run["status"]));
    Ok(())
}

async fn logs(client: &Client, global: &GlobalArgs, id: &str) -> Result<(), CliError> {
    let logs = client
        .get(&format!("/workflows/runs/{id}/logs"))
        .await
        .map_err(api)?;
    let entries = logs["data"].as_array().map_or(&[][..], Vec::as_slice);
    if output::is_json(global.format) {
        return output::json(&entries);
    }
    for entry in entries {
        let values: Vec<String> = entry["values"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|value| match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect();
        println!("{}", values.join(" "));
    }
    Ok(())
}

/// Saved Workflows of the Location: global, then project directories from the project root
/// down to the Location. `CYBER_DISABLE_PROJECT_CONFIG` skips project directories.
pub fn discover(ctx: &Context) -> Listing {
    let project_root = cyber_core::config::project_root(&ctx.location);
    let project = (!ctx.env.flag("CYBER_DISABLE_PROJECT_CONFIG"))
        .then_some((ctx.location.as_path(), project_root.as_path()));
    cyber_workflow::discover(&cyber_workflow::roots(&ctx.paths.config, project))
}

fn scope_name(module: &WorkflowModule) -> &'static str {
    match module.origin {
        Origin::Saved {
            scope: Scope::Project,
            ..
        } => "project",
        Origin::Saved {
            scope: Scope::Global,
            ..
        } => "global",
        Origin::Inline => "inline",
    }
}

fn view(listing: &Listing) -> Value {
    let workflows: Vec<Value> = listing
        .workflows
        .iter()
        .map(|entry| {
            let module = &entry.module;
            let path = match &module.origin {
                Origin::Saved { path, .. } => json!(path),
                Origin::Inline => Value::Null,
            };
            json!({
                "name": module.meta.name,
                "description": module.meta.description,
                "phases": module.meta.phases,
                "args": module.meta.args,
                "scope": scope_name(module),
                "path": path,
                "digest": module.digest,
                "shadowed": entry.shadowed,
            })
        })
        .collect();
    json!({ "workflows": workflows, "issues": listing.issues })
}

fn list(listing: &Listing, global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(&view(listing));
    }
    if listing.workflows.is_empty() && listing.issues.is_empty() {
        println!(
            "No saved workflows. Add modules to .cyber/workflows/ or ~/.config/cyber/workflows/."
        );
    }
    let width = listing
        .workflows
        .iter()
        .map(|e| e.module.meta.name.len())
        .max()
        .unwrap_or(0);
    for entry in &listing.workflows {
        let meta = &entry.module.meta;
        println!(
            "{:width$}  [{}]  {}",
            meta.name,
            scope_name(&entry.module),
            meta.description
        );
    }
    for issue in &listing.issues {
        println!("issue: {}: {}", issue.path.display(), issue.diagnostic);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ended_runs_map_to_exit_codes() {
        let code = |status: &str| ended(&json!({"status": status}));
        assert_eq!(code("completed"), Some(0));
        assert_eq!(code("failed"), Some(1));
        assert_eq!(code("stopped"), Some(1));
        assert_eq!(code("budget_exceeded"), Some(EXIT_BUDGET));
        assert_eq!(code("running"), None);
        assert_eq!(code("queued"), None);
    }

    #[test]
    fn progress_prints_each_settled_agent_once_in_phase_order() {
        let mut progress = Progress::default();
        let run = |phase: &str, a: &str, b: &str| {
            json!({"phase": phase, "agents": [
                {"id": "agt_a", "label": "a", "phase": "Scan", "status": a},
                {"id": "agt_b", "label": "b", "phase": "Scan", "status": b},
            ]})
        };
        assert_eq!(
            progress.changes(&run("Scan", "running", "completed")),
            ["phase: Scan", "agent b completed"]
        );
        // The earlier agent settles later and is printed once; the next phase started
        // after it, so it follows it.
        assert_eq!(
            progress.changes(&run("Rank", "failed", "completed")),
            ["agent a failed", "phase: Rank"]
        );
        assert!(
            progress
                .changes(&run("Rank", "failed", "completed"))
                .is_empty()
        );
    }

    #[test]
    fn run_errors_name_their_kind_once() {
        let text =
            |kind: &str, message: &str| error_text(&json!({"kind": kind, "message": message}));
        assert_eq!(
            text("BudgetExceeded", "BudgetExceeded: max_tokens"),
            "BudgetExceeded: max_tokens"
        );
        assert_eq!(text("ExecutionFailure", "boom"), "ExecutionFailure: boom");
        assert_eq!(
            text("AgentFailed", "AgentFailedly: odd"),
            "AgentFailed: AgentFailedly: odd"
        );
    }

    #[test]
    fn targets_with_a_path_shape_are_files() {
        assert!(looks_like_file("audit.js"));
        assert!(looks_like_file("scripts/audit"));
        assert!(!looks_like_file("audit"));
    }
}
