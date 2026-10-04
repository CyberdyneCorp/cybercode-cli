//! `cyber debug` (`cli-commands` → Doctor and debug commands).

use clap::Subcommand;
use cyber_core::config::redact_secrets;
use cyber_core::paths::DatabaseLocation;
use serde_json::json;

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum DebugCmd {
    /// Print resolved directories and the database location.
    Paths,
    /// Print the resolved configuration with secrets redacted.
    Config {
        /// Include the source layer of every value.
        #[arg(long)]
        sources: bool,
    },
    /// Print build, path, database, configuration and trust information.
    Info,
}

pub fn run(cmd: DebugCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    match cmd {
        DebugCmd::Paths => paths(ctx, global),
        DebugCmd::Config { sources } => config(ctx, sources),
        DebugCmd::Info => info(ctx),
    }
}

fn database_label(ctx: &Context) -> (String, &'static str) {
    match ctx.database() {
        DatabaseLocation::File(path) => (path.display().to_string(), "full"),
        DatabaseLocation::Memory => (":memory:".into(), "ephemeral"),
    }
}

fn paths(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let p = &ctx.paths;
    let (database, _) = database_label(ctx);
    if output::is_json(global.format) {
        return output::json(&json!({
            "data": p.data, "config": p.config, "state": p.state, "cache": p.cache,
            "tmp": p.tmp, "database": database, "location": ctx.location,
        }));
    }
    output::rows(&[
        ("data", p.data.display().to_string()),
        ("config", p.config.display().to_string()),
        ("state", p.state.display().to_string()),
        ("cache", p.cache.display().to_string()),
        ("tmp", p.tmp.display().to_string()),
        ("database", database),
        ("location", ctx.location.display().to_string()),
    ]);
    Ok(())
}

fn config(ctx: &Context, sources: bool) -> Result<(), CliError> {
    let resolved = ctx.config()?;
    report_warnings(&resolved.warnings, &resolved.trust);
    let value = redact_secrets(&resolved.value);
    if sources {
        return output::json(
            &json!({ "value": value, "sources": resolved.sources, "layers": resolved.layers }),
        );
    }
    output::json(&value)
}

fn info(ctx: &Context) -> Result<(), CliError> {
    let resolved = ctx.config()?;
    report_warnings(&resolved.warnings, &resolved.trust);
    let (database, durability) = database_label(ctx);
    output::json(&json!({
        "build": ctx.build,
        "paths": ctx.paths,
        "location": ctx.location,
        "database": { "location": database, "durability": durability },
        "config": { "layers": resolved.layers, "warnings": resolved.warnings },
        "trust": resolved.trust,
    }))
}

fn report_warnings(warnings: &[String], trust: &cyber_core::config::TrustReport) {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    if !trust.trusted {
        eprintln!(
            "note: {} project definition(s) are inactive until approved; run \"cyber trust inspect\"",
            trust.definitions.len()
        );
    }
}
