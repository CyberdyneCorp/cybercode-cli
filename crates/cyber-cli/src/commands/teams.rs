//! `cyber teams` (`agent-teams` → Team definitions, Team view): the Location's teams with
//! their members (status, current task, cost, last message) and task board through the
//! team routes of the background server, stop and resume, and the Location's team
//! definitions with the field each failure names. Every team command needs the `teams`
//! feature: `definitions` where this process reads it (so `--features` counts), the
//! others where the background server does (its own configuration, which `--features`
//! does not reach).

use clap::{Args, Subcommand};
use cyber_client::Client;
use cyber_core::teams::{Definitions, TeamDefinition};
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

/// The task board columns, in board order (`cancelled` last).
const COLUMNS: [&str; 5] = ["todo", "in_progress", "blocked", "done", "cancelled"];
/// The longest last message the members table shows.
const PREVIEW: usize = 48;

#[derive(Debug, Subcommand)]
pub enum TeamsCmd {
    /// The Location's teams, newest first.
    #[command(alias = "ls")]
    List(ListArgs),
    /// A team's members (status, current task, cost, last message) and its task board.
    Show(TeamArg),
    /// Stop a team: teammates finish their current Turn; queued input stays queued.
    Stop(TeamArg),
    /// Resume a stopped team; after its lead was deleted, `--lead` names the new one.
    Resume(ResumeArgs),
    /// Team definitions from `.cyber/teams/*.md` (trusted checkouts) and
    /// `~/.config/cyber/teams/*.md`; a project definition wins on a name clash.
    Definitions,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Only teams with this status.
    #[arg(long, value_parser = ["active", "stopping", "stopped"])]
    pub status: Option<String>,
}

#[derive(Debug, Args)]
pub struct TeamArg {
    /// A `team_` ID, or the name of a team of this Location (the newest one by that name).
    pub team: String,
}

#[derive(Debug, Args)]
pub struct ResumeArgs {
    /// A `team_` ID, or the name of a team of this Location (the newest one by that name).
    pub team: String,
    /// The new lead (ID, exact title or unique ID prefix) of a team whose lead was deleted.
    #[arg(long)]
    pub lead: Option<String>,
}

/// Appended to the server's `FeatureDisabledError` when this process has `--features`.
const FLAG_NOT_FORWARDED: &str =
    "--features applies to this process only; the background server reads its own configuration";

pub fn run(cmd: TeamsCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    if let TeamsCmd::Definitions = cmd {
        return definitions(ctx, global);
    }
    // The background server decides whether teams are on, from its own configuration:
    // the team routes answer `FeatureDisabledError` while they are off there.
    let rt = super::serve::runtime()?;
    rt.block_on(async {
        let info = super::serve::start(ctx).await?;
        let client = Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string());
        let json = output::is_json(global.format);
        match cmd {
            TeamsCmd::List(args) => list(&client, args.status.as_deref(), json).await,
            TeamsCmd::Show(args) => show(&client, &args.team, json).await,
            TeamsCmd::Stop(args) => stop(&client, &args.team, json).await,
            TeamsCmd::Resume(args) => resume(&client, &args, json).await,
            TeamsCmd::Definitions => Ok(()),
        }
    })
}

/// The Location's team definitions, read by this process (so `--features` counts).
fn definitions(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let resolved = ctx.config()?;
    let features = cyber_core::features::Features::from_config(&resolved.value, &resolved.sources);
    if !features.enabled(cyber_core::features::TEAMS) {
        return Err(CliError::usage(cyber_core::teams::DISABLED));
    }
    let definitions =
        cyber_core::teams::definitions(&ctx.location, &ctx.paths.config, &resolved, &ctx.env);
    if output::is_json(global.format) {
        return output::json(&definitions);
    }
    print_definitions(&definitions);
    Ok(())
}

/// One team route request; a declared failure becomes a CLI error (2 for what the
/// caller can fix).
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
        "FeatureDisabledError" if cyber_core::features::flag().is_some() => {
            CliError::usage(format!("{message} ({FLAG_NOT_FORWARDED})"))
        }
        "FeatureDisabledError" | "InvalidRequestError" | "TeamNotFoundError" => {
            CliError::usage(message)
        }
        _ => CliError::runtime(super::tagged(tag, message)),
    })
}

