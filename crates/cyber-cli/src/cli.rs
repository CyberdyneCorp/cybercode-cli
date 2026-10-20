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
    memory::MemoryArgs,
    models::ModelsArgs,
    review::ReviewArgs,
    serve::{ServeArgs, ServiceCmd},
    skills::{CommandsCmd, SkillsCmd},
    stats::StatsArgs,
    trust::TrustCmd,
    workflows::WorkflowsCmd,
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

    /// Disable plugins and every hook and MCP server not managed by organization policy
    /// for this process. Also CYBER_PURE=1.
    #[arg(long, global = true)]
    pub pure: bool,

    /// Turn features on or off for this process, as a comma list where `-name` turns one
    /// off (`teams,-other`). Above CYBER_FEATURES and config; below organization policy.
    /// Commands served by the background server (`cyber teams list`, the TUI) follow its
    /// configuration instead: use `cyber features enable`, or the TUI with `--embedded`.
    #[arg(long, global = true, value_name = "LIST", allow_hyphen_values = true)]
    pub features: Option<String>,
}

/// TUI launch flags (`tui` → Launch flags).
#[derive(Debug, Default, Args)]
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
    /// Allow `bypass` mode in an embedded server without a sandbox or a container.
    #[arg(long, requires = "embedded")]
    pub dangerously_bypass_permissions: bool,
    /// Claude Code's name for running without prompts: start in `bypass` mode with
    /// --dangerously-bypass-permissions, so every action runs without approval and,
    /// outside a sandbox or a container, without OS confinement. Needs --embedded: the
    /// shared server cannot take the override for one TUI (there, use --mode bypass where
    /// its sandbox or a container already admits it). An organization policy that
    /// disables `bypass` still refuses it.
    #[arg(long)]
    pub dangerously_skip_permissions: bool,
    /// Open the Session picker over the Sessions linked to this pull request.
    #[arg(long, value_name = "NUMBER|URL")]
    pub from_pr: Option<String>,
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
    /// Import Claude Code, Codex or OpenCode configuration: detect, preview or write.
    Import(crate::commands::import::ImportArgs),
    /// Inspect installed language servers for the current Location.
    Lsp {
        #[command(subcommand)]
        cmd: crate::commands::intelligence::StatusCmd,
    },
    /// Inspect detected formatters for the current Location.
    Fmt {
        #[command(subcommand)]
        cmd: crate::commands::intelligence::StatusCmd,
    },
    /// Review and edit durable project/global memory.
    Memory(MemoryArgs),
    /// List, show, rename and delete the Location's Sessions.
    Sessions {
        #[command(subcommand)]
        cmd: crate::commands::sessions::SessionsCmd,
    },
    /// Inspect skills: source, scope, permission effect and diagnostics.
    Skills {
        #[command(subcommand)]
        cmd: SkillsCmd,
    },
    /// Inspect slash commands and command discovery diagnostics.
    Commands {
        #[command(subcommand)]
        cmd: CommandsCmd,
    },
    /// Review MCP server definitions, individual server approvals and OAuth credentials.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
    /// Agent teams (feature `teams`): list, show, stop and resume teams; list definitions.
    Teams {
        #[command(subcommand)]
        cmd: crate::commands::teams::TeamsCmd,
    },
    /// List the feature registry; enable or disable a feature in config.
    Features {
        #[command(subcommand)]
        cmd: crate::commands::features::FeaturesCmd,
    },
    /// List saved Workflows; run, inspect and control Workflow Runs.
    Workflows {
        #[command(subcommand)]
        cmd: WorkflowsCmd,
    },
    /// List, remove and prune managed Git worktrees of the current repository.
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
    /// Sign in to or out of a provider (today: `chatgpt`, a ChatGPT Plus/Pro subscription).
    Providers {
        #[command(subcommand)]
        cmd: crate::commands::providers::ProvidersCmd,
    },
    /// Dry-run permission rules; inspect or reset checkout-scoped auto-mode statistics.
    Permissions {
        #[command(subcommand)]
        cmd: crate::commands::permissions::PermissionsCmd,
    },
    /// Inspect the local database.
    Db {
        #[command(subcommand)]
        cmd: DbCmd,
    },
    /// Install, list, enable, disable, update and remove plugins.
    Plugins {
        #[command(subcommand)]
        cmd: crate::commands::plugins::PluginsCmd,
    },
    /// Review hook definitions and individual handler trust.
    Hooks {
        #[command(subcommand)]
        cmd: HooksCmd,
    },
    /// Test, explain or run a command under the effective OS sandbox policy.
    Sandbox {
        #[command(subcommand)]
        cmd: crate::commands::sandbox::SandboxCmd,
    },
    /// Inspect and approve repository-controlled configuration for this checkout.
    Trust {
        #[command(subcommand)]
        cmd: TrustCmd,
    },
    /// Run one prompt non-interactively (alias: cyber -p).
    Exec(Box<ExecArgs>),
    /// Open the TUI on a Session of a paired machine: cyber attach <session>@<peer>.
    Attach(crate::commands::attach::AttachArgs),
    /// Review uncommitted changes, a range, a commit or a pull request.
    Review(Box<ReviewArgs>),
    /// Check out a pull request and resume the Session linked to it.
    Pr(crate::commands::pr::PrArgs),
    /// Generate commit and pull request text with the small model.
    Git {
        #[command(subcommand)]
        cmd: crate::commands::git::GitCmd,
    },
    /// Run the server in the foreground.
    Serve(ServeArgs),
    /// Manage the background server.
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
    /// Send one request to the server API.
    Api(ApiArgs),
    /// Pair with other machines' cyber servers to message their Sessions.
    Peers {
        #[command(subcommand)]
        cmd: crate::commands::peers::PeersCmd,
    },
    /// Send messages between Sessions and read their message history.
    Messages {
        #[command(subcommand)]
        cmd: crate::commands::messages::MessagesCmd,
    },
    /// Check configuration, skills, commands, credentials, catalog, database, sandbox, tools and server.
    Doctor,
    /// Usage statistics: activity, streaks, tokens, cost and per-model totals.
    Stats(StatsArgs),
    /// Run evaluation manifests and write reports.
    Eval {
        #[command(subcommand)]
        cmd: crate::commands::eval::EvalCmd,
    },
    /// Print this command-line reference as Markdown (`just docs-cli` writes docs/cli.md).
    #[command(hide = true)]
    HelpMarkdown,
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
            let Some(Command::Exec(args)) = &cli.command else {
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
            let Some(Command::Exec(args)) = &cli.command else {
                panic!("expected exec")
            };
            assert!(crate::commands::exec::setup::validate(args, &cli.global).is_err());
        }
        let cli = Cli::try_parse_from(["cyber", "exec", "--worktree=../escape", "prompt"]).unwrap();
        let Some(Command::Exec(args)) = &cli.command else {
            panic!("expected exec")
        };
        assert!(crate::commands::exec::setup::validate(args, &cli.global).is_err());
    }
}
