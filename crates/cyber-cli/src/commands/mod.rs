//! Command dispatch.

pub mod db;
pub mod debug;
pub mod models;
pub mod trust;

use serde_json::json;

use crate::cli::{Cli, Command, GlobalArgs};
use crate::context::Context;
use crate::error::CliError;
use crate::{output, tree};

pub fn run(cli: Cli) -> Result<(), CliError> {
    if cli.version {
        return version(&cli.global);
    }
    let Some(command) = cli.command else {
        return Err(CliError::unavailable("the interactive TUI", "M0.5")
            .with_hint("inspect this build with \"cyber debug info\""));
    };
    if let Command::External(args) = &command {
        return Err(tree::external(args));
    }
    let ctx = Context::new(&cli.global)?;
    match command {
        Command::Debug { cmd } => debug::run(cmd, &ctx, &cli.global),
        Command::Models(args) => models::run(args, &ctx, &cli.global),
        Command::Db { cmd } => db::run(cmd, &ctx, &cli.global),
        Command::Trust { cmd } => trust::run(cmd, &ctx, &cli.global),
        Command::External(_) => unreachable!("handled above"),
    }
}

fn version(global: &GlobalArgs) -> Result<(), CliError> {
    let info = cyber_core::version::build_info();
    if output::is_json(global.format) {
        return output::json(&json!({
            "name": "cyber",
            "version": info.version,
            "channel": info.channel,
            "git_sha": info.git_sha,
            "target": info.target,
        }));
    }
    println!("{info}");
    Ok(())
}
