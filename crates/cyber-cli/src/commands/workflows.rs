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
use serde_json::{Map, Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

mod library;
mod runs;

/// `run --wait` when the run exceeded its budget.
pub const EXIT_BUDGET: u8 = 4;
/// Milestone that ships remote runners (`ROADMAP.md` → P3).
const RUNNER_MILESTONE: &str = "P3 (remote runners)";
const POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Subcommand)]
pub enum WorkflowsCmd {
    /// List the Workflow library (project overrides global, both override bundled) and
    /// modules that fail to load.
    List,
    /// Copy a bundled Workflow's source into `.cyber/workflows/` (or the global
    /// directory) to customize it; the copy then overrides the bundled one.
    Eject(library::EjectArgs),
    /// Start a Workflow Run from a saved name or a script file.
    Run(RunArgs),
    /// List recent Workflow Runs of this Location: active runs first, then the newest.
    Runs {
        /// Every Location's runs.
        #[arg(long)]
        all: bool,
        /// Only runs of these statuses, e.g. `running,queued`.
        #[arg(long, value_name = "S[,S...]")]
        status: Option<String>,
        /// Keep the list current until Ctrl-C, which changes no run.
        #[arg(long)]
        watch: bool,
    },
    /// Show one Workflow Run and its agents (`run_…`), or a library Workflow's meta, args
    /// schema, default budget and origin (a Workflow name).
    Show {
        /// A run ID, or a Workflow name.
        target: String,
        /// With a Workflow name, also print the module source.
        #[arg(long)]
        source: bool,
    },
    /// Pause a Workflow Run: in-flight agents finish, no new agent starts.
    Pause { run_id: String },
    /// Resume a paused, failed or stopped Workflow Run, or with `--interrupted` every run a
    /// server restart interrupted.
    Resume {
        #[arg(required_unless_present = "interrupted")]
        run_id: Option<String>,
        /// Accept a changed script or configuration as a new revision.
        #[arg(long, conflicts_with = "interrupted")]
        revision: bool,
        /// Resume every run of this Location that a server restart interrupted; runs
        /// paused by a user stay paused.
        #[arg(long, conflicts_with = "run_id")]
        interrupted: bool,
        /// With `--interrupted`, every Location's interrupted runs.
        #[arg(long, requires = "interrupted")]
        all: bool,
    },
    /// Stop a Workflow Run and cancel its in-flight agents.
    Stop { run_id: String },
    /// Print a Workflow Run's log.
    Logs { run_id: String },
}

#[derive(Debug, Default, Args)]
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
    /// The run's share of the server's Workflow agent slots (at most N agents at once).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub max_concurrent: Option<u32>,
    /// Run on a remote runner (not available yet).
    #[arg(long, value_name = "ID")]
    pub runner: Option<String>,
    /// Follow the run until it ends; the exit code reports how it ended.
    #[arg(long)]
    pub wait: bool,
}

