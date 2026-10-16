//! `cyber stats` (`cli-commands` → Stats command): `GET /api/v1/usage/stats` from the
//! background server, so the numbers match the TUI Stats tab, plus tool calls by name
//! from the `tool_call` projection with `--tools`.

use std::path::{Path, PathBuf};

use clap::Args;
use cyber_core::paths::DatabaseLocation;
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Args)]
pub struct StatsArgs {
    /// Only the last N local days, today included (1 to 371).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=371))]
    pub days: Option<u16>,
    /// Only Sessions of the project containing this directory.
    #[arg(long, value_name = "DIR")]
    pub project: Option<PathBuf>,
    /// One row per model.
    #[arg(long)]
    pub models: bool,
    /// Tool calls by name.
    #[arg(long)]
    pub tools: bool,
}

pub fn run(args: StatsArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let project = args
        .project
        .as_ref()
        .map(|dir| project_dir(&ctx.location, dir))
        .transpose()?;
    let period = args
        .days
        .map_or_else(|| "all".to_owned(), |n| format!("{n}d"));
    let mut path = format!("/usage/stats?period={period}");
    if let Some(project) = &project {
        path.push_str(&format!(
            "&directory={}",
            encode(&project.display().to_string())
        ));
    }
    let rt = super::serve::runtime()?;
    let value = rt.block_on(async {
        let info = super::serve::start(ctx).await?;
        cyber_client::Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string())
            .get(&path)
            .await
            .map_err(|e| CliError::runtime(e.to_string()))
    })?;
    let mut stats = value["data"].clone();
    if !stats.is_object() {
        return Err(CliError::runtime("the server sent no usage statistics"));
    }
    if args.tools {
        let root = project.as_deref().map(cyber_core::config::project_root);
        stats["tools"] = tools(ctx, stats["since_ms"].as_i64(), root.as_deref())?;
    }
    if output::is_json(global.format) {
        return output::json(&stats);
    }
    print_text(&stats, &args);
    Ok(())
}

/// `--project` relative to the Location; it must exist.
fn project_dir(location: &Path, dir: &Path) -> Result<PathBuf, CliError> {
    let dir = location.join(dir);
    std::fs::canonicalize(&dir)
        .map_err(|_| CliError::usage(format!("no such directory: {}", dir.display())))
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Tool calls by name in Sessions active since `since_ms`, most used first.
fn tools(ctx: &Context, since_ms: Option<i64>, root: Option<&Path>) -> Result<Value, CliError> {
    let DatabaseLocation::File(path) = ctx.database() else {
        return Ok(json!([]));
    };
    let db =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| CliError::runtime(e.to_string()))?;
    let query = || -> rusqlite::Result<Vec<(String, String, u64)>> {
        let mut statement = db.prepare(
            "SELECT s.directory, t.name, COUNT(*) FROM tool_call t JOIN session s ON s.id = t.session_id
             WHERE s.updated_at >= ?1 GROUP BY 1, 2",
        )?;
        statement
            .query_map([since_ms.unwrap_or(i64::MIN)], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)?.max(0) as u64))
            })?
            .collect()
    };
    let rows = query().map_err(|e| CliError::runtime(e.to_string()))?;
    let mut counts = std::collections::BTreeMap::<String, u64>::new();
    for (directory, name, calls) in rows {
        if root.is_none_or(|root| Path::new(&directory).starts_with(root)) {
            *counts.entry(name).or_default() += calls;
        }
    }
    let mut tools: Vec<(String, u64)> = counts.into_iter().collect();
    tools.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(Value::Array(
        tools
            .into_iter()
            .map(|(name, calls)| json!({ "name": name, "calls": calls }))
            .collect(),
    ))
}

fn n(value: &Value) -> u64 {
    value.as_u64().unwrap_or(0)
}

/// `$125.43`, or four decimals below a dollar (`$0.0007`).
fn money(cost: f64) -> String {
    if cost >= 1.0 || cost == 0.0 {
        format!("${cost:.2}")
    } else {
        format!("${cost:.4}")
    }
}

/// `—` when every call was unpriced, `≥ $x (N unpriced calls)` when some were.
fn cost(totals: &Value) -> String {
    match (totals["cost"].as_f64(), n(&totals["unpriced_steps"])) {
        (None, _) => "—".into(),
        (Some(c), 0) => money(c),
        (Some(c), unpriced) => format!("≥ {} ({unpriced} unpriced calls)", money(c)),
    }
}

