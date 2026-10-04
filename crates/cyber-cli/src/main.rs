//! The `cyber` binary.

mod cli;
mod commands;
mod context;
mod error;
mod output;
mod tree;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = match cli::Cli::try_parse() {
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
    match commands::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            err.print(format);
            ExitCode::from(err.code)
        }
    }
}