pub fn run(cmd: WorkflowsCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    match &cmd {
        WorkflowsCmd::List => return library::list(ctx, global),
        WorkflowsCmd::Show { target, source } if cyber_workflow::meta::valid_name(target) => {
            return library::show(ctx, global, target, *source);
        }
        WorkflowsCmd::Eject(args) => return library::eject(ctx, global, args),
        _ => {}
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
        WorkflowsCmd::List | WorkflowsCmd::Eject(_) => unreachable!("answered locally"),
        WorkflowsCmd::Run(args) => start(client, ctx, global, args).await,
        WorkflowsCmd::Runs { all, status, watch } => {
            runs::runs(client, global, runs::RunsQuery { all, status }, watch).await
        }
        WorkflowsCmd::Show { target, .. } => show(client, global, &target).await,
        WorkflowsCmd::Pause { run_id } => control(client, global, &run_id, "pause", None).await,
        WorkflowsCmd::Resume {
            run_id: Some(run_id),
            revision,
            ..
        } => {
            let body = json!({"revision": revision});
            control(client, global, &run_id, "resume", Some(body)).await
        }
        WorkflowsCmd::Resume { all, .. } => resume_interrupted(client, global, all).await,
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
pub(crate) fn start_body(ctx: &Context, args: &RunArgs, session: &str) -> Result<Value, CliError> {
    let script_args = match &args.args {
        None => json!({}),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(value @ Value::Object(_)) => value,
            _ => return Err(CliError::usage("--args must be a JSON object")),
        },
    };
    let mut body = json!({"session_id": session, "args": script_args, "budget": budget(args)?});
    if let Some(share) = args.max_concurrent {
        body["max_concurrent"] = share.into();
    }
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
pub(crate) fn ended(run: &Value) -> Option<u8> {
    match run["status"].as_str()? {
        "completed" => Some(0),
        "budget_exceeded" => Some(EXIT_BUDGET),
        "failed" | "stopped" | "paused" => Some(crate::error::EXIT_RUNTIME),
        _ => None,
    }
}

/// What `--wait` already printed.
#[derive(Default)]
pub(crate) struct Progress {
    phase: Option<String>,
    /// The last queue position printed while the run waits for a run slot.
    queue_position: Option<u64>,
    /// Agents whose end was printed, by ID: agents can settle in any order.
    settled: HashSet<String>,
}

impl Progress {
    pub(crate) fn report(&mut self, run: &Value) {
        for line in self.changes(run) {
            eprintln!("{line}");
        }
    }

    /// New progress lines since the last poll. A phase change is printed after the agents
    /// of earlier phases that settled in the same poll and before those of the new phase.
    fn changes(&mut self, run: &Value) -> Vec<String> {
        let position = queue_position(run);
        if position.is_some() && position != self.queue_position {
            self.queue_position = position;
            return vec![format!("queued: {}", queue_text(position))];
        }
        let phase = run["phase"].as_str().map(str::to_owned);
        let agents = run["agents_detail"]
            .as_array()
            .map_or(&[][..], Vec::as_slice);
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

/// A queued run's place in the run queue, 1 for the next to be dispatched.
fn queue_position(run: &Value) -> Option<u64> {
    (run["status"] == "queued")
        .then(|| run["queue_position"].as_u64())
        .flatten()
}

fn queue_text(position: Option<u64>) -> String {
    position.map_or_else(|| "-".into(), |n| format!("#{n} in queue"))
}

/// The status, with the queue position of a queued run.
fn status_text(run: &Value) -> String {
    match queue_position(run) {
        Some(n) => format!("{} (#{n} in queue)", text(&run["status"])),
        None => text(&run["status"]),
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

/// `resume --interrupted`: one line per resumed run on stdout, one per refusal on stderr;
/// exits 1 when any run was refused.
async fn resume_interrupted(
    client: &Client,
    global: &GlobalArgs,
    all: bool,
) -> Result<(), CliError> {
    let path = if all {
        "/workflows/runs/resume_interrupted?directory=all"
    } else {
        "/workflows/runs/resume_interrupted"
    };
    let answer = client.post(path, json!({})).await.map_err(api)?;
    let report = &answer["data"];
    let list = |key: &str| report[key].as_array().cloned().unwrap_or_default();
    let (resumed, refused) = (list("resumed"), list("refused"));
    if output::is_json(global.format) {
        output::json(report)?;
    } else {
        if resumed.is_empty() && refused.is_empty() {
            println!("No interrupted Workflow Runs.");
        }
        for id in &resumed {
            println!("resumed {}", id.as_str().unwrap_or("?"));
        }
        for refusal in &refused {
            eprintln!(
                "not resumed {}: {}",
                refusal["run_id"].as_str().unwrap_or("?"),
                refusal["reason"].as_str().unwrap_or("?")
            );
        }
    }
    if refused.is_empty() {
        Ok(())
    } else {
        Err(CliError::silent(crate::error::EXIT_RUNTIME))
    }
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
    let agents = run["agents_detail"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let count = |status: &str| agents.iter().filter(|a| a["status"] == status).count();
    output::rows(&[
        ("run", text(&run["id"])),
        ("name", text(&run["name"])),
        ("status", status_text(run)),
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
        ("cost", cost_text(agents)),
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

/// The agents' summed cost. A fold from +0.0: an empty f64 `sum` is -0.0, which a queued
/// run (no agents yet) printed as `-0.0000 USD`.
fn cost_text(agents: &[Value]) -> String {
    let cost = agents
        .iter()
        .filter_map(|a| a["cost"].as_f64())
        .fold(0.0, |sum, cost| sum + cost);
    format!("{cost:.4} USD")
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
    println!("{} {}", text(&run["id"]), status_text(run));
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
    fn a_queued_run_prints_its_queue_position_when_it_changes() {
        let mut progress = Progress::default();
        let queued = |n: u64| json!({"status": "queued", "queue_position": n, "agents_detail": []});
        assert_eq!(progress.changes(&queued(2)), ["queued: #2 in queue"]);
        assert!(progress.changes(&queued(2)).is_empty());
        assert_eq!(progress.changes(&queued(1)), ["queued: #1 in queue"]);
        let running = json!({"status": "running", "phase": "Scan", "agents_detail": []});
        assert_eq!(progress.changes(&running), ["phase: Scan"]);
        assert_eq!(status_text(&queued(3)), "queued (#3 in queue)");
        assert_eq!(status_text(&running), "running");
    }

    #[test]
    fn progress_prints_each_settled_agent_once_in_phase_order() {
        let mut progress = Progress::default();
        let run = |phase: &str, a: &str, b: &str| {
            json!({"phase": phase, "agents_detail": [
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
    fn a_run_without_agents_costs_zero_not_negative_zero() {
        assert_eq!(cost_text(&[]), "0.0000 USD");
        let agents = [
            json!({"cost": 0.25}),
            json!({"cost": null}),
            json!({"cost": 0.5}),
        ];
        assert_eq!(cost_text(&agents), "0.7500 USD");
    }

    #[test]
    fn targets_with_a_path_shape_are_files() {
        assert!(looks_like_file("audit.js"));
        assert!(looks_like_file("scripts/audit"));
        assert!(!looks_like_file("audit"));
    }
}
