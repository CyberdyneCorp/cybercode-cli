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
    /// Copy the database while the server runs (online backup). With --artifacts, write a
    /// bundle directory with tool output, snapshots and a manifest of hashes.
    Backup {
        path: std::path::PathBuf,
        #[arg(long)]
        artifacts: bool,
    },
    /// Check a backup bundle against its manifest and the database's integrity.
    Verify { path: std::path::PathBuf },
    /// Replace the database (and bundle artifacts) from a backup; the server must be stopped.
    Restore { path: std::path::PathBuf },
    /// Rebuild the database to reclaim space; the server must be stopped.
    Vacuum,
    #[command(external_subcommand)]
    Other(Vec<String>),
}

const LATER: &[&str] = &["encrypt", "decrypt"];

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
        DbCmd::Backup { path, artifacts } => backup(ctx, global, &path, artifacts),
        DbCmd::Verify { path } => verify(global, &path),
        DbCmd::Restore { path } => restore(ctx, &path),
        DbCmd::Vacuum => vacuum(ctx),
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

fn file(ctx: &Context) -> Result<std::path::PathBuf, CliError> {
    match ctx.database() {
        DatabaseLocation::File(path) if path.exists() => Ok(path),
        DatabaseLocation::File(path) => Err(CliError::usage(format!(
            "no database yet at {}",
            path.display()
        ))),
        DatabaseLocation::Memory => Err(CliError::usage("CYBER_DB=:memory: has no database file")),
    }
}

fn backup(
    ctx: &Context,
    global: &GlobalArgs,
    dest: &std::path::Path,
    artifacts: bool,
) -> Result<(), CliError> {
    let db = file(ctx)?;
    if !artifacts {
        cyber_store::backup::backup(&db, dest)?;
        let check = cyber_store::backup::integrity_check(dest)?;
        return report(
            global,
            &serde_json::json!({ "backup": dest, "integrity": check }),
        );
    }
    let manifest =
        cyber_app::backup::create(&ctx.paths, &db, dest, true).map_err(CliError::runtime)?;
    let bytes: u64 = manifest.files.iter().map(|f| f.size).sum();
    report(
        global,
        &serde_json::json!({ "bundle": dest, "files": manifest.files.len(), "bytes": bytes, "artifacts": manifest.artifacts }),
    )
}

fn verify(global: &GlobalArgs, path: &std::path::Path) -> Result<(), CliError> {
    match cyber_app::backup::verify(path) {
        Ok(m) => report(
            global,
            &serde_json::json!({ "ok": true, "files": m.files.len(), "created_at": m.created_at, "cyber_version": m.cyber_version }),
        ),
        Err(problems) => Err(CliError::runtime(format!(
            "the bundle failed verification:\n  {}",
            problems.join("\n  ")
        ))),
    }
}

fn restore(ctx: &Context, path: &std::path::Path) -> Result<(), CliError> {
    let kept =
        cyber_app::backup::restore(&ctx.paths, &file(ctx)?, path).map_err(CliError::runtime)?;
    println!("restored from {}", path.display());
    for k in kept {
        println!("previous copy kept at {}", k.display());
    }
    Ok(())
}

fn vacuum(ctx: &Context) -> Result<(), CliError> {
    if let Some(reg) = cyber_app::read_registration(&ctx.paths) {
        return Err(CliError::runtime(format!(
            "a server is registered (pid {}); run `cyber service stop` first",
            reg.pid
        )));
    }
    cyber_store::backup::vacuum(&file(ctx)?)?;
    Ok(())
}

fn report(global: &GlobalArgs, value: &Value) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(value);
    }
    for (k, v) in value.as_object().into_iter().flatten() {
        println!(
            "{k}: {}",
            v.as_str().map_or_else(|| v.to_string(), str::to_string)
        );
    }
    Ok(())
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
