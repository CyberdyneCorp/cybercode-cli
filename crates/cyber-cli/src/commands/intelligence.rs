//! Local language-server and formatter discovery without model/database/process startup.
use crate::{cli::GlobalArgs, context::Context, error::CliError, output};
use cyber_core::{
    config::{FormatterSettings, LspSettings},
    intelligence::{ExecutableSearch, detect_formatters, detect_servers},
};
use cyber_server::http::{LspState, LspStatus};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, clap::Subcommand)]
pub enum StatusCmd {
    Status,
}

pub fn lsp(_cmd: StatusCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let config = ctx.config()?;
    let settings = LspSettings::from_config(&config.value).map_err(CliError::usage)?;
    let search = ExecutableSearch::new(&ctx.location, &ctx.paths.cache, &ctx.env);
    let servers = detect_servers(&settings, &search);
    let live = super::serve::runtime()?
        .block_on(cyber_app::registered_lsp_status(&ctx.paths, &ctx.location))
        .map_err(CliError::runtime)?;
    let mut rows: BTreeMap<String, LspRow> = servers
        .into_iter()
        .map(|server| {
            let id = server.definition.id;
            (
                id.clone(),
                LspRow {
                    id,
                    enabled: Some(server.enabled),
                    installed: Some(server.installed),
                    running: live.as_ref().map(|_| false),
                    roots: live.as_ref().map(|_| Vec::new()),
                },
            )
        })
        .collect();
    if let Some(live) = live {
        for status in live {
            let row = rows.entry(status.id.clone()).or_insert_with(|| LspRow {
                id: status.id.clone(),
                enabled: None,
                installed: None,
                running: Some(false),
                roots: Some(Vec::new()),
            });
            row.roots.as_mut().unwrap().push(status);
        }
        for row in rows.values_mut() {
            row.running = observed_running(row.roots.as_ref().unwrap());
        }
    }
    let rows: Vec<_> = rows.into_values().collect();
    if output::is_json(global.format) {
        return output::json(&rows);
    }
    println!("{:<28} {:<8} {:<10} RUNNING", "ID", "ENABLED", "INSTALLED");
    for server in rows {
        println!(
            "{:<28} {:<8} {:<10} {}",
            escaped(&server.id),
            boolean(server.enabled),
            boolean(server.installed),
            boolean(server.running)
        );
        for root in server.roots.into_iter().flatten() {
            let state = match root.status {
                LspState::Starting => "starting",
                LspState::Connected => "connected",
                LspState::Broken => "broken",
            };
            println!("  {state}: {}", escaped(&root.root.to_string_lossy()));
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct LspRow {
    id: String,
    enabled: Option<bool>,
    installed: Option<bool>,
    running: Option<bool>,
    roots: Option<Vec<LspStatus>>,
}

fn observed_running(roots: &[LspStatus]) -> Option<bool> {
    if roots
        .iter()
        .any(|root| matches!(root.status, LspState::Connected))
    {
        Some(true)
    } else if roots.is_empty() {
        Some(false)
    } else {
        None
    }
}

fn boolean(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}
fn escaped(value: &str) -> String {
    value.chars().flat_map(char::escape_debug).collect()
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
