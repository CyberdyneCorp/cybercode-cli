//! `cyber upgrade` (`installation-upgrade` → Upgrade command): resolve the target version
//! from the argument or the channel manifest and the installation method from the
//! running binary, then skip, replace the binary after verification, run the package
//! manager, or print the manual command. A manifest's version is installed only when it
//! is newer; an older one needs `cyber upgrade <version>`. A manager known from its
//! listing alone is only named: its package may be another install than this binary.

use std::time::Duration;

use clap::Args;
use cyber_core::env::EnvSource;
use cyber_core::version::{Channel, ReleaseSource};
use cyber_install::manifest::Lookup;
use cyber_install::method::{self, Detected, InstallMethod, Probe};
use cyber_install::policy;
use cyber_install::upgrade::{Plan, SelfUpdate};
use serde_json::json;

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

/// Connect and per-read timeout of the release downloads.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Args)]
pub struct UpgradeArgs {
    /// Version to install (`1.4.0` or `v1.4.0`); defaults to the channel's latest.
    pub version: Option<String>,
    /// Install method instead of the detected one: script, brew, npm, pnpm, bun, cargo,
    /// scoop, winget, aur, nix, docker or unknown.
    #[arg(long, value_parser = parse_method)]
    pub method: Option<InstallMethod>,
    /// Channel to read the latest version from; defaults to the build's (`dev` reads
    /// `stable`).
    #[arg(long, value_parser = ["stable", "beta", "nightly"])]
    pub channel: Option<String>,
    /// Print what would be done (target version, method, action) and change nothing.
    #[arg(long)]
    pub dry_run: bool,
}

fn parse_method(value: &str) -> Result<InstallMethod, String> {
    InstallMethod::parse(value).ok_or_else(|| format!("unknown install method \"{value}\""))
}

pub fn run(args: UpgradeArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let source = ReleaseSource::resolve(&ctx.env).map_err(CliError::runtime)?;
    let detected = match args.method {
        Some(method) => Detected::named(method),
        None => method::detect_origin(&Probe::current(&ctx.env, &ctx.home)?),
    };
    let method = detected.method;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let from = ctx.build.version;
    let (target, plan) = match &args.version {
        Some(version) => {
            let target = requested(version)?;
            let plan = Plan::new(method, from, &target);
            (target, plan)
        }
        None => {
            let channel = args
                .channel
                .as_deref()
                .map_or(ctx.build.channel, Channel::parse);
            let target = latest(&runtime, &source, ctx, channel)?;
            let plan = Plan::to_latest(method, from, &target);
            (target, plan)
        }
    };
    let (plan, listed) = listing_only(plan, detected);
    if !args.dry_run {
        execute(&plan, &runtime, &source, ctx, &target)?;
    }
    if output::is_json(global.format) {
        let mut result = json!({
            "from": from, "to": target, "method": method, "action": plan.action(),
        });
        if let Plan::Manual { command } = &plan {
            result["command"] = json!(command);
        }
        if args.dry_run {
            result["dry_run"] = json!(true);
        }
        return output::json(&result);
    }
    print_text(&plan, from, &target, method, listed, args.dry_run);
    Ok(())
}

/// Carry out `plan`: replace the binary after verification or run the package manager.
/// `CYBER_OFFLINE` stops both before any download.
fn execute(
    plan: &Plan,
    runtime: &tokio::runtime::Runtime,
    source: &ReleaseSource,
    ctx: &Context,
    target: &str,
) -> Result<(), CliError> {
    if ctx.env.flag("CYBER_OFFLINE") && matches!(plan, Plan::Replace | Plan::Manager { .. }) {
        return Err(CliError::runtime(format!(
            "offline: installing cyber {target} needs the network"
        ))
        .with_hint("unset CYBER_OFFLINE to upgrade"));
    }
    match plan {
        Plan::Replace => self_update(runtime, source, ctx, target),
        Plan::Manager { argv } => run_manager(argv, ctx),
        Plan::Current | Plan::Manual { .. } => Ok(()),
    }
}

