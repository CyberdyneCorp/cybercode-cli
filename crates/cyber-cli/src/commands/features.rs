//! `cyber features list|enable|disable` (`configuration` → Feature flags registry): the
//! registry with each feature's effective value and source, and comment-preserving
//! edits of `features.<name>` in the user or project config document.

use clap::{Subcommand, ValueEnum};
use cyber_core::features::{FeatureStatus, Features};
use serde_json::json;

use crate::cli::GlobalArgs;
use crate::commands::mcp_config;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum FeaturesCmd {
    /// Every feature with its maturity, default, effective value and source.
    List {
        /// Print JSON (same as --format json).
        #[arg(long)]
        json: bool,
    },
    /// Turn a feature on in the user or project configuration.
    Enable {
        name: String,
        #[arg(long, value_enum, default_value_t = FeatureScope::User)]
        scope: FeatureScope,
    },
    /// Turn a feature off in the user or project configuration.
    Disable {
        name: String,
        #[arg(long, value_enum, default_value_t = FeatureScope::User)]
        scope: FeatureScope,
    },
}

/// The config document `enable` and `disable` edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FeatureScope {
    /// `~/.config/cyber/cyber.jsonc`.
    User,
    /// The project's `.cyber/cyber.jsonc`.
    Project,
}

pub fn run(cmd: FeaturesCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    match cmd {
        FeaturesCmd::List { json } => list(ctx, json || output::is_json(global.format)),
        FeaturesCmd::Enable { name, scope } => set(ctx, &name, scope, true),
        FeaturesCmd::Disable { name, scope } => set(ctx, &name, scope, false),
    }
}

/// The effective features of the current Location; an invalid configuration (such as
/// an unknown feature) fails here.
pub fn resolve(ctx: &Context) -> Result<Features, CliError> {
    let resolved = ctx.config()?;
    Ok(Features::from_config(&resolved.value, &resolved.sources))
}

fn list(ctx: &Context, json: bool) -> Result<(), CliError> {
    let resolved = ctx.config()?;
    // The deprecated `experimental.<name>` alias is reported where features are listed.
    for warning in resolved
        .warnings
        .iter()
        .filter(|w| w.starts_with("experimental."))
    {
        eprintln!("warning: {warning}");
    }
    let features = Features::from_config(&resolved.value, &resolved.sources);
    if json {
        return output::json(&features.features);
    }
    let width = features
        .features
        .iter()
        .map(|status| status.name.len())
        .max()
        .unwrap_or(0);
    for status in &features.features {
        println!(
            "{:width$}  {:12}  {}",
            status.name,
            status.maturity.as_str(),
            describe(status)
        );
    }
    Ok(())
}

/// `off (default)`, or `on (cli:--features; default off)`, plus `locked by policy`.
fn describe(status: &FeatureStatus) -> String {
    let mut source = status.source.clone();
    if status.source != cyber_core::features::DEFAULT_SOURCE {
        source.push_str(&format!("; default {}", on_off(status.default)));
    }
    if status.locked {
        source.push_str("; locked by policy");
    }
    format!("{} ({source})", on_off(status.enabled))
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn set(ctx: &Context, name: &str, scope: FeatureScope, on: bool) -> Result<(), CliError> {
    if name == cyber_core::features::COMPUTER_USE
        && on
        && std::env::var_os("CYBER_SESSION_ID").is_some()
    {
        return Err(CliError::usage(
            "computer_use must be enabled by the human owner, outside an agent subprocess",
        ));
    }
    if name == cyber_core::features::COMPUTER_USE && matches!(scope, FeatureScope::Project) {
        return Err(CliError::usage(
            "computer_use is global-only; use --scope user",
        ));
    }
    let entry = cyber_core::features::entry(name)
        .ok_or_else(|| CliError::usage(cyber_core::features::unknown(name)))?;
    let path = mcp_config::document(
        ctx,
        match scope {
            FeatureScope::User => mcp_config::Scope::User,
            FeatureScope::Project => mcp_config::Scope::Project,
        },
    )?;
    cyber_core::config::set_document_member(&path, "features", entry.name, Some(&json!(on)))
        .map_err(CliError::runtime)?;
    let verb = if on { "Enabled" } else { "Disabled" };
    println!("{verb} {} in {}", entry.name, path.display());
    // A higher source still decides; say which one instead of implying the edit applies.
    if let Ok(features) = resolve(ctx)
        && let Some(status) = features.get(entry.name)
        && status.enabled != on
    {
        println!(
            "Note: {} stays {} ({})",
            entry.name,
            on_off(status.enabled),
            status.source
        );
    }
    Ok(())
}
