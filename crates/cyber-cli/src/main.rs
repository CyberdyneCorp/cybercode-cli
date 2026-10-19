//! The `cyber` binary.

mod cli;
mod cli_docs;
mod commands;
mod context;
mod error;
mod output;
mod tree;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // Stamp the executable before anything can replace it, so a later rebuild in place
    // is told apart from this process (`server-api` → Background service management).
    let _ = cyber_core::version::binary();
    // `cyber -p ...` is `cyber exec ...`.
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "-p" || a == "--print") {
        args[1] = "exec".into();
    }
    let cli = match cli::Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            let code = e.exit_code();
            let _ = e.print();
            return ExitCode::from(u8::try_from(code).unwrap_or(2));
        }
    };
    let format = match &cli.command {
        Some(cli::Command::External(args)) => cli.global.format.or_else(|| tree::format_in(args)),
        _ => cli.global.format,
    };
    match run_with_stack(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            err.print(format);
            ExitCode::from(err.code)
        }
    }
}

/// Stack for running a command. The main thread gets 1 MiB on Windows (8 MiB on Linux
/// and macOS), which deep async state machines such as `cyber exec --worktree`
/// overflow, so every command runs on a thread with a stack larger than either.
const COMMAND_STACK: usize = 16 * 1024 * 1024;

fn run_with_stack(cli: cli::Cli) -> Result<(), error::CliError> {
    let command = std::thread::Builder::new()
        .name("cyber".into())
        .stack_size(COMMAND_STACK)
        .spawn(move || commands::run(cli));
    match command {
        Ok(handle) => handle
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
        Err(error) => Err(error::CliError::runtime(format!(
            "cannot start the command thread: {error}"
        ))),
    }
}