fn print_text(
    plan: &Plan,
    from: &str,
    target: &str,
    method: InstallMethod,
    listed: bool,
    dry_run: bool,
) {
    match plan {
        Plan::Current if target == from => println!("cyber {from} is already installed"),
        Plan::Current => println!(
            "cyber {from} is newer than {target}, the channel's latest; nothing changed \
             (run \"cyber upgrade {target}\" to downgrade)"
        ),
        Plan::Replace if dry_run => println!(
            "cyber {from} → {target} via {} (dry run: would download, verify and replace \
             the binary; nothing changed)",
            method.as_str()
        ),
        Plan::Manager { argv } if dry_run => println!(
            "cyber {from} → {target} via {} (dry run: would run: {}; nothing changed)",
            method.as_str(),
            argv.join(" ")
        ),
        Plan::Replace | Plan::Manager { .. } => println!("cyber {from} → {target}"),
        Plan::Manual { command } if listed => println!(
            "{} lists a cyber package, but this binary is not from it; to upgrade that \
             package, run: {command} (or \"cyber upgrade --method {}\" to run it)",
            method.as_str(),
            method.as_str()
        ),
        Plan::Manual { command } => println!("installed via {}; run: {command}", method.as_str()),
    }
}

/// A package manager's command when only its listing names the method is printed, never
/// run: it would change that package and leave the running binary as it is. The flag
/// says the plan was turned into such a note.
fn listing_only(plan: Plan, detected: Detected) -> (Plan, bool) {
    match plan {
        Plan::Manager { argv } if !detected.from_path => {
            let command = argv.join(" ");
            (Plan::Manual { command }, true)
        }
        plan => (plan, false),
    }
}

/// Verify and replace the running binary (and its sandbox helper).
fn self_update(
    runtime: &tokio::runtime::Runtime,
    source: &ReleaseSource,
    ctx: &Context,
    version: &str,
) -> Result<(), CliError> {
    let exe = std::env::current_exe()?;
    let update = SelfUpdate {
        source,
        version,
        target: ctx.build.target,
        exe: &exe,
        timeout: DOWNLOAD_TIMEOUT,
    };
    runtime
        .block_on(update.run())
        .map_err(|e| CliError::runtime(e.to_string()))
}

/// Run the package manager without a shell, resolved on PATH; a failure names the
/// command so the user can run it.
pub(crate) fn run_manager(argv: &[String], ctx: &Context) -> Result<(), CliError> {
    cyber_install::upgrade::run_manager(argv, &ctx.env).map_err(CliError::runtime)
}

/// The warning for a running version older than the cached manifest's `min_supported`
/// (`installation-upgrade` → Minimum supported version): the cache only, never the
/// network, so a start never waits on it.
pub(crate) fn unsupported_warning(ctx: &Context) -> Option<String> {
    let (_, manifest) = cyber_install::manifest::read_cache(&ctx.paths.cache, ctx.build.channel)?;
    let minimum = manifest.min_supported?;
    policy::unsupported(ctx.build.version, Some(&minimum))
        .then(|| cyber_core::version::unsupported_warning(ctx.build.version, &minimum))
}

/// The version the user named, without a leading `v`; it must be semver.
fn requested(version: &str) -> Result<String, CliError> {
    let version = version.strip_prefix('v').unwrap_or(version);
    semver::Version::parse(version)
        .map(|_| version.to_string())
        .map_err(|e| CliError::usage(format!("\"{version}\" is not a version: {e}")))
}

fn latest(
    runtime: &tokio::runtime::Runtime,
    source: &ReleaseSource,
    ctx: &Context,
    channel: Channel,
) -> Result<String, CliError> {
    let lookup = Lookup::new(source, &ctx.paths.cache, &ctx.env);
    runtime
        .block_on(lookup.latest(channel))
        .map(|latest| latest.manifest.version)
        .map_err(|e| CliError::runtime(e.to_string()))
}
