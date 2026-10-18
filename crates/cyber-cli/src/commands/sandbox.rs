//! `cyber sandbox test|explain|run` (`sandbox` → Sandbox CLI).

use clap::{Args, Subcommand};
use cyber_core::paths::DatabaseLocation;
use cyber_tools::sandbox_cli::{Explanation, Probe};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::{CliError, EXIT_RUNTIME};
use crate::output;

#[derive(Debug, Subcommand)]
pub enum SandboxCmd {
    /// Run read, write and network probes under the effective policy.
    Test(Selection),
    /// Print the effective writable roots, network mode, domains and masked variables.
    Explain(Selection),
    /// Run one command under the effective policy.
    Run {
        #[command(flatten)]
        selection: Selection,
        /// The command and its arguments, after `--`.
        #[arg(last = true, required = true, value_name = "COMMAND")]
        command: Vec<String>,
    },
}

/// The policy to evaluate instead of the configured one. Under `cyber sandbox`, the
/// global `--profile` names a sandbox profile (a built-in `read-only`, `workspace` or
/// `full`, or a `sandbox.profiles` entry), not a configuration profile.
#[derive(Debug, Default, Args)]
pub struct Selection {
    /// A policy: read-only, workspace-write or full-access.
    #[arg(long)]
    pub policy: Option<String>,
    #[arg(skip)]
    pub profile: Option<String>,
}

impl Selection {
    fn chosen(&self) -> Option<String> {
        self.policy.clone().or_else(|| self.profile.clone())
    }
}

impl SandboxCmd {
    /// Take the global `--profile` as the sandbox profile.
    pub fn select_profile(&mut self, profile: Option<String>) -> Result<(), CliError> {
        let selection = match self {
            Self::Test(selection) | Self::Explain(selection) => selection,
            Self::Run { selection, .. } => selection,
        };
        if profile.is_some() && selection.policy.is_some() {
            return Err(CliError::usage("give --policy or --profile, not both"));
        }
        selection.profile = profile;
        Ok(())
    }
}

pub fn run(cmd: SandboxCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    match cmd {
        SandboxCmd::Explain(selection) => {
            let host = ctx.local_host(DatabaseLocation::Memory, selection.chosen())?;
            explain(&host.sandbox_explain(&ctx.location), global)
        }
        SandboxCmd::Test(selection) => {
            let host = ctx.local_host(DatabaseLocation::Memory, selection.chosen())?;
            let probes = runtime
                .block_on(host.sandbox_probes(&ctx.location))
                .map_err(CliError::runtime)?;
            report(&probes, global)
        }
        SandboxCmd::Run { selection, command } => {
            let host = ctx.local_host(DatabaseLocation::Memory, selection.chosen())?;
            runtime.block_on(run_one(&host, ctx, &command))
        }
    }
}

async fn run_one(
    host: &cyber_tools::BuiltinHost,
    ctx: &Context,
    command: &[String],
) -> Result<(), CliError> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| CliError::usage("give a command after --"))?;
    let prepared = host
        .sandbox_command(&ctx.location, program, args)
        .await
        .map_err(CliError::runtime)?;
    let status = tokio::process::Command::new(&prepared.program)
        .args(&prepared.args)
        .env_clear()
        .envs(prepared.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&ctx.location)
        .status()
        .await
        .map_err(|error| CliError::runtime(format!("could not start {program}: {error}")))?;
    for refused in prepared
        .refused
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
    {
        eprintln!("Sandbox denied: network {refused}");
    }
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(CliError::silent(u8::try_from(code).unwrap_or(EXIT_RUNTIME))),
        None => Err(CliError::silent(EXIT_RUNTIME)),
    }
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        "(none)".into()
    } else {
        items.join(", ")
    }
}

fn explain(e: &Explanation, global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(e);
    }
    let block = |title: &str, items: &[String]| {
        println!("{title}:");
        if items.is_empty() {
            println!("  (none)");
        }
        for item in items {
            println!("  {item}");
        }
    };
    println!("profile: {}", e.profile);
    println!("policy: {}", e.policy);
    println!("enforcement: {}", e.enforcement);
    println!("isolation: {}", e.isolation);
    block("writable roots", &e.writable_roots);
    block("read-only inside roots", &e.read_only);
    block("unreadable", &e.unreadable);
    block("deny_read", &e.deny_read);
    println!("network: {}", e.network);
    println!("allowed domains: {}", list(&e.allowed_domains));
    println!("denied domains: {}", list(&e.denied_domains));
    println!(
        "environment: {} (login shell: {})",
        e.env_mode,
        if e.login_shell { "yes" } else { "no" }
    );
    let set: Vec<String> = e.env_set.iter().map(|(k, v)| format!("{k}={v}")).collect();
    println!("set: {}", list(&set));
    println!("masked variables: {}", list(&e.masked));
    println!("excluded commands: {}", list(&e.excluded_commands));
    if let Some(notice) = &e.notice {
        println!("notice: {notice}");
    }
    if let Some(error) = &e.error {
        println!("error: {error}");
    }
    Ok(())
}

fn report(probes: &[Probe], global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        output::json(&probes)?;
    } else {
        for p in probes {
            let verdict = if p.passed { "PASS" } else { "FAIL" };
            println!(
                "{verdict}  {:<7} {}  expected {}, got {}",
                p.operation, p.target, p.expected, p.actual
            );
        }
    }
    if probes.iter().all(|p| p.passed) {
        Ok(())
    } else {
        Err(CliError::silent(EXIT_RUNTIME))
    }
}
