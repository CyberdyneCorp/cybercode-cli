//! `cyber uninstall` (`installation-upgrade` → Uninstall command): list the binaries of
//! a `script` install, the marked PATH lines and the cyber directories with sizes, ask
//! (or require `--force` without a terminal), stop the background server, delete the
//! keyring entries cyber's own records name, run the package manager's uninstall, then
//! remove the plan. The manager's command is listed before the confirmation and runs
//! only when the binary's own path shows that manager installed it. It never creates a
//! directory or a log file first, so a dry run changes nothing and Windows has no open
//! log in the data directory.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use clap::Args;
use cyber_core::env::EnvSource;
use cyber_install::method::{self, InstallMethod, Probe};
use cyber_install::uninstall::{self, Flags, Target, UninstallPlan};
use serde_json::json;

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Args)]
pub struct UninstallArgs {
    /// Keep the configuration directory.
    #[arg(long)]
    pub keep_config: bool,
    /// Keep the data directory (sessions, memory, stored logins) and the keyring entries.
    #[arg(long)]
    pub keep_data: bool,
    /// List what would be removed, with sizes, and change nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Remove without asking, and go on when the background server cannot be stopped.
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: &UninstallArgs, global: &GlobalArgs) -> Result<(), CliError> {
    let ctx = Context::existing(global)?;
    let exe = std::env::current_exe()?;
    let probe = Probe::current(&ctx.env, &ctx.home)?;
    let detected = method::detect_origin(&probe);
    let method = detected.method;
    let rc_files = uninstall::rc_candidates(&ctx.env, &ctx.home);
    let cyber_home = ctx.env.get("CYBER_HOME").map(PathBuf::from);
    let target = Target {
        paths: &ctx.paths,
        home: &ctx.home,
        method,
        exe: &exe,
        rc_files: &rc_files,
        cyber_home: cyber_home.as_deref(),
    };
    let flags = Flags {
        keep_config: args.keep_config,
        keep_data: args.keep_data,
    };
    let mut plan =
        UninstallPlan::build(&target, flags).map_err(|e| CliError::runtime(e.to_string()))?;
    let manager = managed_uninstall(method, detected.from_path, &mut plan);
    let json = output::is_json(global.format);
    if args.dry_run {
        return report(&plan, method, manager.as_deref(), json);
    }
    if !json {
        list(&plan, "Will remove", manager.as_deref());
    }
    if !args.force && !confirm(&plan, manager.as_deref())? {
        eprintln!("nothing removed");
        return Ok(());
    }
    stop_server(&ctx, args.force)?;
    let mut failures = Vec::new();
    if !args.keep_data {
        failures.extend(forget_credentials(&ctx));
    }
    if let Some(argv) = &manager
        && let Err(error) = super::upgrade::run_manager(argv, &ctx)
    {
        failures.push(error.message);
    }
    let removed = plan.remove(&ctx.home);
    report(&plan, method, manager.as_deref(), json)?;
    removed.map_err(|e| CliError::runtime(e.to_string()))?;
    if failures.is_empty() {
        return Ok(());
    }
    for failure in &failures {
        eprintln!("warning: {failure}");
    }
    Err(CliError::runtime(
        "uninstall finished with errors; see the warnings above",
    ))
}

/// The manager's uninstall argv when the binary's path shows `method` installed it. A
/// method known from the managers' listings alone says a package is installed somewhere,
/// not that this binary came from it, so its command becomes a note to run by hand.
fn managed_uninstall(
    method: InstallMethod,
    from_path: bool,
    plan: &mut UninstallPlan,
) -> Option<Vec<String>> {
    let argv = uninstall::manager_argv(method)?;
    if from_path {
        return Some(argv);
    }
    plan.notes.push(format!(
        "{} lists a cyber package, but this binary is not from it; to remove that package, \
         run: {}",
        method.as_str(),
        argv.join(" ")
    ));
    None
}

fn report(
    plan: &UninstallPlan,
    method: InstallMethod,
    manager: Option<&[String]>,
    json: bool,
) -> Result<(), CliError> {
    if json {
        return output::json(&json!({
            "method": method,
            "entries": plan.entries,
            "total_size": plan.total_size(),
            "manager_command": manager.map(|argv| argv.join(" ")),
            "notes": plan.notes,
        }));
    }
    let done = plan.entries.iter().any(|entry| entry.removed);
    if done {
        let count = plan.entries.iter().filter(|entry| entry.removed).count();
        println!("Removed {count} items ({})", size(plan.total_size()));
    } else {
        list(plan, "Would remove", manager);
    }
    for note in &plan.notes {
        eprintln!("note: {note}");
    }
    Ok(())
}

