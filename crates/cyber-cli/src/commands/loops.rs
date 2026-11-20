//! `cyber loops` (`loops-scheduling` → Persistent local schedules): prompts the Cyber
//! service runs on a cron schedule even when no client is open, managed through the
//! service's schedule routes.
//!
//! Creating or waking a schedule is the human owner's act: `add`, `resume` and `run` are
//! refused inside an agent subprocess, so the model cannot schedule unattended work.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use cyber_client::Client;
use cyber_core::budget::Budget;
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum LoopsCmd {
    /// Add a schedule the Cyber service runs even when no client is open.
    Add(Box<AddArgs>),
    /// Schedules with their next run (jitter included).
    #[command(alias = "ls")]
    List,
    /// A schedule and its recent runs.
    Show(IdArg),
    /// Stop running a schedule until it is resumed; a run in flight finishes.
    Pause(IdArg),
    /// Resume a paused schedule at its next occurrence.
    Resume(IdArg),
    /// Run a schedule now, outside its cron schedule.
    Run(IdArg),
    /// Remove a schedule; its run history stays in the database.
    #[command(alias = "remove")]
    Rm(IdArg),
}

#[derive(Debug, Args)]
pub struct IdArg {
    /// The schedule's `cron_` ID.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct AddArgs {
    /// A 5-field cron expression in local time: minute hour day-of-month month day-of-week.
    #[arg(long)]
    pub cron: String,
    /// A name shown in listings and notifications (default: the ID).
    #[arg(long)]
    pub name: Option<String>,
    /// The directory runs work in (default: the current Location). Its checkout must be
    /// trusted.
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// The agent of run Sessions.
    #[arg(long)]
    pub agent: Option<String>,
    /// The Mode runs use (default dont-ask). `bypass` needs `loops.allow_bypass` in user or
    /// managed config.
    #[arg(long)]
    pub mode: Option<String>,
    /// The model of new run Sessions (default: the Location's default model).
    #[arg(long)]
    pub model: Option<String>,
    /// Create a new Session for every run (the default).
    #[arg(long, conflicts_with = "session")]
    pub new_session: bool,
    /// Resume this root Session for every run instead.
    #[arg(long)]
    pub session: Option<String>,
    /// When to notify: always, on_change (default) or never.
    #[arg(long, value_parser = ["always", "on_change", "never"])]
    pub notify: Option<String>,
    /// Runs missed while the service was not running: skip (default) or run-once.
    #[arg(long, value_parser = ["skip", "run-once"])]
    pub missed: Option<String>,
    /// Stop the schedule once its runs cost this many USD in total.
    #[arg(long)]
    pub max_cost: Option<f64>,
    /// Stop once its runs used this many tokens in total.
    #[arg(long)]
    pub max_tokens: Option<u64>,
    /// Stop once its runs made this many model calls in total.
    #[arg(long)]
    pub max_turns: Option<u64>,
    /// Stop this long after the schedule was added (e.g. 30d).
    #[arg(long)]
    pub timeout: Option<String>,
    /// The prompt (or `/command`) each run sends.
    pub prompt: String,
}

/// The refusal inside an agent subprocess.
const AGENT_REFUSAL: &str =
    "persistent schedules are managed by the human owner, outside an agent subprocess";

pub fn run(cmd: LoopsCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let wakes = matches!(
        cmd,
        LoopsCmd::Add(_) | LoopsCmd::Resume(_) | LoopsCmd::Run(_)
    );
    if wakes && std::env::var_os("CYBER_SESSION_ID").is_some() {
        return Err(CliError::usage(AGENT_REFUSAL));
    }
    let body = match &cmd {
        LoopsCmd::Add(args) => Some(add_body(args, ctx)?),
        _ => None,
    };
    let rt = super::serve::runtime()?;
    rt.block_on(async {
        let was_running = service_running(ctx).await;
        let info = super::serve::start(ctx).await?;
        let client = Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string());
        let json = output::is_json(global.format);
        match cmd {
            LoopsCmd::Add(_) => add(&client, body.unwrap_or_default(), json, was_running).await,
            LoopsCmd::List => list(&client, json).await,
            LoopsCmd::Show(arg) => show(&client, &arg.id, json).await,
            LoopsCmd::Pause(arg) => act(&client, "POST", &arg.id, "/pause", json).await,
            LoopsCmd::Resume(arg) => act(&client, "POST", &arg.id, "/resume", json).await,
            LoopsCmd::Run(arg) => act(&client, "POST", &arg.id, "/run", json).await,
            LoopsCmd::Rm(arg) => act(&client, "DELETE", &arg.id, "", json).await,
        }
    })
}

