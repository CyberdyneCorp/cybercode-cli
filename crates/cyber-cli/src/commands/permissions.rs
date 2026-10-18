//! `cyber permissions auto show|reset` and `cyber permissions test`.
use crate::{cli::GlobalArgs, context::Context, error::CliError, output};
use clap::{Args, Subcommand};
use cyber_server::runtime::{Runtime, auto_statistics};
use cyber_store::{Store, StoreOptions};

#[derive(Debug, Subcommand)]
pub enum PermissionsCmd {
    /// Show or reset recorded auto-mode decision statistics for this checkout.
    Auto {
        #[command(subcommand)]
        cmd: AutoCmd,
    },
    /// Evaluate a tool call or shell command as a Session would, without running it.
    Test(TestArgs),
}

/// `cyber permissions test [--agent a] [--mode m] (--tool t --resource r | -- <command>)`.
#[derive(Debug, Args)]
pub struct TestArgs {
    #[arg(long)]
    pub agent: Option<String>,
    #[arg(long, requires = "resource", conflicts_with = "command")]
    pub tool: Option<String>,
    #[arg(long, requires = "tool")]
    pub resource: Option<String>,
    /// A shell command, split into commands as `bash` splits it.
    #[arg(last = true, value_name = "COMMAND")]
    pub command: Vec<String>,
}
#[derive(Debug, Subcommand)]
pub enum AutoCmd {
    Show,
    Reset,
}
pub fn run(command: PermissionsCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let cmd = match command {
        PermissionsCmd::Test(args) => return test(args, ctx, global),
        PermissionsCmd::Auto { cmd } => cmd,
    };
    let root = cyber_core::config::project_root(&ctx.location);
    let root =
        std::fs::canonicalize(&root).map_err(|error| CliError::runtime(error.to_string()))?;
    let root = root.display().to_string();
    let store = Store::open(StoreOptions::new(ctx.database(), Runtime::registry()))?;
    if matches!(cmd, AutoCmd::Reset) {
        auto_statistics::reset(&store, &root)?;
    }
    let stats = auto_statistics::show(&store, &root)?;
    if output::is_json(global.format) {
        return output::json(&stats);
    }
    output::rows(&[
        ("checkout", stats.checkout_root),
        ("allowed", stats.allowed.to_string()),
        ("blocked", stats.blocked.to_string()),
        ("fallback", stats.fallback.to_string()),
        ("classifier decisions", stats.classifier.to_string()),
        ("policy decisions", stats.policy.to_string()),
        (
            "recorded since",
            stats
                .recorded_since
                .map_or("none".into(), |time| time.to_string()),
        ),
    ]);
    Ok(())
}

fn test(args: TestArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    if args.tool.is_none() && args.command.is_empty() {
        return Err(CliError::usage(
            "give --tool and --resource, or a shell command after --",
        ));
    }
    let query = cyber_tools::PermissionTest {
        agent: args.agent,
        mode: global.mode.clone(),
        tool: args.tool,
        resource: args.resource,
        command: (!args.command.is_empty()).then(|| args.command.join(" ")),
    };
    let host = ctx.local_host(ctx.database(), None)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let report = runtime
        .block_on(host.permission_test(&ctx.location, &query))
        .map_err(CliError::usage)?;
    if output::is_json(global.format) {
        return output::json(&report);
    }
    for row in &report.results {
        let effect = serde_json::to_value(row.effect)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        println!("{} \u{2192} {effect} ({})", row.resource, row.reason);
    }
    Ok(())
}
