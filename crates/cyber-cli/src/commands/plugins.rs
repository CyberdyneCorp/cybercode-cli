//! `cyber plugins install|list|info|update|remove|enable|disable|validate`
//! (`plugins-marketplace` → Plugin CLI). Installing is the user's own trust decision;
//! enabling or updating a plugin a project or local scope lists shows what it would load
//! and asks first.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueEnum};
use cyber_core::plugins::{Locations, Manager, Pending, PluginScope, PluginStatus};
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ScopeArg {
    User,
    Project,
    Local,
}

impl From<ScopeArg> for PluginScope {
    fn from(scope: ScopeArg) -> Self {
        match scope {
            ScopeArg::User => Self::User,
            ScopeArg::Project => Self::Project,
            ScopeArg::Local => Self::Local,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum PluginsCmd {
    /// Install a plugin from a git URL (`git+<url>[#ref]`) or a local directory.
    Install {
        spec: String,
        #[arg(long, value_enum, default_value = "user")]
        scope: ScopeArg,
        /// The git branch, tag or commit.
        #[arg(long = "ref", value_name = "REF")]
        reference: Option<String>,
        /// The plugin's directory inside the git repository.
        #[arg(long, value_name = "DIR")]
        path: Option<String>,
        /// Install even when `engines.cyber` excludes this version.
        #[arg(long)]
        force: bool,
    },
    /// List plugins of every scope with their state; every plugin is `unsigned`.
    List,
    /// Show one plugin's state, source, digest and components.
    Info { name: String },
    /// Fetch a plugin (or every plugin) again from its source and record the new digest.
    Update {
        name: Option<String>,
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
        /// Trust the new content without asking.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        force: bool,
    },
    /// Remove a plugin from a scope (default: every scope listing it) and revoke its trust.
    Remove {
        name: String,
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
    },
    /// Enable a listed plugin, installing it at its pinned commit and asking to trust it.
    Enable {
        name: String,
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
        /// Trust the plugin without asking.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        force: bool,
    },
    /// Disable a plugin in a scope (default: every scope listing it).
    Disable {
        name: String,
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
    },
    /// Check a plugin directory's manifest and components without installing it.
    Validate { path: PathBuf },
    /// Set a plugin's `userConfig` settings: `--set key=value` for ordinary values, or
    /// answer a prompt for each (secrets without echo, stored in the OS keyring).
    Configure {
        name: String,
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
    },
}

pub fn run(cmd: PluginsCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let manager = Manager::new(Locations::new(&ctx.paths, &ctx.location), ctx.build.version);
    match cmd {
        PluginsCmd::Install {
            spec,
            scope,
            reference,
            path,
            force,
        } => {
            let cwd = std::env::current_dir()?;
            let source =
                cyber_core::plugins::parse_spec(&spec, reference.as_deref(), path.as_deref(), &cwd)
                    .map_err(CliError::usage)?;
            let pending = manager
                .prepare_install(source, scope.into(), force)
                .map_err(refused)?;
            apply(ctx, &manager, pending, true, global, "Installed")
        }
        PluginsCmd::List => list(ctx, global),
        PluginsCmd::Info { name } => info(ctx, global, &name),
        PluginsCmd::Update {
            name,
            scope,
            yes,
            force,
        } => update(
            ctx,
            &manager,
            name,
            scope.map(Into::into),
            (yes, force),
            global,
        ),
        PluginsCmd::Enable {
            name,
            scope,
            yes,
            force,
        } => {
            let pending = manager
                .prepare_enable(&name, scope.map(Into::into), force)
                .map_err(refused)?;
            apply(ctx, &manager, pending, yes, global, "Enabled")
        }
        PluginsCmd::Disable { name, scope } => {
            let scopes = manager
                .disable(&name, scope.map(Into::into))
                .map_err(CliError::usage)?;
            changed(global, "disabled", &name, &scopes)
        }
        PluginsCmd::Remove { name, scope } => {
            let manifest = super::plugin_config::installed_manifest(ctx, &name).ok();
            let scopes = manager
                .remove(&name, scope.map(Into::into))
                .map_err(CliError::usage)?;
            // Secrets belong to one scope and checkout: only the removed entries' go.
            for removed in manifest
                .iter()
                .flat_map(|(_, manifest)| scopes.iter().map(move |scope| (*scope, manifest)))
            {
                super::plugin_config::forget_secrets(ctx, (&name, removed.0), removed.1)?;
            }
            changed(global, "removed", &name, &scopes)
        }
        PluginsCmd::Validate { path } => validate(global, &path),
        PluginsCmd::Configure { name, set } => super::plugin_config::run(ctx, &name, &set),
    }
}

fn refused(message: String) -> CliError {
    let hint = message
        .contains("requires cyber")
        .then_some("pass --force to install it anyway");
    let error = CliError::usage(message);
    match hint {
        Some(hint) => error.with_hint(hint),
        None => error,
    }
}

/// Commit `pending`; a project or local entry that is not yet trusted needs `yes` or a
/// `y` on stdin. Required settings that are not set yet are asked for afterwards.
fn apply(
    ctx: &Context,
    manager: &Manager,
    pending: Pending,
    yes: bool,
    global: &GlobalArgs,
    verb: &str,
) -> Result<(), CliError> {
    let review = !pending.approved && pending.scope != PluginScope::User && !yes;
    if review && !confirmed(&pending).inspect_err(|_| manager.discard(&pending))? {
        manager.discard(&pending);
        return Err(CliError::runtime(format!(
            "plugin {} was not trusted; nothing changed",
            pending.name
        )));
    }
    let report = report(&pending);
    let (name, scope) = (pending.name.clone(), pending.scope);
    let manifest = pending.fetched.manifest.clone();
    if !review {
        // The review prompt already listed them.
        describe_host(&pending.summary)?;
    }
    manager.commit(pending).map_err(CliError::runtime)?;
    super::plugin_config::ask_missing(
        ctx,
        (&name, scope),
        &manifest,
        &mut super::plugin_config::Answers::stdin(),
    )?;
    if output::is_json(global.format) {
        return output::json(&report);
    }
    println!(
        "{verb} {}@{} ({} scope, unsigned) {}",
        report["name"].as_str().unwrap_or_default(),
        report["version"].as_str().unwrap_or_default(),
        report["scope"].as_str().unwrap_or_default(),
        report["digest"].as_str().unwrap_or_default(),
    );
    for warning in report["summary"]["warnings"]
        .as_array()
        .into_iter()
        .flatten()
    {
        eprintln!("warning: {}", warning.as_str().unwrap_or_default());
    }
    Ok(())
}

fn report(pending: &Pending) -> Value {
    let lock = pending.entry.installed.as_ref();
    json!({
        "name": pending.name,
        "scope": pending.scope,
        "version": pending.fetched.manifest.version,
        "source": pending.entry.source,
        "commit": lock.and_then(|lock| lock.commit.clone()),
        "digest": pending.fetched.digest,
        "forced": lock.is_some_and(|lock| lock.forced),
        "root": pending.fetched.root,
        "unsigned": true,
        "summary": pending.summary,
    })
}

/// Show what the plugin would load, then ask on stderr; anything but `y`/`yes` (EOF
/// included) declines.
fn confirmed(pending: &Pending) -> Result<bool, CliError> {
    let summary = &pending.summary;
    let mut err = std::io::stderr();
    writeln!(
        err,
        "Plugin {}@{} ({} scope, unsigned)\n  source: {}\n  digest: {}",
        pending.name,
        pending.fetched.manifest.version,
        pending.scope.as_str(),
        pending.entry.source,
        pending.fetched.digest
    )?;
    writeln!(
        err,
        "  hooks: {}\n  mcp servers: {}\n  agents: {}\n  commands: {}  skills: {}",
        list_or_none(&summary.hooks),
        list_or_none(&summary.mcp),
        list_or_none(&summary.agents),
        summary.commands,
        summary.skills
    )?;
    describe_host(summary)?;
    write!(err, "Trust and enable {}? [y/N] ", pending.name)?;
    err.flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// The host process a plugin runs and the capabilities its sandbox grants, on stderr.
fn describe_host(summary: &cyber_core::plugins::Summary) -> Result<(), CliError> {
    if let Some(host) = &summary.host {
        writeln!(
            std::io::stderr(),
            "  host: {host}\n  capabilities: {}",
            list_or_none(&summary.capabilities)
        )?;
    }
    Ok(())
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".into()
    } else {
        items.join(", ")
    }
}

fn update(
    ctx: &Context,
    manager: &Manager,
    name: Option<String>,
    scope: Option<PluginScope>,
    (yes, force): (bool, bool),
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let names: Vec<String> = match name {
        Some(name) => vec![name],
        None => {
            let mut names: Vec<String> = manager
                .names()
                .map_err(CliError::runtime)?
                .into_iter()
                .filter(|(listed, _)| scope.is_none_or(|scope| scope == *listed))
                .map(|(_, name)| name)
                .collect();
            names.dedup();
            names
        }
    };
    for name in names {
        let pending = manager
            .prepare_update(&name, scope, force)
            .map_err(refused)?;
        apply(ctx, manager, pending, yes, global, "Updated")?;
    }
    Ok(())
}

fn changed(
    global: &GlobalArgs,
    verb: &str,
    name: &str,
    scopes: &[PluginScope],
) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(&json!({ "name": name, "change": verb, "scopes": scopes }));
    }
    let scopes: Vec<&str> = scopes.iter().map(|scope| scope.as_str()).collect();
    println!("Plugin {name} {verb} ({})", scopes.join(", "));
    Ok(())
}

fn statuses(ctx: &Context) -> Result<(Vec<PluginStatus>, bool), CliError> {
    let resolved = ctx.config()?;
    for warning in &resolved.warnings {
        eprintln!("warning: {warning}");
    }
    Ok((resolved.plugins, resolved.pure))
}

fn list(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let (plugins, pure) = statuses(ctx)?;
    if output::is_json(global.format) {
        return output::json(&json!({ "plugins": plugins, "pure": pure }));
    }
    if pure {
        println!("external extensions disabled (--pure)");
    }
    if plugins.is_empty() {
        println!("no plugins installed");
    }
    for plugin in &plugins {
        let state = serde_json::to_value(plugin.state).unwrap_or_default();
        println!(
            "{}  {}  [{}]  {}  unsigned  {}{}",
            plugin.name,
            plugin.version.as_deref().unwrap_or("-"),
            plugin.scope.as_str(),
            state.as_str().unwrap_or_default(),
            plugin.source,
            plugin
                .reason
                .as_deref()
                .map(|reason| format!("  ({reason})"))
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn info(ctx: &Context, global: &GlobalArgs, name: &str) -> Result<(), CliError> {
    let (plugins, _) = statuses(ctx)?;
    let mut entries = Vec::new();
    for plugin in plugins.iter().filter(|plugin| plugin.name == name) {
        let mut entry = serde_json::to_value(plugin).unwrap_or_default();
        if let Some(root) = plugin.root.as_deref().filter(|root| root.is_dir()) {
            entry["manifest"] = inspected(root)
                .map(|(manifest, summary)| json!({ "manifest": manifest, "summary": summary }))
                .unwrap_or_else(|error| json!({ "error": error }));
        }
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err(CliError::usage(format!(
            "plugin {name} is not listed in any scope"
        )));
    }
    if output::is_json(global.format) {
        return output::json(&json!({ "name": name, "entries": entries }));
    }
    for entry in &entries {
        println!(
            "{}",
            serde_json::to_string_pretty(entry).unwrap_or_default()
        );
    }
    Ok(())
}

fn inspected(root: &Path) -> Result<(Value, cyber_core::plugins::Summary), String> {
    let manifest = cyber_core::plugins::Manifest::read(root).map_err(|e| e.to_string())?;
    let digest = cyber_core::plugins::tree_digest(root)?;
    let fetched = cyber_core::plugins::Fetched {
        root: root.to_path_buf(),
        manifest: manifest.clone(),
        digest,
        commit: None,
        fresh: false,
    };
    let summary = cyber_core::plugins::summarize(&manifest.name, PluginScope::User, &fetched);
    Ok((serde_json::to_value(manifest).unwrap_or_default(), summary))
}

fn validate(global: &GlobalArgs, path: &Path) -> Result<(), CliError> {
    let (manifest, summary) = inspected(path).map_err(CliError::usage)?;
    if output::is_json(global.format) {
        return output::json(&json!({ "valid": true, "manifest": manifest, "summary": summary }));
    }
    println!(
        "{}@{} is valid",
        manifest["name"].as_str().unwrap_or_default(),
        manifest["version"].as_str().unwrap_or_default()
    );
    for warning in &summary.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(())
}
