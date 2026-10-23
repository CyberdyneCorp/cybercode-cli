//! Command dispatch.

pub mod api;
pub mod attach;
mod branches;
pub mod db;
pub mod debug;
pub mod doctor;
pub mod eval;
pub mod exec;
pub mod features;
pub mod git;
pub mod hooks;
pub mod import;
pub mod intelligence;
pub mod mcp;
pub mod mcp_browser;
pub mod mcp_config;
pub mod mcp_serve;
pub mod memory;
pub mod messages;
pub mod models;
pub mod peers;
pub mod permissions;
pub mod plugin_config;
pub mod plugins;
pub mod pr;
pub mod providers;
pub mod review;
pub mod sandbox;
pub mod serve;
pub mod sessions;
pub mod skills;
pub mod stats;
pub mod teams;
pub mod trust;
pub mod tui;
pub mod workflows;

use serde_json::json;

use crate::cli::{Cli, Command, GlobalArgs};
use crate::context::Context;
use crate::error::CliError;
use crate::{output, tree};

pub fn run(mut cli: Cli) -> Result<(), CliError> {
    // `cyber sandbox … --profile <name>` names a sandbox profile, not a configuration one.
    if let Some(Command::Sandbox { cmd }) = &mut cli.command {
        cmd.select_profile(cli.global.profile.take())?;
    }
    if cli.version {
        return version(&cli.global);
    }
    if let Some(features) = &cli.global.features {
        cyber_core::features::set_flag(features);
    }
    if cli.global.pure || cyber_core::plugins::pure(&cyber_core::env::ProcessEnv) {
        pure(&mut cli)?;
    }
    html_only_for_export(&cli)?;
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
    if let Command::Import(args) = &command {
        return import::run(args, &cli.global);
    }
    if let Command::HelpMarkdown = &command {
        print!("{}", crate::cli_docs::markdown());
        return Ok(());
    }
    let ctx = Context::new(&cli.global)?;
    // The MCP serve bridge logs to its own file and never loads the project configuration
    // first.
    let command = match command {
        Command::Mcp {
            cmd: mcp::McpCmd::Serve(args),
        } => return mcp_serve::run(args, &ctx),
        command => command,
    };
    start_logging(&ctx, &cli.global)?;
    match command {
        Command::Lsp { cmd } => intelligence::lsp(cmd, &ctx, &cli.global),
        Command::Fmt { cmd } => intelligence::fmt(cmd, &ctx, &cli.global),
        Command::Memory(args) => memory::run(args, &ctx, &cli.global),
        Command::Mcp { cmd } => mcp::run(cmd, &ctx, &cli.global),
        Command::Sessions { cmd } => sessions::run(cmd, &ctx, &cli.global),
        Command::Skills { cmd } => skills::run(cmd, &ctx, &cli.global),
        Command::Commands { cmd } => skills::run_commands(cmd, &ctx, &cli.global),
        Command::Worktree { cmd } => worktrees::run(cmd, &ctx, &cli.global),
        Command::Workflows { cmd } => workflows::run(cmd, &ctx, &cli.global),
        Command::Teams { cmd } => teams::run(cmd, &ctx, &cli.global),
        Command::Features { cmd } => features::run(cmd, &ctx, &cli.global),
        Command::Debug { cmd } => debug::run(cmd, &ctx, &cli.global),
        Command::Models(args) => models::run(args, &ctx, &cli.global),
        Command::Providers { cmd } => providers::run(cmd, &ctx, &cli.global),
        Command::Permissions { cmd } => permissions::run(cmd, &ctx, &cli.global),
        Command::Db { cmd } => db::run(cmd, &ctx, &cli.global),
        Command::Trust { cmd } => trust::run(cmd, &ctx, &cli.global),
        Command::Sandbox { cmd } => sandbox::run(cmd, &ctx, &cli.global),
        Command::Hooks { cmd } => hooks::run(cmd, &ctx, &cli.global),
        Command::Plugins { cmd } => plugins::run(cmd, &ctx, &cli.global),
        Command::Serve(args) => serve::serve(args, &ctx),
        Command::Service { cmd } => serve::service(cmd, &ctx, &cli.global),
        Command::Api(args) => api::run(args, &ctx),
        Command::Messages { cmd } => messages::run(cmd, &ctx, &cli.global),
        Command::Peers { cmd } => peers::run(cmd, &ctx, &cli.global),
        Command::Attach(args) => attach::run(&args, &ctx),
        Command::Exec(args) => exec::run(*args, &ctx, &cli.global),
        Command::Review(args) => review::run(*args, &ctx, &cli.global),
        Command::Git { cmd } => git::run(cmd, &ctx, &cli.global),
        Command::Pr(args) => pr::run(args, &ctx, &cli.global),
        Command::Doctor => doctor::run(&ctx, &cli.global),
        Command::Stats(args) => stats::run(args, &ctx, &cli.global),
        Command::Eval { cmd } => eval::run(cmd, &ctx),
        Command::External(_) | Command::Import(_) | Command::HelpMarkdown => {
            unreachable!("handled above")
        }
    }
}