fn tokens(t: &Value) -> String {
    format!(
        "input {} · output {} · reasoning {} · cache read {} · cache write {}",
        n(&t["input"]),
        n(&t["output"]),
        n(&t["reasoning"]),
        n(&t["cache_read"]),
        n(&t["cache_write"])
    )
}

fn text(value: &Value) -> String {
    value.as_str().map_or_else(|| "—".to_owned(), clean)
}

/// Untrusted server text on one line, control characters removed.
fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn print_text(s: &Value, args: &StatsArgs) {
    let totals = &s["totals"];
    let period = match args.days {
        Some(n) => format!("Last {n} days"),
        None => "All time".into(),
    };
    let scope = if args.project.is_some() {
        "this project"
    } else {
        "all projects"
    };
    println!("{period} · {scope} · {}", text(&s["zone"]));
    let favorite = &s["favorite_model"];
    let favorite = if favorite.is_object() {
        format!(
            "{}/{}",
            text(&favorite["provider"]),
            text(&favorite["model"])
        )
    } else {
        "—".into()
    };
    let longest = &s["longest_session"];
    let longest = if longest.is_object() {
        format!(
            "{} ({})",
            duration(n(&longest["duration_ms"])),
            text(&longest["title"])
        )
    } else {
        "—".into()
    };
    output::rows(&[
        ("Sessions", n(&totals["sessions"]).to_string()),
        ("Turns", n(&totals["turns"]).to_string()),
        ("Total tokens", n(&totals["total_tokens"]).to_string()),
        ("Tokens", tokens(&totals["tokens"])),
        ("Total cost", cost(totals)),
        (
            "Active days",
            format!("{}/{}", n(&s["active_days"]), n(&s["period_days"])),
        ),
        (
            "Current streak",
            format!("{} days", n(&s["current_streak"])),
        ),
        (
            "Longest streak",
            format!("{} days", n(&s["longest_streak"])),
        ),
        ("Most active day", text(&s["most_active_day"])),
        ("Favorite model", favorite),
        ("Longest session", longest),
    ]);
    if s["approximate_days"] == true {
        println!("Older days are UTC days from daily aggregates.");
    }
    if s["backfilling"] == true {
        println!("Indexing history: the numbers are still partial.");
    }
    if args.models {
        print_models(s);
    }
    if let Some(tools) = s["tools"].as_array() {
        println!();
        println!("Tools");
        for tool in tools {
            println!("  {:<24} {}", text(&tool["name"]), n(&tool["calls"]));
        }
    }
}

fn print_models(s: &Value) {
    println!();
    println!(
        "{:<32} {:>14} {:>6} {:>12} {:>7} {:>9}",
        "Model", "Tokens", "Share", "Cost", "Turns", "Sessions"
    );
    for m in s["models"].as_array().into_iter().flatten() {
        let name = format!("{}/{}", text(&m["provider"]), text(&m["model"]));
        let share = m["share"].as_f64().unwrap_or(0.0) * 100.0;
        let cost = match (m["cost"].as_f64(), n(&m["unpriced_steps"])) {
            (None, _) => "—".to_owned(),
            (Some(c), 0) => money(c),
            (Some(c), _) => format!("≥ {}", money(c)),
        };
        println!(
            "{name:<32} {:>14} {:>5.0}% {cost:>12} {:>7} {:>9}",
            n(&m["total_tokens"]),
            share,
            n(&m["turns"]),
            n(&m["sessions"])
        );
    }
}

/// `3h 21m 0s`, `5d 23h 31m`, `4m 2s`, `12s`, `340ms`, as the TUI shows them.
fn duration(ms: u64) -> String {
    let s = ms / 1000;
    let (d, h, m, sec) = (s / 86_400, s / 3600 % 24, s / 60 % 60, s % 60);
    match s {
        0 => format!("{ms}ms"),
        1..60 => format!("{sec}s"),
        60..3600 => format!("{m}m {sec}s"),
        3600..86_400 => format!("{h}h {m}m {sec}s"),
        _ => format!("{d}d {h}h {m}m"),
    }
}
