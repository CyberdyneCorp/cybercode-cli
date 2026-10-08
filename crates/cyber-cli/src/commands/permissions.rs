//! `cyber permissions auto show|reset`.
use crate::{cli::GlobalArgs, context::Context, error::CliError, output};
use clap::Subcommand;
use cyber_server::runtime::{Runtime, auto_statistics};
use cyber_store::{Store, StoreOptions};

#[derive(Debug, Subcommand)]
pub enum PermissionsCmd {
    /// Show or reset recorded auto-mode decision statistics for this checkout.
    Auto {
        #[command(subcommand)]
        cmd: AutoCmd,
    },
}
#[derive(Debug, Subcommand)]
pub enum AutoCmd {
    Show,
    Reset,
}
pub fn run(command: PermissionsCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let root = cyber_core::config::project_root(&ctx.location);
    let root =
        std::fs::canonicalize(&root).map_err(|error| CliError::runtime(error.to_string()))?;
    let root = root.display().to_string();
    let store = Store::open(StoreOptions::new(ctx.database(), Runtime::registry()))?;
    let PermissionsCmd::Auto { cmd } = command;
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