/// `--pure` (`plugins-marketplace` → Pure mode): every configuration load in this
/// process leaves out plugins and non-managed hooks and MCP servers. A shared server
/// cannot honor that for one client, so every command that runs Turns runs its own
/// in-process server, and the ones that can only start Turns on the shared server are
/// refused.
fn pure(cli: &mut Cli) -> Result<(), CliError> {
    cyber_core::plugins::force_pure();
    match &mut cli.command {
        Some(Command::Exec(args)) => in_process(&mut args.embedded, args.ephemeral, &args.attach),
        Some(Command::Review(args)) => in_process(&mut args.embedded, args.ephemeral, &args.attach),
        None | Some(Command::External(_)) => {
            cli.launch.embedded = true;
            Ok(())
        }
        Some(command) => match shared_turns(command) {
            Some(name) => Err(CliError::usage(format!(
                "--pure cannot run `cyber {name}`: it starts Turns on the shared server"
            ))),
            None => Ok(()),
        },
    }
}

/// Exec and review under `--pure`: never attach, run in process unless ephemeral.
fn in_process(
    embedded: &mut bool,
    ephemeral: bool,
    attach: &Option<String>,
) -> Result<(), CliError> {
    if attach.is_some() {
        return Err(CliError::usage(
            "--pure cannot attach to a shared server; it runs an in-process one",
        ));
    }
    *embedded |= !ephemeral;
    Ok(())
}

/// The commands that start Turns on the shared server and have no in-process form.
fn shared_turns(command: &Command) -> Option<&'static str> {
    match command {
        Command::Api(_) => Some("api"),
        Command::Messages { .. } => Some("messages"),
        Command::Pr(_) => Some("pr"),
        Command::Mcp {
            cmd: mcp::McpCmd::Serve(_),
        } => Some("mcp serve"),
        Command::Workflows {
            cmd: workflows::WorkflowsCmd::Run(_),
        } => Some("workflows run"),
        Command::Workflows {
            cmd: workflows::WorkflowsCmd::Resume { .. },
        } => Some("workflows resume"),
        _ => None,
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

/// `<tag>: <message>` for a server error, once: peer errors already open their message
/// with their tag (`PeerUnreachableError: peer `desk` is unreachable`).
pub(crate) fn tagged(tag: &str, message: &str) -> String {
    match message.strip_prefix(tag) {
        Some(rest) if tag.is_empty() || rest.starts_with(':') => message.to_owned(),
        _ => format!("{tag}: {message}"),
    }
}

pub(crate) mod worktrees;

/// `--format html` belongs to `cyber sessions export` alone; anything else refuses it.
fn html_only_for_export(cli: &Cli) -> Result<(), CliError> {
    let export = matches!(
        &cli.command,
        Some(Command::Sessions {
            cmd: sessions::SessionsCmd::Export { .. }
        })
    );
    if cli.global.format == Some(crate::cli::Format::Html) && !export {
        return Err(CliError::usage(
            "--format html is only for \"cyber sessions export\"",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tagged_tests {
    /// Regression: `cyber messages send` and `cyber attach` printed
    /// `PeerUnreachableError: PeerUnreachableError: peer `desk` is unreachable`.
    #[test]
    fn a_message_that_names_its_tag_is_not_prefixed_again() {
        let peer = "PeerUnreachableError: peer `desk` is unreachable: connection failed";
        assert_eq!(super::tagged("PeerUnreachableError", peer), peer);
        assert_eq!(
            super::tagged("ConflictError", "a grant named `laptop` is active"),
            "ConflictError: a grant named `laptop` is active"
        );
        assert_eq!(
            super::tagged("PeerAuthError", "PeerAuthErrors happen"),
            "PeerAuthError: PeerAuthErrors happen"
        );
    }
}
