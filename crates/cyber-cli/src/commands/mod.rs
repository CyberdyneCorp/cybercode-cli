//! Command dispatch.

pub mod api;
pub mod db;
pub mod debug;
pub mod doctor;
pub mod eval;
pub mod exec;
pub mod models;
pub mod permissions;
pub mod serve;
pub mod trust;
pub mod tui;

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
        return tui::run(&cli.launch, &cli.global, None);
    };
    if let Command::External(args) = &command {
        // `cyber [project]`: a directory argument opens the TUI there.
        if let [project] = args.as_slice()
            && std::path::Path::new(project).is_dir()
        {
            return tui::run(&cli.launch, &cli.global, Some(project));
        }
        return Err(tree::external(args));
    }
    let ctx = Context::new(&cli.global)?;
    start_logging(&ctx, &cli.global)?;
    match command {
        Command::Worktree { cmd } => worktrees::run(cmd, &ctx, &cli.global),
        Command::Debug { cmd } => debug::run(cmd, &ctx, &cli.global),
        Command::Models(args) => models::run(args, &ctx, &cli.global),
        Command::Permissions { cmd } => permissions::run(cmd, &ctx, &cli.global),
        Command::Db { cmd } => db::run(cmd, &ctx, &cli.global),
        Command::Trust { cmd } => trust::run(cmd, &ctx, &cli.global),
        Command::Serve(args) => serve::serve(args, &ctx),
        Command::Service { cmd } => serve::service(cmd, &ctx, &cli.global),
        Command::Api(args) => api::run(args, &ctx),
        Command::Exec(args) => exec::run(*args, &ctx, &cli.global),
        Command::Doctor => doctor::run(&ctx, &cli.global),
        Command::Eval { cmd } => eval::run(cmd, &ctx),
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

/// `<data>/log`, at `--log-level`, else `CYBER_LOG_LEVEL`, else `info`.
pub(crate) fn start_logging(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let requested = global
        .log_level
        .clone()
        .or_else(|| std::env::var("CYBER_LOG_LEVEL").ok());
    let level = match requested {
        Some(name) => cyber_core::log::Level::parse(&name).ok_or_else(|| {
            CliError::usage(format!(
                "unknown log level {name:?}; use error, warn, info, debug or trace"
            ))
        })?,
        None => cyber_core::log::Level::Info,
    };
    cyber_core::log::init(&ctx.paths.data.join("log"), level, global.print_logs);
    cyber_core::log::debug(
        "cli",
        "start",
        json!({ "args": std::env::args().skip(1).collect::<Vec<_>>() }),
    );
    Ok(())
}

pub(crate) mod worktrees;
