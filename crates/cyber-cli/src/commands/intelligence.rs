//! Local language-server and formatter discovery without model/database/process startup.
use crate::{cli::GlobalArgs, context::Context, error::CliError, output};
use cyber_core::{
    config::{FormatterSettings, LspSettings},
    intelligence::{ExecutableSearch, detect_formatters, detect_servers},
};
use serde_json::json;

#[derive(Debug, clap::Subcommand)]
pub enum StatusCmd {
    Status,
}

pub fn lsp(_cmd: StatusCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let config = ctx.config()?;
    let settings = LspSettings::from_config(&config.value).map_err(CliError::usage)?;
    let search = ExecutableSearch::new(&ctx.location, &ctx.paths.cache, &ctx.env);
    let servers = detect_servers(&settings, &search);
    let rows:Vec<_>=servers.iter().map(|s|json!({"id":s.definition.id,"enabled":s.enabled,"installed":s.installed,"running":false})).collect();
    if output::is_json(global.format) {
        return output::json(&rows);
    }
    println!("{:<28} {:<8} {:<10} RUNNING", "ID", "ENABLED", "INSTALLED");
    for server in servers {
        println!(
            "{:<28} {:<8} {:<10} false",
            server.definition.id, server.enabled, server.installed
        );
    }
    Ok(())
}
pub fn fmt(_cmd: StatusCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let config = ctx.config()?;
    let settings = FormatterSettings::from_config(&config.value).map_err(CliError::usage)?;
    let search = ExecutableSearch::new(&ctx.location, &ctx.paths.cache, &ctx.env);
    let formatters = detect_formatters(&settings, &search);
    let rows:Vec<_>=formatters.iter().map(|f|json!({"id":f.definition.id,"extensions":f.definition.extensions,"enabled":f.enabled,"installed":f.installed,"detected_by":f.detected_by})).collect();
    if output::is_json(global.format) {
        return output::json(&rows);
    }
    println!(
        "{:<24} {:<8} {:<30} DETECTED BY",
        "ID", "ENABLED", "EXTENSIONS"
    );
    for formatter in formatters {
        println!(
            "{:<24} {:<8} {:<30} {}",
            formatter.definition.id,
            formatter.enabled,
            formatter.definition.extensions.join(","),
            formatter.detected_by
        );
    }
    Ok(())
}