/// The entries with sizes, then the package manager's command.
fn list(plan: &UninstallPlan, heading: &str, manager: Option<&[String]>) {
    if plan.entries.is_empty() {
        println!("Nothing to remove");
    } else {
        println!("{heading}:");
    }
    for entry in &plan.entries {
        let kind = serde_json::to_value(entry.kind).unwrap_or_default();
        println!(
            "  {:<9} {:>10}  {}",
            kind.as_str().unwrap_or_default(),
            size(entry.size),
            entry.path.display()
        );
    }
    if let Some(argv) = manager {
        println!("package manager: {}", argv.join(" "));
    }
}

/// `Remove these N items (X MiB)[ and run <command>]? [y/N]` on the terminal; without
/// one, exit 2.
fn confirm(plan: &UninstallPlan, manager: Option<&[String]>) -> Result<bool, CliError> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return Err(
            CliError::usage("cyber uninstall needs a terminal to confirm")
                .with_hint("pass --force to remove without asking"),
        );
    }
    let run = manager.map_or(String::new(), |argv| format!(" and run {}", argv.join(" ")));
    eprint!(
        "Remove these {} items ({}){run}? [y/N] ",
        plan.entries.len(),
        size(plan.total_size())
    );
    std::io::stderr().flush()?;
    let mut answer = String::new();
    stdin.lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
}

/// Stop the registered server before its directories go; a failure aborts unless
/// `--force`.
fn stop_server(ctx: &Context, force: bool) -> Result<(), CliError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    match runtime.block_on(cyber_app::stop_service(&ctx.paths)) {
        Ok(_) => Ok(()),
        Err(error) if force => {
            eprintln!("warning: cannot stop the background server: {error}; continuing (--force)");
            Ok(())
        }
        Err(error) => Err(
            CliError::runtime(format!("cannot stop the background server: {error}"))
                .with_hint("stop it with `cyber service stop`, or pass --force"),
        ),
    }
}

/// Delete the keyring entries cyber's records name: MCP OAuth credentials from their
/// index, peer tokens of the configured remotes, and the secret settings of the
/// resolved plugins. Provider logins are files in the data directory. Failures are
/// returned, never fatal.
fn forget_credentials(ctx: &Context) -> Vec<String> {
    use cyber_app::peer_tokens::{PeerTokenStore, PeerTokens};
    use cyber_tools::mcp::oauth;
    let mut failures = Vec::new();
    match oauth::indexed_servers(&oauth::index_file(&ctx.paths)) {
        Ok(servers) if !servers.is_empty() => {
            let store = oauth::open_store(&ctx.paths);
            for server in servers {
                if let Err(error) = store.remove_server(&server) {
                    failures.push(format!("MCP OAuth credentials of {server}: {error}"));
                }
            }
        }
        Ok(_) => {}
        Err(error) => failures.push(format!("MCP OAuth credential index: {error}")),
    }
    match super::peers::settings(ctx) {
        Ok(settings) => {
            let tokens = PeerTokenStore::open(&ctx.paths);
            for remote in settings.remotes {
                if let Err(error) = tokens.delete(&remote.name) {
                    failures.push(format!("peer token of {}: {error}", remote.name));
                }
            }
        }
        Err(error) => failures.push(format!("peer tokens: {}", error.message)),
    }
    failures.extend(forget_plugin_secrets(ctx));
    failures
}

fn forget_plugin_secrets(ctx: &Context) -> Vec<String> {
    let resolved = match ctx.config() {
        Ok(resolved) => resolved,
        Err(error) => return vec![format!("plugin secrets: {}", error.message)],
    };
    let mut failures = Vec::new();
    for plugin in &resolved.plugins {
        let Some(root) = plugin.root.as_ref().filter(|root| root.is_dir()) else {
            continue;
        };
        let forgotten = cyber_core::plugins::Manifest::read(root)
            .map_err(|error| error.to_string())
            .and_then(|manifest| {
                super::plugin_config::forget_secrets(ctx, (&plugin.name, plugin.scope), &manifest)
                    .map_err(|error| error.message)
            });
        if let Err(error) = forgotten {
            failures.push(format!("secrets of plugin {}: {error}", plugin.name));
        }
    }
    failures
}

fn size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KIB {
        format!("{bytes} B")
    } else if value < KIB * KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{:.1} MiB", value / (KIB * KIB))
    }
}
