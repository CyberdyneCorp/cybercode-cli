//! Argument definitions (`cli-commands`).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::commands::{
    api::ApiArgs,
    db::DbCmd,
    debug::DebugCmd,
    exec::ExecArgs,
    hooks::HooksCmd,
    mcp::McpCmd,
    models::ModelsArgs,
    serve::{ServeArgs, ServiceCmd},
    trust::TrustCmd,
    worktrees::WorktreeCmd,
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

    /// Minimum log level: error, warn, info (default), debug or trace. Also CYBER_LOG_LEVEL.
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log_level: Option<String>,

    /// Mirror log lines to stderr.
    #[arg(long, global = true)]
    pub print_logs: bool,

    /// Output format. The only output-format flag.
    #[arg(long, global = true, value_enum)]
    pub format: Option<Format>,
}

/// TUI launch flags (`tui` → Launch flags).
#[derive(Debug, Args)]
pub struct LaunchArgs {
    /// Start a new Session in a managed worktree; omit NAME to generate one.
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "NAME")]
    pub worktree: Option<String>,
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
    /// Review MCP server definitions and individual server approvals.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
    /// Inspect managed Git worktrees of the current repository.
    Worktree {
        #[command(subcommand)]
        cmd: WorktreeCmd,
    },
    /// Inspect resolved paths, configuration and build information.
    Debug {
        #[command(subcommand)]
        cmd: DebugCmd,
    },
    /// List models from the catalog, available first.
    Models(ModelsArgs),
    /// Inspect or reset checkout-scoped auto-mode statistics.
    Permissions {
        #[command(subcommand)]
        cmd: crate::commands::permissions::PermissionsCmd,
    },
    /// Inspect the local database.
    Db {
        #[command(subcommand)]
        cmd: DbCmd,
    },
    /// Review hook definitions and individual handler trust.
    Hooks {
        #[command(subcommand)]
        cmd: HooksCmd,
    },
    /// Inspect and approve repository-controlled configuration for this checkout.
    Trust {
        #[command(subcommand)]
        cmd: TrustCmd,
    },
    /// Run one prompt non-interactively (alias: cyber -p).
    Exec(Box<ExecArgs>),
    /// Run the server in the foreground.
    Serve(ServeArgs),
    /// Manage the background server.
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
    /// Send one request to the server API.
    Api(ApiArgs),
    /// Check configuration, credentials, catalog, database, sandbox, tools and server.
    Doctor,
    /// Run evaluation manifests and write reports.
    Eval {
        #[command(subcommand)]
        cmd: crate::commands::eval::EvalCmd,
    },
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[cfg(test)]
mod worktree_tests {
    use super::*;

    #[test]
    fn worktree_list_is_a_management_command() {
        let cli = Cli::try_parse_from(["cyber", "worktree", "list", "--format", "json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Worktree {
                cmd: WorktreeCmd::List
            })
        ));
        assert_eq!(cli.global.format, Some(Format::Json));
    }

    #[test]
    fn worktree_start_flags_support_named_and_generated_sessions() {
        let launch = Cli::try_parse_from(["cyber", "--worktree", "fix-login"]).unwrap();
        assert_eq!(launch.launch.worktree.as_deref(), Some("fix-login"));
        let launch = Cli::try_parse_from(["cyber", "--worktree"]).unwrap();
        assert_eq!(launch.launch.worktree.as_deref(), Some(""));
        for (args, name) in [
            (
                vec!["cyber", "exec", "--worktree", "fix-login", "fix"],
                "fix-login",
            ),
            (vec!["cyber", "exec", "--worktree", "--", "fix"], ""),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let Some(Command::Exec(args)) = cli.command else {
                panic!("expected exec")
            };
            assert_eq!(args.worktree.as_deref(), Some(name));
            assert_eq!(args.prompt, ["fix"]);
        }
    }

    #[test]
    fn isolated_exec_refuses_resume_ephemeral_and_invalid_names() {
        for option in ["--continue", "--ephemeral", "--fork"] {
            let cli =
                Cli::try_parse_from(["cyber", "exec", "--worktree=fix", option, "prompt"]).unwrap();
            let Some(Command::Exec(args)) = cli.command else {
                panic!("expected exec")
            };
            assert!(crate::commands::exec::setup::validate(&args).is_err());
        }
        let cli = Cli::try_parse_from(["cyber", "exec", "--worktree=../escape", "prompt"]).unwrap();
        let Some(Command::Exec(args)) = cli.command else {
            panic!("expected exec")
        };
        assert!(crate::commands::exec::setup::validate(&args).is_err());
    }
}
