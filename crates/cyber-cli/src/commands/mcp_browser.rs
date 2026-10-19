//! `cyber mcp browser install|health|cleanup` (`browser-verification` → Supported
//! optional browser integration). The browser is an MCP server, so its commands live
//! under `cyber mcp`; the command tree admits no new top-level command.
//!
//! - `install` writes the version-pinned browser server under `mcp.<server>` and turns
//!   `browser.enabled` on, in the user document by default. A project or local install
//!   is refused while the checkout has unapproved configuration, and the server then
//!   starts only once trust and its digest are approved.
//! - `health` starts the configured server once, under the same trust, approval and
//!   sandbox rules as a Session's browser, and checks the tools verification needs.
//! - `cleanup` removes per-Session profiles that no running process holds.

use clap::{Args, Subcommand};
use cyber_core::browser::{DEFAULT_SERVER, PINNED_PACKAGE, pinned_definition};
use cyber_core::trust::TrustStore;
use serde_json::json;

use super::mcp_config::Scope;
use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum BrowserCmd {
    /// Write the pinned browser MCP server and enable browser verification.
    Install(InstallArgs),
    /// Start the configured browser server once and check the tools it offers.
    Health,
    /// Remove per-Session browser profiles that no running process holds.
    Cleanup,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    #[arg(long, value_enum, default_value_t = Scope::User)]
    pub scope: Scope,
    /// The MCP server name (letters, digits, `-` and `_`).
    #[arg(long, default_value = DEFAULT_SERVER)]
    pub server: String,
}

pub fn run(cmd: BrowserCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    match cmd {
        BrowserCmd::Install(args) => install(&args, ctx, global),
        BrowserCmd::Health => health(ctx, global),
        BrowserCmd::Cleanup => cleanup(ctx, global),
    }
}

fn install(args: &InstallArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    super::mcp_config::valid_name(&args.server)?;
    if args.scope != Scope::User {
        let report = ctx.trust()?;
        if report.digest.is_some() && !report.trusted {
            return Err(CliError::usage(
                "this checkout's project configuration is not trusted; a project browser is not installed",
            )
            .with_hint("review it with cyber trust inspect, or install for yourself with --scope user"));
        }
    }
    let path = super::mcp_config::document(ctx, args.scope)?;
    let definition = pinned_definition();
    cyber_core::config::set_document_member(&path, "mcp", &args.server, Some(&definition))
        .map_err(CliError::runtime)?;
    cyber_core::config::set_document_member(&path, "browser", "enabled", Some(&json!(true)))
        .map_err(CliError::runtime)?;
    let server = (args.server != DEFAULT_SERVER).then(|| json!(args.server));
    cyber_core::config::set_document_member(&path, "browser", "server", server.as_ref())
        .map_err(CliError::runtime)?;
    let digest = serde_json::from_value::<cyber_core::config::McpServer>(definition.clone())
        .ok()
        .and_then(|server| server.digest(&args.server).ok());
    let approval = (args.scope != Scope::User).then(|| {
        format!(
            "the project browser starts after approval: cyber trust approve, then cyber mcp trust {} --digest {}",
            args.server,
            digest.as_deref().unwrap_or("<digest>")
        )
    });
    if output::is_json(global.format) {
        return output::json(&json!({
            "server": args.server, "file": path, "definition": definition,
            "package": PINNED_PACKAGE, "digest": digest, "approval": approval,
        }));
    }
    println!(
        "installed browser server {} ({PINNED_PACKAGE}) in {}",
        args.server,
        path.display()
    );
    if let Some(approval) = approval {
        println!("{approval}");
    }
    println!("each Session gets its own browser profile; check it with: cyber mcp browser health");
    Ok(())
}

fn health(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let resolved = ctx.config()?;
    let trust = TrustStore::new(ctx.paths.trust_file());
    let root = ctx.paths.data.join("browser");
    let helper = cyber_sandbox::find_helper();
    let location = std::fs::canonicalize(&ctx.location)?;
    let health = super::serve::runtime()?
        .block_on(cyber_tools::browser_health(
            cyber_tools::BrowserHealthCheck {
                resolved: &resolved,
                trust: &trust,
                location: &location,
                home: &ctx.home,
                browser_root: &root,
                helper: helper.as_deref(),
            },
        ))
        .map_err(CliError::runtime)?;
    if output::is_json(global.format) {
        output::json(&json!({
            "server": health.server, "pinned": health.pinned,
            "tools": health.tools, "missing": health.missing, "healthy": health.healthy(),
        }))?;
    } else {
        output::rows(&[
            ("server", health.server.clone()),
            ("pinned", health.pinned.to_string()),
            ("tools", health.tools.join(", ")),
            ("missing", missing(&health.missing)),
        ]);
    }
    if health.healthy() {
        return Ok(());
    }
    Err(CliError::runtime(format!(
        "browser server {} does not offer {}",
        health.server,
        health.missing.join(", ")
    )))
}

fn missing(names: &[String]) -> String {
    if names.is_empty() {
        "none".into()
    } else {
        names.join(", ")
    }
}

fn cleanup(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let report = cyber_tools::cleanup_browser_profiles(&ctx.paths.data.join("browser"))?;
    if output::is_json(global.format) {
        return output::json(&json!({"removed": report.removed, "in_use": report.in_use}));
    }
    println!(
        "removed {} browser profile(s); {} in use",
        report.removed.len(),
        report.in_use.len()
    );
    Ok(())
}
