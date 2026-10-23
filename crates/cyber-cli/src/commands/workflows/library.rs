//! The Workflow library on the command line (`workflows` → Saved and bundled workflows):
//! `list`, `show <name>` and `eject`. They read the Location's library directly, without a
//! server.

use std::path::{Path, PathBuf};

use clap::Args;
use cyber_server::runtime::workflow_start::LibrarySources;
use cyber_workflow::{Listing, Origin, Scope, WorkflowModule, bundled};
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Default, Args)]
pub struct EjectArgs {
    /// The bundled Workflow to copy.
    pub name: String,
    /// Write it to the global `workflows/` directory instead of the project.
    #[arg(long)]
    pub global: bool,
    /// Replace an existing file.
    #[arg(long)]
    pub force: bool,
}

/// The Location's library: bundled, then global, then project directories from the project
/// root down to the Location. `CYBER_DISABLE_PROJECT_CONFIG` skips project directories;
/// `workflows.bundled: false` and `CYBER_DISABLE_BUNDLED_WORKFLOWS=1` hide bundled ones.
pub fn discover(ctx: &Context) -> Result<Listing, CliError> {
    let config = ctx.config()?.value;
    let sources = LibrarySources::of(&config, &ctx.env);
    let project_root = cyber_core::config::project_root(&ctx.location);
    let project = sources
        .project
        .then_some((ctx.location.as_path(), project_root.as_path()));
    Ok(cyber_workflow::discover_library(
        &cyber_workflow::roots(&ctx.paths.config, project),
        sources.bundled,
    ))
}

/// `bundled`, `global`, `project` or `inline`.
pub fn origin_name(module: &WorkflowModule) -> &'static str {
    match module.origin {
        Origin::Saved {
            scope: Scope::Project,
            ..
        } => "project",
        Origin::Saved {
            scope: Scope::Global,
            ..
        } => "global",
        Origin::Bundled { .. } => "bundled",
        Origin::Inline => "inline",
    }
}

fn path_of(module: &WorkflowModule) -> Value {
    match &module.origin {
        Origin::Saved { path, .. } => json!(path),
        Origin::Bundled { .. } | Origin::Inline => Value::Null,
    }
}

fn entry_view(module: &WorkflowModule, shadowed: &[PathBuf]) -> Value {
    let raw = &module.meta.raw;
    json!({
        "name": module.meta.name,
        "description": module.meta.description,
        "phases": module.meta.phases,
        "args": module.meta.args,
        "budget": raw.get("budget"),
        "estimate": raw.get("estimate"),
        "tools": module.meta.tools,
        "origin": origin_name(module),
        "scope": origin_name(module),
        "path": path_of(module),
        "digest": module.digest,
        "shadowed": shadowed,
    })
}

fn view(listing: &Listing) -> Value {
    let workflows: Vec<Value> = listing
        .workflows
        .iter()
        .map(|entry| entry_view(&entry.module, &entry.shadowed))
        .collect();
    json!({ "workflows": workflows, "issues": listing.issues })
}

pub fn list(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let listing = discover(ctx)?;
    if output::is_json(global.format) {
        return output::json(&view(&listing));
    }
    if listing.workflows.is_empty() && listing.issues.is_empty() {
        println!("No workflows. Add modules to .cyber/workflows/ or ~/.config/cyber/workflows/.");
    }
    let width = listing
        .workflows
        .iter()
        .map(|e| e.module.meta.name.len())
        .max()
        .unwrap_or(0)
        .max("NAME".len());
    if !listing.workflows.is_empty() {
        println!("{:width$}  {:7}  DESCRIPTION", "NAME", "ORIGIN");
    }
    for entry in &listing.workflows {
        let meta = &entry.module.meta;
        let shadows = if entry.shadowed.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = entry
                .shadowed
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            format!("  (shadows {})", names.join(", "))
        };
        println!(
            "{:width$}  {:7}  {}{shadows}",
            meta.name,
            origin_name(&entry.module),
            meta.description
        );
    }
    for issue in &listing.issues {
        println!("issue: {}: {}", issue.path.display(), issue.diagnostic);
    }
    Ok(())
}

pub fn show(ctx: &Context, global: &GlobalArgs, name: &str, source: bool) -> Result<(), CliError> {
    let listing = discover(ctx)?;
    let entry = listing
        .workflows
        .iter()
        .find(|entry| entry.module.meta.name == name)
        .ok_or_else(|| CliError::runtime(format!("No Workflow named {name:?} in this Location")))?;
    let module = &entry.module;
    let mut value = entry_view(module, &entry.shadowed);
    if source {
        value["source"] = module.source.clone().into();
    }
    if output::is_json(global.format) {
        return output::json(&value);
    }
    let pretty = |value: &Value| serde_json::to_string_pretty(value).unwrap_or_default();
    println!("{}  ({})", module.meta.name, origin_name(module));
    println!("{}", module.meta.description);
    if !module.meta.phases.is_empty() {
        let phases: Vec<&str> = module
            .meta
            .phases
            .iter()
            .map(|p| p.title.as_str())
            .collect();
        println!("phases: {}", phases.join(" → "));
    }
    if let Some(estimate) = module.meta.raw.get("estimate").and_then(Value::as_str) {
        println!("estimate: {estimate}");
    }
    if let Some(budget) = module.meta.raw.get("budget") {
        println!("default budget: {}", budget);
    }
    if !module.meta.tools.is_empty() {
        println!("direct tools: {}", module.meta.tools.join(", "));
    }
    println!("digest: {}", module.digest);
    for shadowed in &entry.shadowed {
        println!("shadows: {}", shadowed.display());
    }
    if let Some(args) = &module.meta.args {
        println!("args schema:\n{}", pretty(args));
    }
    if source {
        println!("\n{}", module.source);
    }
    Ok(())
}

/// Where an ejected copy goes: `<project root>/.cyber/workflows/<name>.js`, or the global
/// `workflows/` directory.
fn eject_path(ctx: &Context, name: &str, global: bool) -> PathBuf {
    let root = if global {
        ctx.paths.config.join("workflows")
    } else {
        cyber_core::config::project_root(&ctx.location)
            .join(".cyber")
            .join("workflows")
    };
    root.join(format!("{name}.js"))
}

pub fn eject(ctx: &Context, global: &GlobalArgs, args: &EjectArgs) -> Result<(), CliError> {
    let module = bundled::module(&args.name).ok_or_else(|| {
        let names: Vec<&str> = bundled::modules()
            .iter()
            .map(|m| m.meta.name.as_str())
            .collect();
        CliError::runtime(format!(
            "No bundled Workflow named {:?}; bundled: {}",
            args.name,
            names.join(", ")
        ))
    })?;
    let path = eject_path(ctx, &args.name, args.global);
    write_new(&path, &module.source, args.force)?;
    if output::is_json(global.format) {
        return output::json(&json!({"name": args.name, "path": path}));
    }
    println!(
        "Wrote {}; it now overrides the bundled {}.",
        path.display(),
        args.name
    );
    Ok(())
}

/// Write `source` to `path`, refusing to replace an existing file unless `force`.
fn write_new(path: &Path, source: &str, force: bool) -> Result<(), CliError> {
    if !force && std::fs::symlink_metadata(path).is_ok() {
        return Err(CliError::runtime(format!(
            "{} already exists; pass --force to replace it",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CliError::runtime(format!("could not create {}: {error}", parent.display()))
        })?;
    }
    std::fs::write(path, source)
        .map_err(|error| CliError::runtime(format!("could not write {}: {error}", path.display())))
}