/// Whether a healthy service was registered before this command (which starts one).
async fn service_running(ctx: &Context) -> bool {
    match cyber_app::read_registration(&ctx.paths) {
        Some(registration) => cyber_app::probe_registration(&registration)
            .await
            .is_some_and(|probe| probe.serves(&registration)),
        None => false,
    }
}

fn add_body(args: &AddArgs, ctx: &Context) -> Result<Value, CliError> {
    let directory = match &args.dir {
        Some(dir) => std::fs::canonicalize(dir)
            .map_err(|e| CliError::usage(format!("--dir {}: {e}", dir.display())))?,
        None => ctx.location.clone(),
    };
    let budget = budget(args)?;
    let mut body = json!({
        "cron": args.cron,
        "prompt": args.prompt,
        "directory": directory.display().to_string(),
    });
    let optional = [
        ("name", args.name.clone()),
        ("agent", args.agent.clone()),
        ("mode", args.mode.clone()),
        ("model", args.model.clone()),
        ("session_id", args.session.clone()),
        ("notify", args.notify.clone()),
        ("missed", args.missed.clone()),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            body[key] = json!(value);
        }
    }
    if let Some(budget) = budget {
        body["budget"] =
            serde_json::to_value(budget).map_err(|e| CliError::runtime(e.to_string()))?;
    }
    Ok(body)
}

fn budget(args: &AddArgs) -> Result<Option<Budget>, CliError> {
    let wall = match &args.timeout {
        Some(text) => Some(
            cyber_core::loops::duration(text)
                .ok_or_else(|| CliError::usage("--timeout needs a duration such as 12h or 30d"))?
                as f64,
        ),
        None => None,
    };
    let budget = Budget {
        max_cost_usd: args.max_cost,
        max_tokens: args.max_tokens,
        max_turns: args.max_turns,
        max_wall_seconds: wall,
        ..Budget::default()
    };
    let any = budget.max_cost_usd.is_some()
        || budget.max_tokens.is_some()
        || budget.max_turns.is_some()
        || budget.max_wall_seconds.is_some();
    if any {
        budget.validate().map_err(CliError::usage)?;
    }
    Ok(any.then_some(budget))
}

