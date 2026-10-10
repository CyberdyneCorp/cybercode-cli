//! Read-only migration detection bypasses configuration/bootstrap/log creation.
use crate::{cli::GlobalArgs, error::CliError, output};
use cyber_core::{
    env::{EnvSource, ProcessEnv},
    import::{SourceRoots, detect_sources},
    paths,
};

pub fn detect(global: &GlobalArgs) -> Result<(), CliError> {
    let env = ProcessEnv;
    let home = paths::home_dir(&env)
        .ok_or_else(|| CliError::runtime("cannot determine the home directory"))?;
    let directory = global
        .cwd
        .clone()
        .map_or_else(std::env::current_dir, Ok)?
        .canonicalize()?;
    let project_root = git_root(&directory).unwrap_or_else(|| directory.clone());
    let roots = SourceRoots {
        project_root,
        directory,
        home,
        codex_home: env.get("CODEX_HOME").map(Into::into),
    };
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
