//! Read-only migration detection and previews bypass configuration/bootstrap/log creation.
use crate::{cli::GlobalArgs, error::CliError, output};
use cyber_core::{
    env::{EnvSource, ProcessEnv},
    import::{SourceRoots, detect_sources},
    paths,
};

#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    #[arg(long, conflicts_with_all=["tool","dry_run"])]
    pub detect: bool,
    #[arg(value_enum, required_unless_present = "detect")]
    pub tool: Option<ImportTool>,
    #[arg(long, required_unless_present = "detect")]
    pub dry_run: bool,
    #[arg(long, value_enum, default_value = "project")]
    pub scope: Scope,
}
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ImportTool {
    Claude,
    Codex,
    Opencode,
    Auto,
}
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Scope {
    Project,
    Global,
}

pub fn run(args: &ImportArgs, global: &GlobalArgs) -> Result<(), CliError> {
    if args.detect {
        return detect(global);
    }
    if !args.dry_run {
        return Err(CliError::usage(
            "migration writes are not implemented; use --dry-run",
        ));
    }
    let roots = source_roots(global)?;
    let tool = match args
        .tool
        .ok_or_else(|| CliError::usage("choose a source tool"))?
    {
        ImportTool::Claude => Some(cyber_core::import::SourceTool::Claude),
        ImportTool::Codex => Some(cyber_core::import::SourceTool::Codex),
        ImportTool::Opencode => Some(cyber_core::import::SourceTool::OpenCode),
        ImportTool::Auto => None,
    };
    let scope = match args.scope {
        Scope::Project => cyber_core::import::ImportScope::Project,
        Scope::Global => cyber_core::import::ImportScope::Global,
    };
    let native = paths::Paths::resolve(&ProcessEnv, &roots.home).config;
    let preview = cyber_core::import::preview_import(&roots, tool, scope, &native)
        .map_err(|e| CliError::runtime(e.reason))?;
    preview.verify().map_err(|e| CliError::runtime(e.reason))?;
    let view = preview.output();
    if output::is_json(global.format) {
        return output::json(view);
    }
    println!("Target: {}", escaped(&view.target.to_string_lossy()));
    println!("Raw file-layer preview; profiles and substitutions are not evaluated.");
    for layer in &view.native_layers {
        println!("Native layer: {}", escaped(&layer.to_string_lossy()));
    }
    if view.diff.is_empty() {
        println!("nothing to do for supported fields");
    } else {
        print!("{}", view.diff);
    }
    for item in &view.report {
        println!(
            "{}: {} {} — {}",
            item.status,
            escaped(&item.source.to_string_lossy()),
            escaped(&item.field),
            item.reason
        );
    }
    for required in &view.required_environment {
        println!(
            "Set {} from the source field {}",
            required.variable, required.field
        );
    }
    println!(
        "Incomplete preview: remaining source adapters, exact byte diffs, complete provenance and reviewed writes are not implemented. No files written."
    );
    Ok(())
}
fn source_roots(global: &GlobalArgs) -> Result<SourceRoots, CliError> {
    let env = ProcessEnv;
    let home = paths::home_dir(&env)
        .ok_or_else(|| CliError::runtime("cannot determine the home directory"))?;
    let directory = global
        .cwd
        .clone()
        .map_or_else(std::env::current_dir, Ok)?
        .canonicalize()?;
    let project_root = git_root(&directory).unwrap_or_else(|| directory.clone());
    Ok(SourceRoots {
        project_root,
        directory,
        home,
        codex_home: env.get("CODEX_HOME").map(Into::into),
    })
}
pub fn detect(global: &GlobalArgs) -> Result<(), CliError> {
    let roots = source_roots(global)?;
    let report = detect_sources(&roots).map_err(|e| CliError::runtime(e.reason))?;
    if output::is_json(global.format) {
        return output::json(&report);
    }
    for tool in &report.tools {
        let name = match tool.tool {
            cyber_core::import::SourceTool::OpenCode => "opencode",
            cyber_core::import::SourceTool::Codex => "codex",
            cyber_core::import::SourceTool::Claude => "claude",
        };
        let c = &tool.counts;
        println!(
            "{name}: {} agents, {} commands, {} skills, {} MCP servers, {} hooks; sessions: unknown; read-time: unknown",
            c.agents, c.commands, c.skills, c.mcp_servers, c.hooks
        );
        for file in &tool.files {
            println!("  {}", escaped(&file.path.to_string_lossy()));
        }
    }
    for issue in &report.issues {
        println!(
            "issue: {}: {}",
            escaped(&issue.path.to_string_lossy()),
            issue.reason
        );
    }
    println!(
        "Coverage incomplete: referenced sources, sessions and effective read-time compatibility need resolution. Counts include raw definitions in separate layers."
    );
    Ok(())
}
fn git_root(directory: &std::path::Path) -> Option<std::path::PathBuf> {
    // Metadata only; no hooks, helpers, subprocesses or project-id cache writes.
    cyber_core::project::worktree_root(directory)
}
fn escaped(value: &str) -> String {
    value.chars().flat_map(char::escape_debug).collect()
}
