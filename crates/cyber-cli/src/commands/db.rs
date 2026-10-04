//! `cyber db` (`storage-events` → Database command).

use clap::Subcommand;
use cyber_core::paths::DatabaseLocation;
use serde_json::Value;

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum DbCmd {
    /// Print the database path.
    Path,
    /// Run a read-only SQL query (TSV, or JSON with --format json).
    Query { sql: String },
    #[command(external_subcommand)]
    Other(Vec<String>),
}

const LATER: &[&str] = &["vacuum", "backup", "restore", "encrypt", "decrypt"];

pub fn run(cmd: DbCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    match cmd {
        DbCmd::Path => {
            match ctx.database() {
                DatabaseLocation::File(path) => println!("{}", path.display()),
                DatabaseLocation::Memory => println!(":memory:"),
            }
            Ok(())
        }
        DbCmd::Query { sql } => query(ctx, global, &sql),
        DbCmd::Other(args) => {
            let name = args.first().map(String::as_str).unwrap_or_default();
            if LATER.contains(&name) {
                Err(CliError::unavailable(&format!("`cyber db {name}`"), "M0.3"))
            } else {
                Err(CliError::usage(format!("unknown db command \"{name}\"")))
            }
        }
    }
}

fn query(ctx: &Context, global: &GlobalArgs, sql: &str) -> Result<(), CliError> {
    let DatabaseLocation::File(path) = ctx.database() else {
        return Err(CliError::usage(
            "CYBER_DB=:memory: has no database to query",
        ));
    };
    let result = cyber_store::query_readonly(&path, sql)?;
    if output::is_json(global.format) {
        let rows: Vec<serde_json::Map<String, Value>> = result
            .rows
            .iter()
            .map(|row| {
                result
                    .columns
                    .iter()
                    .cloned()
                    .zip(row.iter().cloned())
                    .collect()
            })
            .collect();
        return output::json(&rows);
    }
    println!("{}", result.columns.join("\t"));
    for row in &result.rows {
        let cells: Vec<String> = row.iter().map(cell).collect();
        println!("{}", cells.join("\t"));
    }
    Ok(())
}

fn cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.replace(['\t', '\n'], " "),
        other => other.to_string(),
    }
}
