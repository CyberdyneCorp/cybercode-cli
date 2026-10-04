//! Argument definitions (`cli-commands`).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::commands::{
    api::ApiArgs,
    db::DbCmd,
    debug::DebugCmd,
    exec::ExecArgs,
    models::ModelsArgs,
    serve::{ServeArgs, ServiceCmd},
    trust::TrustCmd,
};

#[derive(Debug, Parser)]
#[command(
    name = "cyber",
    about = "Cyber Code: a model-independent coding agent you can interrupt, inspect and resume",
    disable_version_flag = true,
    allow_external_subcommands = true
)]
pub struct Cli {
    /// Print version information.
    #[arg(short = 'V', long)]
    pub version: bool,

    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(flatten)]
    pub launch: LaunchArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// Working directory (the Location). The only working-directory flag.
    #[arg(long, global = true, value_name = "PATH")]
    pub cwd: Option<PathBuf>,

    /// Named profile from the `profiles` config key.
    #[arg(long, global = true, value_name = "NAME")]
    pub profile: Option<String>,

    /// Set one config value for this process, as a dotted key and a JSON or string value.
    #[arg(long = "config", global = true, value_name = "KEY=VALUE")]
    pub config: Vec<String>,

    /// Model as provider/model[#variant].
    #[arg(short = 'm', long, global = true, value_name = "MODEL")]
    pub model: Option<String>,

    /// Permission mode: default, accept-edits, plan, auto, dont-ask or bypass.
    #[arg(long, global = true, value_name = "MODE")]
    pub mode: Option<String>,

    /// Output format. The only output-format flag.
    #[arg(long, global = true, value_enum)]
    pub format: Option<Format>,
}

/// TUI launch flags (`tui` → Launch flags).
#[derive(Debug, Args)]
pub struct LaunchArgs {
    /// Resume the most recent Session of the Location.
    #[arg(short = 'c', long = "continue")]
    pub resume_last: bool,
    /// Resume a Session by ID or name; without a value, open the picker.
    #[arg(short = 'r', long, num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,
    /// Copy the selected Session into a new one.
    #[arg(long)]
    pub fork: bool,
    #[arg(long)]
    pub agent: Option<String>,
    /// Submit an initial prompt (piped stdin is prepended).
    #[arg(long)]
    pub prompt: Option<String>,
    /// Run a private in-process server instead of the background service.
    #[arg(long)]
    pub embedded: bool,
    /// Sandbox policy for an embedded server: read-only, workspace-write or full-access.
    #[arg(long)]
    pub sandbox: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Table,
    Json,
    Text,
    StreamJson,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect resolved paths, configuration and build information.
    Debug {
        #[command(subcommand)]
        cmd: DebugCmd,
    },
    /// List models from the catalog, available first.
    Models(ModelsArgs),
    /// Inspect the local database.
    Db {
        #[command(subcommand)]
        cmd: DbCmd,
    },
    /// Inspect and approve repository-controlled configuration for this checkout.
    Trust {
        #[command(subcommand)]
        cmd: TrustCmd,
    },
    /// Run one prompt non-interactively (alias: cyber -p).
    Exec(ExecArgs),
    /// Run the server in the foreground.
    Serve(ServeArgs),
    /// Manage the background server.
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
    /// Send one request to the server API.
    Api(ApiArgs),
    /// Run evaluation manifests and write reports.
    Eval {
        #[command(subcommand)]
        cmd: crate::commands::eval::EvalCmd,
    },
    #[command(external_subcommand)]
    External(Vec<String>),
}