async fn list(client: &Client, status: Option<&str>, json: bool) -> Result<(), CliError> {
    let path = match status {
        Some(status) => format!("/teams?status={status}"),
        None => "/teams".to_owned(),
    };
    let teams = call(client, "GET", &path, None).await?["data"].take();
    if json {
        return output::json(&teams);
    }
    let teams = teams.as_array().cloned().unwrap_or_default();
    if teams.is_empty() {
        println!("No teams.");
        return Ok(());
    }
    let rows = teams
        .iter()
        .map(|team| {
            vec![
                field(team, "id"),
                field(team, "name"),
                status_of(team),
                team["teammates"].as_u64().unwrap_or_default().to_string(),
                format!(
                    "{}/{}",
                    team["tasks_done"].as_u64().unwrap_or_default(),
                    team["tasks"].as_u64().unwrap_or_default()
                ),
                cost(&team["cost_usd"]),
                field(team, "lead_id"),
            ]
        })
        .collect();
    let columns = [
        ("ID", false),
        ("NAME", false),
        ("STATUS", false),
        ("TEAMMATES", true),
        ("TASKS", true),
        ("COST", true),
        ("LEAD", false),
    ];
    print_table(&columns, rows);
    Ok(())
}

/// The ID of `wanted`, encoded for a path: a `team_` ID as is, else the newest team of
/// the Location by that name.
async fn team_id(client: &Client, wanted: &str) -> Result<String, CliError> {
    if wanted.starts_with("team_") {
        return Ok(super::messages::query(wanted));
    }
    let teams = call(client, "GET", "/teams", None).await?;
    teams["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|team| team["name"] == wanted)
        .and_then(|team| team["id"].as_str().map(super::messages::query))
        .ok_or_else(|| {
            CliError::usage(format!("No team {wanted} in this Location"))
                .with_hint("run \"cyber teams list\"")
        })
}

async fn show(client: &Client, wanted: &str, json: bool) -> Result<(), CliError> {
    let id = team_id(client, wanted).await?;
    let view = call(client, "GET", &format!("/teams/{id}"), None).await?["data"].take();
    if json {
        return output::json(&view);
    }
    print_view(&view);
    Ok(())
}

async fn stop(client: &Client, wanted: &str, json: bool) -> Result<(), CliError> {
    let id = team_id(client, wanted).await?;
    let team = call(client, "POST", &format!("/teams/{id}/stop"), None).await?["data"].take();
    report(&team, json)
}

async fn resume(client: &Client, args: &ResumeArgs, json: bool) -> Result<(), CliError> {
    let id = team_id(client, &args.team).await?;
    let lead = match args.lead.as_deref() {
        Some(wanted) => super::sessions::resolve(client, wanted).await?["id"]
            .as_str()
            .map(str::to_owned),
        None => None,
    };
    let path = format!("/teams/{id}/resume");
    let team = call(client, "POST", &path, Some(json!({ "lead": lead }))).await?["data"].take();
    report(&team, json)
}

/// `team_… <status>`, or the team as JSON.
fn report(team: &Value, json: bool) -> Result<(), CliError> {
    if json {
        return output::json(team);
    }
    println!("{} {}", field(team, "id"), status_of(team));
    Ok(())
}

fn print_view(view: &Value) {
    let team = &view["team"];
    println!(
        "{} {} {} (lead {})",
        field(team, "id"),
        field(team, "name"),
        status_of(team),
        field(team, "lead_id")
    );
    println!("{}", spend_line(view));
    let rows = view["members"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|member| {
            let task = &member["current_task"];
            let task = if task.is_object() {
                format!("{} {}", field(task, "id"), field(task, "title"))
            } else {
                "-".to_owned()
            };
            vec![
                field(member, "name"),
                field(member, "role"),
                field(member, "status"),
                task,
                cost(&member["cost_usd"]),
                last_message(&member["last_message"]),
            ]
        })
        .collect();
    println!();
    let columns = [
        ("MEMBER", false),
        ("ROLE", false),
        ("STATUS", false),
        ("TASK", false),
        ("COST", true),
        ("LAST MESSAGE", false),
    ];
    print_table(&columns, rows);
    print_board(
        view["tasks"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default(),
    );
}

/// `spent $0.0024 (3 turns, 330 tokens)`, with the budget when the team has one.
fn spend_line(view: &Value) -> String {
    let spent = &view["spent"];
    let line = format!(
        "spent {} ({} turns, {} tokens)",
        cost(&spent["cost_usd"]),
        spent["turns"].as_u64().unwrap_or_default(),
        spent["tokens"].as_u64().unwrap_or_default()
    );
    let limits: Vec<String> = view["budget"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| value.is_number())
        .map(|(name, value)| format!("{name} {value}"))
        .collect();
    if limits.is_empty() {
        return line;
    }
    format!("{line}; budget {}", limits.join(", "))
}

/// The board by status, each task with its owner and dependencies.
fn print_board(tasks: &[Value]) {
    if tasks.is_empty() {
        println!("\nNo tasks.");
        return;
    }
    for column in COLUMNS {
        let in_column: Vec<&Value> = tasks.iter().filter(|t| t["status"] == column).collect();
        if in_column.is_empty() {
            continue;
        }
        println!("\n{column} ({})", in_column.len());
        for task in in_column {
            let owner = task["owner"].as_str().map(|o| format!(" @{}", clean(o)));
            let depends: Vec<&str> = task["depends_on"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let depends = (!depends.is_empty()).then(|| format!(" after {}", depends.join(",")));
            println!(
                "  {}  {}{}{}",
                field(task, "id"),
                field(task, "title"),
                owner.unwrap_or_default(),
                depends.unwrap_or_default()
            );
        }
    }
}

/// `out to lead: schema ready` / `in from api: …`, cut to [`PREVIEW`] characters.
fn last_message(message: &Value) -> String {
    if !message.is_object() {
        return "-".to_owned();
    }
    let way = if message["direction"] == "in" {
        "from"
    } else {
        "to"
    };
    let text = clean(message["text"].as_str().unwrap_or_default());
    let text = if text.chars().count() > PREVIEW {
        format!("{}…", text.chars().take(PREVIEW - 1).collect::<String>())
    } else {
        text
    };
    format!("{way} {}: {text}", field(message, "peer"))
}

/// `stopped (user)` for a stopped team, else its status.
fn status_of(team: &Value) -> String {
    match team["stop_reason"].as_str() {
        Some(reason) if team["status"] != "active" => {
            format!("{} ({})", field(team, "status"), clean(reason))
        }
        _ => field(team, "status"),
    }
}

/// Columns `(header, right-aligned)` as wide as their widest cell, two spaces apart; the
/// last column is not padded.
fn print_table(columns: &[(&str, bool)], rows: Vec<Vec<String>>) {
    let widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(at, (header, _))| {
            rows.iter()
                .map(|row| row[at].chars().count())
                .fold(header.chars().count(), usize::max)
        })
        .collect();
    let header = columns
        .iter()
        .map(|(header, _)| (*header).to_owned())
        .collect();
    for row in std::iter::once(header).chain(rows) {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(
                |(at, cell)| match (at + 1 == columns.len(), columns[at].1) {
                    (true, _) => cell.clone(),
                    (false, true) => format!("{cell:>width$}", width = widths[at]),
                    (false, false) => format!("{cell:<width$}", width = widths[at]),
                },
            )
            .collect();
        println!("{}", cells.join("  "));
    }
}

fn cost(value: &Value) -> String {
    format!("${:.4}", value.as_f64().unwrap_or_default())
}

fn field(value: &Value, key: &str) -> String {
    clean(value[key].as_str().unwrap_or_default())
}

/// Text from Sessions and models; keep it on one line without control characters.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

fn print_definitions(definitions: &Definitions) {
    if definitions.definitions.is_empty() {
        println!("No team definitions.");
    }
    let width = definitions
        .definitions
        .iter()
        .map(|d| d.name.len())
        .max()
        .unwrap_or(0)
        .max(4);
    if !definitions.definitions.is_empty() {
        println!("{:width$}  {:7}  TEAMMATES  DESCRIPTION", "NAME", "SCOPE");
    }
    for definition in &definitions.definitions {
        println!(
            "{:width$}  {:7}  {}  {}",
            definition.name,
            scope(definition),
            members(definition),
            definition.description
        );
    }
    if !definitions.issues.is_empty() {
        println!("\nInvalid definitions:");
    }
    for issue in &definitions.issues {
        println!("  {}: {}", issue.path.display(), issue.message);
    }
}

fn scope(definition: &TeamDefinition) -> &'static str {
    match definition.scope {
        cyber_core::teams::DefinitionScope::Project => "project",
        cyber_core::teams::DefinitionScope::User => "user",
    }
}

fn members(definition: &TeamDefinition) -> String {
    definition
        .teammates
        .iter()
        .map(|t| t.name.as_str())
        .collect::<Vec<_>>()
        .join(",")
}
