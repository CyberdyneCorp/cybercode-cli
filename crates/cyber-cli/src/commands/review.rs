//! `cyber review [target]` (`cli-commands` → Review command): the `review`
//! command run like `cyber exec --command review`, reporting validated findings.

use clap::Args;
use cyber_core::review::Target;

use super::exec::{self, ExecArgs};
use crate::cli::{Format, GlobalArgs};
use crate::context::Context;
use crate::error::CliError;

#[derive(Debug, Args)]
#[command(
    after_help = "TARGET is empty (uncommitted changes: staged, unstaged and untracked), <base>..<head>, a single commit, or pr <number> (its commits must already be local). Put `--` before a target that starts with `-`."
)]
pub struct ReviewArgs {
    /// What to review: nothing for uncommitted changes, `<base>..<head>`, a commit, or `pr <number>`.
    #[arg(value_name = "TARGET")]
    pub target: Vec<String>,
    /// Start a new Session in a managed worktree; omit NAME to generate one.
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "NAME")]
    pub worktree: Option<String>,
    /// Reuse the most recent root Session of the Location.
    #[arg(long = "continue")]
    pub resume_last: bool,
    /// Reuse a Session by ID or name.
    #[arg(long)]
    pub session: Option<String>,
    /// Run in memory: nothing is written to the shared database or snapshots.
    #[arg(long)]
    pub ephemeral: bool,
    /// Run a private in-process server instead of the background service.
    #[arg(long)]
    pub embedded: bool,
    /// Target a remote server URL.
    #[arg(long)]
    pub attach: Option<String>,
    /// Password for --attach (else CYBER_SERVER_PASSWORD).
    #[arg(long)]
    pub password: Option<String>,
    #[arg(long)]
    pub max_tokens: Option<u64>,
    /// Stop after this cost in USD.
    #[arg(long)]
    pub max_cost: Option<f64>,
    /// Stop after this long, e.g. 30m, 90s.
    #[arg(long)]
    pub timeout: Option<String>,
    /// Exit with code 5 when any action was denied.
    #[arg(long)]
    pub fail_on_deny: bool,
}

impl ReviewArgs {
    fn exec(self, target: &Target) -> ExecArgs {
        ExecArgs {
            worktree: self.worktree,
            prompt: vec![target.normalized()],
            resume_last: self.resume_last,
            session: self.session,
            ephemeral: self.ephemeral,
            embedded: self.embedded,
            attach: self.attach,
            password: self.password,
            command: Some("review".into()),
            max_tokens: self.max_tokens,
            max_cost: self.max_cost,
            timeout: self.timeout,
            fail_on_deny: self.fail_on_deny,
            ..ExecArgs::default()
        }
    }
}

pub fn run(args: ReviewArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let format = global.format.unwrap_or(Format::Text);
    // Validated before any server contact: an invalid target admits nothing.
    let text = args.target.join(" ");
    let target = Target::parse(&text)
        .map_err(|error| CliError::silent(exec::report::target_rejected(format, &text, &error)))?;
    exec::review(args.exec(&target), ctx, global, target)
}