/// One schedule route request; a declared failure becomes a usage error (exit 2).
async fn call(
    client: &Client,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, CliError> {
    let (status, value) = client
        .raw(method, path, body, &[])
        .await
        .map_err(|e| CliError::runtime(e.to_string()))?;
    if (200..300).contains(&status) {
        return Ok(value);
    }
    let tag = value["_tag"].as_str().unwrap_or_default();
    let message = value["message"]
        .as_str()
        .unwrap_or("the server refused the request");
    Err(match tag {
        "InvalidRequestError" | "ScheduleNotFoundError" => CliError::usage(message),
        _ => CliError::runtime(super::tagged(tag, message)),
    })
}

fn schedule_path(id: &str) -> Result<String, CliError> {
    if !cyber_core::loops::is_cron_id(id) {
        return Err(
            CliError::usage(format!("{id} is not a schedule ID (cron_…)"))
                .with_hint("run \"cyber loops list\""),
        );
    }
    Ok(format!("/schedules/{id}"))
}

async fn add(client: &Client, body: Value, json: bool, was_running: bool) -> Result<(), CliError> {
    let schedule = call(client, "POST", "/schedules", Some(body)).await?["data"].take();
    if json {
        output::json(&schedule)?;
    } else {
        println!(
            "{} added · {} · next run {}",
            field(&schedule, "id"),
            field(&schedule, "cron"),
            when(&schedule["next_due_ms"])
        );
    }
    if !was_running {
        eprintln!(
            "warning: schedules run only while the Cyber service runs. It was not running, so it \
             was started now; after a reboot or logout start it again with `cyber service start`."
        );
    }
    Ok(())
}

async fn list(client: &Client, json: bool) -> Result<(), CliError> {
    let schedules = call(client, "GET", "/schedules", None).await?["data"].take();
    if json {
        return output::json(&schedules);
    }
    let schedules = schedules.as_array().cloned().unwrap_or_default();
    if schedules.is_empty() {
        println!("No schedules. Add one with: cyber loops add --cron \"0 2 * * *\" \"<prompt>\"");
        return Ok(());
    }
    let rows = schedules
        .iter()
        .map(|s| {
            vec![
                field(s, "id"),
                field(s, "name"),
                field(s, "cron"),
                when(&s["next_due_ms"]),
                field(s, "status"),
                s["runs"].as_u64().unwrap_or_default().to_string(),
                field(s, "mode"),
                field(s, "directory"),
            ]
        })
        .collect();
    let columns = [
        ("ID", false),
        ("NAME", false),
        ("CRON", false),
        ("NEXT RUN", false),
        ("STATUS", false),
        ("RUNS", true),
        ("MODE", false),
        ("DIRECTORY", false),
    ];
    super::teams::print_table(&columns, rows);
    Ok(())
}

async fn show(client: &Client, id: &str, json: bool) -> Result<(), CliError> {
    let schedule = call(client, "GET", &schedule_path(id)?, None).await?["data"].take();
    if json {
        return output::json(&schedule);
    }
    let session = schedule["session_id"]
        .as_str()
        .map_or_else(|| "new Session per run".to_owned(), str::to_owned);
    output::rows(&[
        ("id", field(&schedule, "id")),
        ("name", field(&schedule, "name")),
        ("cron", field(&schedule, "cron")),
        ("next run", when(&schedule["next_due_ms"])),
        ("status", field(&schedule, "status")),
        ("directory", field(&schedule, "directory")),
        ("mode", field(&schedule, "mode")),
        ("session", session),
        ("notify", field(&schedule, "notify")),
        (
            "runs",
            schedule["runs"].as_u64().unwrap_or_default().to_string(),
        ),
        (
            "cost",
            format!("${:.2}", schedule["cost_usd"].as_f64().unwrap_or_default()),
        ),
        ("prompt", field(&schedule, "prompt")),
    ]);
    if let Some(reason) = schedule["reason"].as_str() {
        println!("reason: {reason}");
    }
    for run in schedule["recent"].as_array().into_iter().flatten() {
        let reason = run["reason"]
            .as_str()
            .map(|r| format!(" · {r}"))
            .unwrap_or_default();
        println!(
            "  #{} {} {}{reason}",
            run["n"].as_u64().unwrap_or_default(),
            when(&run["due_ms"]),
            field(run, "outcome"),
        );
    }
    Ok(())
}

async fn act(
    client: &Client,
    method: &str,
    id: &str,
    verb: &str,
    json: bool,
) -> Result<(), CliError> {
    let path = format!("{}{verb}", schedule_path(id)?);
    let value = call(client, method, &path, None).await?["data"].take();
    if json {
        return output::json(&value);
    }
    match verb {
        "" => println!("{id} removed"),
        "/run" => println!(
            "{id} run started ({})",
            value["run_id"].as_str().unwrap_or("-")
        ),
        _ => println!(
            "{id} {} · next run {}",
            field(&value, "status"),
            when(&value["next_due_ms"])
        ),
    }
    Ok(())
}

fn field(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("-").to_owned()
}

/// A time in ms as a local timestamp, or `-`.
fn when(value: &Value) -> String {
    value
        .as_i64()
        .map_or_else(|| "-".into(), cyber_core::cron::local_time_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(extra: &[&str]) -> AddArgs {
        use clap::Parser;
        #[derive(Parser)]
        struct Wrap {
            #[command(flatten)]
            add: AddArgs,
        }
        let mut all = vec!["x", "--cron", "0 2 * * *"];
        all.extend_from_slice(extra);
        all.push("review the night's commits");
        Wrap::parse_from(all).add
    }

    #[test]
    fn budgets_come_from_the_flags() {
        assert_eq!(budget(&args(&[])).unwrap(), None);
        let b = budget(&args(&["--max-cost", "2", "--timeout", "30d"]))
            .unwrap()
            .unwrap();
        assert_eq!(
            (b.max_cost_usd, b.max_wall_seconds),
            (Some(2.0), Some(2_592_000.0))
        );
        assert!(budget(&args(&["--timeout", "soon"])).is_err());
    }

    #[test]
    fn schedule_ids_are_checked_before_any_request() {
        assert_eq!(
            schedule_path("cron_01ABC").unwrap(),
            "/schedules/cron_01ABC"
        );
        assert!(schedule_path("lop_01ABC").is_err());
        assert!(schedule_path("../x").is_err());
    }
}
